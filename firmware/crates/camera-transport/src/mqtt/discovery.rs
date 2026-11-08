//! Home Assistant per-entity discovery derived from the catalog, and the one-off move from a
//! device-discovery document.
use super::{Category, Config, Domain, Entity};
use serde_json::{Value, json};
use std::io;

/// One message for the broker. An empty retained payload clears the topic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publication {
    pub topic: String,
    pub payload: Vec<u8>,
    pub retain: bool,
}

impl Publication {
    pub fn retained(topic: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            topic: topic.into(),
            payload: payload.into(),
            retain: true,
        }
    }
}

/// Home Assistant's documented migration payload for a discovery topic.
pub const MIGRATE_MARKER: &[u8] = br#"{"migrate_discovery": true}"#;

/// The sequence that moves entities from a retained device-discovery document to per-entity
/// discovery without deleting them in Home Assistant: the migration marker on the device topic, then
/// `superseded` (empty retained payloads that remove an earlier firmware's separate configs before
/// a new config can land on their topics), then `announce` (per-entity configs with the same unique ids, states, availability), then an empty
/// retained payload that clears the device topic. `retained` is what the broker holds on the device
/// topic; when it is empty there is nothing to migrate and `announce` is returned unchanged. A marker
/// left by an interrupted earlier run is not published twice.
pub fn migrate_from_device_discovery(
    device_topic: &str,
    retained: &[u8],
    superseded: Vec<Publication>,
    announce: Vec<Publication>,
) -> Vec<Publication> {
    if retained.is_empty() {
        return announce;
    }
    let mut out = Vec::with_capacity(announce.len() + superseded.len() + 2);
    if !is_marker(retained) {
        out.push(Publication::retained(device_topic, MIGRATE_MARKER));
    }
    out.extend(superseded);
    out.extend(announce);
    out.push(Publication::retained(device_topic, Vec::new()));
    out
}

fn is_marker(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .is_ok_and(|v| v.get("migrate_discovery") == Some(&Value::Bool(true)))
}

impl Config {
    /// Retained discovery configs, one per entity, in catalog order.
    pub fn discovery(&self) -> io::Result<Vec<Publication>> {
        self.validate()?;
        self.entities
            .iter()
            .map(|e| {
                let payload = match self.entity_config(e) {
                    Some(value) => serde_json::to_vec(&value)
                        .map_err(|_| io::Error::other("cannot encode discovery"))?,
                    None => Vec::new(),
                };
                Ok(Publication::retained(
                    self.config_topic(&e.domain, &e.id),
                    payload,
                ))
            })
            .collect()
    }

    pub(crate) fn config_topic(&self, domain: &Domain, id: &str) -> String {
        format!(
            "{}/{}/{}/{id}/config",
            self.discovery_prefix,
            domain.component(),
            self.node_id
        )
    }

    /// The full state topic of a state id: the entity's own (possibly overridden) state topic, or
    /// `<prefix>/<id>/state`.
    pub(crate) fn state_topic_of(&self, id: &str) -> String {
        self.entities
            .iter()
            .find(|e| e.id == id)
            .and_then(Entity::state_path)
            .map_or_else(|| self.topic(&format!("{id}/state")), |p| self.topic(&p))
    }

    fn entity_config(&self, e: &Entity) -> Option<Value> {
        let component = e.domain.component();
        let mut device = json!({
            "identifiers": [self.node_id],
            "name": self.device.name,
            "manufacturer": self.device.manufacturer,
            "model": self.device.model,
        });
        if let Some(version) = &self.device.sw_version {
            device["sw_version"] = json!(version);
        }
        let mut v = json!({
            "name": e.name,
            "unique_id": format!("{}_{}", self.node_id, e.id),
            "device": device,
            "availability_topic": self.availability(),
        });
        if let Some(prefix) = &self.entity_id_prefix {
            v["default_entity_id"] = json!(format!("{component}.{prefix}_{}", e.id));
        }
        if let Some(path) = e.state_path() {
            v["state_topic"] = json!(self.topic(&path));
        }
        if let Some(path) = e.command_path() {
            v["command_topic"] = json!(self.topic(&path));
            v["retain"] = json!(false);
        }
        match &e.domain {
            Domain::Number {
                min,
                max,
                step,
                mode,
            } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
                v["step"] = json!(step);
                v["mode"] = json!(match mode {
                    super::Mode::Slider => "slider",
                    super::Mode::Box => "box",
                });
            }
            Domain::Select { options } => {
                if options.is_empty() {
                    return None;
                }
                v["options"] = json!(options);
            }
            Domain::Switch | Domain::BinarySensor => {
                v["payload_on"] = json!(e.payload(|p| p.on, "ON"));
                v["payload_off"] = json!(e.payload(|p| p.off, "OFF"));
            }
            Domain::Button => v["payload_press"] = json!(e.payload(|p| p.press, "PRESS")),
            Domain::Cover => {
                v["payload_open"] = json!(e.payload(|p| p.open, "OPEN"));
                v["payload_close"] = json!(e.payload(|p| p.close, "CLOSE"));
                v["payload_stop"] = Value::Null;
                v["state_open"] = json!(e.payload(|p| p.state_open, "open"));
                v["state_closed"] = json!(e.payload(|p| p.state_closed, "closed"));
                v["optimistic"] = json!(false);
            }
            Domain::Event { types } => v["event_types"] = json!(types),
            Domain::Sensor | Domain::Text => {}
        }
        if let Some(unit) = e.unit {
            v["unit_of_measurement"] = json!(unit);
        }
        if let Some(class) = e.device_class {
            v["device_class"] = json!(class);
        }
        if let Some(icon) = e.icon {
            v["icon"] = json!(icon);
        }
        if let Some(class) = e.state_class {
            v["state_class"] = json!(class);
        }
        if let Some(category) = e.category {
            v["entity_category"] = json!(match category {
                Category::Config => "config",
                Category::Diagnostic => "diagnostic",
            });
        }
        if !e.enabled_by_default {
            v["enabled_by_default"] = json!(false);
        }
        if let Some(id) = &e.attributes {
            v["json_attributes_topic"] = json!(self.state_topic_of(id));
        }
        if let Some(template) = e.payloads.and_then(|p| p.value_template) {
            v["value_template"] = json!(template);
        }
        if self.qos > 0 {
            v["qos"] = json!(self.qos);
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config;
    use super::super::*;
    use serde_json::{Value, json};

    fn one(entity: Entity) -> (String, Value) {
        let p = config(vec![entity]).discovery().unwrap().remove(0);
        assert!(p.retain);
        (
            p.topic,
            serde_json::from_slice(&p.payload).unwrap_or(Value::Null),
        )
    }
    fn common(v: &Value, id: &str) {
        assert_eq!(v["unique_id"], format!("cam_test_{id}"));
        assert_eq!(v["availability_topic"], "camera/test/availability");
        assert_eq!(
            v["device"],
            json!({"identifiers":["cam_test"],"name":"Test","manufacturer":"Maker","model":"Model","sw_version":"1.0"})
        );
    }

    #[test]
    fn number_carries_range_mode_category_and_nonretained_command() {
        let (topic, v) = one(
            Entity::new("volume", "Volume", Domain::number(-30, 6, Mode::Box))
                .unit("dB")
                .category(Category::Config)
                .slot(3),
        );
        assert_eq!(topic, "homeassistant/number/cam_test/volume/config");
        common(&v, "volume");
        assert_eq!(v["min"], -30);
        assert_eq!(v["max"], 6);
        assert_eq!(v["step"], 1);
        assert_eq!(v["mode"], "box");
        assert_eq!(v["unit_of_measurement"], "dB");
        assert_eq!(v["entity_category"], "config");
        assert_eq!(v["command_topic"], "camera/test/volume/set");
        assert_eq!(v["state_topic"], "camera/test/volume/state");
        assert_eq!(v["retain"], false);
        assert!(v.get("default_entity_id").is_none());
    }

    #[test]
    fn switch_select_button_text_have_their_payloads() {
        let (_, s) = one(Entity::new("led", "LED", Domain::Switch));
        assert_eq!(
            (s["payload_on"].as_str(), s["payload_off"].as_str()),
            (Some("ON"), Some("OFF"))
        );
        let (t, sel) = one(Entity::new("ir", "IR", Domain::select(["day", "night"])));
        assert_eq!(t, "homeassistant/select/cam_test/ir/config");
        assert_eq!(sel["options"], json!(["day", "night"]));
        let (_, b) = one(Entity::new("reboot", "Reboot", Domain::Button).device_class("restart"));
        assert_eq!(b["payload_press"], "PRESS");
        assert_eq!(b["device_class"], "restart");
        assert!(b.get("state_topic").is_none());
        assert_eq!(b["command_topic"], "camera/test/reboot/set");
        let (t, text) = one(Entity::new("label", "Label", Domain::Text));
        assert_eq!(t, "homeassistant/text/cam_test/label/config");
        assert_eq!(text["command_topic"], "camera/test/label/set");
        assert_eq!(text["state_topic"], "camera/test/label/state");
    }

    #[test]
    fn sensors_are_read_only_and_secondary_ones_disabled() {
        let (_, v) = one(Entity::new("load", "Load", Domain::Sensor)
            .state_class("measurement")
            .category(Category::Diagnostic)
            .disabled_by_default());
        assert!(v.get("command_topic").is_none() && v.get("retain").is_none());
        assert_eq!(v["state_class"], "measurement");
        assert_eq!(v["entity_category"], "diagnostic");
        assert_eq!(v["enabled_by_default"], false);
        let (t, b) = one(Entity::new("reed", "Door", Domain::BinarySensor)
            .device_class("door")
            .state_topic("reed")
            .payloads(Payloads {
                on: Some("open"),
                off: Some("closed"),
                ..Payloads::default()
            }));
        assert_eq!(t, "homeassistant/binary_sensor/cam_test/reed/config");
        assert_eq!(b["state_topic"], "camera/test/reed");
        assert_eq!(b["payload_on"], "open");
        assert!(b.get("enabled_by_default").is_none());
    }

    #[test]
    fn cover_and_event_follow_the_gate_contract() {
        let mut cfg = config(vec![
            Entity::new("gate", "", Domain::Cover)
                .unnamed()
                .device_class("gate")
                .state_topic("door/state")
                .command_topic("door/command")
                .attributes("lock_actuator")
                .payloads(Payloads {
                    open: Some("open"),
                    close: Some("close"),
                    ..Payloads::default()
                })
                .route("door"),
            Entity::new("lock_actuator", "Lock actuator", Domain::BinarySensor)
                .state_topic("lock/state")
                .payloads(Payloads {
                    value_template: Some("{{ value_json.lock_actuator }}"),
                    ..Payloads::default()
                }),
            Entity::new(
                "doorbell",
                "Doorbell",
                Domain::Event {
                    types: vec!["press", "release"],
                },
            )
            .device_class("doorbell")
            .state_topic("call_button"),
        ]);
        cfg.qos = 1;
        let all: Vec<(String, Value)> = cfg
            .discovery()
            .unwrap()
            .into_iter()
            .map(|p| (p.topic, serde_json::from_slice(&p.payload).unwrap()))
            .collect();
        let (topic, cover) = &all[0];
        assert_eq!(topic, "homeassistant/cover/cam_test/gate/config");
        assert_eq!(cover["name"], Value::Null);
        assert_eq!(cover["state_topic"], "camera/test/door/state");
        assert_eq!(cover["command_topic"], "camera/test/door/command");
        assert_eq!(cover["json_attributes_topic"], "camera/test/lock/state");
        assert_eq!(cover["payload_open"], "open");
        assert_eq!(cover["payload_stop"], Value::Null);
        assert_eq!(cover["state_closed"], "closed");
        assert_eq!(cover["optimistic"], false);
        assert_eq!(cover["qos"], 1);
        assert_eq!(all[1].1["value_template"], "{{ value_json.lock_actuator }}");
        let (topic, event) = &all[2];
        assert_eq!(topic, "homeassistant/event/cam_test/doorbell/config");
        assert_eq!(event["event_types"], json!(["press", "release"]));
        assert_eq!(event["state_topic"], "camera/test/call_button");
        assert!(event.get("command_topic").is_none());
    }

    #[test]
    fn select_without_options_is_cleared_and_entity_ids_are_optional() {
        let mut cfg = config(vec![
            Entity::new("preset", "Preset", Domain::select(Vec::<String>::new())),
            Entity::new("pan", "Pan", Domain::number(0, 10, Mode::Slider)),
        ]);
        cfg.entity_id_prefix = Some("test".into());
        let out = cfg.discovery().unwrap();
        assert_eq!(out[0].topic, "homeassistant/select/cam_test/preset/config");
        assert!(out[0].payload.is_empty() && out[0].retain);
        let pan: Value = serde_json::from_slice(&out[1].payload).unwrap();
        assert_eq!(pan["default_entity_id"], "number.test_pan");
    }

    #[test]
    fn catalog_limits_duplicates_and_misplaced_controls_are_refused() {
        let numbers = |n: usize| {
            (0..n)
                .map(|i| Entity::new(format!("n{i}"), "N", Domain::number(0, 1, Mode::Box)))
                .collect::<Vec<_>>()
        };
        assert_eq!(config(numbers(96)).discovery().unwrap().len(), 96);
        assert!(config(numbers(97)).discovery().is_err());
        let mut cfg = config(numbers(97));
        cfg.limits.entities = 120;
        assert!(cfg.discovery().is_ok(), "the limit is configurable");
        let mut dup = numbers(2);
        dup[1].id = dup[0].id.clone();
        assert!(config(dup).discovery().is_err());
        assert!(
            config(vec![Entity::new("s", "S", Domain::Sensor).slot(1)])
                .discovery()
                .is_err()
        );
        assert!(
            config(vec![Entity::new("a/b", "S", Domain::Sensor)])
                .discovery()
                .is_err()
        );
        assert!(
            config(vec![
                Entity::new("s", "S", Domain::Sensor).state_topic("a//b")
            ])
            .discovery()
            .is_err()
        );
    }

    #[test]
    fn migration_marks_then_announces_then_clears_the_device_topic() {
        let device = "homeassistant/device/node/config";
        let announce = vec![
            Publication::retained("homeassistant/cover/node/gate/config", b"{}".to_vec()),
            Publication::retained("p/availability", b"online".to_vec()),
        ];
        let old = Publication::retained("homeassistant/event/node/access/config", Vec::new());
        let doc = br#"{"components":{}}"#;
        let out = migrate_from_device_discovery(device, doc, vec![old.clone()], announce.clone());
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], Publication::retained(device, MIGRATE_MARKER));
        assert_eq!(
            out[1], old,
            "an earlier separate config is cleared before any new config"
        );
        assert_eq!(out[2..4], announce[..]);
        assert_eq!(out[4], Publication::retained(device, Vec::new()));
        assert!(out.iter().all(|p| p.retain));
        let resumed = migrate_from_device_discovery(
            device,
            MIGRATE_MARKER,
            vec![old.clone()],
            announce.clone(),
        );
        assert_eq!(resumed.len(), 4, "an interrupted run does not mark twice");
        assert_eq!(resumed[0], old, "but clears the earlier configs again");
        assert_eq!(resumed[3].payload, b"");
        assert_eq!(
            migrate_from_device_discovery(device, b"", vec![old], announce.clone()),
            announce,
            "nothing retained: plain announce, nothing cleared"
        );
    }
}
