//! The one owner of the runtime settings: their schema, validation, the values in force and their
//! persistence in `/mnt/para/cam-settings.toml` (written only by the daemon).
//!
//! [`specs`] is the only list of keys. File parsing, MQTT payload validation, Home Assistant discovery
//! and the `docs/settings.md` table all derive from it. A bad file or field never fails startup: it
//! falls back to the field's default and is reported as a [`Problem`].
//!
//! The values in force and their file are the shared `camera_transport::settings::Values` with the
//! TOML codec: every value is an `i32` (a switch 0/1, a select its option index). The apply groups,
//! the stock first-run defaults and the rules between fields stay here.

use std::collections::BTreeMap;
use std::io;
use std::sync::LazyLock;
use std::time::Duration;

use crate::pipeline::{MAIN_MAX_BITRATE, MAIN_SIZES, MainStream};
use crate::ptz::{self, Profile};
use camera_transport::settings::{self as shared, TomlCodec, Values};
pub use camera_transport::settings::{Kind, Problem, Rejected, SAVE_WITHIN, Spec};
use hdal::audio::{Aec, Anr, Tuning};

/// Where the daemon keeps the settings.
pub const PATH: &str = "/mnt/para/cam-settings.toml";
const MAX_FILE: u64 = shared::MAX_FILE as u64;
/// What the daemon writes first.
const HEADER: &str = "# Written by cam; edits while it runs are overwritten.\n";

/// Which owner applies a setting. The hardware thread re-applies the whole group on any change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Media thread: microphone, speaker, AEC, ANR.
    Audio,
    /// Media thread: main encoder path.
    MainStream,
    /// ISP common item 44.
    Orientation,
    /// ISP image-quality item (`crate::isp_iq`).
    Picture,
    Ir,
    Led,
    /// Pan/tilt profile, from the next move.
    Motion,
    /// Zoom motor speed (AN41908 driver), from the next zoom move.
    Lens,
    /// Autofocus triggers and watchdog (`crate::zoom::af`).
    Autofocus,
}

/// Where a setting shows in Home Assistant: its section device, linked to the camera device. Derived from the
/// apply group, which stays what it is (who applies the value); the status LED stays on the camera device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Audio,
    Picture,
    MainStream,
    NightVision,
    PanTilt,
    Lens,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Self::Audio,
        Self::Picture,
        Self::MainStream,
        Self::NightVision,
        Self::PanTilt,
        Self::Lens,
    ];

    pub fn of(group: Group) -> Option<Self> {
        match group {
            Group::Audio => Some(Self::Audio),
            Group::Orientation | Group::Picture => Some(Self::Picture),
            Group::MainStream => Some(Self::MainStream),
            Group::Ir => Some(Self::NightVision),
            Group::Led => None,
            Group::Motion => Some(Self::PanTilt),
            Group::Lens | Group::Autofocus => Some(Self::Lens),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Picture => "Picture",
            Self::MainStream => "Main stream",
            Self::NightVision => "Night vision",
            Self::PanTilt => "Pan & tilt",
            Self::Lens => "Lens",
        }
    }

    /// The device identifier suffix: lowercase, letters, digits and underscores.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Picture => "picture",
            Self::MainStream => "main_stream",
            Self::NightVision => "night_vision",
            Self::PanTilt => "pan_tilt",
            Self::Lens => "lens",
        }
    }
}

/// One setting: the shared spec (its slot is its place in [`specs`]), who applies it, and the stock
/// attribute that seeds its first-run value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub spec: Spec,
    pub group: Group,
    /// Attribute of `/mnt/para/0_isp.cfg`'s `<channel>` read once for the first-run default; the
    /// spec's default otherwise.
    pub stock: Option<&'static str>,
}

impl Setting {
    pub const fn new(spec: Spec, group: Group) -> Self {
        Self {
            spec,
            group,
            stock: None,
        }
    }

    /// The Home Assistant section device this setting sits on; `None` is the camera device.
    pub fn section(&self) -> Option<Section> {
        Section::of(self.group)
    }

    /// Parses an MQTT payload: a whole number, `ON`/`OFF` or an option.
    pub fn parse(&self, text: &str) -> Result<i32, String> {
        self.spec.parse(text).map_err(|e| e.0)
    }
}

const fn number(
    key: &'static str,
    name: &'static str,
    group: Group,
    range: (i32, i32),
    unit: Option<&'static str>,
    default: i32,
) -> Setting {
    Setting::new(Spec::number(0, key, name, range, unit, default), group)
}

const fn switch(key: &'static str, name: &'static str, group: Group, on: bool) -> Setting {
    Setting::new(Spec::switch(0, key, name, on), group)
}

const fn select(
    key: &'static str,
    name: &'static str,
    group: Group,
    options: &'static [&'static str],
    default: i32,
) -> Setting {
    Setting::new(Spec::select(0, key, name, options, default), group)
}

pub const IR_MODES: &[&str] = &["auto", "day", "night"];
pub const MAIN_RESOLUTIONS: &[&str] = &["2592x1944", "1920x1440", "1280x960"];
/// The speeds the AN41908 driver distinguishes (`crate::zoom::ZOOM_SPEEDS`); full range 24 s at 20,
/// 5.5 s at 80 (measured, `docs/hardware/zoom-focus.md`).
pub const ZOOM_SPEED_OPTIONS: &[&str] = &["20", "40", "60", "80", "100"];

/// ANR parameter ranges accepted by the driver (`docs/settings.md`, established on the camera).
pub const ANR_SUPPRESS: (i32, i32) = (3, 35);
pub const ANR_HPF: (i32, i32) = (0, 1000);
pub const ANR_BIAS: (i32, i32) = (1, 9);
/// Main frame-rate range: the sensor delivers 30 fps.
pub const MAIN_FPS: (i32, i32) = (5, 30);
/// How far behind the head the dashboard picture runs: a tap is resolved against the pose this long before it
/// arrived. Measured on the camera's go2rtc stream (`docs/hardware/position3d.md`).
pub const TAP_DELAY_DEFAULT_MS: i32 = 500;

const BASE: &[Setting] = &[
    number(
        "mic_volume",
        "Mic volume",
        Group::Audio,
        (0, 100),
        Some("%"),
        100,
    ),
    number(
        "speaker_volume",
        "Speaker volume",
        Group::Audio,
        (0, 100),
        Some("%"),
        100,
    ),
    switch("aec", "Echo cancellation", Group::Audio, false),
    number(
        "aec_noise_level",
        "AEC noise level",
        Group::Audio,
        (-60, -3),
        Some("dB"),
        -60,
    ),
    number(
        "aec_echo_level",
        "AEC echo level",
        Group::Audio,
        (-60, -30),
        Some("dB"),
        -60,
    ),
    number(
        "aec_notch_radius",
        "AEC notch radius",
        Group::Audio,
        (0, 1000),
        None,
        992,
    ),
    switch(
        "aec_leak_estimate",
        "AEC leak estimate",
        Group::Audio,
        false,
    ),
    number(
        "aec_leak_value",
        "AEC leak value",
        Group::Audio,
        (25, 99),
        None,
        99,
    ),
    switch("anr", "Noise reduction", Group::Audio, false),
    number(
        "anr_suppress_level",
        "ANR suppress level",
        Group::Audio,
        ANR_SUPPRESS,
        None,
        30,
    ),
    number(
        "anr_hpf_cutoff",
        "ANR high-pass cutoff",
        Group::Audio,
        ANR_HPF,
        Some("Hz"),
        160,
    ),
    number(
        "anr_bias_sensitive",
        "ANR bias sensitivity",
        Group::Audio,
        ANR_BIAS,
        None,
        9,
    ),
    Setting {
        stock: Some("mirror"),
        ..switch("mirror", "Mirror", Group::Orientation, false)
    },
    Setting {
        stock: Some("flip"),
        ..switch("flip", "Flip", Group::Orientation, false)
    },
    number(
        "ir_to_night",
        "IR to-night threshold",
        Group::Ir,
        (0, 4095),
        None,
        2300,
    ),
    number(
        "ir_to_day",
        "IR to-day threshold",
        Group::Ir,
        (0, 4095),
        None,
        1900,
    ),
    number(
        "ir_hold",
        "IR switch hold",
        Group::Ir,
        (1, 60),
        Some("s"),
        4,
    ),
    switch("ir_leds_at_night", "IR LEDs at night", Group::Ir, true),
    select("ir", "IR mode", Group::Ir, IR_MODES, 0),
    switch("led", "Status LED", Group::Led, true),
    number(
        "ptz_speed",
        "Speed",
        Group::Motion,
        (100, ptz::MAX_PPS as i32),
        Some("pps"),
        ptz::MAX_PPS as i32,
    ),
    number(
        "ptz_start_speed",
        "Start speed",
        Group::Motion,
        (100, ptz::MAX_PPS as i32),
        Some("pps"),
        300,
    ),
    number(
        "ptz_ramp",
        "Ramp",
        Group::Motion,
        (0, 3000),
        Some("ms"),
        1500,
    ),
    number(
        "ptz_hold_timeout",
        "Hold timeout",
        Group::Motion,
        (1, 60),
        Some("s"),
        20,
    ),
    number(
        "tap_delay",
        "Tap delay",
        Group::Motion,
        (0, 3000),
        Some("ms"),
        TAP_DELAY_DEFAULT_MS,
    ),
    select(
        "zoom_speed",
        "Zoom speed",
        Group::Lens,
        ZOOM_SPEED_OPTIONS,
        3,
    ),
    switch("autofocus", "Autofocus", Group::Autofocus, true),
    number(
        "af_watch_hold",
        "Autofocus watchdog hold",
        Group::Autofocus,
        (10, 300),
        Some("s"),
        30,
    ),
    number(
        "af_watch_drop",
        "Autofocus watchdog drop",
        Group::Autofocus,
        (20, 90),
        Some("%"),
        60,
    ),
    number(
        "af_move_drop",
        "Autofocus move drop",
        Group::Autofocus,
        (50, 99),
        Some("%"),
        80,
    ),
    number(
        "af_climb_drop",
        "Autofocus climb drop",
        Group::Autofocus,
        (1, 20),
        Some("%"),
        2,
    ),
    number(
        "main_bitrate",
        "Main bitrate",
        Group::MainStream,
        (256, MAIN_MAX_BITRATE as i32 / 1000),
        Some("kbps"),
        1024,
    ),
    select(
        "main_resolution",
        "Main resolution",
        Group::MainStream,
        MAIN_RESOLUTIONS,
        1,
    ),
    number(
        "main_fps",
        "Main frame rate",
        Group::MainStream,
        MAIN_FPS,
        Some("fps"),
        15,
    ),
];

static SETTINGS: LazyLock<Vec<Setting>> = LazyLock::new(|| {
    let mut all = BASE.to_vec();
    all.extend(
        crate::isp_iq::ITEMS
            .iter()
            .map(crate::isp_iq::Item::setting),
    );
    for (slot, s) in all.iter_mut().enumerate() {
        s.spec.slot = slot as u16;
        if let Some(section) = s.section() {
            s.spec = s.spec.section(section.slug());
        }
    }
    all
});

/// The shared specs of [`specs`], in the same order (what `Values` holds).
static SPECS: LazyLock<Vec<Spec>> = LazyLock::new(|| SETTINGS.iter().map(|s| s.spec).collect());

/// Every setting, in a fixed order; a setting's slot is its index.
pub fn specs() -> &'static [Setting] {
    &SETTINGS
}

pub fn spec(key: &str) -> Option<&'static Setting> {
    specs().iter().find(|s| s.spec.key == key)
}

fn index(key: &str) -> Option<usize> {
    specs().iter().position(|s| s.spec.key == key)
}

/// First-run defaults that come from the vendor's own configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StockDefaults(BTreeMap<&'static str, i32>);

impl StockDefaults {
    /// Reads every `stock` attribute of the specs from `0_isp.cfg`'s single `<channel>`; an attribute
    /// that is missing or out of range keeps the spec's own default.
    pub fn from_vendor(text: &str) -> Self {
        let mut found = BTreeMap::new();
        let attrs = text
            .split_once("<channel ")
            .and_then(|(_, t)| t.split_once('>'))
            .map(|(a, _)| a)
            .unwrap_or_default();
        for s in specs() {
            let Some(name) = s.stock else { continue };
            let raw = attrs.split_ascii_whitespace().find_map(|a| {
                let (k, v) = a.split_once('=')?;
                (k == name).then(|| v.trim_end_matches('/').trim_matches('"'))
            });
            let value = match (s.spec.kind, raw) {
                (Kind::Switch, Some("0")) => Some(0),
                (Kind::Switch, Some("1")) => Some(1),
                (Kind::Number { .. } | Kind::Select(_), Some(n)) => n
                    .parse()
                    .ok()
                    .map(|n| crate::isp_iq::from_vendor(s.spec.key, n))
                    .and_then(|v| i32::try_from(v).ok())
                    .filter(|v| s.spec.valid(*v)),
                _ => None,
            };
            if let Some(v) = value {
                found.insert(s.spec.key, v);
            }
        }
        Self(found)
    }

    fn default_of(&self, s: &Setting) -> i32 {
        self.0.get(s.spec.key).copied().unwrap_or(s.spec.default)
    }
}

/// A snapshot of the values in force, one per spec, each always valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings(Vec<i32>);

impl Settings {
    pub fn defaults(stock: &StockDefaults) -> Self {
        Self(specs().iter().map(|s| stock.default_of(s)).collect())
    }

    /// The `i32` value: a number, a switch 0/1, a select's option index.
    pub fn get(&self, key: &str) -> Option<i32> {
        self.0.get(index(key)?).copied()
    }

    fn int(&self, key: &str) -> i32 {
        self.get(key)
            .unwrap_or_else(|| unreachable!("{key} is not a setting"))
    }

    fn on(&self, key: &str) -> bool {
        self.get(key) == Some(1)
    }

    fn choice(&self, key: &str) -> usize {
        self.int(key) as usize
    }

    /// Every setting with its MQTT state text.
    pub fn states(&self) -> impl Iterator<Item = (&'static str, String)> + '_ {
        specs()
            .iter()
            .zip(&self.0)
            .map(|(s, v)| (s.spec.key, s.spec.render(*v)))
    }

    pub fn audio(&self) -> Tuning {
        let pct = |k| self.int(k).clamp(0, 100) as u8;
        Tuning {
            aec: Aec {
                enabled: self.on("aec"),
                leak_estimate: self.on("aec_leak_estimate"),
                leak_value: self.int("aec_leak_value"),
                noise_level: self.int("aec_noise_level"),
                echo_level: self.int("aec_echo_level"),
                notch_radius: self.int("aec_notch_radius"),
            },
            anr: Anr {
                enabled: self.on("anr"),
                suppress_level: self.int("anr_suppress_level"),
                hpf_cutoff: self.int("anr_hpf_cutoff"),
                bias_sensitive: self.int("anr_bias_sensitive"),
            },
            mic_volume: pct("mic_volume"),
            speaker_volume: pct("speaker_volume"),
        }
    }

    /// How long before a tap arrived the picture it was made on was shown.
    pub fn tap_delay(&self) -> Duration {
        Duration::from_millis(self.int("tap_delay") as u64)
    }

    /// Speed profile of absolute moves and holds, and how long a hold may run without a release.
    pub fn motion(&self) -> (Profile, Duration) {
        let ms = |k| Duration::from_millis(self.int(k) as u64);
        (
            Profile::new(
                self.int("ptz_start_speed") as u32,
                self.int("ptz_speed") as u32,
                ms("ptz_ramp"),
            ),
            Duration::from_secs(self.int("ptz_hold_timeout") as u64),
        )
    }

    /// Auto thresholds, hold time, and whether night mode lights the IR LEDs.
    pub fn ir(&self) -> (crate::ir::Thresholds, Duration, bool) {
        (
            crate::ir::Thresholds {
                to_night: self.int("ir_to_night") as u32,
                to_day: self.int("ir_to_day") as u32,
            },
            Duration::from_secs(self.int("ir_hold") as u64),
            self.on("ir_leds_at_night"),
        )
    }

    pub fn ir_mode(&self) -> &'static str {
        IR_MODES[self.choice("ir")]
    }

    pub fn zoom_speed(&self) -> u32 {
        ZOOM_SPEED_OPTIONS[self.choice("zoom_speed")]
            .parse()
            .unwrap_or(80)
    }

    /// Whether zoom, moves, day/night and the watchdog refocus; the watchdog's hold time and drop; how much VA may
    /// fall after a pan/tilt before a search; and the climb's bracket drop.
    pub fn autofocus(&self) -> crate::zoom::af::AfConfig {
        crate::zoom::af::AfConfig {
            enabled: self.on("autofocus"),
            watch_hold: Duration::from_secs(self.int("af_watch_hold") as u64),
            watch_drop_pct: self.int("af_watch_drop") as u32,
            move_keep_pct: self.int("af_move_drop") as u32,
            climb_drop_pct: self.int("af_climb_drop") as u32,
        }
    }

    pub fn led(&self) -> bool {
        self.on("led")
    }

    pub fn orientation(&self) -> crate::orientation::Orientation {
        crate::orientation::Orientation::new(self.on("mirror"), self.on("flip"))
    }

    pub fn main_stream(&self) -> MainStream {
        MainStream {
            dim: MAIN_SIZES[self.choice("main_resolution")],
            bitrate_bps: self.int("main_bitrate") as u32 * 1000,
            fps: self.int("main_fps") as u32,
        }
    }

    /// The item value of a picture control (a select's index is its value).
    pub fn picture(&self, key: &str) -> Option<i64> {
        spec(key).filter(|s| !matches!(s.spec.kind, Kind::Switch))?;
        self.get(key).map(i64::from)
    }

    /// These settings with the audio and main-stream fields replaced by what the media thread reports
    /// it runs, for the MQTT states.
    pub fn with_applied(&self, audio: Option<&Tuning>, main: Option<&MainStream>) -> Self {
        let mut s = self.clone();
        if let Some(a) = audio {
            for (k, v) in [
                ("mic_volume", i32::from(a.mic_volume)),
                ("speaker_volume", i32::from(a.speaker_volume)),
                ("aec", i32::from(a.aec.enabled)),
                ("aec_noise_level", a.aec.noise_level),
                ("aec_echo_level", a.aec.echo_level),
                ("aec_notch_radius", a.aec.notch_radius),
                ("aec_leak_estimate", i32::from(a.aec.leak_estimate)),
                ("aec_leak_value", a.aec.leak_value),
                ("anr", i32::from(a.anr.enabled)),
                ("anr_suppress_level", a.anr.suppress_level),
                ("anr_hpf_cutoff", a.anr.hpf_cutoff),
                ("anr_bias_sensitive", a.anr.bias_sensitive),
            ] {
                s.set_shown(k, v);
            }
        }
        if let Some(m) = main {
            if let Some(i) = MAIN_SIZES.iter().position(|d| *d == m.dim) {
                s.set_shown("main_resolution", i as i32);
            }
            if let Ok(kbps) = i32::try_from(m.bitrate_bps / 1000) {
                s.set_shown("main_bitrate", kbps);
            }
            if let Ok(fps) = i32::try_from(m.fps) {
                s.set_shown("main_fps", fps);
            }
        }
        s
    }

    /// Only values the spec accepts; a driver report outside the range keeps the setting.
    fn set_shown(&mut self, key: &str, v: i32) {
        if spec(key).is_some_and(|s| s.spec.valid(v)) {
            self.set(key, v);
        }
    }

    /// Rules between fields: hysteresis of the IR thresholds, start speed within the speed.
    fn coherent(&self) -> Result<(), (&'static str, &'static str)> {
        if self.int("ir_to_night") <= self.int("ir_to_day") {
            return Err(("ir_to_night", "ir_to_day"));
        }
        if self.int("ptz_start_speed") > self.int("ptz_speed") {
            return Err(("ptz_start_speed", "ptz_speed"));
        }
        Ok(())
    }

    fn set(&mut self, key: &str, v: i32) {
        if let Some(i) = index(key) {
            self.0[i] = v;
        }
    }
}

/// The `docs/settings.md` table, rendered from [`specs`] (a test keeps the document in step).
pub fn markdown_table() -> String {
    let mut out = String::from(
        "| Key | Name | Section | HA type | Range | First-run default | Applies |\n|---|---|---|---|---|---|---|\n",
    );
    for s in specs() {
        let (ty, range) = match s.spec.kind {
            Kind::Number { min, max, unit, .. } => (
                "number",
                format!(
                    "{min}…{max}{}",
                    unit.map(|u| format!(" {u}")).unwrap_or_default()
                ),
            ),
            Kind::Switch => ("switch", "ON/OFF".into()),
            Kind::Select(o) => ("select", o.join(", ")),
        };
        let default = s.spec.render(s.spec.default);
        let default = match s.stock {
            Some(attr) => format!("stock `0_isp.cfg` `{attr}` (else {default})"),
            None => default,
        };
        let applies = match s.group {
            Group::Audio => "live, media thread",
            Group::MainStream => "main path restart (≤ 2 s)",
            Group::Orientation => "live, ISP item 44 with readback",
            Group::Picture => "live, with readback",
            Group::Ir => "next auto tick / live",
            Group::Led => "live",
            Group::Motion => "next move",
            Group::Lens => "next zoom move",
            Group::Autofocus => "live",
        };
        let section = s.section().map_or("camera", Section::name);
        out.push_str(&format!(
            "| `{}` | {} | {section} | {ty} | {range} | {default} | {applies} |\n",
            s.spec.key, s.spec.name
        ));
    }
    out
}

/// The text of the diagnostic `settings` sensor.
pub fn summary(problems: &[Problem]) -> String {
    if problems.is_empty() {
        return "ok".into();
    }
    let keys: Vec<&str> = problems
        .iter()
        .map(|p| p.key.as_deref().unwrap_or("file"))
        .collect();
    format!("invalid: {}", keys.join(", "))
}

/// A setting that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub spec: &'static Setting,
    pub value: i32,
}

/// The durable-write steps the store performs, in this order: temp file (mode 0600), its fsync, rename
/// over the settings file, fsync of the directory.
pub trait Disk: Send {
    /// `None` when the file does not exist.
    fn read(&mut self) -> io::Result<Option<String>>;
    fn write_temp(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn sync_temp(&mut self) -> io::Result<()>;
    fn rename_temp(&mut self) -> io::Result<()>;
    fn sync_dir(&mut self) -> io::Result<()>;
}

/// The shared store's view of a step-wise [`Disk`]: one write is the four durable steps in order.
pub struct Steps<D>(D);

impl<D: Disk> shared::Disk for Steps<D> {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        Ok(self.0.read()?.map(String::into_bytes))
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_temp(bytes)?;
        self.0.sync_temp()?;
        self.0.rename_temp()?;
        self.0.sync_dir()
    }
}

/// Owns the values in force and writes them back: within [`SAVE_WITHIN`] of the first unsaved change,
/// never postponed by later changes, so a burst of slider moves costs one write per window; a failed
/// write is retried one window later. On top of the shared `Values`: stock first-run defaults and the
/// rules between fields.
pub struct Store<D: Disk> {
    values: Values<'static, Steps<D>, TomlCodec>,
    /// The values in force, kept in step with `values` for cheap typed reads.
    settings: Settings,
    /// Fields reset because they broke a rule with another.
    inconsistent: Vec<Problem>,
}

impl<D: Disk> Store<D> {
    /// Loads the file; never fails. Absent: defaults. Invalid file or field: that default plus a problem.
    /// Unknown keys are ignored so a downgrade keeps working.
    pub fn load(disk: D, stock: &StockDefaults) -> (Self, Vec<Problem>) {
        let (mut values, _) =
            Values::load(SPECS.as_slice(), Steps(disk), TomlCodec { header: HEADER });
        let defaults = Settings::defaults(stock);
        // A first-run value from the stock file is in force and written with the next save.
        for s in specs() {
            if !values.is_set(s.spec.slot) {
                let _ = values.observe(s.spec.slot, stock.default_of(s));
            }
        }
        let mut settings = Settings(
            specs()
                .iter()
                .map(|s| values.get(s.spec.slot).unwrap_or(s.spec.default))
                .collect(),
        );
        let mut inconsistent = Vec::new();
        if let Err((a, b)) = settings.coherent() {
            for k in [a, b] {
                if let (Some(s), Some(v)) = (spec(k), defaults.get(k)) {
                    let _ = values.observe(s.spec.slot, v);
                    settings.set(k, v);
                }
                inconsistent.push(Problem {
                    key: Some(k.into()),
                    reason: format!("inconsistent with {}", if k == a { b } else { a }),
                });
            }
        }
        let store = Self {
            values,
            settings,
            inconsistent,
        };
        let problems = store.problems();
        (store, problems)
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Problems from loading, plus a failed write until the next one succeeds.
    pub fn problems(&self) -> Vec<Problem> {
        let mut all = self.values.problems().to_vec();
        all.extend(self.inconsistent.iter().cloned());
        all
    }

    /// Validates `text` for `key` and puts it in force. `now` is any monotonic time; it starts the
    /// save window if nothing was unsaved.
    pub fn set(
        &mut self,
        key: &str,
        text: &str,
        now: Duration,
    ) -> Result<Option<Change>, Rejected> {
        let spec = spec(key).ok_or_else(|| Rejected(format!("unknown setting {key}")))?;
        let value = spec.spec.parse(text)?;
        self.put(spec, value, now)
    }

    /// As [`Store::set`] with a value already typed, e.g. what the hardware reports it applied.
    pub fn put(
        &mut self,
        spec: &'static Setting,
        value: i32,
        now: Duration,
    ) -> Result<Option<Change>, Rejected> {
        if !spec.spec.valid(value) {
            return Err(Rejected(format!("{}: out of range", spec.spec.key)));
        }
        if self.settings.get(spec.spec.key) == Some(value) {
            return Ok(None);
        }
        let mut next = self.settings.clone();
        next.set(spec.spec.key, value);
        if let Err((a, b)) = next.coherent() {
            return Err(Rejected(format!("{a} must stay consistent with {b}")));
        }
        self.values.put(spec.spec.slot, value, now)?;
        self.settings = next;
        Ok(Some(Change { spec, value }))
    }

    /// Takes the main stream the media thread reports it runs (after a refusal or a revert) as the
    /// setting, so a value the encoders cannot sustain is not persisted.
    pub fn adopt_main(&mut self, m: &MainStream, now: Duration) {
        let shown = self.settings.with_applied(None, Some(m));
        for key in ["main_resolution", "main_bitrate", "main_fps"] {
            if let (Some(spec), Some(v)) = (spec(key), shown.get(key)) {
                let _ = self.put(spec, v, now);
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.values.dirty()
    }

    /// Writes the snapshot once the window of the oldest unsaved change has run out.
    pub fn tick(&mut self, now: Duration) -> io::Result<()> {
        self.values.tick(now)
    }

    /// Writes now if anything is unsaved (orderly shutdown).
    pub fn flush(&mut self) -> io::Result<()> {
        self.values.flush()
    }

    /// Read access and the change feed for another owner (ONVIF).
    pub fn reader(&self) -> shared::Reader<'static> {
        self.values.reader()
    }

    pub fn disk(&self) -> &D {
        &self.values.disk().0
    }

    pub fn disk_mut(&mut self) -> &mut D {
        &mut self.values.disk_mut().0
    }
}

impl Disk for Box<dyn Disk> {
    fn read(&mut self) -> io::Result<Option<String>> {
        (**self).read()
    }
    fn write_temp(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write_temp(bytes)
    }
    fn sync_temp(&mut self) -> io::Result<()> {
        (**self).sync_temp()
    }
    fn rename_temp(&mut self) -> io::Result<()> {
        (**self).rename_temp()
    }
    fn sync_dir(&mut self) -> io::Result<()> {
        (**self).sync_dir()
    }
}

/// What the hardware owner receives: the loaded store and the hand-off to the media thread.
pub struct Owner {
    pub store: Store<Box<dyn Disk>>,
    pub media: std::sync::Arc<crate::media_tuning::Handoff>,
    pub focus_map: crate::zoom::focus_map::FocusMap,
    pub presets: crate::presets::Presets,
}

impl Owner {
    /// Loads `path` (seeding first-run defaults from the stock `0_isp.cfg` text) and prepares the
    /// media hand-off with the persisted audio and main-stream values; `map_disk` holds the learned focus map,
    /// `presets_disk` the positions saved over `cam.toml`'s `presets`. Problems are logged once.
    pub fn load(
        disk: Box<dyn Disk>,
        stock_isp: &str,
        map_disk: Box<dyn Disk>,
        presets: std::collections::BTreeMap<String, crate::presets::Preset>,
        presets_disk: Box<dyn Disk>,
    ) -> Self {
        let (store, problems) = Store::load(disk, &StockDefaults::from_vendor(stock_isp));
        for p in &problems {
            eprintln!("settings: {p}; using the default");
        }
        let (focus_map, problem) = crate::zoom::focus_map::FocusMap::load(map_disk);
        if let Some(p) = problem {
            eprintln!("settings: {p}; starting with an empty map");
        }
        let (presets, problems) = crate::presets::Presets::load(presets, presets_disk);
        for p in &problems {
            eprintln!("settings: {p}; using cam.toml");
        }
        let s = store.settings();
        let media = std::sync::Arc::new(crate::media_tuning::Handoff::new(
            crate::pipeline::MediaTuning {
                audio: Some(s.audio()),
                main: Some(s.main_stream()),
            },
        ));
        Self {
            store,
            media,
            focus_map,
            presets,
        }
    }
}

/// The settings file next to its temp file, on Linux.
pub struct FileDisk {
    path: std::path::PathBuf,
    temp: Option<std::fs::File>,
}

impl FileDisk {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            temp: None,
        }
    }

    fn temp_path(&self) -> std::path::PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_owned();
        name.push(".tmp");
        self.path.with_file_name(name)
    }
}

impl Disk for FileDisk {
    fn read(&mut self) -> io::Result<Option<String>> {
        match crate::files::read_device_or_file(&self.path, MAX_FILE) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| io::Error::other("not UTF-8")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write_temp(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut f = crate::files::create_private(&self.temp_path())?;
        f.write_all(bytes)?;
        self.temp = Some(f);
        Ok(())
    }

    fn sync_temp(&mut self) -> io::Result<()> {
        self.temp
            .take()
            .ok_or_else(|| io::Error::other("no temp file"))?
            .sync_all()
    }

    fn rename_temp(&mut self) -> io::Result<()> {
        std::fs::rename(self.temp_path(), &self.path)
    }

    fn sync_dir(&mut self) -> io::Result<()> {
        let dir = self.path.parent().unwrap_or(std::path::Path::new("."));
        std::fs::File::open(dir)?.sync_all()
    }
}

/// Host fake: records every step and keeps the "file" in memory.
#[derive(Debug, Default)]
pub struct FakeDisk {
    pub file: Option<String>,
    pub temp: Option<String>,
    pub ops: Vec<&'static str>,
    pub fail_read: bool,
    pub fail_write: bool,
}

impl Disk for FakeDisk {
    fn read(&mut self) -> io::Result<Option<String>> {
        self.ops.push("read");
        if self.fail_read {
            return Err(io::Error::other("fake read failure"));
        }
        Ok(self.file.clone())
    }
    fn write_temp(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.ops.push("write_temp");
        if self.fail_write {
            return Err(io::Error::other("fake write failure"));
        }
        self.temp = Some(String::from_utf8_lossy(bytes).into_owned());
        Ok(())
    }
    fn sync_temp(&mut self) -> io::Result<()> {
        self.ops.push("sync_temp");
        Ok(())
    }
    fn rename_temp(&mut self) -> io::Result<()> {
        self.ops.push("rename_temp");
        self.file = self.temp.take();
        Ok(())
    }
    fn sync_dir(&mut self) -> io::Result<()> {
        self.ops.push("sync_dir");
        Ok(())
    }
}

#[cfg(test)]
mod tests;
