//! MQTT delivery owns no hardware. The application supplies its real capability
//! catalog and state samples; no sensor value or successful command is invented.
use rumqttc::{Client, Connection, Event, LastWill, MqttOptions, Outgoing, Packet, QoS};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

/// Fixed catalog bound; queued messages and each descriptor remain bounded too.
pub const MAX_ENTITIES: usize = 96;

pub struct Broker {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

pub struct Config {
    pub broker: Broker,
    pub node_id: String,
    pub device_name: String,
    pub manufacturer: String,
    pub model: String,
    pub software_version: String,
    pub prefix: String,
    pub discovery_prefix: String,
    pub entities: Vec<Entity>,
    /// Enables only the shared wire route; the application owns optics policy.
    pub position3d: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Absolute(i32),
    Relative(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}
impl Direction {
    /// Screen coordinates: positive pan is right, positive tilt is up.
    pub const fn axes(self) -> (i32, i32) {
        match self {
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
            Self::Up => (0, 1),
            Self::Down => (0, -1),
            Self::UpLeft => (-1, 1),
            Self::UpRight => (1, 1),
            Self::DownLeft => (-1, -1),
            Self::DownRight => (1, -1),
        }
    }
}

fn vector_command(text: &str) -> Option<Command> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Vector {
        pan: i8,
        tilt: i8,
    }
    let value: Vector = serde_json::from_str(text).ok()?;
    Some(match (value.pan, value.tilt) {
        (-1, 0) => Command::Move(Direction::Left),
        (1, 0) => Command::Move(Direction::Right),
        (0, 1) => Command::Move(Direction::Up),
        (0, -1) => Command::Move(Direction::Down),
        (-1, 1) => Command::Move(Direction::UpLeft),
        (1, 1) => Command::Move(Direction::UpRight),
        (-1, -1) => Command::Move(Direction::DownLeft),
        (1, -1) => Command::Move(Direction::DownRight),
        (0, 0) => Command::Release,
        _ => return None,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrMode {
    Day,
    Night,
    Auto,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Pan(Position),
    Tilt(Position),
    Move(Direction),
    Velocity(crate::motion_control::Velocity),
    Home,
    Preset(crate::motion_control::PresetName),
    SavePreset(crate::motion_control::PresetName),
    RemovePreset(crate::motion_control::PresetName),
    Position3d(crate::position3d::Request),
    Stop,
    /// Graceful held-motion release; explicit Stop remains the hard stop.
    Release,
    IrMode(IrMode),
    IrLight(bool),
    Led(bool),
    Reboot,
    MicVolume(u8),
    SpeakerVolume(u8),
    /// Opaque setting identity and validated scalar; hardware policy belongs to
    /// the injected application catalog, never to this transport.
    Setting {
        slot: u16,
        value: i32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Number,
    Button,
    Select,
    Switch,
    Sensor,
    BinarySensor,
}
impl Domain {
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Button => "button",
            Self::Select => "select",
            Self::Switch => "switch",
            Self::Sensor => "sensor",
            Self::BinarySensor => "binary_sensor",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub domain: Domain,
    pub minimum: i32,
    pub maximum: i32,
    pub options: Vec<String>,
    pub unit: Option<String>,
    pub diagnostic: bool,
    pub control_slot: Option<u16>,
}

impl Entity {
    /// Candidate catalog. The caller MUST remove capabilities not qualified on
    /// its hardware. Position limits come from the hardware owner's one source.
    pub fn mi360_catalog(pan_max: i32, tilt_max: i32) -> Vec<Self> {
        let mut result = Vec::new();
        for (id, name, domain, maximum, unit) in [
            ("pan", "Pan", Domain::Number, pan_max, None),
            ("tilt", "Tilt", Domain::Number, tilt_max, None),
            ("stop", "Stop", Domain::Button, 0, None),
            ("ir_mode", "IR mode", Domain::Select, 0, None),
            ("ir_lamp", "IR lamp", Domain::Switch, 0, None),
            ("status_led", "Status LED", Domain::Switch, 0, None),
            (
                "mic_volume",
                "Microphone volume",
                Domain::Number,
                100,
                Some("%"),
            ),
            (
                "speaker_volume",
                "Speaker volume",
                Domain::Number,
                100,
                Some("%"),
            ),
            ("reboot", "Reboot", Domain::Button, 0, None),
            (
                "pipeline_health",
                "Pipeline health",
                Domain::BinarySensor,
                0,
                None,
            ),
            ("uptime", "Uptime", Domain::Sensor, 0, Some("s")),
            ("cpu_busy", "CPU busy", Domain::Sensor, 0, Some("%")),
            (
                "memory_available",
                "Memory available",
                Domain::Sensor,
                0,
                Some("MiB"),
            ),
            (
                "daemon_memory",
                "Daemon memory",
                Domain::Sensor,
                0,
                Some("MiB"),
            ),
            (
                "wifi_signal",
                "Wi-Fi signal",
                Domain::Sensor,
                0,
                Some("dBm"),
            ),
            ("main_fps", "Main FPS", Domain::Sensor, 0, Some("fps")),
            ("sub_fps", "Sub FPS", Domain::Sensor, 0, Some("fps")),
            (
                "main_bitrate",
                "Main bitrate",
                Domain::Sensor,
                0,
                Some("kbit/s"),
            ),
            (
                "sub_bitrate",
                "Sub bitrate",
                Domain::Sensor,
                0,
                Some("kbit/s"),
            ),
        ] {
            result.push(Self {
                id: id.into(),
                name: name.into(),
                domain,
                minimum: 0,
                maximum,
                options: if id == "ir_mode" {
                    vec!["day".into(), "night".into()]
                } else {
                    vec![]
                },
                unit: unit.map(str::to_owned),
                diagnostic: matches!(domain, Domain::Sensor | Domain::BinarySensor),
                control_slot: None,
            });
        }
        result
    }
}

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn topic_root(text: &str) -> bool {
    !text.is_empty() && text.len() <= 128 && text.split('/').all(identifier)
}

fn number_payload(entity: &Entity, text: &str) -> Option<i32> {
    let text = text.strip_suffix(".0").unwrap_or(text);
    text.parse::<i32>()
        .ok()
        .filter(|value| *value >= entity.minimum && *value <= entity.maximum)
}

impl Config {
    fn validate(&self) -> io::Result<()> {
        let ids: BTreeSet<_> = self.entities.iter().map(|e| e.id.as_str()).collect();
        if self.broker.host.trim().is_empty()
            || self.broker.port == 0
            || !identifier(&self.node_id)
            || !topic_root(&self.prefix)
            || !topic_root(&self.discovery_prefix)
            || self.entities.is_empty()
            || self.entities.len() > MAX_ENTITIES
            || [
                &self.device_name,
                &self.manufacturer,
                &self.model,
                &self.software_version,
            ]
            .iter()
            .any(|value| value.len() > 128)
            || ids.len() != self.entities.len()
            || self.entities.iter().any(|e| {
                !identifier(&e.id)
                    || e.name.len() > 128
                    || e.options.len() > 8
                    || e.options.iter().any(|option| !identifier(option))
                    || e.unit.as_ref().is_some_and(|unit| unit.len() > 32)
                    || (e.domain == Domain::Number && e.minimum > e.maximum)
                    || (e.control_slot.is_some()
                        && !matches!(
                            e.domain,
                            Domain::Number | Domain::Select | Domain::Switch | Domain::Button
                        ))
            })
            || ((ids.contains("pan") || ids.contains("tilt")) && !ids.contains("stop"))
        {
            return Err(io::Error::other(
                "invalid MQTT identity, capability catalog or broker",
            ));
        }
        Ok(())
    }

    pub fn discovery(&self) -> io::Result<Vec<(String, Vec<u8>)>> {
        self.validate()?;
        let device = json!({"identifiers":[self.node_id],"name":self.device_name,"manufacturer":self.manufacturer,"model":self.model,"sw_version":self.software_version});
        self.entities.iter().map(|e| {
            let domain=e.domain.name();
            let name_prefix=self.node_id.strip_prefix("cam_").unwrap_or(&self.node_id);
            let mut value=json!({"name":e.name,"unique_id":format!("{}_{}",self.node_id,e.id),"default_entity_id":format!("{domain}.{name_prefix}_{}",e.id),"device":device,"availability_topic":format!("{}/availability",self.prefix)});
            if e.domain!=Domain::Button { value["state_topic"]=json!(format!("{}/{}/state",self.prefix,e.id)); }
            if !matches!(e.domain,Domain::Sensor|Domain::BinarySensor) { value["command_topic"]=json!(format!("{}/{}/set",self.prefix,e.id)); }
            match e.domain {
                Domain::Number=> { value["min"]=json!(e.minimum);value["max"]=json!(e.maximum);value["step"]=json!(1);value["mode"]=json!("slider"); }
                Domain::Button=> {value["payload_press"]=json!("PRESS"); if e.id=="reboot" {value["device_class"]=json!("restart");} }
                Domain::Select=> {value["options"]=json!(e.options);}
                Domain::Switch|Domain::BinarySensor=> {value["payload_on"]=json!("ON");value["payload_off"]=json!("OFF");}
                Domain::Sensor=> {if e.unit.is_some() {value["state_class"]=json!("measurement");}}
            }
            if let Some(unit)=&e.unit {value["unit_of_measurement"]=json!(unit);}
            if e.diagnostic {value["entity_category"]=json!("diagnostic");}
            if e.control_slot.is_some() {value["entity_category"]=json!("config");}
            let topic=format!("{}/{}/{}/{}/config",self.discovery_prefix,domain,self.node_id,e.id);
            serde_json::to_vec(&value).map(|bytes|(topic,bytes)).map_err(|_|io::Error::other("cannot encode discovery"))
        }).collect()
    }

    fn packet_command(&self, packet: &rumqttc::Publish) -> Option<Command> {
        if packet.retain || packet.dup {
            return None;
        }
        self.command(&packet.topic, &packet.payload)
    }

    fn command(&self, topic: &str, payload: &[u8]) -> Option<Command> {
        let id = topic
            .strip_prefix(&format!("{}/", self.prefix))?
            .strip_suffix("/set")?;
        // JSON contains four named coordinates plus an optional operation;
        // keep scalar limits unchanged and bound JSON before serde allocates.
        let maximum = if matches!(id, "position3d" | "motion") {
            256
        } else {
            64
        };
        if payload.len() > maximum {
            return None;
        }
        let raw = std::str::from_utf8(payload).ok()?;
        if id == "motion" {
            use crate::motion_control::Request;
            return Some(match Request::parse(raw).ok()? {
                Request::Velocity(v) => Command::Velocity(v),
                Request::Release => Command::Release,
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
                Request::Preset(name) => Command::Preset(name),
                Request::SavePreset(name) => Command::SavePreset(name),
                Request::RemovePreset(name) => Command::RemovePreset(name),
                Request::Point { x, y } => Command::Position3d(crate::position3d::Request {
                    start: (x, y),
                    end: (x, y),
                    operation: crate::position3d::Operation::ZoomTo,
                }),
            });
        }
        if id == "position3d" {
            // Preserve canonical JSON whitespace/validation semantics too.
            return self
                .position3d
                .then(|| crate::position3d::Request::parse(raw).ok())
                .flatten()
                .map(Command::Position3d);
        }
        let text = raw.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return None;
        }
        if let Some(entity) = self
            .entities
            .iter()
            .find(|entity| entity.id == id && entity.control_slot.is_some())
        {
            let value = match entity.domain {
                Domain::Number => number_payload(entity, text)?,
                Domain::Select => {
                    i32::try_from(entity.options.iter().position(|option| option == text)?).ok()?
                }
                Domain::Switch => match text {
                    "ON" => 1,
                    "OFF" => 0,
                    _ => return None,
                },
                Domain::Button => match text {
                    "PRESS" => 0,
                    _ => return None,
                },
                _ => return None,
            };
            return Some(Command::Setting {
                slot: entity.control_slot?,
                value,
            });
        }
        if id == "move"
            && self
                .entities
                .iter()
                .any(|e| e.id == "pan" || e.id == "tilt")
        {
            return match text {
                "left" => Some(Command::Move(Direction::Left)),
                "right" => Some(Command::Move(Direction::Right)),
                "up" => Some(Command::Move(Direction::Up)),
                "down" => Some(Command::Move(Direction::Down)),
                "stop" => Some(Command::Stop),
                "release" => Some(Command::Release),
                _ => vector_command(text),
            };
        }
        if let Some(axis) = id.strip_suffix("_relative") {
            if !self.entities.iter().any(|e| e.id == axis) {
                return None;
            }
            let steps = text
                .parse::<i32>()
                .ok()
                .filter(|v| v.unsigned_abs() <= 256)?;
            return match axis {
                "pan" => Some(Command::Pan(Position::Relative(steps))),
                "tilt" => Some(Command::Tilt(Position::Relative(steps))),
                _ => None,
            };
        }
        let entity = self.entities.iter().find(|e| e.id == id)?;
        let number = || number_payload(entity, text);
        let boolean = || match text {
            "ON" => Some(true),
            "OFF" => Some(false),
            _ => None,
        };
        match id {
            "pan" => number().map(|p| Command::Pan(Position::Absolute(p))),
            "tilt" => number().map(|p| Command::Tilt(Position::Absolute(p))),
            "mic_volume" => number()
                .and_then(|n| u8::try_from(n).ok())
                .map(Command::MicVolume),
            "speaker_volume" => number()
                .and_then(|n| u8::try_from(n).ok())
                .map(Command::SpeakerVolume),
            "stop" if text == "PRESS" => Some(Command::Stop),
            "home" if text == "PRESS" => Some(Command::Home),
            "reboot" if text == "PRESS" => Some(Command::Reboot),
            "ir_lamp" => boolean().map(Command::IrLight),
            "status_led" => boolean().map(Command::Led),
            "ir_mode" if entity.options.iter().any(|v| v == text) => match text {
                "day" => Some(Command::IrMode(IrMode::Day)),
                "night" => Some(Command::IrMode(IrMode::Night)),
                "auto" => Some(Command::IrMode(IrMode::Auto)),
                _ => None,
            },
            _ => None,
        }
    }
}

struct Update {
    id: String,
    value: String,
}
#[derive(Clone)]
pub struct Publisher {
    tx: SyncSender<Update>,
    ids: Arc<BTreeSet<String>>,
    online: Arc<AtomicBool>,
}
impl Publisher {
    pub fn is_connected(&self) -> bool {
        self.online.load(Ordering::Acquire)
    }
    pub fn state(&self, id: &str, value: impl Into<String>) -> io::Result<()> {
        let value = value.into();
        if !self.ids.contains(id) || value.len() > 512 {
            return Err(io::Error::other("unknown or oversized MQTT state"));
        }
        self.tx
            .try_send(Update {
                id: id.into(),
                value,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "MQTT state queue unavailable"))
    }
}

struct Queued {
    command: Command,
    received: Instant,
    generation: u32,
}
// One network producer publishes the new epoch before the urgent kind, and
// before it can queue later motion. Priority is monotonic while pending: a
// graceful Release must never downgrade explicit/disconnect Stop.
fn interrupt_commands(generation: &AtomicU32, urgent_kind: &AtomicU32, command: Command) -> bool {
    let kind = match command {
        Command::Release => 1,
        Command::Stop => 2,
        _ => return false,
    };
    generation.fetch_add(1, Ordering::AcqRel);
    urgent_kind.fetch_max(kind, Ordering::AcqRel);
    true
}

pub struct Commands {
    rx: Receiver<Queued>,
    generation: Arc<AtomicU32>,
    urgent_kind: Arc<AtomicU32>,
}
impl Commands {
    /// Stop/Release invalidate older queued commands and bypass a full queue.
    /// Fresh commands in the new epoch remain ordered after the urgent command.
    pub fn try_next(&self, now: Instant) -> Option<Command> {
        match self.urgent_kind.swap(0, Ordering::AcqRel) {
            0 => {}
            1 => return Some(Command::Release),
            _ => return Some(Command::Stop),
        }
        while let Ok(item) = self.rx.try_recv() {
            if item.generation == self.generation.load(Ordering::Acquire)
                && now.saturating_duration_since(item.received) <= Duration::from_secs(2)
            {
                return Some(item.command);
            }
        }
        None
    }
}

pub struct Worker {
    config: Config,
    client: Client,
    connection: Connection,
    updates: Receiver<Update>,
    commands: SyncSender<Queued>,
    generation: Arc<AtomicU32>,
    urgent_kind: Arc<AtomicU32>,
    online: Arc<AtomicBool>,
}

pub fn channel(config: Config) -> io::Result<(Publisher, Commands, Worker)> {
    config.validate()?;
    let mut options = MqttOptions::new(&config.node_id, &config.broker.host, config.broker.port);
    options
        .set_clean_session(true)
        .set_keep_alive(Duration::from_secs(5))
        .set_inflight(16)
        .set_max_packet_size(8192, 8192);
    options.set_last_will(LastWill::new(
        format!("{}/availability", config.prefix),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    if !config.broker.username.is_empty() {
        options.set_credentials(&config.broker.username, &config.broker.password);
    }
    let (client, mut connection) = Client::new(options, 128);
    let mut network = connection.eventloop.network_options();
    network.set_connection_timeout(1);
    network.set_tcp_send_buffer_size(16 * 1024);
    network.set_tcp_recv_buffer_size(16 * 1024);
    connection.eventloop.set_network_options(network);
    let (tx, updates) = sync_channel(64);
    let (commands, rx) = sync_channel(8);
    let generation = Arc::new(AtomicU32::new(0));
    let urgent_kind = Arc::new(AtomicU32::new(0));
    let online = Arc::new(AtomicBool::new(false));
    let publisher = Publisher {
        tx,
        ids: Arc::new(config.entities.iter().map(|e| e.id.clone()).collect()),
        online: online.clone(),
    };
    let receiver = Commands {
        rx,
        generation: generation.clone(),
        urgent_kind: urgent_kind.clone(),
    };
    Ok((
        publisher,
        receiver,
        Worker {
            config,
            client,
            connection,
            updates,
            commands,
            generation,
            urgent_kind,
            online,
        },
    ))
}

impl Worker {
    fn cancel_commands(&self) {
        interrupt_commands(&self.generation, &self.urgent_kind, Command::Stop);
    }

    pub fn run(mut self, stop: &AtomicBool, progress: Option<&dyn Fn()>) -> io::Result<()> {
        let mut connected = false;
        let mut states = BTreeMap::<String, String>::new();
        let mut pending = BTreeMap::<String, Vec<u8>>::new();
        while !stop.load(Ordering::Relaxed) {
            for _ in 0..64 {
                let Ok(update) = self.updates.try_recv() else {
                    break;
                };
                if connected {
                    pending.insert(
                        format!("{}/{}/state", self.config.prefix, update.id),
                        update.value.as_bytes().to_vec(),
                    );
                }
                states.insert(update.id, update.value);
            }
            for _ in 0..32 {
                let Some((topic, payload)) = pending.pop_first() else {
                    break;
                };
                if self
                    .client
                    .try_publish(&topic, QoS::AtLeastOnce, true, payload.clone())
                    .is_err()
                {
                    pending.insert(topic, payload);
                    break;
                }
            }
            let mut announce = false;
            match self.connection.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(Event::Incoming(Packet::ConnAck(ack)))) => {
                    if ack.session_present {
                        return Err(io::Error::other("MQTT resumed a clean session"));
                    }
                    self.cancel_commands();
                    self.online.store(true, Ordering::Release);
                    connected = true;
                    announce = true;
                    let _ = self
                        .client
                        .try_subscribe(format!("{}/+/set", self.config.prefix), QoS::AtLeastOnce);
                    let _ = self
                        .client
                        .try_subscribe("homeassistant/status", QoS::AtLeastOnce);
                }
                Ok(Ok(Event::Incoming(Packet::Publish(packet)))) => {
                    if packet.topic == "homeassistant/status"
                        && packet.payload.as_ref() == b"online"
                    {
                        announce = true;
                    } else if let Some(command) = self.config.packet_command(&packet)
                        && !interrupt_commands(&self.generation, &self.urgent_kind, command)
                    {
                        let _ = self.commands.try_send(Queued {
                            command,
                            received: Instant::now(),
                            generation: self.generation.load(Ordering::Acquire),
                        });
                    }
                }
                Ok(Err(_)) => {
                    self.online.store(false, Ordering::Release);
                    connected = false;
                    pending.clear();
                    self.connection.eventloop.pending.clear();
                    self.cancel_commands();
                    for _ in 0..10 {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                _ => {}
            }
            if announce {
                pending.clear();
                for (topic, payload) in self.config.discovery()? {
                    pending.insert(topic, payload);
                }
                for (id, value) in &states {
                    pending.insert(
                        format!("{}/{id}/state", self.config.prefix),
                        value.as_bytes().to_vec(),
                    );
                }
                pending.insert(
                    format!("{}/availability", self.config.prefix),
                    b"online".to_vec(),
                );
            }
            if let Some(progress) = progress {
                progress();
            }
        }
        self.cancel_commands();
        let _ = self.client.try_publish(
            format!("{}/availability", self.config.prefix),
            QoS::AtMostOnce,
            true,
            "offline",
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut marker = false;
        let mut disconnecting = false;
        while Instant::now() < deadline {
            match self.connection.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(Event::Outgoing(Outgoing::Publish(0)))) => marker = true,
                Ok(Ok(Event::Outgoing(Outgoing::Disconnect))) | Ok(Err(_)) => break,
                _ => {}
            }
            if marker && self.connection.eventloop.state.inflight() == 0 && !disconnecting {
                disconnecting = self.client.try_disconnect().is_ok();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn config() -> Config {
        Config {
            broker: Broker {
                host: "127.0.0.1".into(),
                port: 1883,
                username: String::new(),
                password: String::new(),
            },
            node_id: "cam_mi360".into(),
            device_name: "Mi360".into(),
            manufacturer: "Xiaomi".into(),
            model: "MJSXJ10CM".into(),
            software_version: "test".into(),
            prefix: "camera/mi360".into(),
            discovery_prefix: "homeassistant".into(),
            entities: Entity::mi360_catalog(2404, 713),
            position3d: false,
        }
    }
    #[test]
    fn discovery_is_one_device_with_only_injected_capabilities_and_no_fake_states() {
        let mut cfg = config();
        cfg.entities
            .retain(|e| ["pan", "stop", "pipeline_health"].contains(&e.id.as_str()));
        let publications = cfg.discovery().unwrap();
        assert_eq!(publications.len(), 3);
        assert_eq!(
            publications[0].0,
            "homeassistant/number/cam_mi360/pan/config"
        );
        let health: Value = serde_json::from_slice(&publications[2].1).unwrap();
        assert_eq!(
            health["default_entity_id"],
            "binary_sensor.mi360_pipeline_health"
        );
        assert_eq!(health["device"]["identifiers"], json!(["cam_mi360"]));
        assert_eq!(health["state_topic"], "camera/mi360/pipeline_health/state");
        assert!(health.get("value_template").is_none());
    }
    #[test]
    fn command_parser_checks_scope_real_capabilities_and_injected_limits() {
        let mut cfg = config();
        assert_eq!(
            cfg.command("camera/mi360/pan/set", b"2404"),
            Some(Command::Pan(Position::Absolute(2404)))
        );
        assert_eq!(
            cfg.command("camera/mi360/tilt/set", b"713.0"),
            Some(Command::Tilt(Position::Absolute(713)))
        );
        for (topic, payload) in [
            ("camera/other/pan/set", "5"),
            ("camera/mi360/pan/set", "2405"),
            ("camera/mi360/tilt/set", "NaN"),
            ("camera/mi360/tilt/set", "-1"),
            ("camera/mi360/ir_mode/set", "auto"),
            ("camera/mi360/reboot/set", "ON"),
            ("camera/mi360/pan_relative/set", "257"),
        ] {
            assert!(cfg.command(topic, payload.as_bytes()).is_none());
        }
        assert_eq!(
            cfg.command("camera/mi360/stop/set", b"PRESS"),
            Some(Command::Stop)
        );
        cfg.entities.retain(|e| e.id != "speaker_volume");
        assert!(
            cfg.command("camera/mi360/speaker_volume/set", b"10")
                .is_none()
        );
    }
    fn setting(id: &str, domain: Domain, slot: u16) -> Entity {
        Entity {
            id: id.into(),
            name: "Injected setting".into(),
            domain,
            minimum: -3,
            maximum: 4,
            options: vec!["first".into(), "second".into()],
            unit: None,
            diagnostic: false,
            control_slot: Some(slot),
        }
    }
    #[test]
    fn injected_settings_obey_the_advertised_number_select_and_boolean_contracts() {
        let mut cfg = config();
        cfg.entities.extend([
            setting("adjustment", Domain::Number, 17),
            setting("profile", Domain::Select, 42),
            setting("toggle", Domain::Switch, u16::MAX),
        ]);
        for (id, payload, slot, value) in [
            ("adjustment", "-3", 17, -3),
            ("adjustment", "4", 17, 4),
            ("adjustment", "3.0", 17, 3),
            ("profile", "first", 42, 0),
            ("profile", "second", 42, 1),
            ("toggle", "ON", u16::MAX, 1),
            ("toggle", "OFF", u16::MAX, 0),
        ] {
            assert_eq!(
                cfg.command(&format!("camera/mi360/{id}/set"), payload.as_bytes()),
                Some(Command::Setting { slot, value })
            );
        }
        for (id, payload) in [
            ("adjustment", "-4"),
            ("adjustment", "5"),
            ("adjustment", "1.5"),
            ("adjustment", "NaN"),
            ("adjustment", "2147483648"),
            ("profile", "third"),
            ("profile", "1"),
            ("toggle", "true"),
            ("toggle", "0"),
        ] {
            assert_eq!(
                cfg.command(&format!("camera/mi360/{id}/set"), payload.as_bytes()),
                None
            );
        }
        let discovery = cfg.discovery().unwrap();
        let setting: Value = serde_json::from_slice(
            &discovery
                .iter()
                .find(|(topic, _)| topic.ends_with("/adjustment/config"))
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(setting["entity_category"], "config");
        cfg.entities.retain(|entity| entity.id != "adjustment");
        assert_eq!(cfg.command("camera/mi360/adjustment/set", b"1"), None);
    }
    #[test]
    fn injected_dispatch_cannot_escape_scope_or_accept_malformed_payloads() {
        let mut cfg = config();
        cfg.entities.push(setting("value", Domain::Number, 123));
        for (topic, payload) in [
            ("camera/other/value/set", b"1".as_slice()),
            ("camera/mi360/value", b"1"),
            ("camera/mi360/value/set/extra", b"1"),
            ("camera/mi360/value/set", b"1\0"),
            ("camera/mi360/value/set", &[255]),
        ] {
            assert_eq!(cfg.command(topic, payload), None);
        }
        // Explicit injection, rather than a key-name table, chooses dispatch;
        // the unchanged default catalog still exercises the old Pan variant.
        cfg.entities
            .iter_mut()
            .find(|entity| entity.id == "pan")
            .unwrap()
            .control_slot = Some(77);
        assert_eq!(
            cfg.command("camera/mi360/pan/set", b"12"),
            Some(Command::Setting {
                slot: 77,
                value: 12
            })
        );
        cfg.entities
            .push(setting("not_a_control", Domain::Sensor, 9));
        assert!(cfg.discovery().is_err());
        assert_eq!(cfg.command("camera/mi360/not_a_control/set", b"1"), None);
    }
    #[test]
    fn injected_buttons_are_typed_nonretained_commands_with_button_discovery() {
        let mut cfg = config();
        cfg.entities
            .push(setting("apply_pending", Domain::Button, 110));
        assert_eq!(
            cfg.command("camera/mi360/apply_pending/set", b"PRESS"),
            Some(Command::Setting {
                slot: 110,
                value: 0
            })
        );
        for payload in [
            b"ON".as_slice(),
            b"OFF",
            b"0",
            b"press",
            b"",
            b"PRESS\0",
            &[255],
        ] {
            assert_eq!(cfg.command("camera/mi360/apply_pending/set", payload), None);
        }
        let mut packet =
            rumqttc::Publish::new("camera/mi360/apply_pending/set", QoS::AtLeastOnce, "PRESS");
        assert_eq!(
            cfg.packet_command(&packet),
            Some(Command::Setting {
                slot: 110,
                value: 0
            })
        );
        packet.retain = true;
        assert_eq!(
            cfg.packet_command(&packet),
            None,
            "replayed retained button cannot actuate"
        );
        packet.retain = false;
        packet.dup = true;
        assert_eq!(
            cfg.packet_command(&packet),
            None,
            "QoS retransmission cannot repeat a button"
        );
        let discovery = cfg.discovery().unwrap();
        let (topic, bytes) = discovery
            .iter()
            .find(|(topic, _)| topic.ends_with("/apply_pending/config"))
            .unwrap();
        assert_eq!(topic, "homeassistant/button/cam_mi360/apply_pending/config");
        let value: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(value["payload_press"], "PRESS");
        assert_eq!(value["command_topic"], "camera/mi360/apply_pending/set");
        assert_eq!(value["entity_category"], "config");
        assert!(value.get("state_topic").is_none());
        assert!(value.get("state_class").is_none());
        for domain in [Domain::Sensor, Domain::BinarySensor] {
            cfg.entities.last_mut().unwrap().domain = domain;
            assert!(cfg.discovery().is_err());
            assert_eq!(
                cfg.command("camera/mi360/apply_pending/set", b"PRESS"),
                None
            );
        }
        cfg.entities.pop();
        assert_eq!(
            cfg.command("camera/mi360/apply_pending/set", b"PRESS"),
            None,
            "omitted capability stays unavailable"
        );
    }

    #[test]
    fn string_diagnostics_and_unitless_sensors_do_not_claim_measurements() {
        let mut cfg = config();
        let mut status = setting("settings_status", Domain::Sensor, 0);
        status.control_slot = None;
        status.diagnostic = true;
        cfg.entities.push(status);
        for (topic, bytes) in cfg.discovery().unwrap() {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            if topic.ends_with("/settings_status/config") {
                assert!(value.get("state_class").is_none());
                assert_eq!(value["entity_category"], "diagnostic");
                assert_eq!(value["state_topic"], "camera/mi360/settings_status/state");
                assert!(value.get("command_topic").is_none());
            } else if topic.contains("/sensor/") {
                assert_eq!(value["state_class"], "measurement");
                assert!(value.get("unit_of_measurement").is_some());
            }
        }
    }

    #[test]
    fn catalog_limit_accepts_96_descriptors_and_rejects_97() {
        let mut cfg = config();
        cfg.entities = (0..MAX_ENTITIES)
            .map(|slot| setting(&format!("setting_{slot}"), Domain::Number, slot as u16))
            .collect();
        assert_eq!(cfg.discovery().unwrap().len(), 96);
        cfg.entities
            .push(setting("over_limit", Domain::Button, 111));
        assert!(cfg.discovery().is_err());
        cfg.entities.pop();
        cfg.entities[1].id = cfg.entities[0].id.clone();
        assert!(
            cfg.discovery().is_err(),
            "duplicate identity validation remains active"
        );
    }

    #[test]
    fn position3d_uses_the_shared_typed_contract_only_when_enabled() {
        use crate::position3d::{Operation, Request};
        let mut cfg = config();
        let point = br#"{"start_x":0,"start_y":255,"end_x":0,"end_y":255}"#;
        assert_eq!(cfg.command("camera/mi360/position3d/set", point), None);
        let count = cfg.discovery().unwrap().len();
        cfg.position3d = true;
        assert_eq!(
            cfg.discovery().unwrap().len(),
            count,
            "wire-only route adds no synthetic entity"
        );
        let command = Command::Position3d(Request {
            start: (0, 255),
            end: (0, 255),
            operation: Operation::ZoomTo,
        });
        assert_eq!(
            cfg.command("camera/mi360/position3d/set", point),
            Some(command)
        );
        let copied = command;
        assert_eq!(command, copied, "Command remains Copy");
        assert!(matches!(command,Command::Position3d(request) if request.is_point()));
        let boxed = br#"{"start_x":0,"start_y":0,"end_x":255,"end_y":255,"operation":"zoomout"}"#;
        assert_eq!(
            cfg.command("camera/mi360/position3d/set", boxed),
            Some(Command::Position3d(Request {
                start: (0, 0),
                end: (255, 255),
                operation: Operation::ZoomOut
            })),
            "transport preserves box intent for application capability policy"
        );
        let whitespace = b"{\n \"start_x\": 1, \"start_y\": 2, \"end_x\": 1, \"end_y\": 2\n}";
        assert_eq!(
            cfg.command("camera/mi360/position3d/set", whitespace),
            Some(Command::Position3d(
                Request::parse(std::str::from_utf8(whitespace).unwrap()).unwrap()
            ))
        );
        for topic in [
            "camera/other/position3d/set",
            "camera/mi360/position3d/set/extra",
            "camera/mi360/position3d",
        ] {
            assert_eq!(cfg.command(topic, point), None);
        }
    }

    #[test]
    fn bounded_vectors_cover_both_axes_and_neutral_releases() {
        let cfg = config();
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
            Direction::UpLeft,
            Direction::UpRight,
            Direction::DownLeft,
            Direction::DownRight,
        ] {
            let (pan, tilt) = direction.axes();
            let payload = serde_json::to_vec(&json!({"pan":pan,"tilt":tilt})).unwrap();
            assert_eq!(
                cfg.command("camera/mi360/move/set", &payload),
                Some(Command::Move(direction))
            );
        }
        assert_eq!(
            cfg.command("camera/mi360/move/set", br#"{"pan":0,"tilt":0}"#),
            Some(Command::Release)
        );
        for invalid in [
            r#"{"pan":2,"tilt":0}"#,
            r#"{"pan":1,"tilt":-2}"#,
            r#"{"pan":1.0,"tilt":0}"#,
            r#"{"pan":true,"tilt":0}"#,
            r#"{"pan":1}"#,
            r#"{"pan":0,"tilt":1,"speed":900}"#,
            r#"{"pan":0,"pan":1,"tilt":1}"#,
            "null",
            "[]",
        ] {
            assert_eq!(
                cfg.command("camera/mi360/move/set", invalid.as_bytes()),
                None,
                "{invalid}"
            );
        }
    }

    #[test]
    fn position3d_rejects_malformed_fraction_overflow_unknown_and_unbounded_json() {
        let mut cfg = config();
        cfg.position3d = true;
        for payload in [
            "",
            "[]",
            "null",
            "128,128",
            "{",
            "{}",
            r#"{"start_x":256,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":-1,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":1.5,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":1.0,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":0,"start_y":0,"end_x":0}"#,
            r#"{"start_x":0,"start_y":0,"end_x":0,"end_y":0,"operation":"spin"}"#,
            r#"{"start_x":0,"start_y":0,"end_x":0,"end_y":0,"speed":3}"#,
            r#"{"start_x":0,"start_x":1,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":0,"start_y":0,"end_x":0,"end_y":0} trailing"#,
        ] {
            assert_eq!(
                cfg.command("camera/mi360/position3d/set", payload.as_bytes()),
                None,
                "{payload}"
            );
        }
        assert_eq!(cfg.command("camera/mi360/position3d/set", &[255]), None);
        let mut payload = br#"{"start_x":0,"start_y":0,"end_x":0,"end_y":0}"#.to_vec();
        payload.resize(256, b' ');
        assert!(
            cfg.command("camera/mi360/position3d/set", &payload)
                .is_some()
        );
        payload.push(b' ');
        assert_eq!(cfg.command("camera/mi360/position3d/set", &payload), None);
        assert_eq!(
            cfg.command("camera/mi360/pan/set", &[b'1'; 65]),
            None,
            "scalar budget unchanged"
        );
    }

    #[test]
    fn position3d_rejects_retained_duplicate_stale_and_pre_stop_commands() {
        use crate::position3d::Request;
        let mut cfg = config();
        cfg.position3d = true;
        let payload = r#"{"start_x":66,"start_y":189,"end_x":66,"end_y":189}"#;
        let mut packet =
            rumqttc::Publish::new("camera/mi360/position3d/set", QoS::AtLeastOnce, payload);
        let command = Command::Position3d(Request::parse(payload).unwrap());
        assert_eq!(cfg.packet_command(&packet), Some(command));
        packet.retain = true;
        assert_eq!(cfg.packet_command(&packet), None);
        packet.retain = false;
        packet.dup = true;
        assert_eq!(cfg.packet_command(&packet), None);
        let (tx, rx) = sync_channel(8);
        let generation = Arc::new(AtomicU32::new(0));
        let urgent = Arc::new(AtomicU32::new(0));
        let now = Instant::now();
        let commands = Commands {
            rx,
            generation: generation.clone(),
            urgent_kind: urgent.clone(),
        };
        tx.try_send(Queued {
            command,
            received: now,
            generation: 0,
        })
        .unwrap();
        assert_eq!(commands.try_next(now + Duration::from_secs(3)), None);
        tx.try_send(Queued {
            command,
            received: now,
            generation: 0,
        })
        .unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        urgent.store(2, Ordering::Release);
        assert_eq!(commands.try_next(now), Some(Command::Stop));
        assert_eq!(commands.try_next(now), None);
        tx.try_send(Queued {
            command,
            received: now,
            generation: 1,
        })
        .unwrap();
        assert_eq!(
            commands.try_next(now + Duration::from_millis(500)),
            Some(command)
        );
    }

    #[test]
    fn release_is_a_distinct_typed_move_command_with_the_existing_capability_and_replay_guards() {
        let mut cfg = config();
        assert_eq!(
            cfg.command("camera/mi360/move/set", b"release"),
            Some(Command::Release)
        );
        assert_eq!(
            cfg.command("camera/mi360/move/set", b"stop"),
            Some(Command::Stop)
        );
        assert_eq!(
            cfg.command("camera/mi360/stop/set", b"PRESS"),
            Some(Command::Stop)
        );
        for topic in [
            "camera/mi360/stop/set",
            "camera/mi360/release/set",
            "camera/mi360/pan/set",
            "camera/other/move/set",
        ] {
            assert_eq!(cfg.command(topic, b"release"), None);
        }
        let mut packet =
            rumqttc::Publish::new("camera/mi360/move/set", QoS::AtLeastOnce, "release");
        assert_eq!(cfg.packet_command(&packet), Some(Command::Release));
        packet.retain = true;
        assert_eq!(cfg.packet_command(&packet), None);
        packet.retain = false;
        packet.dup = true;
        assert_eq!(cfg.packet_command(&packet), None);
        cfg.entities
            .retain(|entity| !matches!(entity.id.as_str(), "pan" | "tilt"));
        assert_eq!(cfg.command("camera/mi360/move/set", b"release"), None);
    }

    #[test]
    fn release_bypasses_a_full_queue_discards_old_motion_and_preserves_new_epoch_motion() {
        let (tx, rx) = sync_channel(8);
        let generation = Arc::new(AtomicU32::new(0));
        let urgent = Arc::new(AtomicU32::new(0));
        let now = Instant::now();
        let commands = Commands {
            rx,
            generation: generation.clone(),
            urgent_kind: urgent.clone(),
        };
        for _ in 0..8 {
            tx.try_send(Queued {
                command: Command::Move(Direction::Left),
                received: now,
                generation: 0,
            })
            .unwrap();
        }
        assert!(interrupt_commands(&generation, &urgent, Command::Release));
        assert_eq!(
            commands.try_next(now),
            Some(Command::Release),
            "full queue cannot lose release"
        );
        assert_eq!(
            commands.try_next(now),
            None,
            "moves queued before release are discarded"
        );
        tx.try_send(Queued {
            command: Command::Move(Direction::Right),
            received: now,
            generation: generation.load(Ordering::Acquire),
        })
        .unwrap();
        assert_eq!(
            commands.try_next(now),
            Some(Command::Move(Direction::Right))
        );

        // Fresh motion can be queued while Release is still pending; it must
        // be retained and delivered only after the urgent release.
        assert!(interrupt_commands(&generation, &urgent, Command::Release));
        tx.try_send(Queued {
            command: Command::Move(Direction::Up),
            received: now,
            generation: generation.load(Ordering::Acquire),
        })
        .unwrap();
        assert_eq!(commands.try_next(now), Some(Command::Release));
        assert_eq!(commands.try_next(now), Some(Command::Move(Direction::Up)));
    }

    #[test]
    fn explicit_stop_dominates_release_in_either_pending_order() {
        for order in [
            [Command::Release, Command::Stop],
            [Command::Stop, Command::Release],
        ] {
            let (tx, rx) = sync_channel(8);
            let generation = Arc::new(AtomicU32::new(0));
            let urgent = Arc::new(AtomicU32::new(0));
            let now = Instant::now();
            let commands = Commands {
                rx,
                generation: generation.clone(),
                urgent_kind: urgent.clone(),
            };
            for command in order {
                assert!(interrupt_commands(&generation, &urgent, command));
                tx.try_send(Queued {
                    command: Command::Move(Direction::Left),
                    received: now,
                    generation: generation.load(Ordering::Acquire),
                })
                .unwrap();
            }
            assert_eq!(commands.try_next(now), Some(Command::Stop));
            // Only the move from the latest epoch survives either stop order.
            assert_eq!(commands.try_next(now), Some(Command::Move(Direction::Left)));
            assert_eq!(commands.try_next(now), None);
            assert!(!interrupt_commands(
                &generation,
                &urgent,
                Command::Move(Direction::Down)
            ));
            assert_eq!(generation.load(Ordering::Acquire), 2);
        }
    }

    #[test]
    fn urgent_stop_cannot_be_lost_or_followed_by_stale_motion() {
        let (tx, rx) = sync_channel(8);
        let generation = Arc::new(AtomicU32::new(0));
        let urgent = Arc::new(AtomicU32::new(0));
        let now = Instant::now();
        for _ in 0..8 {
            tx.try_send(Queued {
                command: Command::Move(Direction::Left),
                received: now,
                generation: 0,
            })
            .unwrap();
        }
        let commands = Commands {
            rx,
            generation: generation.clone(),
            urgent_kind: urgent.clone(),
        };
        generation.fetch_add(1, Ordering::AcqRel);
        urgent.store(2, Ordering::Release);
        assert_eq!(commands.try_next(now), Some(Command::Stop));
        assert_eq!(commands.try_next(now), None);
        tx.try_send(Queued {
            command: Command::Move(Direction::Right),
            received: now,
            generation: 1,
        })
        .unwrap();
        assert_eq!(commands.try_next(now + Duration::from_secs(3)), None);
    }
}
