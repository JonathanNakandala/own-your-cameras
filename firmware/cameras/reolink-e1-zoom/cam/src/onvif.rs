//! The E1 Zoom side of ONVIF: profiles from what the encoders run, PTZ and presets through the same motion
//! queue MQTT uses, and snapshots from the media thread's hardware JPEG. Everything else is
//! `camera_onvif`'s, shared with the other cameras.

use crate::media_tuning::Handoff;
use crate::mqtt::{Command, Motion, Move};
use crate::pipeline::{MainStream, PipelineConfig};
use camera_onvif::{
    Absolute, DeviceIdentity, H264Profile, ImagingError, ImagingOptions, ImagingPort,
    ImagingSettings, IrCutFilter, MediaProfile, OnvifCamera, Pose, Preset, PtzError, PtzFeatures,
    PtzPort, Relative, SnapshotError, Velocity, VideoEncoding,
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

/// The preset the camera goes to at startup and for the Home button (`mqtt/mod.rs`).
const HOME_PRESET: &str = "Default";

/// `SendAuxiliaryCommand` strings and the `ir` mode each sets: the IR lamp is the day/night mode, the same
/// switch as Imaging's `IrCutFilter` (On is night with the LEDs, Off is day colour).
const AUX_COMMANDS: [&str; 3] = ["tt:IRLamp|On", "tt:IRLamp|Off", "tt:IRLamp|Auto"];
const AUX_IR_LAMP: [(&str, &str); 3] = [
    ("tt:IRLamp|On", "night"),
    ("tt:IRLamp|Off", "day"),
    ("tt:IRLamp|Auto", "auto"),
];

/// How much of the log `GetSystemLog` returns: the log is capped at 256 KiB and a SOAP reply should not be.
const LOG_TAIL: usize = 32 * 1024;

/// The last `max` bytes of `log` as text, starting at a line: a cut through a line or a character is dropped.
fn tail(log: &[u8], max: usize) -> String {
    let cut = log.len().saturating_sub(max);
    let kept = &log[cut..];
    // A cut that falls inside a line drops the rest of that line; one that falls on a line start keeps it.
    let mid_line = cut > 0 && log[cut - 1] != b'\n';
    let kept = match (mid_line, kept.iter().position(|b| *b == b'\n')) {
        (true, Some(i)) => &kept[i + 1..],
        _ => kept,
    };
    String::from_utf8_lossy(kept).into_owned()
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

    fn imaging(&self) -> Option<&dyn ImagingPort> {
        Some(self)
    }

    /// The same path as the Reboot button: settings are saved first, then the camera restarts.
    fn reboot(&self) -> Option<Result<(), String>> {
        Some(self.motion.submit(Command::Reboot))
    }

    fn keeps_log(&self) -> bool {
        true
    }

    fn system_log(&self) -> Option<String> {
        let bytes = crate::files::read_regular(
            std::path::Path::new(crate::logcap::LOG_FILE),
            crate::logcap::CAP_BYTES + 1024,
        )
        .ok()?;
        Some(tail(&bytes, LOG_TAIL))
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
            set_home: true,
        }
    }

    /// Home is the `Default` preset: the camera goes there at startup and for the Home button, so saving
    /// it moves home. An existing `Default` is overwritten; with none, the preset table needs room.
    fn set_home(&self) -> Result<(), PtzError> {
        self.set_preset(None, Some(HOME_PRESET)).map(|_| ())
    }

    fn aux_commands(&self) -> &'static [&'static str] {
        &AUX_COMMANDS
    }

    fn aux_command(&self, command: &str) -> Result<String, PtzError> {
        let (_, mode) = AUX_IR_LAMP
            .iter()
            .find(|(c, _)| *c == command)
            .ok_or(PtzError::Unsupported)?;
        self.motion
            .submit(Command::Setting("ir", (*mode).into()))
            .map_err(failed)?;
        Ok("OK".into())
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

/// The picture levels ONVIF can set, as the settings that carry them.
const LEVELS: [&str; 4] = [
    "isp_brightness",
    "isp_saturation",
    "isp_contrast",
    "isp_sharpness",
];

/// The `(min, max)` of a numeric setting, from the one table MQTT and the files use.
fn range_of(key: &str) -> Option<(f32, f32)> {
    match crate::settings::spec(key)?.spec.kind {
        camera_transport::settings::Kind::Number { min, max, .. } => Some((min as f32, max as f32)),
        _ => None,
    }
}

impl E1Onvif {
    /// A level validated against its setting's range, as the `Command::Setting` that applies it.
    fn level(key: &'static str, value: f32) -> Result<Command, ImagingError> {
        let spec = crate::settings::spec(key)
            .ok_or_else(|| ImagingError::Invalid(format!("no {key} setting")))?;
        let text = value.round().to_string();
        spec.parse(&text).map_err(|_| {
            ImagingError::Invalid(format!("{} is outside its range", spec.spec.name))
        })?;
        Ok(Command::Setting(spec.spec.key, text))
    }

    fn read(&self, key: &str) -> Option<f32> {
        self.motion.state(key)?.parse().ok()
    }
}

impl ImagingPort for E1Onvif {
    fn options(&self) -> ImagingOptions {
        ImagingOptions {
            brightness: range_of("isp_brightness"),
            saturation: range_of("isp_saturation"),
            contrast: range_of("isp_contrast"),
            sharpness: range_of("isp_sharpness"),
            ir_cut: true,
            auto_focus: true,
        }
    }

    /// What the camera reports it runs (the published state), not what was last asked for.
    fn settings(&self) -> ImagingSettings {
        ImagingSettings {
            brightness: self.read("isp_brightness"),
            saturation: self.read("isp_saturation"),
            contrast: self.read("isp_contrast"),
            sharpness: self.read("isp_sharpness"),
            // The filter is in (ON) for day colour and out (OFF) at night.
            ir_cut: self.motion.state("ir").and_then(|m| match m.as_str() {
                "day" => Some(IrCutFilter::On),
                "night" => Some(IrCutFilter::Off),
                "auto" => Some(IrCutFilter::Auto),
                _ => None,
            }),
            auto_focus: self.motion.state("autofocus").map(|s| s == "ON"),
        }
    }

    fn apply(&self, s: ImagingSettings) -> Result<(), ImagingError> {
        let mut commands = Vec::new();
        for (key, value) in
            LEVELS
                .into_iter()
                .zip([s.brightness, s.saturation, s.contrast, s.sharpness])
        {
            if let Some(v) = value {
                commands.push(Self::level(key, v)?);
            }
        }
        if let Some(mode) = s.ir_cut {
            let text = match mode {
                IrCutFilter::On => "day",
                IrCutFilter::Off => "night",
                IrCutFilter::Auto => "auto",
            };
            commands.push(Command::Setting("ir", text.into()));
        }
        if let Some(auto) = s.auto_focus {
            commands.push(Command::Setting(
                "autofocus",
                if auto { "ON" } else { "OFF" }.into(),
            ));
        }
        // Everything was validated first: a bad value changes nothing.
        for command in commands {
            self.motion.submit(command).map_err(ImagingError::Failed)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> E1Onvif {
        E1Onvif::new(
            DeviceIdentity::default(),
            Arc::new(Handoff::default()),
            Arc::new(Motion::default()),
        )
    }

    #[test]
    fn the_ir_lamp_commands_are_the_ir_modes_and_nothing_else_is_accepted() {
        let c = camera();
        assert_eq!(
            c.aux_commands(),
            ["tt:IRLamp|On", "tt:IRLamp|Off", "tt:IRLamp|Auto"]
        );
        // The list and the table that runs the commands cannot drift apart.
        assert_eq!(AUX_COMMANDS, AUX_IR_LAMP.map(|(command, _)| command));
        // Every listed mode is a real value of the `ir` setting.
        let ir = crate::settings::spec("ir").unwrap();
        for (_, mode) in AUX_IR_LAMP {
            assert!(ir.parse(mode).is_ok(), "{mode}");
        }
        assert_eq!(c.aux_command("tt:Wiper|On"), Err(PtzError::Unsupported));
        // Before the motion owner runs the queue does not exist, and that is reported.
        assert!(matches!(
            c.aux_command("tt:IRLamp|On"),
            Err(PtzError::Failed(_))
        ));
    }

    #[test]
    fn setting_home_is_saving_the_default_preset() {
        assert!(camera().features().set_home);
        assert!(matches!(camera().set_home(), Err(PtzError::Failed(_))));
        // The name is a valid preset name, or saving home would always be refused.
        assert!(PresetName::parse(HOME_PRESET).is_ok());
    }

    #[test]
    fn the_log_tail_starts_at_a_line_and_keeps_a_short_log_whole() {
        assert_eq!(tail(b"one\ntwo\n", 100), "one\ntwo\n");
        assert_eq!(tail(b"", 10), "");
        // Cut mid-line: the partial first line goes.
        assert_eq!(tail(b"alpha\nbeta\ngamma\n", 9), "gamma\n");
        // Cut exactly on a line start keeps that line; one byte further in drops it.
        assert_eq!(tail(b"alpha\nbeta\n", 5), "beta\n");
        assert_eq!(tail(b"alpha\nbeta\n", 4), "");
        // A cut through a multi-byte character cannot yield invalid text, even with no newline to start at.
        let log = "é".repeat(10);
        let out = tail(log.as_bytes(), 5);
        assert!(out.chars().all(|c| c == 'é' || c == '\u{fffd}'), "{out:?}");
    }

    #[test]
    fn a_reboot_without_a_running_owner_is_reported_not_swallowed() {
        // The motion owner starts with MQTT; before that the queue does not exist.
        let err = camera().reboot().unwrap().unwrap_err();
        assert!(err.contains("not running"), "{err}");
        assert!(camera().keeps_log());
    }

    #[test]
    fn the_imaging_ranges_are_the_settings_ranges() {
        let o = camera().options();
        assert_eq!(o.brightness, Some((0.0, 200.0)));
        assert_eq!(o.sharpness, Some((0.0, 200.0)));
        assert!(o.ir_cut && o.auto_focus);
    }

    #[test]
    fn levels_are_validated_into_the_settings_that_carry_them() {
        assert_eq!(
            E1Onvif::level("isp_contrast", 150.4).unwrap(),
            Command::Setting("isp_contrast", "150".into())
        );
        for bad in [200.6, -0.6, f32::NAN] {
            assert!(
                matches!(
                    E1Onvif::level("isp_contrast", bad),
                    Err(ImagingError::Invalid(_))
                ),
                "{bad}"
            );
        }
        // A bad value anywhere in a request stops the whole request before anything is queued.
        let bad = ImagingSettings {
            sharpness: Some(500.0),
            ..Default::default()
        };
        assert!(matches!(camera().apply(bad), Err(ImagingError::Invalid(_))));
    }

    #[test]
    fn settings_read_back_what_the_camera_published() {
        let c = camera();
        assert_eq!(
            c.settings(),
            ImagingSettings::default(),
            "nothing published yet"
        );
        c.motion.set_states(
            &[
                ("isp_brightness", "120"),
                ("ir", "night"),
                ("autofocus", "OFF"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into(),
        );
        let s = c.settings();
        assert_eq!(s.brightness, Some(120.0));
        assert_eq!(s.ir_cut, Some(IrCutFilter::Off));
        assert_eq!(s.auto_focus, Some(false));
        assert_eq!(s.contrast, None);
    }

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
