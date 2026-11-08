//! A configured camera id and name publish exactly what the firmware published before its identity became
//! configuration: the fixture was recorded from that code, with the id and name it had then replaced.

use cam::mqtt::Config;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/discovery-hall.json");
const CAMERA: &str =
    "[camera]\nid = \"hall\"\nname = \"Hall camera\"\n[mqtt]\nhost = \"broker.example\"\n";
const PRESETS: &str = "[presets.Desk]\npan = 6195\ntilt = 650\nzoom = 0\n[presets.Door]\npan = 100\ntilt = 200\nzoom = 512\n";

fn publications(cfg: &Config) -> Value {
    cfg.discovery()
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "topic": p.topic,
                "payload": if p.payload.is_null() { String::new() } else { p.payload.to_string() },
                "retain": p.retain,
            })
        })
        .collect()
}

#[test]
fn configured_discovery_topics_and_client_id_are_unchanged() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let with = Config::parse(&format!("{CAMERA}{PRESETS}")).unwrap();
    let without = Config::parse(CAMERA).unwrap();
    assert_eq!(publications(&with), fixture["discovery_with_presets"]);
    assert_eq!(publications(&without), fixture["discovery_without_presets"]);
    let camera = &with.camera;
    assert_eq!(camera.availability(), fixture["availability"]);
    assert_eq!(camera.client_id(), fixture["client_id"]);
    assert_eq!(camera.topic("+/set"), fixture["subscribe"]);
    assert_eq!(
        format!("hostname:{}", camera.hostname()),
        fixture["dhcp_hostname"]
    );
}

#[test]
fn an_absent_camera_section_names_the_camera_e1zoom() {
    let cfg = Config::parse("[mqtt]\nhost = \"broker.example\"\n").unwrap();
    assert_eq!((cfg.camera.id(), cfg.camera.name()), ("e1zoom", "E1 Zoom"));
    let pan = &cfg.discovery()[0];
    assert_eq!(pan.topic, "homeassistant/number/cam_e1zoom/pan/config");
    assert_eq!(pan.payload["unique_id"], "cam_e1zoom_pan");
    assert_eq!(
        pan.payload["device"]["identifiers"],
        serde_json::json!(["cam_e1zoom"])
    );
    assert_eq!(pan.payload["device"]["name"], "E1 Zoom");
    assert_eq!(
        pan.payload["availability_topic"],
        "camera/e1zoom/availability"
    );
    assert_eq!(pan.payload["command_topic"], "camera/e1zoom/pan/set");
    assert_eq!(cfg.camera.client_id(), "cam_e1zoom");
    assert_eq!(cfg.camera.hostname(), "e1zoom");
}

#[test]
fn commands_arrive_only_under_the_configured_id() {
    let cfg = Config::parse("[camera]\nid = \"hall\"\n").unwrap();
    assert_eq!(cfg.camera.name(), "E1 Zoom");
    assert_eq!(
        cfg.command("camera/hall/pan/set", b"10", false),
        Ok(Some(cam::mqtt::Command::Pan(10)))
    );
    assert_eq!(cfg.command("camera/e1zoom/pan/set", b"10", false), Ok(None));
}

#[test]
fn invalid_ids_and_names_are_refused() {
    for bad in [
        "id = \"\"",
        "id = \"Hall1\"",
        "id = \"in-door\"",
        "id = \"a/b\"",
        "id = \"+\"",
        &format!("id = \"{}\"", "a".repeat(33)),
        "name = \"\"",
        "name = \"tab\\there\"",
        "name = \"line\\nbreak\"",
        &format!("name = \"{}\"", "n".repeat(65)),
        "colour = \"red\"",
    ] {
        assert!(
            Config::parse(&format!("[camera]\n{bad}\n")).is_err(),
            "{bad}"
        );
    }
    assert!(
        Config::parse(&format!(
            "[camera]\nid = \"{}\"\nname = \"{}\"\n",
            "a_9".repeat(10),
            "Ä".repeat(64)
        ))
        .is_ok()
    );
}

/// Leading the settings' names with their section changed only names: every topic, unique id, command topic and
/// state topic is what the firmware published before the sections (frozen in `fixtures/discovery-hall-ids.json`),
/// every name is the old one (through the four PTZ renames) with its section in front for a setting, and
/// everything sits on the one camera device.
#[test]
fn section_devices_changed_no_topic_unique_id_or_command_topic() {
    let frozen: Value =
        serde_json::from_str(include_str!("fixtures/discovery-hall-ids.json")).unwrap();
    let renamed = [
        ("PTZ speed", "Speed"),
        ("PTZ start speed", "Start speed"),
        ("PTZ ramp", "Ramp"),
        ("PTZ hold timeout", "Hold timeout"),
    ];
    for (key, cfg) in [
        ("discovery_with_presets", format!("{CAMERA}{PRESETS}")),
        ("discovery_without_presets", CAMERA.to_owned()),
    ] {
        let now = publications(&Config::parse(&cfg).unwrap());
        let old = frozen[key].as_array().unwrap();
        // every old topic is still published, and nothing new but the calibrate button
        let old_topics: Vec<&str> = old.iter().map(|r| r["topic"].as_str().unwrap()).collect();
        let now_topics: Vec<&str> = now
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["topic"].as_str().unwrap())
            .collect();
        // Telemetry moved to the shared catalog's ids: a retired sensor is cleared at runtime instead.
        let id_of = |t: &str| t.rsplit('/').nth(1).unwrap_or_default().to_owned();
        let retired = |t: &str| cam::telemetry::RETIRED.contains(&id_of(t).as_str());
        for t in old_topics.iter().filter(|t| !retired(t)) {
            assert!(now_topics.contains(t), "{t} vanished");
        }
        for r in old {
            let topic = r["topic"].as_str().unwrap();
            if retired(topic) {
                continue;
            }
            let p = now
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["topic"] == topic)
                .unwrap();
            let payload = p["payload"].as_str().unwrap();
            if payload.is_empty() {
                assert!(r["unique_id"].is_null(), "{topic}");
                continue;
            }
            let pl: Value = serde_json::from_str(payload).unwrap();
            for field in ["unique_id", "command_topic", "state_topic"] {
                assert_eq!(pl[field], r[field], "{topic} {field}");
            }
            if cam::telemetry::IDS.contains(&id_of(topic).as_str()) {
                continue; // the shared catalog names it
            }
            let old_name = r["name"].as_str().unwrap();
            let expected = renamed
                .iter()
                .find(|(from, _)| *from == old_name)
                .map_or(old_name, |(_, to)| to);
            let name = pl["name"].as_str().unwrap();
            assert!(
                name == expected || name.ends_with(&format!(" {expected}")),
                "{topic}: {name} is not {expected} with a section in front"
            );
            assert_eq!(
                pl["device"]["identifiers"],
                serde_json::json!(["cam_hall"]),
                "{topic}: one device"
            );
        }
    }
}

/// Prints the discovery output in the fixture's shape: `cargo test -p cam --test discovery_fixture
/// print_discovery_fixture -- --ignored --nocapture > cam/tests/fixtures/discovery-hall.json` regenerates it, a
/// deliberate one-off reviewed in its PR.
#[test]
#[ignore]
fn print_discovery_fixture() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let with = Config::parse(&format!("{CAMERA}{PRESETS}")).unwrap();
    let without = Config::parse(CAMERA).unwrap();
    let mut out = serde_json::Map::new();
    out.insert("discovery_with_presets".into(), publications(&with));
    out.insert("discovery_without_presets".into(), publications(&without));
    for key in ["availability", "client_id", "subscribe", "dhcp_hostname"] {
        out.insert(key.into(), fixture[key].clone());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(out)).unwrap()
    );
}
