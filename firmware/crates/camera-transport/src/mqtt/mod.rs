//! The one MQTT layer for every camera: Home Assistant discovery, command routing, state and event
//! publication over one small blocking MQTT 3.1.1 client. It owns no hardware: each camera supplies its real capability catalog
//! ([`Entity`]), its command parser ([`Parser`]) and its state samples; no value or successful command
//! is invented here.
//!
//! Rules every camera gets:
//! - Commands replayed from the broker's retained store are never executed.
//! - States are retained, published only when their value changes, coalesced per topic and replayed
//!   after every reconnect and HA birth.
//! - An announcement clears retired entities (and any pending removal) before any discovery config.
//! - Events are not retained, never coalesced, never replayed, and dropped when older than 10 s.
//! - Stop and Release bypass a full command queue and discard older queued commands.
mod client;
mod command;
mod discovery;
mod entity;
mod hardware;
mod packet;
pub mod ptz;
mod publisher;
mod worker;

pub use command::{Command, Commands, Parser, Queued, Rejected, Urgent};
pub use discovery::{MIGRATE_MARKER, Publication, migrate_from_device_discovery};
pub use entity::{Category, Control, Domain, Entity, Mode, Payloads, Topics};
pub use hardware::{Hardware, HardwareLoop, supersede};
pub use ptz::{Direction, Position, Ptz, Zoom};
pub use publisher::{EVENT_TTL, Publisher};
pub use worker::{Hooks, Submitter, UrgentHook, Worker, channel, channel_with_epoch};

use std::collections::BTreeSet;
use std::io;
use std::time::Duration;

pub struct Broker {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

/// The Home Assistant device every entity belongs to.
pub struct Device {
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub sw_version: Option<String>,
}

/// Bounds of one camera's MQTT layer.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Catalog size.
    pub entities: usize,
    /// Commands waiting for the camera.
    pub commands: usize,
    /// Events, acknowledgments and other one-off messages waiting for the broker.
    pub outbox: usize,
    /// Publications written to the broker per worker pass; the rest wait for the next pass.
    pub outbound: usize,
    pub max_packet: usize,
    /// A queued command older than this is dropped instead of executed; `None` keeps it.
    pub command_age: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entities: 96,
            commands: 8,
            outbox: 64,
            outbound: 128,
            max_packet: 8192,
            command_age: Some(Duration::from_secs(2)),
        }
    }
}

pub struct Config {
    pub broker: Broker,
    /// Client id, device identifier, unique-id prefix and discovery node.
    pub node_id: String,
    pub device: Device,
    /// Root of every state and command topic.
    pub prefix: String,
    pub discovery_prefix: String,
    pub entities: Vec<Entity>,
    /// Advertises `default_entity_id = "<component>.<entity_id_prefix>_<id>"`.
    pub entity_id_prefix: Option<String>,
    /// Entities earlier releases published that no longer exist. Every announcement clears their
    /// discovery config and state (empty retained payloads) before it sends any discovery config.
    pub retired: Vec<(Domain, String)>,
    /// QoS advertised in discovery (0 leaves Home Assistant's default).
    pub qos: u8,
    /// Move an installation from one retained device-discovery document
    /// (`<discovery_prefix>/device/<node>/config`) to per-entity discovery on the first connection.
    pub migrate_from_device_discovery: bool,
    /// Separate discovery configs an earlier firmware kept beside its device document, by
    /// component and object id (an id may equal a current entity's). The device-discovery migration
    /// clears them right after its marker and before any new config, so Home Assistant removes
    /// those entities instead of binding a new config to their discovery topic. Only that one-off
    /// migration clears them: a migrated broker never sees them again.
    pub superseded: Vec<(Domain, String)>,
    /// Retained state topics (paths under `prefix`) of those earlier entities, cleared with them.
    pub superseded_states: Vec<String>,
    /// A lost broker connection stops held motion (queues a Stop).
    pub stop_on_disconnect: bool,
    pub limits: Limits,
}

impl Config {
    pub fn new(
        broker: Broker,
        node_id: impl Into<String>,
        prefix: impl Into<String>,
        device: Device,
        entities: Vec<Entity>,
    ) -> Self {
        Self {
            broker,
            node_id: node_id.into(),
            device,
            prefix: prefix.into(),
            discovery_prefix: "homeassistant".into(),
            entities,
            entity_id_prefix: None,
            retired: Vec::new(),
            qos: 0,
            migrate_from_device_discovery: false,
            superseded: Vec::new(),
            superseded_states: Vec::new(),
            stop_on_disconnect: true,
            limits: Limits::default(),
        }
    }

    pub(crate) fn topic(&self, path: &str) -> String {
        format!("{}/{path}", self.prefix)
    }
    pub(crate) fn availability(&self) -> String {
        self.topic("availability")
    }
    pub(crate) fn device_discovery_topic(&self) -> String {
        format!("{}/device/{}/config", self.discovery_prefix, self.node_id)
    }

    /// Empty retained discovery config and state of every retired entity, in order.
    pub(crate) fn retirement(&self) -> Vec<Publication> {
        self.retired
            .iter()
            .flat_map(|(domain, id)| {
                [
                    Publication::retained(self.config_topic(domain, id), Vec::new()),
                    Publication::retained(self.state_topic_of(id), Vec::new()),
                ]
            })
            .collect()
    }

    /// Empty retained payloads for the earlier firmware's separate configs and states, in order.
    pub(crate) fn supersession(&self) -> Vec<Publication> {
        let configs = self
            .superseded
            .iter()
            .map(|(domain, id)| Publication::retained(self.config_topic(domain, id), Vec::new()));
        let states = self
            .superseded_states
            .iter()
            .map(|path| Publication::retained(self.topic(path), Vec::new()));
        configs.chain(states).collect()
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        let ids: BTreeSet<_> = self.entities.iter().map(|e| e.id.as_str()).collect();
        let short = |s: &str| s.len() <= 128;
        let path = |p: &Option<String>| p.as_deref().is_none_or(topic_path);
        let bad_entity = |e: &Entity| {
            !identifier(&e.id)
                || !e.name.as_deref().is_none_or(short)
                || e.attributes.as_deref().is_some_and(|a| !identifier(a))
                || e.topics
                    .as_ref()
                    .is_some_and(|t| !path(&t.state) || !path(&t.command))
                || match &e.domain {
                    Domain::Number { min, max, step, .. } => min > max || *step < 1,
                    Domain::Select { options } => {
                        options.len() > 32
                            || options.iter().any(|o| {
                                o.is_empty() || o.len() > 64 || o.chars().any(char::is_control)
                            })
                    }
                    Domain::Event { types } => types.is_empty() || types.len() > 16,
                    _ => false,
                }
                || (e.control != Control::None && !e.domain.commands())
        };
        // MQTT length-prefixed fields; the encoder refuses longer ones rather than truncating.
        let field = |s: &str| s.len() <= usize::from(u16::MAX);
        if self.broker.host.trim().is_empty()
            || self.broker.port == 0
            || !field(&self.broker.username)
            || !field(&self.broker.password)
            || !identifier(&self.node_id)
            || !topic_path(&self.prefix)
            || !topic_path(&self.discovery_prefix)
            || self.entities.is_empty()
            || self.entities.len() > self.limits.entities
            || ids.len() != self.entities.len()
            || self.qos > 1
            || [
                &self.device.name,
                &self.device.manufacturer,
                &self.device.model,
            ]
            .iter()
            .any(|s| !short(s))
            || !self.device.sw_version.as_deref().is_none_or(short)
            || self.entities.iter().any(bad_entity)
            || self
                .retired
                .iter()
                .any(|(_, id)| !identifier(id) || ids.contains(id.as_str()))
            || self.superseded.iter().any(|(_, id)| !identifier(id))
            || !self.superseded_states.iter().all(|p| topic_path(p))
        {
            return Err(io::Error::other(
                "invalid MQTT identity, capability catalog or broker",
            ));
        }
        Ok(())
    }
}

pub(crate) fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
pub(crate) fn topic_path(text: &str) -> bool {
    !text.is_empty() && text.len() <= 128 && text.split('/').all(identifier)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    pub fn config(entities: Vec<Entity>) -> Config {
        Config::new(
            Broker {
                host: "127.0.0.1".into(),
                port: 1883,
                username: String::new(),
                password: String::new(),
            },
            "cam_test",
            "camera/test",
            Device {
                name: "Test".into(),
                manufacturer: "Maker".into(),
                model: "Model".into(),
                sw_version: Some("1.0".into()),
            },
            entities,
        )
    }
}
