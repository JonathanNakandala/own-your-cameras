//! The gate on the shared MQTT layer: its Home Assistant entities, its door commands and how input
//! edges become states and events. Unique ids and topics are the ones the device-discovery document
//! used, so Home Assistant keeps the entities and their history through the move to per-entity
//! discovery (`migrate_from_device_discovery`).
//!
//! Topics under `<prefix>`: `availability` (`online`/`offline`), `door/state` (`open`/`closed`: open while
//! the strike is released or the reed is open), `reed` (the contact alone), `lock/state` (JSON with
//! `lock_actuator`), `call_button` and `access` (event JSON), `door/command` (incoming).
use crate::config::Config;
use crate::inputs::{Edge, LockPulse};
use camera_transport::mqtt::{self, Domain, Entity, Payloads};
use std::time::{SystemTime, UNIX_EPOCH};

/// `door/command` payloads the application acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorCommand {
    Open,
    OpenQuiet,
    /// Release the strike now instead of waiting for the pulse to run out.
    Close,
}

/// Bare, quoted and the earlier JSON payloads are all accepted.
pub fn parse_command(payload: &[u8]) -> Option<DoorCommand> {
    let compact: String = std::str::from_utf8(payload)
        .ok()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    match compact.as_str() {
        "open" | "\"open\"" | "{\"command\":\"open\"}" => Some(DoorCommand::Open),
        "open_quiet" | "\"open_quiet\"" | "{\"command\":\"open_quiet\"}" => {
            Some(DoorCommand::OpenQuiet)
        }
        "close" | "\"close\"" | "{\"command\":\"close\"}" => Some(DoorCommand::Close),
        _ => None,
    }
}

const DOOR: &str = "door";
const COMMAND: &str = "door/command";

fn press(id: &str, name: &str, icon: &'static str, payload: &'static str) -> Entity {
    Entity::new(id, name, Domain::Button)
        .icon(icon)
        .command_topic(COMMAND)
        .payloads(Payloads {
            press: Some(payload),
            ..Payloads::default()
        })
        .route(DOOR)
}

pub fn entities() -> Vec<Entity> {
    vec![
        Entity::new("gate", "", Domain::Cover)
            .unnamed()
            .device_class("gate")
            .state_topic("door/state")
            .command_topic(COMMAND)
            .attributes("lock_actuator")
            .payloads(Payloads {
                open: Some("open"),
                close: Some("close"),
                ..Payloads::default()
            })
            .route(DOOR),
        press("open", "Open with announcement", "mdi:gate-open", "open"),
        press("open_quiet", "Open quietly", "mdi:volume-off", "open_quiet"),
        press("close", "Close now", "mdi:gate", "close"),
        Entity::new("reed", "Door", Domain::BinarySensor)
            .device_class("door")
            .state_topic("reed")
            .payloads(Payloads {
                on: Some("open"),
                off: Some("closed"),
                ..Payloads::default()
            }),
        Entity::new("lock_actuator", "Lock actuator", Domain::BinarySensor)
            .device_class("lock")
            .state_topic("lock/state")
            .payloads(Payloads {
                on: Some("unlocked"),
                off: Some("locked"),
                value_template: Some(
                    "{{ 'None' if value_json.lock_actuator_unlocked is none else value_json.lock_actuator }}",
                ),
                ..Payloads::default()
            }),
        Entity::new("doorbell", "Doorbell", Domain::Event { types: vec!["press", "release"] })
            .device_class("doorbell")
            .state_topic("call_button"),
        Entity::new("access", "Access", Domain::Event { types: vec!["granted", "denied", "remote"] })
            .icon("mdi:dialpad")
            .state_topic("access"),
    ]
}

/// Door entities, then one config entity per live setting, then the diagnostic sensors.
pub fn all_entities() -> Vec<Entity> {
    let mut all = entities();
    all.extend(crate::settings::SPECS.iter().map(|s| s.entity()));
    all.extend(
        crate::telemetry::sensors()
            .iter()
            .map(crate::telemetry::entity),
    );
    all
}

/// The shared layer's configuration for this gate.
pub fn transport(cfg: &Config, version: &str) -> mqtt::Config {
    let mut t = mqtt::Config::new(
        mqtt::Broker {
            host: cfg.mqtt_host.clone(),
            port: cfg.mqtt_port,
            username: cfg.mqtt_user.clone(),
            password: cfg.mqtt_password.clone(),
        },
        cfg.mqtt_node.clone(),
        cfg.mqtt_prefix.clone(),
        mqtt::Device {
            name: cfg.device_name.clone(),
            manufacturer: "Hikvision".into(),
            model: "DS-KD8003-IME1".into(),
            sw_version: Some(version.into()),
        },
        all_entities(),
    );
    // QoS 0 end to end, as the worker subscribes: a QoS 1 hop from Home Assistant to the broker
    // would only promise a delivery the camera's QoS 0 subscription does not keep.
    t.migrate_from_device_discovery = true;
    // An earlier firmware published PIN access as two separate configs beside its device document,
    // one on the Access event's own topic with another unique id. Left retained, Home Assistant
    // binds the new Access config to that entity; the migration removes them first.
    t.superseded = vec![
        (Domain::Event { types: Vec::new() }, "access".into()),
        (Domain::Sensor, "last_pin_user".into()),
    ];
    t.superseded_states = SUPERSEDED_STATES.iter().map(|&s| s.into()).collect();
    // Nothing moves; a stale door command is dropped by the queue's age limit instead.
    t.stop_on_disconnect = false;
    t
}

/// Retained states of the earlier separate access entities, under `<prefix>`.
const SUPERSEDED_STATES: [&str; 4] = [
    "access/event",
    "access/last",
    "access/availability",
    "access/diagnostics",
];

/// Door commands arrive on `door/command`, shared by the cover and the three buttons.
pub struct Parser;

impl mqtt::Parser for Parser {
    type App = DoorCommand;
    fn parse(&self, id: &str, text: &str) -> Result<Option<mqtt::Command<DoorCommand>>, String> {
        if id != DOOR {
            return Ok(None);
        }
        parse_command(text.as_bytes())
            .map(|c| Some(mqtt::Command::App(c)))
            .ok_or_else(|| "unknown door command".into())
    }
}

/// `lock/state` attributes: the lock actuator, and the pulse a countdown is drawn from (`pulse_s` the full
/// scale, `relock_at` the UTC deadline when the clock is in sync, `relock_in_s` the clock-free fallback).
pub fn lock_state(pulse: &Option<LockPulse>) -> String {
    match pulse {
        None => r#"{"lock_actuator":"locked","lock_actuator_unlocked":false,"pulse_s":null,"relock_at":null,"relock_in_s":0}"#.to_owned(),
        Some(p) => format!(
            r#"{{"lock_actuator":"unlocked","lock_actuator_unlocked":true,"pulse_s":{},"relock_at":{},"relock_in_s":{}}}"#,
            p.pulse_ms as f64 / 1000.0,
            p.relock_at.map_or("null".to_owned(), |t| format!(r#""{}""#, iso_utc(t))),
            p.remaining_ms as f64 / 1000.0,
        ),
    }
}

/// `2026-11-08T14:03:27.412Z`: ISO 8601 UTC with milliseconds (`Date.parse`, HA's `as_datetime`).
pub fn iso_utc(t: SystemTime) -> String {
    let ms = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let (secs, ms) = (ms / 1000, ms % 1000);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// `call_button` and `access` event payloads (Home Assistant `event` platform).
pub fn event(event_type: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!(r#"{{"event_type":"{event_type}","name":"{n}"}}"#),
        None => format!(r#"{{"event_type":"{event_type}"}}"#),
    }
}

/// What MQTT publishes for an input edge.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    State(&'static str, String),
    Event(&'static str, String),
}

pub fn publication(edge: &Edge) -> Option<Out> {
    let open = |open: bool| if open { "open" } else { "closed" }.to_owned();
    Some(match edge {
        Edge::Reed(o) => Out::State("reed", open(*o)),
        Edge::Cover(o) => Out::State("gate", open(*o)),
        Edge::Lock(pulse) => Out::State("lock_actuator", lock_state(pulse)),
        Edge::CallButton(pressed) => Out::Event(
            "doorbell",
            event(if *pressed { "press" } else { "release" }, None),
        ),
        Edge::Granted(name) => Out::Event("access", event("granted", Some(name))),
        Edge::Denied => Out::Event("access", event("denied", None)),
        Edge::Remote => Out::Event("access", event("remote", None)),
        Edge::Tamper(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::mqtt::Parser as _;
    use serde_json::Value;

    fn config() -> Config {
        Config {
            mqtt_host: "mqtt.example".into(),
            device_name: "Front gate".into(),
            ..Config::default()
        }
    }
    fn discovery() -> Vec<(String, Value)> {
        transport(&config(), "0.1.0")
            .discovery()
            .unwrap()
            .into_iter()
            .map(|p| {
                assert!(p.retain);
                (p.topic, serde_json::from_slice(&p.payload).unwrap())
            })
            .collect()
    }

    #[test]
    fn unique_ids_are_the_device_discovery_ones_byte_for_byte() {
        let ids: Vec<String> = discovery()[..8]
            .iter()
            .map(|(_, v)| v["unique_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            ids,
            [
                "ds_kd8003_ime1_gate",
                "ds_kd8003_ime1_open",
                "ds_kd8003_ime1_open_quiet",
                "ds_kd8003_ime1_close",
                "ds_kd8003_ime1_reed",
                "ds_kd8003_ime1_lock_actuator",
                "ds_kd8003_ime1_doorbell",
                "ds_kd8003_ime1_access",
            ]
        );
    }

    #[test]
    fn migration_removes_the_earlier_separate_access_entities() {
        let t = transport(&config(), "0.1.0");
        assert!(t.migrate_from_device_discovery);
        let ids: Vec<(&str, &str)> = t
            .superseded
            .iter()
            .map(|(d, id)| (d.component(), id.as_str()))
            .collect();
        // `homeassistant/event/<node>/access/config` is also the new Access config's topic.
        assert_eq!(ids, [("event", "access"), ("sensor", "last_pin_user")]);
        assert!(t.superseded_states.iter().any(|s| s == "access/last"));
        assert!(
            t.superseded_states.iter().all(|s| s != "access"),
            "the new Access event topic is not cleared"
        );
        assert!(t.discovery().is_ok(), "superseded ids may be current ones");
    }

    #[test]
    fn per_entity_discovery_keeps_topics_and_uses_plain_payloads() {
        let mut all = discovery();
        all.truncate(8);
        let topics: Vec<&str> = all.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            topics,
            [
                "homeassistant/cover/ds_kd8003_ime1/gate/config",
                "homeassistant/button/ds_kd8003_ime1/open/config",
                "homeassistant/button/ds_kd8003_ime1/open_quiet/config",
                "homeassistant/button/ds_kd8003_ime1/close/config",
                "homeassistant/binary_sensor/ds_kd8003_ime1/reed/config",
                "homeassistant/binary_sensor/ds_kd8003_ime1/lock_actuator/config",
                "homeassistant/event/ds_kd8003_ime1/doorbell/config",
                "homeassistant/event/ds_kd8003_ime1/access/config",
            ]
        );
        let get = |id: &str| {
            &all.iter()
                .find(|(t, _)| t.contains(&format!("/{id}/")))
                .unwrap()
                .1
        };
        let gate = get("gate");
        assert_eq!(gate["name"], Value::Null);
        assert_eq!(gate["device_class"], "gate");
        assert_eq!(gate["state_topic"], "ds-kd8003-ime1/door/state");
        assert_eq!(gate["command_topic"], "ds-kd8003-ime1/door/command");
        assert_eq!(gate["json_attributes_topic"], "ds-kd8003-ime1/lock/state");
        assert_eq!(
            (
                gate["payload_open"].as_str(),
                gate["payload_close"].as_str()
            ),
            (Some("open"), Some("close"))
        );
        assert_eq!(gate["qos"], Value::Null, "QoS 0, as subscribed");
        assert_eq!(gate["availability_topic"], "ds-kd8003-ime1/availability");
        assert_eq!(gate["device"]["identifiers"][0], "ds_kd8003_ime1");
        assert_eq!(gate["device"]["name"], "Front gate");
        assert_eq!(gate["device"]["sw_version"], "0.1.0");
        assert_eq!(get("open_quiet")["payload_press"], "open_quiet");
        assert_eq!(get("close")["command_topic"], "ds-kd8003-ime1/door/command");
        assert_eq!(get("reed")["state_topic"], "ds-kd8003-ime1/reed");
        assert_eq!(get("reed")["payload_on"], "open");
        assert_eq!(get("lock_actuator")["payload_on"], "unlocked");
        assert_eq!(get("doorbell")["state_topic"], "ds-kd8003-ime1/call_button");
        assert_eq!(get("access")["event_types"][0], "granted");
        assert!(all.iter().all(|(_, v)| v.get("entity_category").is_none()));
    }

    #[test]
    fn settings_are_config_entities_and_sensors_diagnostics() {
        let all = discovery();
        let n = crate::settings::SPECS.len();
        assert_eq!(all.len(), 8 + n + crate::telemetry::IDS.len());
        let (settings, sensors) = all[8..].split_at(n);
        for (topic, v) in settings {
            assert_eq!(v["entity_category"], "config", "{topic}");
            let key = v["unique_id"]
                .as_str()
                .unwrap()
                .trim_start_matches("ds_kd8003_ime1_");
            assert_eq!(
                v["command_topic"],
                format!("ds-kd8003-ime1/{key}/set"),
                "{topic}"
            );
        }
        let open = &settings[0].1;
        assert_eq!(
            (
                open["min"].as_i64(),
                open["max"].as_i64(),
                open["unit_of_measurement"].as_str()
            ),
            (Some(1), Some(120), Some("s"))
        );
        for (topic, v) in sensors {
            assert_eq!(v["entity_category"], "diagnostic", "{topic}");
            assert_eq!(v["state_class"], "measurement", "{topic}");
            assert!(topic.starts_with("homeassistant/sensor/"), "{topic}");
        }
        let load = sensors.iter().find(|(t, _)| t.contains("/load/")).unwrap();
        assert_eq!(load.1["enabled_by_default"], false);
    }

    #[test]
    fn door_commands_accept_bare_quoted_and_json_payloads() {
        assert_eq!(parse_command(b"open"), Some(DoorCommand::Open));
        assert_eq!(
            parse_command(b"{ \"command\": \"open\" }"),
            Some(DoorCommand::Open)
        );
        assert_eq!(
            parse_command(b"\"open_quiet\""),
            Some(DoorCommand::OpenQuiet)
        );
        assert_eq!(parse_command(b"close"), Some(DoorCommand::Close));
        assert_eq!(parse_command(b"lock"), None);
        assert_eq!(parse_command(b"\xff"), None);
        assert_eq!(
            Parser.parse("door", "open"),
            Ok(Some(mqtt::Command::App(DoorCommand::Open)))
        );
        assert!(Parser.parse("door", "lock").is_err());
        assert_eq!(
            Parser.parse("open", "open"),
            Ok(None),
            "only the door route"
        );
    }

    #[test]
    fn edges_become_states_and_events() {
        assert_eq!(
            publication(&Edge::Reed(true)),
            Some(Out::State("reed", "open".into()))
        );
        assert_eq!(
            publication(&Edge::Cover(false)),
            Some(Out::State("gate", "closed".into()))
        );
        let Some(Out::State("lock_actuator", lock)) = publication(&Edge::Lock(Some(LockPulse {
            pulse_ms: 5_000,
            remaining_ms: 5_000,
            relock_at: None,
        }))) else {
            panic!()
        };
        let v: Value = serde_json::from_str(&lock).unwrap();
        assert_eq!(
            (
                v["lock_actuator"].as_str(),
                v["lock_actuator_unlocked"].as_bool()
            ),
            (Some("unlocked"), Some(true))
        );
        let Some(Out::Event("access", granted)) = publication(&Edge::Granted("resident".into()))
        else {
            panic!()
        };
        let v: Value = serde_json::from_str(&granted).unwrap();
        assert_eq!(
            (v["event_type"].as_str(), v["name"].as_str()),
            (Some("granted"), Some("resident"))
        );
        assert_eq!(
            publication(&Edge::CallButton(true)),
            Some(Out::Event("doorbell", r#"{"event_type":"press"}"#.into()))
        );
        assert_eq!(publication(&Edge::Tamper(true)), None);
    }

    #[test]
    fn the_locked_payload_has_no_countdown() {
        let v: Value = serde_json::from_str(&lock_state(&None)).unwrap();
        assert_eq!(v["lock_actuator"], "locked");
        assert_eq!(v["lock_actuator_unlocked"], false);
        assert!(v["pulse_s"].is_null() && v["relock_at"].is_null());
        assert_eq!(v["relock_in_s"], 0);
    }

    #[test]
    fn the_unlocked_payload_carries_the_pulse_with_and_without_a_clock() {
        let at = UNIX_EPOCH + std::time::Duration::from_millis(1_794_146_607_412);
        let pulse = LockPulse {
            pulse_ms: 120_000,
            remaining_ms: 119_950,
            relock_at: Some(at),
        };
        let v: Value = serde_json::from_str(&lock_state(&Some(pulse))).unwrap();
        assert_eq!(v["lock_actuator"], "unlocked");
        assert_eq!(v["lock_actuator_unlocked"], true);
        assert_eq!(v["pulse_s"].as_f64(), Some(120.0));
        assert_eq!(v["relock_at"], "2026-11-08T14:03:27.412Z");
        assert_eq!(v["relock_in_s"].as_f64(), Some(119.95));
        let v: Value = serde_json::from_str(&lock_state(&Some(LockPulse {
            relock_at: None,
            ..pulse
        })))
        .unwrap();
        assert!(v["relock_at"].is_null());
        assert_eq!(v["relock_in_s"].as_f64(), Some(119.95));
    }

    #[test]
    fn iso_times_are_utc_with_milliseconds() {
        let at = |ms: u64| iso_utc(UNIX_EPOCH + std::time::Duration::from_millis(ms));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(at(951_782_400_005), "2000-02-29T00:00:00.005Z");
        assert_eq!(at(4_107_542_399_999), "2100-02-28T23:59:59.999Z");
    }
}
