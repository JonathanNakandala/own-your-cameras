//! The E1 Zoom side of ONVIF: profiles from what the encoders run, PTZ and presets through the same motion
//! queue MQTT uses, and snapshots from the media thread's hardware JPEG. Everything else is
//! `camera_onvif`'s, shared with the other cameras.

use crate::media_tuning::Handoff;
use crate::mqtt::{Command, Motion, Move};
use crate::pipeline::{MainStream, PipelineConfig};
use camera_onvif::{
    DeviceIdentity, H264Profile, MediaProfile, OnvifCamera, Preset, PtzError, PtzPort,
    SnapshotError, Velocity, VideoEncoding,
};
use camera_transport::motion_control::{self, PresetName};
use std::sync::Arc;
use std::time::Instant;

pub struct E1Onvif {
    identity: DeviceIdentity,
    media: Arc<Handoff>,
    motion: Arc<Motion>,
    stock: PipelineConfig,
}

impl E1Onvif {
    pub fn new(identity: DeviceIdentity, media: Arc<Handoff>, motion: Arc<Motion>) -> Self {
        Self {
            identity,
            media,
            motion,
            stock: PipelineConfig::e1_zoom(),
        }
    }
}

fn encoding(m: MainStream) -> VideoEncoding {
    VideoEncoding {
        width: m.dim.w,
        height: m.dim.h,
        fps: m.fps,
        bitrate_kbps: m.bitrate_bps / 1000,
        // One key frame per second (`MainStream::spec`).
        gop: m.fps,
        profile: H264Profile::Main,
    }
}

fn failed(e: String) -> PtzError {
    PtzError::Failed(e)
}

/// ONVIF generic velocity (-1..1) as the motion contract's percentage of each axis's ceiling.
fn percent(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 100.0).round() as i16
}

impl OnvifCamera for E1Onvif {
    fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    fn profiles(&self) -> Vec<MediaProfile> {
        let main = self
            .media
            .applied()
            .1
            .unwrap_or_else(|| MainStream::of(&self.stock.streams[0]));
        vec![
            MediaProfile {
                token: "main",
                stream_path: camera_transport::MAIN_PCMU_PATH,
                video: encoding(main),
                audio: true,
            },
            MediaProfile {
                token: "sub",
                stream_path: camera_transport::SUB_PCMU_PATH,
                video: encoding(MainStream::of(&self.stock.streams[1])),
                audio: true,
            },
        ]
    }

    fn audio_output(&self) -> bool {
        true
    }

    fn ptz(&self) -> Option<&dyn PtzPort> {
        Some(self)
    }

    fn digital_inputs(&self) -> &[&'static str] {
        &[]
    }

    fn snapshot(&self, deadline: Instant) -> Result<Vec<u8>, SnapshotError> {
        self.media.snapshot(deadline).map_err(SnapshotError::Failed)
    }
}

impl PtzPort for E1Onvif {
    fn zoom(&self) -> bool {
        true
    }

    fn max_presets(&self) -> usize {
        crate::presets::MAX_PRESETS
    }

    fn continuous_move(&self, v: Velocity) -> Result<(), PtzError> {
        let velocity = motion_control::Velocity {
            pan: percent(v.pan),
            tilt: percent(v.tilt),
            zoom: percent(v.zoom),
        };
        let command = if velocity.is_zero() {
            Command::Move(Move::Stop)
        } else {
            Command::Velocity(velocity.validate().map_err(failed)?)
        };
        self.motion.submit(command).map_err(failed)
    }

    /// ONVIF Stop ends a move the ordinary way (deceleration), as the MQTT release does.
    fn stop(&self) -> Result<(), PtzError> {
        self.motion
            .submit(Command::Move(Move::Stop))
            .map_err(failed)
    }

    fn home(&self) -> Result<(), PtzError> {
        self.motion.submit(Command::Home).map_err(failed)
    }

    fn presets(&self) -> Vec<Preset> {
        self.motion
            .presets()
            .into_iter()
            .map(|name| Preset {
                token: name.clone(),
                name,
            })
            .collect()
    }

    fn goto_preset(&self, token: &str) -> Result<(), PtzError> {
        if !self.motion.presets().iter().any(|n| n == token) {
            return Err(PtzError::NoSuchPreset);
        }
        self.motion
            .submit(Command::Preset(token.into()))
            .map_err(failed)
    }

    /// The preset's token is its name: saving under an existing token overwrites it, a new name creates it.
    fn set_preset(&self, token: Option<&str>, name: Option<&str>) -> Result<String, PtzError> {
        let name = token.or(name).ok_or(PtzError::InvalidName)?;
        PresetName::parse(name).map_err(|_| PtzError::InvalidName)?;
        let presets = self.motion.presets();
        if token.is_some() && !presets.iter().any(|n| n == name) {
            return Err(PtzError::NoSuchPreset);
        }
        if !presets.iter().any(|n| n == name) && presets.len() >= crate::presets::MAX_PRESETS {
            return Err(PtzError::TooManyPresets);
        }
        self.motion
            .submit(Command::SavePreset(name.into()))
            .map_err(failed)?;
        Ok(name.into())
    }

    fn remove_preset(&self, token: &str) -> Result<(), PtzError> {
        if !self.motion.presets().iter().any(|n| n == token) {
            return Err(PtzError::NoSuchPreset);
        }
        self.motion
            .submit(Command::RemovePreset(token.into()))
            .map_err(failed)
    }

    fn moving(&self) -> bool {
        false
    }
}
