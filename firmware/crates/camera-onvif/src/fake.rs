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
        }
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

pub struct FakeCamera {
    pub identity: DeviceIdentity,
    pub ptz: Option<FakePtz>,
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
