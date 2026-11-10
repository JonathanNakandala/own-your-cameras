//! The port each camera implements: the only per-camera surface of ONVIF. Everything else in this
//! crate is shared, so every camera speaks the same protocol.

use std::time::Instant;

/// Who the camera is. Built at runtime from the device; nothing here is ever a constant in the repo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    /// The board or hardware revision, `HardwareId`.
    pub hardware: String,
    /// The owner's name for the camera (`onvif://www.onvif.org/name/…`).
    pub name: String,
    pub hostname: String,
    /// The network interface ONVIF answers on, its MAC and IPv4 prefix length.
    pub interface: String,
    pub mac: Option<[u8; 6]>,
    pub prefix_len: u8,
    pub dhcp: bool,
    pub dns: Vec<String>,
    pub ntp: Vec<String>,
    /// The WS-Discovery endpoint reference, stable across reboots (see [`DeviceIdentity::endpoint`]).
    pub uuid: String,
}

/// The fixed namespace of camera endpoint UUIDs (a random constant, the same for every camera).
const ENDPOINT_NAMESPACE: [u8; 16] = [
    0x5c, 0x1e, 0x8a, 0x47, 0x3d, 0x02, 0x4b, 0x8e, 0x9f, 0x61, 0x2a, 0x7c, 0xd4, 0x90, 0x13, 0xe5,
];

impl DeviceIdentity {
    /// The name-based UUID of a camera with this MAC: the same after every reboot and reflash.
    pub fn endpoint(mac: &[u8; 6]) -> String {
        crate::codec::uuid_v5(&ENDPOINT_NAMESPACE, mac)
    }

    /// `aa:bb:cc:dd:ee:ff`, or empty.
    pub fn mac_text(&self) -> String {
        self.mac.map_or_else(String::new, |m| {
            m.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        })
    }

    /// The serial number clients see: the MAC without separators (Home Assistant needs one of them).
    pub fn serial(&self) -> String {
        self.mac_text().replace(':', "")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Profile {
    Baseline,
    Main,
    High,
}

impl H264Profile {
    pub fn onvif(self) -> &'static str {
        match self {
            Self::Baseline => "Baseline",
            Self::Main => "Main",
            Self::High => "High",
        }
    }
}

/// The true current settings of one H.264 encoder (read-only over ONVIF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoEncoding {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub gop: u32,
    pub profile: H264Profile,
}

/// One media profile: `main` or `sub`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProfile {
    /// `main` / `sub`: profile token, also the base of its configuration tokens.
    pub token: &'static str,
    /// The RTSP path `GetStreamUri` names (`main_pcmu`).
    pub stream_path: &'static str,
    pub video: VideoEncoding,
    /// G.711 µ-law 8 kHz microphone on track 1.
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub token: String,
    pub name: String,
}

/// Generic-space velocities, each -1.0..=1.0.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Velocity {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
}

/// A relative move. Pan and tilt are fractions of the **current field of view** (ONVIF `TranslationSpaceFov`:
/// 1.0 is one full picture width/height, positive is right/up), each -1.0..=1.0. Zoom is a fraction of the
/// lens travel (`TranslationGenericSpace`), -1.0..=1.0, positive is in. An absent axis does not move.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Relative {
    pub pan_tilt: Option<(f32, f32)>,
    pub zoom: Option<f32>,
}

/// An absolute move in `PositionGenericSpace`. Pan and tilt run -1.0..=1.0 across the head's whole travel,
/// zoom 0.0..=1.0 across the lens's. An absent axis does not move.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Absolute {
    pub pan_tilt: Option<(f32, f32)>,
    pub zoom: Option<f32>,
}

/// Where the head and lens are, in the same units as [`Absolute`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
}

/// What a camera's PTZ offers beyond continuous moves and presets; each is advertised in the ONVIF spaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PtzFeatures {
    /// [`PtzPort::relative_move`].
    pub relative: bool,
    /// [`PtzPort::absolute_move`].
    pub absolute: bool,
    /// [`PtzPort::pose`] reports a position in `GetStatus`.
    pub position: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtzError {
    NoSuchPreset,
    TooManyPresets,
    InvalidName,
    /// The camera does not do this kind of move.
    Unsupported,
    /// The motion owner refused or failed the request.
    Failed(String),
}

/// PTZ over the camera's own motion owner (the same code MQTT drives).
pub trait PtzPort: Send + Sync {
    fn zoom(&self) -> bool;
    fn max_presets(&self) -> usize;
    fn continuous_move(&self, velocity: Velocity) -> Result<(), PtzError>;
    fn stop(&self) -> Result<(), PtzError>;
    fn home(&self) -> Result<(), PtzError>;
    fn presets(&self) -> Vec<Preset>;
    fn goto_preset(&self, token: &str) -> Result<(), PtzError>;
    /// Saves the current position: over `token` when given, else as a new preset called `name`.
    /// Returns the preset's token.
    fn set_preset(&self, token: Option<&str>, name: Option<&str>) -> Result<String, PtzError>;
    fn remove_preset(&self, token: &str) -> Result<(), PtzError>;
    /// True from the moment a move is accepted until the head and lens have settled, queued moves included.
    fn moving(&self) -> bool;
    fn features(&self) -> PtzFeatures {
        PtzFeatures::default()
    }
    fn relative_move(&self, _: Relative) -> Result<(), PtzError> {
        Err(PtzError::Unsupported)
    }
    fn absolute_move(&self, _: Absolute) -> Result<(), PtzError> {
        Err(PtzError::Unsupported)
    }
    /// The confirmed pose, `None` while it is unknown (before homing, after a stop).
    fn pose(&self) -> Option<Pose> {
        None
    }
}

/// `IrCutFilter`: ON keeps the filter in the light path (day colour), OFF takes it out (night, IR), AUTO lets
/// the camera decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrCutFilter {
    On,
    Off,
    Auto,
}

/// The picture settings ONVIF's Imaging service reads and writes. Levels are in the camera's own units (the
/// ranges are in [`ImagingOptions`]); an absent field is unsupported when read and unchanged when written.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ImagingSettings {
    pub brightness: Option<f32>,
    pub saturation: Option<f32>,
    pub contrast: Option<f32>,
    pub sharpness: Option<f32>,
    pub ir_cut: Option<IrCutFilter>,
    /// Focus mode: true is `AUTO`, false `MANUAL`.
    pub auto_focus: Option<bool>,
}

/// What a camera lets Imaging change: the `(min, max)` of each level, and whether the IR-cut filter and the
/// focus mode can be set.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ImagingOptions {
    pub brightness: Option<(f32, f32)>,
    pub saturation: Option<(f32, f32)>,
    pub contrast: Option<(f32, f32)>,
    pub sharpness: Option<(f32, f32)>,
    pub ir_cut: bool,
    pub auto_focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagingError {
    /// A value outside its range, or a setting the camera does not have.
    Invalid(String),
    /// The camera's settings owner refused or failed the request.
    Failed(String),
}

/// Picture controls over the camera's own settings owner (the same code MQTT drives).
pub trait ImagingPort: Send + Sync {
    fn options(&self) -> ImagingOptions;
    /// The values in force; every supported field is present.
    fn settings(&self) -> ImagingSettings;
    /// Applies the present fields, all or none: a value out of range is [`ImagingError::Invalid`] and
    /// nothing changes.
    fn apply(&self, settings: ImagingSettings) -> Result<(), ImagingError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// The encoder is not up yet, is being rebuilt, or missed the deadline.
    NotReady,
    Failed(String),
}

pub trait OnvifCamera: Send + Sync {
    fn identity(&self) -> &DeviceIdentity;
    /// `main` first; its resolution is the video source's.
    fn profiles(&self) -> Vec<MediaProfile>;
    /// A speaker that takes the RTSP backchannel.
    fn audio_output(&self) -> bool;
    fn ptz(&self) -> Option<&dyn PtzPort>;
    /// Reboots the camera (ONVIF `SystemReboot`). `None`: this camera does not offer it. The reply may reach
    /// the client before the reboot starts.
    fn reboot(&self) -> Option<Result<(), String>> {
        None
    }
    /// Whether [`OnvifCamera::system_log`] has anything (cheap: it is asked for every capability document).
    fn keeps_log(&self) -> bool {
        false
    }
    /// The tail of the camera's own log, for `GetSystemLog`; `None` when it keeps none.
    fn system_log(&self) -> Option<String> {
        None
    }
    /// Picture controls; `None` when the camera offers none.
    fn imaging(&self) -> Option<&dyn ImagingPort> {
        None
    }
    /// `tns1:Device/Trigger/DigitalInput` input tokens (`call_button`, `door`…); empty when none.
    fn digital_inputs(&self) -> &[&'static str];
    /// One hardware JPEG of the main stream, finished before `deadline`.
    fn snapshot(&self, deadline: Instant) -> Result<Vec<u8>, SnapshotError>;
}
