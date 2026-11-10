//! The E1 Zoom on the shared MQTT layer (`camera_transport::mqtt`): its catalog, its command vocabulary and
//! the wiring of its hardware, telemetry and direct stop path. Hardware stays behind `Hardware`.
pub mod device;

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub camera: crate::identity::Identity,
    pub mqtt: Option<Broker>,
    #[serde(default)]
    pub presets: BTreeMap<String, crate::presets::Preset>,
    #[serde(default)]
    pub onvif: Onvif,
    #[serde(default)]
    pub time: Time,
}

/// `[time]`: NTP servers the camera clock follows (`host` or `host:port`); none leaves the clock alone.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Time {
    #[serde(default)]
    pub ntp: Vec<String>,
}

/// `[onvif]`: the ONVIF service (Profile S, discovery, snapshots), on by default.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Onvif {
    /// HTTP port of the ONVIF services and `/snapshot.jpg`; 0 turns ONVIF off.
    #[serde(default = "default_onvif_port")]
    pub port: u16,
}
fn default_onvif_port() -> u16 {
    80
}
impl Default for Onvif {
    fn default() -> Self {
        Self {
            port: default_onvif_port(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Broker {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}
fn default_port() -> u16 {
    1883
}
impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broker")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}
impl Config {
    /// Reads `cam.toml`, which holds the broker password: owner-only, at most 16 KiB.
    pub fn read(path: &std::path::Path) -> std::io::Result<Self> {
        let text = crate::files::read_private(path, 16 * 1024)?;
        Self::parse(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 16 * 1024 {
            return Err("cam.toml exceeds size limit".into());
        }
        // TOML errors include the input line, which may contain a password.
        let cfg: Self = toml::from_str(text).map_err(|_| "invalid cam.toml".to_string())?;
        cfg.camera.validate()?;
        if cfg
            .mqtt
            .as_ref()
            .is_some_and(|b| b.host.trim().is_empty() || b.port == 0)
        {
            return Err("MQTT needs a host and nonzero port".into());
        }
        if cfg.presets.len() > 32 {
            return Err("too many presets (max 32)".into());
        }
        for (name, p) in &cfg.presets {
            crate::presets::check(name, p)?;
        }
        Ok(cfg)
    }
}

use camera_transport::mqtt::{self as transport, Category, Domain, Entity, Mode, Ptz, ptz};
use serde_json::Value;

mod motion;
pub use motion::Motion;

/// A discovery message as the tests and fixtures read it.
pub struct Publication {
    pub topic: String,
    /// Null means an empty retained tombstone (remove an obsolete optional entity).
    pub payload: Value,
    pub retain: bool,
}

const SYNTAX: ptz::Syntax = ptz::Syntax {
    pan: Some(crate::ptz::PAN_MAX as i32),
    tilt: Some(crate::ptz::TILT_MAX as i32),
    zoom: Some(ZOOM_MAX as i32),
    relative: ptz::Relative::Prefix,
    fractions: false,
    vectors: false,
    move_stop_releases: true,
    position3d: true,
};

impl Config {
    /// One device, the camera. A setting's section leads its name, so the configuration list on the device page
    /// sorts section by section (Home Assistant groups entities on a device only by category; separate linked
    /// devices per section were tried and rejected: the camera is one device).
    pub fn entities(&self) -> Vec<Entity> {
        self.entities_for(&self.preset_names())
    }

    /// [`Config::entities`] whose preset select offers `presets` (the table in force: created and removed
    /// presets change it at runtime, and the select is announced again then).
    pub fn entities_for(&self, presets: &[String]) -> Vec<Entity> {
        use crate::settings::{Kind, Section};
        let named = |name: &str, section: Option<Section>| match section {
            Some(s) => format!("{} {name}", s.name()),
            None => name.to_owned(),
        };
        let diagnostic = |e: Entity| e.category(Category::Diagnostic);
        let mut entities = vec![
            Entity::new(
                "pan",
                "Pan",
                Domain::number(0, crate::ptz::PAN_MAX as i32, Mode::Box),
            ),
            Entity::new(
                "tilt",
                "Tilt",
                Domain::number(0, crate::ptz::TILT_MAX as i32, Mode::Box),
            ),
            Entity::new(
                "zoom",
                "Zoom",
                Domain::number(0, ZOOM_MAX as i32, Mode::Slider),
            ),
            Entity::new("preset", "Preset", Domain::select(presets.iter().cloned())),
            Entity::new("reboot", "Reboot", Domain::Button)
                .device_class("restart")
                .category(Category::Config),
            Entity::new("refocus", "Refocus", Domain::Button).icon("mdi:image-filter-center-focus"),
            Entity::new(
                "calibrate_focus",
                named("Calibrate focus (15 min)", Some(Section::Lens)),
                Domain::Button,
            )
            .icon("mdi:crosshairs-gps")
            .category(Category::Config),
            diagnostic(
                Entity::new("autofocus_state", "Autofocus", Domain::Sensor)
                    .icon("mdi:camera-iris")
                    .attributes("autofocus_attributes"),
            ),
            diagnostic(Entity::new(
                "pipeline",
                "Pipeline health",
                Domain::BinarySensor,
            )),
            diagnostic(
                Entity::new("light", "Light sensor", Domain::Sensor)
                    .icon("mdi:brightness-6")
                    .state_class("measurement"),
            ),
            diagnostic(Entity::new("settings", "Settings", Domain::Sensor).icon("mdi:cog-outline")),
        ];
        for setting in crate::settings::specs() {
            let s = &setting.spec;
            let domain = match s.kind {
                Kind::Number {
                    min, max, slider, ..
                } => Domain::number(min, max, if slider { Mode::Slider } else { Mode::Box }),
                Kind::Switch => Domain::Switch,
                Kind::Select(options) => Domain::select(options.iter().copied()),
            };
            let mut e = Entity::new(s.key, named(s.name, setting.section()), domain)
                .category(Category::Config);
            if let Kind::Number { unit, .. } = s.kind {
                e.unit = unit;
            }
            e.icon = s.icon;
            entities.push(e);
        }
        entities.extend(crate::telemetry::sensors().map(|t| t.entity()));
        entities
    }

    /// The shared layer's configuration for this camera, or `None` without a broker.
    pub fn transport(&self) -> Option<transport::Config> {
        self.transport_with(&self.preset_names())
    }

    /// [`Config::transport`] with the presets in force.
    pub fn transport_with(&self, presets: &[String]) -> Option<transport::Config> {
        self.mqtt
            .as_ref()
            .map(|broker| self.transport_for(broker, presets))
    }

    /// `cam.toml`'s preset names.
    fn preset_names(&self) -> Vec<String> {
        self.presets.keys().cloned().collect()
    }

    fn transport_for(&self, broker: &Broker, presets: &[String]) -> transport::Config {
        let camera = &self.camera;
        let mut cfg = transport::Config::new(
            transport::Broker {
                host: broker.host.clone(),
                port: broker.port,
                username: broker.username.clone(),
                password: broker.password.clone(),
            },
            camera.client_id(),
            camera.topic("").trim_end_matches('/').to_owned(),
            transport::Device {
                name: camera.name().to_owned(),
                manufacturer: "Reolink".into(),
                model: "E1 Zoom (cam)".into(),
                sw_version: None,
            },
            self.entities_for(presets),
        );
        cfg.stop_on_disconnect = false;
        // Cleared ahead of every announcement, so Home Assistant frees their entity ids first.
        cfg.retired = crate::telemetry::RETIRED
            .iter()
            .map(|id| (Domain::Sensor, (*id).to_owned()))
            .collect();
        cfg.limits = transport::Limits {
            entities: 160,
            outbox: 256,
            outbound: OUTBOUND,
            max_packet: 16 * 1024,
            command_age: None,
            ..transport::Limits::default()
        };
        cfg
    }

    /// Discovery with `cam.toml`'s presets; [`Config::discovery_for`] with the presets in force.
    pub fn discovery(&self) -> Vec<Publication> {
        self.discovery_for(&self.preset_names())
    }

    /// Discovery whose preset select offers `presets` (none clears the select's config).
    pub fn discovery_for(&self, presets: &[String]) -> Vec<Publication> {
        // Discovery does not depend on the broker; a camera without one still describes itself.
        let placeholder = Broker {
            host: "broker".into(),
            port: default_port(),
            username: String::new(),
            password: String::new(),
        };
        let cfg = self.transport_for(self.mqtt.as_ref().unwrap_or(&placeholder), presets);
        cfg.discovery()
            .expect("the E1 Zoom catalog is valid")
            .into_iter()
            .map(|p| Publication {
                topic: p.topic,
                payload: serde_json::from_slice(&p.payload).unwrap_or(Value::Null),
                retain: p.retain,
            })
            .collect()
    }
}

/// Press-and-hold motion from the card: `camera/<id>/move/set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    ZoomIn,
    ZoomOut,
    /// Release: ends the hold after its running chunk; never the emergency stop.
    Stop,
}

/// The highest zoom the lens driver accepts on the E1 Zoom (the last ZFTable row; `docs/hardware/zoom-focus.md`).
pub const ZOOM_MAX: u32 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Pan(u32),
    Tilt(u32),
    Zoom(u32),
    PanBy(i32),
    TiltBy(i32),
    ZoomBy(i32),
    /// Pan and tilt together to an absolute position, both axes in one move (ONVIF `AbsoluteMove`).
    PanTilt(u32, u32),
    /// Pan and tilt by a fraction of the picture shown, in thousandths: `x` of its width right, `y` of its
    /// height up (ONVIF `RelativeMove` in the field-of-view space), both axes in one move.
    Fov {
        x: i16,
        y: i16,
    },
    /// Go to the named preset; its position is resolved on the hardware thread.
    Preset(String),
    /// Save where the camera stands as the named preset's position (`preset_save/set`).
    SavePreset(String),
    /// Forget a preset (created at runtime or from `cam.toml`).
    RemovePreset(String),
    /// A validated payload for a key of `crate::settings::specs`.
    Setting(&'static str, String),
    Move(Move),
    Velocity(crate::motion_control::Velocity),
    Home,
    /// Centre a point, or centre and zoom to a box, on the picture (`position3d/set`).
    Position3d(crate::position3d::Request),
    Reboot,
    /// One autofocus search now (the Refocus button).
    Refocus,
    /// Walk the calibration grid and learn every point (the Calibrate focus button).
    CalibrateFocus,
    /// Diagnostic: park the focus motor here (`focus/set`, not advertised); the watchdog may correct it.
    Focus(u32),
    Stop,
}
impl Command {
    /// Whether this command makes the head or lens move (ONVIF `MoveStatus` reads `MOVING` from its arrival).
    pub fn moves(&self) -> bool {
        match self {
            Self::Pan(_)
            | Self::Tilt(_)
            | Self::Zoom(_)
            | Self::PanBy(_)
            | Self::TiltBy(_)
            | Self::ZoomBy(_)
            | Self::PanTilt(..)
            | Self::Fov { .. }
            | Self::Preset(_)
            | Self::Velocity(_)
            | Self::Home
            | Self::Position3d(_) => true,
            Self::Move(m) => *m != Move::Stop,
            _ => false,
        }
    }
}

impl Config {
    /// Retained commands are never executed, including a retained reboot from an old connection.
    pub fn command(
        &self,
        topic: &str,
        payload: &[u8],
        retained: bool,
    ) -> Result<Option<Command>, String> {
        if retained {
            return Ok(None);
        }
        let Some(id) = topic
            .strip_prefix(&self.camera.topic(""))
            .and_then(|t| t.strip_suffix("/set"))
        else {
            return Ok(None);
        };
        let text = std::str::from_utf8(payload).map_err(|_| "command is not UTF-8")?;
        self.parse_id(id, text)
    }

    fn parse_id(&self, id: &str, text: &str) -> Result<Option<Command>, String> {
        if let Some(parsed) = ptz::parse(&SYNTAX, id, text) {
            return from_ptz(parsed?).map(Some);
        }
        let cmd = match id {
            "preset" | "preset_save" => {
                // Any valid name: a save creates the preset, a recall of an unknown one fails on the hardware
                // thread, which owns the table in force.
                crate::motion_control::PresetName::parse(text)?;
                if id == "preset" {
                    Command::Preset(text.into())
                } else {
                    Command::SavePreset(text.into())
                }
            }
            "focus" => Command::Focus(
                text.parse::<u32>()
                    .ok()
                    .filter(|n| *n <= crate::zoom::FOCUS_MAX)
                    .ok_or("position outside travel")?,
            ),
            "reboot" if text == "PRESS" => Command::Reboot,
            "refocus" if text == "PRESS" => Command::Refocus,
            "calibrate_focus" if text == "PRESS" => Command::CalibrateFocus,
            "reboot" | "refocus" | "calibrate_focus" => {
                return Err("invalid command payload".into());
            }
            _ => match crate::settings::spec(id) {
                Some(spec) => {
                    spec.parse(text)?;
                    Command::Setting(spec.spec.key, text.trim().into())
                }
                None => return Ok(None),
            },
        };
        Ok(Some(cmd))
    }
}

/// The shared PTZ vocabulary in this camera's commands. A release ends the hold after its running chunk,
/// in order with other commands; only the hard stop is urgent.
fn from_ptz(command: transport::Command<Ptz>) -> Result<Command, String> {
    let at = |p: transport::Position, by: fn(i32) -> Command, to: fn(u32) -> Command| match p {
        transport::Position::Absolute(n) => to(n as u32),
        transport::Position::Relative(n) => by(n),
    };
    Ok(match command {
        transport::Command::Stop => Command::Stop,
        transport::Command::Release => Command::Move(Move::Stop),
        transport::Command::Setting { .. } => return Err("not a PTZ command".into()),
        transport::Command::App(p) => match p {
            Ptz::Pan(p) => at(p, Command::PanBy, Command::Pan),
            Ptz::Tilt(p) => at(p, Command::TiltBy, Command::Tilt),
            Ptz::Zoom(p) => at(p, Command::ZoomBy, Command::Zoom),
            Ptz::Move(d) => Command::Move(match d {
                transport::Direction::Left => Move::Left,
                transport::Direction::Right => Move::Right,
                transport::Direction::Up => Move::Up,
                transport::Direction::Down => Move::Down,
                _ => return Err("invalid move".into()),
            }),
            Ptz::ZoomMove(transport::Zoom::In) => Command::Move(Move::ZoomIn),
            Ptz::ZoomMove(transport::Zoom::Out) => Command::Move(Move::ZoomOut),
            Ptz::Velocity(v) => Command::Velocity(v),
            Ptz::Home => Command::Home,
            Ptz::Preset(name) => Command::Preset(name.as_str()?.to_owned()),
            Ptz::SavePreset(name) => Command::SavePreset(name.as_str()?.to_owned()),
            Ptz::RemovePreset(name) => Command::RemovePreset(name.as_str()?.to_owned()),
            Ptz::Position3d(r) => Command::Position3d(r),
        },
    })
}

impl transport::Parser for Config {
    type App = Command;
    fn parse(&self, id: &str, text: &str) -> Result<Option<transport::Command<Command>>, String> {
        Ok(self.parse_id(id, text)?.map(|c| match c {
            Command::Stop => transport::Command::Stop,
            c => transport::Command::App(c),
        }))
    }
}

/// Device boundary; the broker worker owns it, so motor waits never block MQTT polling.
pub trait Hardware: Send {
    fn failed(&self) -> bool {
        false
    }
    fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn motion_epoch(&self) -> Option<Arc<std::sync::atomic::AtomicU32>> {
        None
    }
    fn apply(&mut self, command: &Command) -> Result<(), String>;
    /// [`Hardware::apply`] for a command that arrived at `received` on the hardware thread's clock (the same
    /// clock its `tick` gets), so a tap can be resolved against the picture it was made on.
    fn apply_at(&mut self, command: &Command, received: std::time::Duration) -> Result<(), String> {
        let _ = received;
        self.apply(command)
    }
    /// Periodic work (IR auto, holds, saving settings). Returns how soon it wants to run again; `None`
    /// is the idle cadence.
    fn tick(&mut self, now: std::time::Duration) -> Result<Option<std::time::Duration>, String>;
    fn states(&mut self) -> Result<BTreeMap<String, String>, String>;
    /// Orderly shutdown (save unsaved settings).
    fn shutdown(&mut self) {}
    /// True once after something changed asynchronously (the media thread applied a request), so
    /// states are read and their changes published now rather than at the next five-second readback.
    fn changed(&mut self) -> bool {
        false
    }
    /// The preset names in force, when the hardware owns a preset table (`None`: `cam.toml`'s).
    fn preset_names(&self) -> Option<Vec<String>> {
        None
    }
    /// The confirmed pan, tilt and zoom in steps, `None` while pan/tilt is unknown.
    fn pose(&mut self) -> Option<Pose> {
        None
    }
    /// A hold (pan/tilt or zoom) is running, so the head or lens is still moving after `apply` returned.
    fn busy(&self) -> bool {
        false
    }
}

/// Confirmed pan, tilt and zoom in motor steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pose {
    pub pan: u32,
    pub tilt: u32,
    pub zoom: u32,
}

impl Hardware for Box<dyn Hardware> {
    fn preset_names(&self) -> Option<Vec<String>> {
        (**self).preset_names()
    }
    fn pose(&mut self) -> Option<Pose> {
        (**self).pose()
    }
    fn busy(&self) -> bool {
        (**self).busy()
    }
    fn failed(&self) -> bool {
        (**self).failed()
    }
    fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
        (**self).stop_endpoint()
    }
    fn motion_epoch(&self) -> Option<Arc<std::sync::atomic::AtomicU32>> {
        (**self).motion_epoch()
    }
    fn apply(&mut self, command: &Command) -> Result<(), String> {
        (**self).apply(command)
    }
    fn apply_at(&mut self, command: &Command, received: std::time::Duration) -> Result<(), String> {
        (**self).apply_at(command, received)
    }
    fn tick(&mut self, now: std::time::Duration) -> Result<Option<std::time::Duration>, String> {
        (**self).tick(now)
    }
    fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
        (**self).states()
    }
    fn shutdown(&mut self) {
        (**self).shutdown()
    }
    fn changed(&mut self) -> bool {
        (**self).changed()
    }
}

#[derive(Default)]
pub struct PipelineHealth(Mutex<[Option<Instant>; 2]>);
impl PipelineHealth {
    pub fn frame(&self, stream: usize) {
        if let Ok(mut frames) = self.0.lock()
            && let Some(t) = frames.get_mut(stream)
        {
            *t = Some(Instant::now());
        }
    }
    pub fn healthy(&self) -> bool {
        self.0.lock().is_ok_and(|f| {
            f.iter()
                .all(|t| t.is_some_and(|t| t.elapsed() < Duration::from_secs(3)))
        })
    }
}

use crate::runtime::{Beats, Progress};

/// Dropped inside the scope closure, before the scope's implicit joins on unwind.
struct HardwareShutdown<'a> {
    stop: &'a AtomicBool,
    _lease: Option<crate::runtime::WorkerLease>,
}
impl Drop for HardwareShutdown<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
/// Set when the hardware thread ends, also by unwinding.
struct Finished<'a>(&'a AtomicBool);
impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct LogBudget(std::sync::atomic::AtomicUsize);
impl LogBudget {
    // `fetch_update` is named `try_update` from Rust 1.99, which the supported 1.88 does not have.
    #[allow(deprecated)]
    fn line(&self, message: &str) {
        match self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        {
            Ok(1) => eprintln!("mqtt: further events suppressed"),
            Ok(_) => eprintln!("mqtt: {message}"),
            Err(_) => {}
        }
    }
}

/// This camera's device on the shared hardware thread: states gain pipeline health, positions that are
/// unknown publish empty retained states rather than keep a stale preset or coordinate, and a created or
/// removed preset updates the preset select's options and the list ONVIF reads.
struct Shared<'a, H> {
    hw: H,
    health: Arc<PipelineHealth>,
    motion: &'a Motion,
    publisher: transport::Publisher,
    log: &'a LogBudget,
}
impl<H: Hardware> Shared<'_, H> {
    /// Publishes whether the head or lens still moves (a hold is running) and, when `read_pose`, where they
    /// stand, for ONVIF `GetStatus`.
    fn publish_motion(&mut self, read_pose: bool) {
        let pose = read_pose.then(|| self.hw.pose()).flatten();
        self.motion
            .settle(self.hw.busy(), read_pose.then_some(pose));
    }

    fn sync_presets(&self) {
        if let Some(names) = self.hw.preset_names()
            && self.motion.set_presets(names.clone())
            && let Err(e) = self.publisher.set_options("preset", names)
        {
            self.log.line(&format!("preset options not updated: {e}"));
        }
    }
}
impl<H: Hardware> transport::Hardware<Command> for Shared<'_, H> {
    fn failed(&self) -> bool {
        self.hw.failed()
    }
    fn apply_at(
        &mut self,
        command: &transport::Command<Command>,
        received: Duration,
    ) -> Result<(), String> {
        let moves = matches!(command, transport::Command::App(c) if c.moves());
        if moves {
            self.motion.set_busy(true);
        }
        let result = match command {
            transport::Command::App(c) => self.hw.apply_at(c, received),
            transport::Command::Stop => self.hw.apply_at(&Command::Stop, received),
            transport::Command::Release => self.hw.apply_at(&Command::Move(Move::Stop), received),
            transport::Command::Setting { .. } => Err("no slot settings on this camera".into()),
        };
        self.sync_presets();
        self.publish_motion(moves || matches!(command, transport::Command::Stop));
        result
    }
    fn tick(&mut self, now: Duration) -> Result<Option<Duration>, String> {
        let result = self.hw.tick(now);
        // A hold ends inside a tick; the pose is read once, as it settles.
        let was_busy = self.motion.busy();
        if was_busy || self.hw.busy() {
            self.publish_motion(was_busy);
        }
        result
    }
    fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
        let mut states = self.hw.states()?;
        states.insert(
            "pipeline".into(),
            if self.health.healthy() { "ON" } else { "OFF" }.into(),
        );
        for id in ["pan", "tilt", "preset"] {
            states.entry(id.into()).or_default();
        }
        self.motion.set_states(&states);
        Ok(states)
    }
    fn changed(&mut self) -> bool {
        self.hw.changed()
    }
    fn shutdown(&mut self) {
        self.hw.shutdown()
    }
    fn supersedable(&self, command: &transport::Command<Command>) -> bool {
        // Each tap's target is absolute from its own picture, so only the newest of a run matters.
        matches!(command, transport::Command::App(Command::Position3d(_)))
    }
}

/// Poll MQTT independently of blocking hardware operations. Queues are bounded, sessions clean,
/// commands non-retained, and discovery/state are replayed on every reconnect and HA birth.
pub fn run(
    cfg: Config,
    hw: impl Hardware,
    health: Arc<PipelineHealth>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    run_with(cfg, hw, health, stop, None)
}

/// As [`run`], reporting `Mqtt` progress after each network/reconnect iteration and `Hardware` progress
/// from the nested hardware thread (which also holds its own `WorkerLease`). A blocked hardware call
/// therefore ages out even though the network loop keeps going.
pub fn run_with(
    cfg: Config,
    hw: impl Hardware,
    health: Arc<PipelineHealth>,
    stop: &AtomicBool,
    beats: Option<&Beats>,
) -> std::io::Result<()> {
    run_full(cfg, hw, health, stop, beats, None, None)
}

/// Outbound request queue of the MQTT client (bounded; a full queue keeps the update for the next pass).
pub const OUTBOUND: usize = 256;

/// The MQTT loop stops reporting progress when the telemetry thread has not finished a round for this
/// long, so a stalled telemetry read ages out under the `Mqtt` budget without blocking MQTT itself.
const TELEMETRY_STALL: Duration = Duration::from_secs(3 * crate::telemetry::INTERVAL.as_secs());

/// As [`run_with`], plus the diagnostic telemetry thread publishing every `telemetry::INTERVAL`.
pub fn run_full(
    cfg: Config,
    hw: impl Hardware,
    health: Arc<PipelineHealth>,
    stop: &AtomicBool,
    beats: Option<&Beats>,
    telemetry: Option<crate::telemetry::Task>,
    motion: Option<Arc<Motion>>,
) -> std::io::Result<()> {
    // The preset list other services (ONVIF) share with MQTT: the hardware's table, else `cam.toml`'s.
    let presets = hw
        .preset_names()
        .unwrap_or_else(|| cfg.presets.keys().cloned().collect());
    let Some(transport_cfg) = cfg.transport_with(&presets) else {
        return Ok(());
    };
    let endpoint = hw.stop_endpoint();
    let epoch = hw.motion_epoch().unwrap_or_default();
    let motion = motion.unwrap_or_default();
    motion.set_presets(presets);
    let (publisher, commands, worker) = transport::channel_with_epoch(transport_cfg, cfg, epoch)?;
    // ONVIF submits into the same queue, on the same clock and epoch as MQTT.
    motion.attach(worker.submitter(), endpoint.clone());
    let log = LogBudget(std::sync::atomic::AtomicUsize::new(64));
    let started = commands.started();
    let telemetry_round = std::sync::atomic::AtomicU64::new(0);
    let hardware_done = AtomicBool::new(false);
    let network_stop = AtomicBool::new(false);
    std::thread::scope(|scope| -> std::io::Result<()> {
        let shutdown = HardwareShutdown {
            stop,
            _lease: beats.map(Beats::lease),
        };
        let (log, telemetry_round, hardware_done) = (&log, &telemetry_round, &hardware_done);
        let watching_telemetry = telemetry.is_some();
        if let Some(mut task) = telemetry {
            let lease = beats.map(Beats::lease);
            let publisher = publisher.clone();
            std::thread::Builder::new()
                .name("telemetry".into())
                .spawn_scoped(scope, move || {
                    let _lease = lease;
                    let mut next = Instant::now();
                    while !stop.load(Ordering::Relaxed) {
                        if Instant::now() >= next {
                            let round = task.round(started.elapsed(), publisher.is_connected());
                            for (id, value) in round.states {
                                if let Err(e) = publisher.state(id, value) {
                                    log.line(&e.to_string());
                                }
                            }
                            telemetry_round
                                .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                            next += crate::telemetry::INTERVAL;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                })?;
        }
        let lease = beats.map(Beats::lease);
        let hardware_beats = beats.cloned();
        let hardware_publisher = publisher.clone();
        let hardware = std::thread::Builder::new()
            .name("mqtt-hardware".into())
            .spawn_scoped(scope, move || {
                let _finished = Finished(hardware_done);
                let _lease = lease;
                let beat = || {
                    if let Some(beats) = &hardware_beats {
                        beats.beat(Progress::Hardware);
                    }
                };
                let sink = |line: &str| log.line(line);
                let mut device = Shared {
                    hw,
                    health,
                    motion: &motion,
                    publisher: hardware_publisher.clone(),
                    log,
                };
                transport::HardwareLoop {
                    beat: Some(&beat),
                    log: Some(&sink),
                    ..transport::HardwareLoop::default()
                }
                .run(&mut device, &commands, &hardware_publisher, stop);
            })?;
        let progress = || {
            if stop.load(Ordering::Relaxed) || hardware_done.load(Ordering::Acquire) {
                network_stop.store(true, Ordering::Release);
            }
            let telemetry_fresh = !watching_telemetry
                || started.elapsed().saturating_sub(Duration::from_millis(
                    telemetry_round.load(Ordering::Relaxed),
                )) < TELEMETRY_STALL;
            if let Some(beats) = beats
                && telemetry_fresh
            {
                beats.beat(Progress::Mqtt);
            }
        };
        // The direct stop path: the stop owner halts motion and advances the epoch, so queued motion is
        // discarded without waiting behind a blocked hardware call.
        let urgent = |command: &transport::Command<Command>| {
            if let transport::Command::Stop = command {
                match endpoint.as_deref() {
                    Some(path) => {
                        let result = crate::control_stop::request(path);
                        // Acknowledgments describe this request, never a replayable cached stop.
                        let _ =
                            publisher.ack("stop", if result.is_ok() { "stopped" } else { "error" });
                        publisher.request_refresh();
                    }
                    None => log.line("stop owner unavailable"),
                }
            }
            transport::Urgent::Handled
        };
        let sink = |line: &str| log.line(line);
        let network = worker.run(
            &network_stop,
            transport::Hooks {
                on_progress: Some(&progress),
                on_urgent: Some(&urgent),
                log: Some(&sink),
            },
        );
        let unexpected = !stop.load(Ordering::Relaxed);
        // Cancel feeding and request stop before either explicit or implicit join.
        drop(shutdown);
        hardware
            .join()
            .map_err(|_| std::io::Error::other("hardware worker panicked"))?;
        network?;
        if unexpected {
            return Err(std::io::Error::other("hardware worker exited unexpectedly"));
        }
        Ok(())
    })
}
