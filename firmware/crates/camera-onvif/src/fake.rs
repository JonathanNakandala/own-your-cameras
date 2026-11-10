//! Test doubles: a camera with two profiles, an optional PTZ that records what it was asked, digital
//! inputs and a counting snapshot encoder. Identities are placeholders.

use crate::camera::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct FakePtz {
    pub log: Mutex<Vec<String>>,
    pub presets: Mutex<Vec<Preset>>,
    pub moving: AtomicBool,
}

impl FakePtz {
    pub fn new() -> Self {
        Self {
            log: Mutex::new(Vec::new()),
            presets: Mutex::new(vec![Preset {
                token: "1".into(),
                name: "Default".into(),
            }]),
            moving: AtomicBool::new(false),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn note(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }
}

impl PtzPort for FakePtz {
    fn zoom(&self) -> bool {
        true
    }
    fn max_presets(&self) -> usize {
        3
    }
    fn continuous_move(&self, v: Velocity) -> Result<(), PtzError> {
        self.moving.store(true, Ordering::Relaxed);
        self.note(format!("move {} {} {}", v.pan, v.tilt, v.zoom));
        Ok(())
    }
    fn stop(&self) -> Result<(), PtzError> {
        self.moving.store(false, Ordering::Relaxed);
        self.note("stop".into());
        Ok(())
    }
    fn home(&self) -> Result<(), PtzError> {
        self.note("home".into());
        Ok(())
    }
    fn presets(&self) -> Vec<Preset> {
        self.presets.lock().unwrap().clone()
    }
    fn goto_preset(&self, token: &str) -> Result<(), PtzError> {
        if !self.presets().iter().any(|p| p.token == token) {
            return Err(PtzError::NoSuchPreset);
        }
        self.note(format!("goto {token}"));
        Ok(())
    }
    fn set_preset(&self, token: Option<&str>, name: Option<&str>) -> Result<String, PtzError> {
        let mut presets = self.presets.lock().unwrap();
        if let Some(token) = token {
            return presets
                .iter()
                .any(|p| p.token == token)
                .then(|| token.to_string())
                .ok_or(PtzError::NoSuchPreset);
        }
        if presets.len() >= 3 {
            return Err(PtzError::TooManyPresets);
        }
        let name = name.ok_or(PtzError::InvalidName)?;
        let token = (presets.len() + 1).to_string();
        presets.push(Preset {
            token: token.clone(),
            name: name.into(),
        });
        Ok(token)
    }
    fn remove_preset(&self, token: &str) -> Result<(), PtzError> {
        let mut presets = self.presets.lock().unwrap();
        let before = presets.len();
        presets.retain(|p| p.token != token);
        (presets.len() < before)
            .then_some(())
            .ok_or(PtzError::NoSuchPreset)
    }
    fn moving(&self) -> bool {
        self.moving.load(Ordering::Relaxed)
    }
    fn features(&self) -> crate::PtzFeatures {
        crate::PtzFeatures {
            relative: true,
            absolute: true,
            position: true,
            set_home: true,
        }
    }
    fn set_home(&self) -> Result<(), PtzError> {
        self.note("sethome".into());
        Ok(())
    }
    fn aux_commands(&self) -> &'static [&'static str] {
        &["tt:IRLamp|On", "tt:IRLamp|Off"]
    }
    fn aux_command(&self, command: &str) -> Result<String, PtzError> {
        self.note(format!("aux {command}"));
        Ok("OK".into())
    }
    fn relative_move(&self, r: crate::Relative) -> Result<(), PtzError> {
        self.note(format!("relative {:?} {:?}", r.pan_tilt, r.zoom));
        Ok(())
    }
    fn absolute_move(&self, a: crate::Absolute) -> Result<(), PtzError> {
        self.note(format!("absolute {:?} {:?}", a.pan_tilt, a.zoom));
        Ok(())
    }
    fn pose(&self) -> Option<crate::Pose> {
        Some(crate::Pose {
            pan: 0.25,
            tilt: -0.5,
            zoom: 0.75,
        })
    }
}

/// A camera with brightness 0..200 and the IR-cut and focus-mode switches; records what it was asked.
pub struct FakeImaging {
    pub log: Mutex<Vec<String>>,
    pub settings: Mutex<crate::ImagingSettings>,
}

impl FakeImaging {
    pub fn new() -> Self {
        Self {
            log: Mutex::new(Vec::new()),
            settings: Mutex::new(crate::ImagingSettings {
                brightness: Some(100.0),
                saturation: Some(100.0),
                contrast: Some(100.0),
                sharpness: Some(60.0),
                ir_cut: Some(crate::IrCutFilter::Auto),
                auto_focus: Some(true),
            }),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Default for FakeImaging {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::ImagingPort for FakeImaging {
    fn options(&self) -> crate::ImagingOptions {
        crate::ImagingOptions {
            brightness: Some((0.0, 200.0)),
            saturation: Some((0.0, 200.0)),
            contrast: Some((0.0, 200.0)),
            sharpness: Some((0.0, 200.0)),
            ir_cut: true,
            auto_focus: true,
        }
    }
    fn settings(&self) -> crate::ImagingSettings {
        *self.settings.lock().unwrap()
    }
    fn apply(&self, s: crate::ImagingSettings) -> Result<(), crate::ImagingError> {
        if s.brightness.is_some_and(|b| !(0.0..=200.0).contains(&b)) {
            return Err(crate::ImagingError::Invalid(
                "Brightness is 0 to 200".into(),
            ));
        }
        self.log.lock().unwrap().push(format!("{s:?}"));
        let mut now = self.settings.lock().unwrap();
        now.brightness = s.brightness.or(now.brightness);
        now.ir_cut = s.ir_cut.or(now.ir_cut);
        now.auto_focus = s.auto_focus.or(now.auto_focus);
        Ok(())
    }
}

/// The camera's system side: counts reboots and keeps a one-line log.
pub struct FakeSystem {
    pub reboots: AtomicUsize,
    pub relays: Mutex<Vec<String>>,
}

pub struct FakeCamera {
    pub identity: DeviceIdentity,
    pub ptz: Option<FakePtz>,
    pub imaging: Option<FakeImaging>,
    pub system: Option<FakeSystem>,
    pub inputs: Vec<&'static str>,
    pub encodes: AtomicUsize,
    pub encode_time: Duration,
}

impl FakeCamera {
    pub fn new(ptz: bool, inputs: &[&'static str]) -> Self {
        Self {
            identity: DeviceIdentity {
                manufacturer: "Maker".into(),
                model: "Model X".into(),
                firmware: "test".into(),
                hardware: "rev1".into(),
                name: "Test camera".into(),
                hostname: "camera".into(),
                interface: "eth0".into(),
                mac: Some([0x02, 0, 0, 0, 0, 1]),
                prefix_len: 24,
                dhcp: true,
                dns: vec!["192.0.2.53".into()],
                ntp: vec!["pool.ntp.org".into()],
                uuid: DeviceIdentity::endpoint(&[0x02, 0, 0, 0, 0, 1]),
            },
            ptz: ptz.then(FakePtz::new),
            imaging: ptz.then(FakeImaging::new),
            system: ptz.then(|| FakeSystem {
                reboots: AtomicUsize::new(0),
                relays: Mutex::new(Vec::new()),
            }),
            inputs: inputs.to_vec(),
            encodes: AtomicUsize::new(0),
            encode_time: Duration::from_millis(50),
        }
    }
}

impl OnvifCamera for FakeCamera {
    fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }
    fn profiles(&self) -> Vec<MediaProfile> {
        vec![
            MediaProfile {
                token: "main",
                stream_path: "main_pcmu",
                video: VideoEncoding {
                    width: 2560,
                    height: 1440,
                    fps: 15,
                    bitrate_kbps: 2048,
                    gop: 30,
                    profile: H264Profile::Main,
                },
                audio: true,
            },
            MediaProfile {
                token: "sub",
                stream_path: "sub_pcmu",
                video: VideoEncoding {
                    width: 640,
                    height: 360,
                    fps: 15,
                    bitrate_kbps: 256,
                    gop: 30,
                    profile: H264Profile::Main,
                },
                audio: true,
            },
        ]
    }
    fn audio_output(&self) -> bool {
        true
    }
    fn ptz(&self) -> Option<&dyn PtzPort> {
        self.ptz.as_ref().map(|p| p as &dyn PtzPort)
    }
    fn relay_outputs(&self) -> &'static [&'static str] {
        if self.system.is_some() {
            &["status_led", "ir_leds"]
        } else {
            &[]
        }
    }
    fn set_relay_output(&self, token: &str, active: bool) -> Result<(), crate::RelayError> {
        let system = self.system.as_ref().ok_or(crate::RelayError::NoSuchRelay)?;
        if !self.relay_outputs().contains(&token) {
            return Err(crate::RelayError::NoSuchRelay);
        }
        system
            .relays
            .lock()
            .unwrap()
            .push(format!("{token} {active}"));
        Ok(())
    }
    fn reboot(&self) -> Option<Result<(), String>> {
        let system = self.system.as_ref()?;
        system.reboots.fetch_add(1, Ordering::SeqCst);
        Some(Ok(()))
    }
    fn keeps_log(&self) -> bool {
        self.system.is_some()
    }
    fn system_log(&self) -> Option<String> {
        self.system
            .as_ref()
            .map(|_| "talk: session ended <ok> & done\n".into())
    }
    fn imaging(&self) -> Option<&dyn crate::ImagingPort> {
        self.imaging.as_ref().map(|i| i as &dyn crate::ImagingPort)
    }
    fn digital_inputs(&self) -> &[&'static str] {
        &self.inputs
    }
    fn snapshot(&self, deadline: Instant) -> Result<Vec<u8>, SnapshotError> {
        self.encodes.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.encode_time);
        if Instant::now() > deadline {
            return Err(SnapshotError::NotReady);
        }
        Ok(vec![0xff, 0xd8, 0xff, 0xd9])
    }
}
