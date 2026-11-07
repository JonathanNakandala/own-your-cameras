//! The Mi360 side of ONVIF: profiles from the video graph the camera booted with, PTZ and presets as the
//! same commands MQTT sends (the main loop routes both alike), and snapshots from the media owner's JPEG
//! channel. Everything else is `camera_onvif`'s, shared with the other cameras.

use crate::media::RustVideoSpec;
use camera_onvif::snapshot::Waiters;
use camera_onvif::{
    DeviceIdentity, H264Profile, MediaProfile, OnvifCamera, Preset, PtzError, PtzPort,
    SnapshotError, Velocity, VideoEncoding,
};
use camera_transport::motion_control::{self, PresetName};
use camera_transport::mqtt::Command;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Presets the camera keeps (`presets::Presets`).
pub const MAX_PRESETS: usize = 16;

pub struct Mi360Onvif {
    pub identity: DeviceIdentity,
    pub video: RustVideoSpec,
    /// Into the main loop, beside MQTT's commands.
    pub commands: SyncSender<Command>,
    /// The preset names the hardware thread keeps current.
    pub presets: Arc<Mutex<Vec<String>>>,
    pub snapshots: Arc<Waiters>,
    /// Microphone and speaker run (`[runtime] audio`).
    pub audio: bool,
}

fn profile(idc: u32) -> H264Profile {
    match idc {
        77 => H264Profile::Main,
        100 => H264Profile::High,
        _ => H264Profile::Baseline,
    }
}

fn encoding(width: u32, height: u32, fps: u32, kbps: u32, idc: u32) -> VideoEncoding {
    VideoEncoding {
        width,
        height,
        fps,
        bitrate_kbps: kbps,
        // One-second GOP at each channel's rate (`sdk/imp_bridge.c`).
        gop: fps,
        profile: profile(idc),
    }
}

/// ONVIF generic velocity (-1..1) as the motion contract's percentage of each axis's ceiling.
fn percent(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 100.0).round() as i16
}

impl Mi360Onvif {
    fn send(&self, command: Command) -> Result<(), PtzError> {
        self.commands
            .try_send(command)
            .map_err(|_| PtzError::Failed("the camera is busy".into()))
    }

    fn names(&self) -> Vec<String> {
        self.presets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl OnvifCamera for Mi360Onvif {
    fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    fn profiles(&self) -> Vec<MediaProfile> {
        let v = self.video;
        vec![
            MediaProfile {
                token: "main",
                stream_path: camera_transport::MAIN_PCMU_PATH,
                video: encoding(
                    v.main_width,
                    v.main_height,
                    v.main_fps,
                    v.main_bitrate,
                    v.main_profile,
                ),
                audio: self.audio,
            },
            MediaProfile {
                token: "sub",
                stream_path: camera_transport::SUB_PCMU_PATH,
                video: encoding(
                    v.sub_width,
                    v.sub_height,
                    v.sub_fps,
                    v.sub_bitrate,
                    v.sub_profile,
                ),
                audio: self.audio,
            },
        ]
    }

    fn audio_output(&self) -> bool {
        self.audio
    }

    fn ptz(&self) -> Option<&dyn PtzPort> {
        Some(self)
    }

    fn digital_inputs(&self) -> &[&'static str] {
        &[]
    }

    fn snapshot(&self, deadline: Instant) -> Result<Vec<u8>, SnapshotError> {
        self.snapshots.wait(deadline).map_err(SnapshotError::Failed)
    }
}

impl PtzPort for Mi360Onvif {
    fn zoom(&self) -> bool {
        false
    }

    fn max_presets(&self) -> usize {
        MAX_PRESETS
    }

    fn continuous_move(&self, v: Velocity) -> Result<(), PtzError> {
        let velocity = motion_control::Velocity {
            pan: percent(v.pan),
            tilt: percent(v.tilt),
            zoom: 0,
        };
        if velocity.is_zero() {
            return self.send(Command::Stop);
        }
        let velocity = velocity.validate().map_err(PtzError::Failed)?;
        self.send(Command::Velocity(velocity))
    }

    fn stop(&self) -> Result<(), PtzError> {
        self.send(Command::Stop)
    }

    fn home(&self) -> Result<(), PtzError> {
        self.send(Command::Home)
    }

    fn presets(&self) -> Vec<Preset> {
        self.names()
            .into_iter()
            .map(|name| Preset {
                token: name.clone(),
                name,
            })
            .collect()
    }

    fn goto_preset(&self, token: &str) -> Result<(), PtzError> {
        if !self.names().iter().any(|n| n == token) {
            return Err(PtzError::NoSuchPreset);
        }
        let name = PresetName::parse(token).map_err(|_| PtzError::NoSuchPreset)?;
        self.send(Command::Preset(name))
    }

    /// The preset's token is its name: saving under an existing token overwrites it, a new name creates it.
    fn set_preset(&self, token: Option<&str>, name: Option<&str>) -> Result<String, PtzError> {
        let text = token.or(name).ok_or(PtzError::InvalidName)?;
        let parsed = PresetName::parse(text).map_err(|_| PtzError::InvalidName)?;
        let names = self.names();
        let known = names.iter().any(|n| n == text);
        if token.is_some() && !known {
            return Err(PtzError::NoSuchPreset);
        }
        if !known && names.len() >= MAX_PRESETS {
            return Err(PtzError::TooManyPresets);
        }
        self.send(Command::SavePreset(parsed))?;
        Ok(text.into())
    }

    fn remove_preset(&self, token: &str) -> Result<(), PtzError> {
        if !self.names().iter().any(|n| n == token) {
            return Err(PtzError::NoSuchPreset);
        }
        let name = PresetName::parse(token).map_err(|_| PtzError::NoSuchPreset)?;
        self.send(Command::RemovePreset(name))
    }

    fn moving(&self) -> bool {
        false
    }
}
