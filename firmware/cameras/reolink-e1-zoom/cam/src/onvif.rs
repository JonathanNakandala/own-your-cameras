//! The E1 Zoom side of ONVIF: profiles from what the encoders run, PTZ and presets through the same motion
//! queue MQTT uses, and snapshots from the media thread's hardware JPEG. Everything else is
//! `camera_onvif`'s, shared with the other cameras.

use crate::media_tuning::Handoff;
use crate::mqtt::{Command, Motion, Move};
use crate::pipeline::{MainStream, PipelineConfig};
use camera_onvif::{
    Absolute, DeviceIdentity, H264Profile, MediaProfile, OnvifCamera, Pose, Preset, PtzError,
    PtzFeatures, PtzPort, Relative, SnapshotError, Velocity, VideoEncoding,
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

/// ONVIF -1..1 or 0..1 across a travel of `max` steps, from a position in steps.
fn unit(steps: u32, max: u32, signed: bool) -> f32 {
    let u = steps as f32 / max as f32;
    if signed { u * 2.0 - 1.0 } else { u }
}

/// The inverse of [`unit`]: a value already clamped by the service becomes steps in `0..=max`.
fn steps(value: f32, max: u32, signed: bool) -> u32 {
    let u = if signed { (value + 1.0) / 2.0 } else { value };
    (u.clamp(0.0, 1.0) * max as f32).round() as u32
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

    /// Moving from the moment a move is queued until the head and lens settle (a hold still running counts).
    fn moving(&self) -> bool {
        self.motion.moving()
    }

    fn features(&self) -> PtzFeatures {
        PtzFeatures {
            relative: true,
            absolute: true,
            position: true,
        }
    }

    /// Pan and tilt together by fractions of the picture, one concurrent move on the measured lens map
    /// (`Optics::shift_fov`); zoom is a fraction of the lens travel, applied after.
    fn relative_move(&self, r: Relative) -> Result<(), PtzError> {
        if let Some((x, y)) = r.pan_tilt {
            let milli = |v: f32| (v.clamp(-1.0, 1.0) * 1000.0).round() as i16;
            self.motion
                .submit(Command::Fov {
                    x: milli(x),
                    y: milli(y),
                })
                .map_err(failed)?;
        }
        if let Some(z) = r.zoom {
            let by = (z.clamp(-1.0, 1.0) * crate::mqtt::ZOOM_MAX as f32).round() as i32;
            self.motion.submit(Command::ZoomBy(by)).map_err(failed)?;
        }
        Ok(())
    }

    /// Pan and tilt across the head's whole travel in one move, zoom across the lens's. Raw head
    /// coordinates: the picture's mirror and flip do not apply.
    fn absolute_move(&self, a: Absolute) -> Result<(), PtzError> {
        if let Some((x, y)) = a.pan_tilt {
            self.motion
                .submit(Command::PanTilt(
                    steps(x, crate::ptz::PAN_MAX, true),
                    steps(y, crate::ptz::TILT_MAX, true),
                ))
                .map_err(failed)?;
        }
        if let Some(z) = a.zoom {
            self.motion
                .submit(Command::Zoom(steps(z, crate::mqtt::ZOOM_MAX, false)))
                .map_err(failed)?;
        }
        Ok(())
    }

    fn pose(&self) -> Option<Pose> {
        let p = self.motion.pose()?;
        Some(Pose {
            pan: unit(p.pan, crate::ptz::PAN_MAX, true),
            tilt: unit(p.tilt, crate::ptz::TILT_MAX, true),
            zoom: unit(p.zoom, crate::mqtt::ZOOM_MAX, false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onvif_units_map_across_the_travel_and_back() {
        let (pan, tilt, zoom) = (
            crate::ptz::PAN_MAX,
            crate::ptz::TILT_MAX,
            crate::mqtt::ZOOM_MAX,
        );
        assert_eq!(steps(-1.0, pan, true), 0);
        assert_eq!(steps(0.0, pan, true), pan / 2);
        assert_eq!(steps(1.0, tilt, true), tilt);
        assert_eq!(steps(0.0, zoom, false), 0);
        assert_eq!(steps(1.0, zoom, false), zoom);
        // Out-of-range values clamp rather than wrap.
        assert_eq!(steps(7.0, pan, true), pan);
        assert_eq!(steps(-7.0, zoom, false), 0);
        for at in [0, 1, pan / 3, pan] {
            let back = steps(unit(at, pan, true), pan, true);
            assert!(back.abs_diff(at) <= 1, "{at} -> {back}");
        }
        assert_eq!(unit(zoom, zoom, false), 1.0);
    }
}
