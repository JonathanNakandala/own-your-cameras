//! MQTT 3.1.1 control and Home Assistant discovery. Hardware stays behind `Hardware`.
pub mod device;

use serde::Deserialize;
use std::collections::BTreeMap;

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

use serde_json::{Value, json};

mod motion;
pub use motion::Motion;

/// A command on its way to the hardware thread: the motion epoch it was accepted under and when it arrived.
#[derive(Debug)]
pub struct Queued {
    pub generation: u64,
    pub received: Duration,
    pub command: Command,
}

/// Drops every tap (`Position3d`) that a later tap follows with nothing but taps in between: each tap's target
/// is absolute from its own picture, so only the newest matters. Other commands keep their order and are never
/// dropped. Returns how many were dropped.
pub fn supersede(queue: &mut std::collections::VecDeque<Queued>) -> usize {
    let mut keep = Vec::with_capacity(queue.len());
    let mut pending_tap: Option<Queued> = None;
    let mut dropped = 0;
    for q in queue.drain(..) {
        match q.command {
            Command::Position3d(_) => {
                if pending_tap.replace(q).is_some() {
                    dropped += 1;
                }
            }
            _ => {
                if let Some(tap) = pending_tap.take() {
                    keep.push(tap);
                }
                keep.push(q);
            }
        }
    }
    if let Some(tap) = pending_tap {
        keep.push(tap);
    }
    queue.extend(keep);
    dropped
}

pub struct Publication {
    pub topic: String,
    /// Null means an empty retained tombstone (remove an obsolete optional entity).
    pub payload: Value,
    pub retain: bool,
}

impl Config {
    /// Discovery with `cam.toml`'s presets; [`Config::discovery_for`] with the presets in force.
    pub fn discovery(&self) -> Vec<Publication> {
        let names: Vec<String> = self.presets.keys().cloned().collect();
        self.discovery_for(&names)
    }

    /// Discovery whose preset select offers `presets` (the table in force: created and removed presets
    /// change it at runtime, and it is announced again then).
    pub fn discovery_for(&self, presets: &[String]) -> Vec<Publication> {
        use crate::settings::Section;
        let camera = &self.camera;
        let device = camera.client_id();
        // One device: the camera. A setting's section leads its name, so the configuration list on the device
        // page sorts section by section (Home Assistant groups entities on a device only by category; separate
        // linked devices per section were tried and rejected: the camera is one device).
        let device_block = json!({"identifiers":[device],"name":camera.name(),"manufacturer":"Reolink","model":"E1 Zoom (cam)"});
        let mut entities: Vec<(&str, String, String, Value)> = Vec::new();
        let mut add =
            |domain: &'static str, id: &str, name: &str, extra: Value, section: Option<Section>| {
                let name = match section {
                    Some(s) => format!("{} {name}", s.name()),
                    None => name.to_owned(),
                };
                entities.push((domain, id.to_owned(), name, extra));
            };
        add(
            "number",
            "pan",
            "Pan",
            json!({"min":0,"max":crate::ptz::PAN_MAX,"step":1,"mode":"box"}),
            None,
        );
        add(
            "number",
            "tilt",
            "Tilt",
            json!({"min":0,"max":crate::ptz::TILT_MAX,"step":1,"mode":"box"}),
            None,
        );
        add(
            "number",
            "zoom",
            "Zoom",
            json!({"min":0,"max":ZOOM_MAX,"step":1,"mode":"slider"}),
            None,
        );
        add(
            "select",
            "preset",
            "Preset",
            json!({"options":presets}),
            None,
        );
        add(
            "button",
            "reboot",
            "Reboot",
            json!({"payload_press":"PRESS","device_class":"restart","entity_category":"config"}),
            None,
        );
        add(
            "button",
            "refocus",
            "Refocus",
            json!({"payload_press":"PRESS","icon":"mdi:image-filter-center-focus"}),
            None,
        );
        add(
            "button",
            "calibrate_focus",
            "Calibrate focus (15 min)",
            json!({"payload_press":"PRESS","icon":"mdi:crosshairs-gps","entity_category":"config"}),
            Some(Section::Lens),
        );
        add(
            "sensor",
            "autofocus_state",
            "Autofocus",
            json!({"icon":"mdi:camera-iris","json_attributes_topic":camera.topic("autofocus_attributes/state")}),
            None,
        );
        add(
            "binary_sensor",
            "pipeline",
            "Pipeline health",
            json!({"payload_on":"ON","payload_off":"OFF"}),
            None,
        );
        add(
            "sensor",
            "light",
            "Light sensor",
            json!({"icon":"mdi:brightness-6","state_class":"measurement"}),
            None,
        );
        add(
            "sensor",
            "settings",
            "Settings",
            json!({"icon":"mdi:cog-outline"}),
            None,
        );
        for s in crate::settings::specs() {
            let (domain, mut extra) = match s.kind {
                crate::settings::Kind::Number {
                    min,
                    max,
                    unit,
                    slider,
                } => {
                    let mut e = json!({"min":min,"max":max,"step":1,"mode":if slider {"slider"} else {"box"}});
                    if let Some(u) = unit {
                        e["unit_of_measurement"] = json!(u);
                    }
                    ("number", e)
                }
                crate::settings::Kind::Switch => {
                    ("switch", json!({"payload_on":"ON","payload_off":"OFF"}))
                }
                crate::settings::Kind::Select(options) => ("select", json!({"options":options})),
            };
            extra["entity_category"] = json!("config");
            if let Some(i) = s.icon {
                extra["icon"] = json!(i);
            }
            add(domain, s.key, s.name, extra, s.section());
        }
        for t in crate::telemetry::SENSORS {
            let mut extra = json!({"state_class":"measurement"});
            if let Some(u) = t.unit {
                extra["unit_of_measurement"] = json!(u);
            }
            if let Some(c) = t.device_class {
                extra["device_class"] = json!(c);
            }
            if let Some(i) = t.icon {
                extra["icon"] = json!(i);
            }
            if t.key == "uptime" {
                extra["state_class"] = json!("total_increasing");
            }
            add("sensor", t.key, t.name, extra, None);
        }
        let mut messages = Vec::new();
        for (domain, id, name, extra) in entities {
            let mut payload = json!({
                "name":name, "unique_id":format!("{device}_{id}"),
                "device":device_block,
                "availability_topic":camera.availability(),
            });
            let obj = payload.as_object_mut().expect("JSON object literal");
            obj.extend(extra.as_object().expect("JSON object literal").clone());
            if matches!(domain, "sensor" | "binary_sensor") {
                obj.insert("entity_category".into(), json!("diagnostic"));
            } else {
                obj.insert(
                    "command_topic".into(),
                    json!(camera.topic(&format!("{id}/set"))),
                );
                obj.insert("retain".into(), json!(false));
            }
            if domain != "button" {
                obj.insert(
                    "state_topic".into(),
                    json!(camera.topic(&format!("{id}/state"))),
                );
            }
            messages.push(Publication {
                topic: format!("homeassistant/{domain}/{device}/{id}/config"),
                payload: if id == "preset" && presets.is_empty() {
                    Value::Null
                } else {
                    payload
                },
                retain: true,
            });
        }
        messages
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

impl Move {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            "zoom_in" => Self::ZoomIn,
            "zoom_out" => Self::ZoomOut,
            "stop" => Self::Stop,
            _ => return None,
        })
    }
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
        let number = |max| {
            text.parse::<u32>()
                .ok()
                .filter(|n| *n <= max)
                .ok_or("position outside travel")
        };
        let relative = |max: i32| {
            text.strip_prefix("relative:")
                .and_then(|n| n.parse::<i32>().ok())
                .filter(|n| (-max..=max).contains(n))
                .ok_or("relative position outside travel")
        };
        let cmd = match id {
            "pan" if text.starts_with("relative:") => {
                Command::PanBy(relative(crate::ptz::PAN_MAX as i32)?)
            }
            "tilt" if text.starts_with("relative:") => {
                Command::TiltBy(relative(crate::ptz::TILT_MAX as i32)?)
            }
            "zoom" if text.starts_with("relative:") => Command::ZoomBy(relative(ZOOM_MAX as i32)?),
            "pan" => Command::Pan(number(crate::ptz::PAN_MAX)?),
            "tilt" => Command::Tilt(number(crate::ptz::TILT_MAX)?),
            "zoom" => Command::Zoom(number(ZOOM_MAX)?),
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
            "move" => Command::Move(Move::parse(text).ok_or("invalid move")?),
            "motion" => {
                use crate::motion_control::Request;
                match Request::parse(text)? {
                    Request::Velocity(v) => Command::Velocity(v),
                    Request::Release => Command::Move(Move::Stop),
                    Request::Stop => Command::Stop,
                    Request::Home => Command::Home,
                    Request::Region {
                        start,
                        end,
                        zoom_out,
                    } => Command::Position3d(crate::position3d::Request {
                        start,
                        end,
                        operation: if zoom_out {
                            crate::position3d::Operation::ZoomOut
                        } else {
                            crate::position3d::Operation::ZoomTo
                        },
                    }),
                    Request::Preset(name) => Command::Preset(name.as_str()?.to_owned()),
                    Request::SavePreset(name) => Command::SavePreset(name.as_str()?.to_owned()),
                    Request::RemovePreset(name) => Command::RemovePreset(name.as_str()?.to_owned()),
                    Request::Point { x, y } => Command::Position3d(crate::position3d::Request {
                        start: (x, y),
                        end: (x, y),
                        operation: crate::position3d::Operation::ZoomTo,
                    }),
                }
            }
            "position3d" => Command::Position3d(crate::position3d::Request::parse(text)?),
            "focus" => Command::Focus(number(crate::zoom::FOCUS_MAX)?),
            "reboot" if text == "PRESS" => Command::Reboot,
            "refocus" if text == "PRESS" => Command::Refocus,
            "calibrate_focus" if text == "PRESS" => Command::CalibrateFocus,
            "stop" if text == "PRESS" => Command::Stop,
            "reboot" | "refocus" | "calibrate_focus" | "stop" => {
                return Err("invalid command payload".into());
            }
            _ => match crate::settings::spec(id) {
                Some(spec) => {
                    spec.parse(text)?;
                    Command::Setting(spec.key, text.trim().into())
                }
                None => return Ok(None),
            },
        };
        Ok(Some(cmd))
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
    fn motion_epoch(&self) -> Option<Arc<std::sync::atomic::AtomicU64>> {
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
    /// states are published now rather than at the next five-second refresh.
    fn changed(&mut self) -> bool {
        false
    }
    /// The preset names in force, when the hardware owns a preset table (`None`: `cam.toml`'s).
    fn preset_names(&self) -> Option<Vec<String>> {
        None
    }
}

impl Hardware for Box<dyn Hardware> {
    fn preset_names(&self) -> Option<Vec<String>> {
        (**self).preset_names()
    }
    fn failed(&self) -> bool {
        (**self).failed()
    }
    fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
        (**self).stop_endpoint()
    }
    fn motion_epoch(&self) -> Option<Arc<std::sync::atomic::AtomicU64>> {
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

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
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
use rumqttc::{Client, Event, LastWill, MqttOptions, Outgoing, Packet, QoS};

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
fn send(client: &Client, log: &LogBudget, topic: impl Into<String>, value: impl Into<Vec<u8>>) {
    if client
        .try_publish(topic, QoS::AtLeastOnce, true, value)
        .is_err()
    {
        log.line(format_args!("outbound queue full; update will retry"));
    }
}
struct LogBudget(std::sync::atomic::AtomicUsize);
impl LogBudget {
    // `fetch_update` is named `try_update` from Rust 1.99, which the supported 1.88 does not have.
    #[allow(deprecated)]
    fn line(&self, message: std::fmt::Arguments<'_>) {
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
fn announce(client: &Client, cfg: &Config, presets: &Mutex<Vec<String>>, log: &LogBudget) {
    let presets = presets.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for p in cfg.discovery_for(&presets) {
        send(
            client,
            log,
            p.topic,
            if p.payload.is_null() {
                Vec::new()
            } else {
                p.payload.to_string().into_bytes()
            },
        );
    }
    send(client, log, cfg.camera.availability(), "online");
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

/// Outbound request queue of the MQTT client (bounded; a full queue drops the update, which retries).
pub const OUTBOUND: usize = 256;

/// Idle cadence of the hardware thread; a hold asks for faster polls through `Hardware::tick`.
const HARDWARE_IDLE: Duration = Duration::from_millis(500);
/// The MQTT loop stops reporting progress when the telemetry thread has not finished a round for this
/// long, so a stalled telemetry read ages out under the `Mqtt` budget without blocking MQTT itself.
const TELEMETRY_STALL: Duration = Duration::from_secs(3 * crate::telemetry::INTERVAL.as_secs());

/// As [`run_with`], plus the diagnostic telemetry thread publishing every `telemetry::INTERVAL`.
pub fn run_full(
    cfg: Config,
    mut hw: impl Hardware,
    health: Arc<PipelineHealth>,
    stop: &AtomicBool,
    beats: Option<&Beats>,
    telemetry: Option<crate::telemetry::Task>,
    motion: Option<Arc<Motion>>,
) -> std::io::Result<()> {
    let Some(broker) = &cfg.mqtt else {
        return Ok(());
    };
    let camera = &cfg.camera;
    let mut options = MqttOptions::new(camera.client_id(), &broker.host, broker.port);
    options.set_keep_alive(Duration::from_secs(5));
    options.set_inflight(16);
    options.set_max_packet_size(16 * 1024, 16 * 1024);
    options.set_last_will(LastWill::new(
        camera.availability(),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    if !broker.username.is_empty() {
        options.set_credentials(&broker.username, &broker.password);
    }
    // Discovery and one state per entity are ~150 packets per refresh; 256 keeps the bound.
    let (client, mut connection) = Client::new(options, OUTBOUND);
    // recv_timeout cancels an in-progress CONNECT. Let the network's own timeout fire first,
    // otherwise a slow CONNACK is abandoned on every poll and the client can never connect.
    let mut network = connection.eventloop.network_options();
    network.set_connection_timeout(1);
    network.set_tcp_send_buffer_size(16 * 1024);
    network.set_tcp_recv_buffer_size(16 * 1024);
    connection.eventloop.set_network_options(network);
    let log = LogBudget(std::sync::atomic::AtomicUsize::new(64));
    let connected = AtomicBool::new(false);
    let refresh = AtomicBool::new(true);
    let endpoint = hw.stop_endpoint();
    let epoch = hw.motion_epoch().unwrap_or_default();
    // The queue, clock and preset list other services (ONVIF) share with MQTT.
    let motion = motion.unwrap_or_default();
    let rx = motion
        .take_receiver()
        .ok_or_else(|| std::io::Error::other("the motion queue already has an owner"))?;
    let tx = motion.sender();
    *motion.presets.lock().unwrap_or_else(|e| e.into_inner()) = hw
        .preset_names()
        .unwrap_or_else(|| cfg.presets.keys().cloned().collect());
    if let Some(epoch) = hw.motion_epoch() {
        motion.attach(epoch, endpoint.clone());
    }
    let presets = &motion.presets;
    let config = &cfg;
    // The one clock: commands are stamped on it when they arrive, the hardware thread ticks on it.
    let started = motion.started;
    let telemetry_round = std::sync::atomic::AtomicU64::new(0);
    let hardware_result = std::thread::scope(|scope| -> std::io::Result<()> {
        let shutdown = HardwareShutdown {
            stop,
            _lease: beats.map(Beats::lease),
        };
        let (client, connected, refresh, log) = (&client, &connected, &refresh, &log);
        let hardware_beats = beats.cloned();
        let lease = beats.map(Beats::lease);
        let hardware_epoch = epoch.clone();
        let telemetry_round = &telemetry_round;
        let watching_telemetry = telemetry.is_some();
        if let Some(mut task) = telemetry {
            let lease = beats.map(Beats::lease);
            std::thread::Builder::new()
                .name("telemetry".into())
                .spawn_scoped(scope, move || {
                    let _lease = lease;
                    let mut next = Instant::now();
                    while !stop.load(Ordering::Relaxed) {
                        if Instant::now() >= next {
                            let states = task.sample(started.elapsed());
                            if connected.load(Ordering::Relaxed) {
                                for (id, value) in states {
                                    send(client, log, camera.topic(&format!("{id}/state")), value);
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
        let hardware = std::thread::Builder::new()
            .name("mqtt-hardware".into())
            .spawn_scoped(scope, move || {
                let _lease = lease;
                let beat = || {
                    if let Some(beats) = &hardware_beats {
                        beats.beat(Progress::Hardware);
                    }
                };
                beat();
                let mut next = Instant::now();
                let mut wait = HARDWARE_IDLE;
                let mut queue: std::collections::VecDeque<Queued> =
                    std::collections::VecDeque::new();
                let tick =
                    |hw: &mut _, wait: &mut Duration| match Hardware::tick(hw, started.elapsed()) {
                        Ok(w) => *wait = w.unwrap_or(HARDWARE_IDLE).min(HARDWARE_IDLE),
                        Err(e) => {
                            *wait = HARDWARE_IDLE;
                            log.line(format_args!("hardware tick failed: {e}"));
                        }
                    };
                while !stop.load(Ordering::Relaxed) {
                    let next_command = match queue.pop_front() {
                        Some(q) => Ok(q),
                        None => rx.recv_timeout(wait),
                    };
                    let command = match next_command {
                        Ok(q) => {
                            // Everything else that already arrived: a tap behind a newer tap is dropped.
                            queue.push_front(q);
                            while let Ok(more) = rx.try_recv() {
                                queue.push_back(more);
                            }
                            let dropped = supersede(&mut queue);
                            if dropped > 0 {
                                log.line(format_args!("position3d superseded ({dropped})"));
                            }
                            let Some(q) = queue.pop_front() else {
                                continue;
                            };
                            // tick services idle stop requests before accepting queued motion.
                            if hw.failed() {
                                return;
                            }
                            tick(&mut hw, &mut wait);
                            if hw.failed() {
                                return;
                            }
                            if q.generation != hardware_epoch.load(Ordering::Acquire) {
                                continue;
                            }
                            if let Err(e) = hw.apply_at(&q.command, q.received) {
                                log.line(format_args!("hardware command failed: {e}"));
                            }
                            // A created or removed preset changes the select's options: announce again.
                            if let Some(names) = hw.preset_names() {
                                let changed = {
                                    let mut held =
                                        presets.lock().unwrap_or_else(|e| e.into_inner());
                                    let changed = *held != names;
                                    *held = names;
                                    changed
                                };
                                if changed && connected.load(Ordering::Relaxed) {
                                    announce(client, config, presets, log);
                                }
                            }
                            true
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                        Err(_) => break,
                    };
                    if hw.failed() {
                        return;
                    }
                    tick(&mut hw, &mut wait);
                    if hw.failed() {
                        return;
                    }
                    if connected.load(Ordering::Relaxed)
                        && (command
                            || hw.changed()
                            || refresh.swap(false, Ordering::Relaxed)
                            || Instant::now() >= next)
                    {
                        match hw.states() {
                            Ok(mut states) => {
                                states.insert(
                                    "pipeline".into(),
                                    if health.healthy() { "ON" } else { "OFF" }.into(),
                                );
                                // Clear stale states rather than keep the previous preset or unavailable sensor reading.
                                for id in ["pan", "tilt", "preset"] {
                                    states.entry(id.into()).or_default();
                                }
                                for (id, value) in states {
                                    send(client, log, camera.topic(&format!("{id}/state")), value);
                                }
                            }
                            Err(e) => log.line(format_args!("state read failed: {e}")),
                        }
                        next = Instant::now() + Duration::from_secs(5);
                    }
                    beat();
                }
                hw.shutdown();
            })?;
        while !stop.load(Ordering::Relaxed) && !hardware.is_finished() {
            match connection.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(Event::Incoming(Packet::ConnAck(ack)))) => {
                    // CONNECT always requests a clean session. Accepting a resumed session can
                    // retain/replay pending requests across repeated broken connections.
                    if ack.session_present {
                        return Err(std::io::Error::other("broker resumed a clean MQTT session"));
                    }
                    let _ = client.try_subscribe(camera.topic("+/set"), QoS::AtLeastOnce);
                    let _ = client.try_subscribe("homeassistant/status", QoS::AtLeastOnce);
                    announce(client, &cfg, presets, log);
                    refresh.store(true, Ordering::Relaxed);
                    connected.store(true, Ordering::Relaxed);
                    log.line(format_args!("connected, discovery published"));
                }
                Ok(Ok(Event::Incoming(Packet::Publish(p)))) => {
                    if p.topic == "homeassistant/status" && p.payload.as_ref() == b"online" {
                        announce(client, &cfg, presets, log);
                        refresh.store(true, Ordering::Relaxed);
                    } else {
                        match cfg.command(&p.topic, &p.payload, p.retain) {
                            Ok(Some(Command::Stop)) => match endpoint.as_deref() {
                                Some(path) => {
                                    let result = crate::control_stop::request(path);
                                    // Acknowledgments describe this request, never a replayable cached stop.
                                    let _ = client.try_publish(
                                        camera.topic("stop/state"),
                                        QoS::AtLeastOnce,
                                        false,
                                        if result.is_ok() { "stopped" } else { "error" },
                                    );
                                    refresh.store(true, Ordering::Relaxed);
                                }
                                None => log.line(format_args!("stop owner unavailable")),
                            },
                            Ok(Some(c)) => {
                                if tx
                                    .try_send(Queued {
                                        generation: epoch.load(Ordering::Acquire),
                                        received: started.elapsed(),
                                        command: c,
                                    })
                                    .is_err()
                                {
                                    log.line(format_args!("command queue full; command rejected"));
                                }
                            }
                            Ok(None) => {}
                            Err(e) => log.line(format_args!("command rejected: {e}")),
                        }
                    }
                }
                Ok(Err(_)) => {
                    connected.store(false, Ordering::Relaxed);
                    // There is no resumable session; discovery/state are regenerated on CONNECT.
                    connection.eventloop.pending.clear();
                    log.line(format_args!("broker unavailable; reconnecting"));
                    // Interruptible retry delay; no tight loop when the broker refuses credentials.
                    for _ in 0..10 {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                _ => {}
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
        }
        let unexpected = !stop.load(Ordering::Relaxed);
        // Cancel feeding and request stop before either explicit or implicit join.
        drop(shutdown);
        hardware
            .join()
            .map_err(|_| std::io::Error::other("hardware worker panicked"))?;
        if unexpected {
            return Err(std::io::Error::other("hardware worker exited unexpectedly"));
        }
        Ok(())
    });
    // QoS0 is a drain marker behind all worker publications. Wait for outstanding QoS1 ACKs
    // before disconnecting, so closing a socket with unread ACKs cannot reset its final writes.
    let _ = client.try_publish(camera.availability(), QoS::AtMostOnce, true, "offline");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut marker = false;
    let mut disconnecting = false;
    while Instant::now() < deadline {
        match connection.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(Event::Outgoing(Outgoing::Publish(0)))) => marker = true,
            Ok(Ok(Event::Outgoing(Outgoing::Disconnect))) | Ok(Err(_)) => break,
            _ => {}
        }
        if marker && connection.eventloop.state.inflight() == 0 && !disconnecting {
            disconnecting = client.try_disconnect().is_ok();
        }
    }
    hardware_result
}

#[cfg(test)]
mod supersede_tests {
    use super::*;
    use std::collections::VecDeque;

    fn tap(received: u64) -> Queued {
        Queued {
            generation: 0,
            received: Duration::from_millis(received),
            command: Command::Position3d(
                crate::position3d::Request::parse(
                    r#"{"start_x":10,"start_y":10,"end_x":10,"end_y":10,"operation":"zoomto"}"#,
                )
                .unwrap(),
            ),
        }
    }

    fn other(received: u64, command: Command) -> Queued {
        Queued {
            generation: 0,
            received: Duration::from_millis(received),
            command,
        }
    }

    fn kinds(q: &VecDeque<Queued>) -> Vec<String> {
        q.iter()
            .map(|q| match &q.command {
                Command::Position3d(_) => format!("tap@{}", q.received.as_millis()),
                c => format!("{c:?}"),
            })
            .collect()
    }

    #[test]
    fn two_taps_in_a_row_keep_only_the_newer() {
        let mut q: VecDeque<Queued> = [tap(1), tap(2), tap(3)].into();
        assert_eq!(supersede(&mut q), 2);
        assert_eq!(kinds(&q), ["tap@3"]);
    }

    #[test]
    fn a_tap_followed_by_a_stop_keeps_both_in_order() {
        let mut q: VecDeque<Queued> = [tap(1), other(2, Command::Stop)].into();
        assert_eq!(supersede(&mut q), 0);
        assert_eq!(kinds(&q), ["tap@1", "Stop"]);
    }

    #[test]
    fn a_non_tap_between_two_taps_keeps_both_taps() {
        let mut q: VecDeque<Queued> = [tap(1), other(2, Command::Pan(5)), tap(3), tap(4)].into();
        assert_eq!(supersede(&mut q), 1);
        assert_eq!(kinds(&q), ["tap@1", "Pan(5)", "tap@4"]);
    }
}
