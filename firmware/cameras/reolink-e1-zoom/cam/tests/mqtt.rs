use cam::mqtt::Config;

const CONFIG: &str = r#"
[mqtt]
host = "broker.example"
username = "camera"
password = "test-only"
[presets.Desk]
pan = 6195
tilt = 650
zoom = 0
"#;

#[test]
fn config_loads_broker_and_presets_and_rejects_unsafe_targets() {
    let cfg = Config::parse(CONFIG).unwrap();
    assert_eq!(cfg.mqtt.as_ref().unwrap().port, 1883);
    assert_eq!(cfg.presets["Desk"].pan, 6195);
    for bad in [
        CONFIG.replace("6195", "7361"),
        CONFIG.replace("650", "1241"),
        CONFIG.replace("zoom = 0", "zoom = 1025"),
        CONFIG.replace("host =", "hosst ="),
    ] {
        assert!(Config::parse(&bad).is_err());
    }
    assert!(Config::parse("").unwrap().mqtt.is_none());
    assert!(!format!("{:?}", cfg.mqtt).contains("test-only"));
}

#[test]
fn discovery_puts_everything_on_one_device_with_settings_named_by_section() {
    let cfg = Config::parse(CONFIG).unwrap();
    let messages = cfg.discovery();
    // pan, tilt, zoom, preset, reboot, refocus, calibrate focus, autofocus, pipeline, light, settings + every
    // setting + telemetry
    assert_eq!(
        messages.len(),
        11 + cam::settings::specs().len() + cam::telemetry::SENSORS.len()
    );
    let category = |id: &str| {
        messages
            .iter()
            .find(|m| m.topic.ends_with(&format!("/{id}/config")))
            .map(|m| m.payload["entity_category"].clone())
    };
    assert_eq!(category("aec"), Some(serde_json::json!("config")));
    assert_eq!(category("cpu"), Some(serde_json::json!("diagnostic")));
    assert_eq!(category("pan"), Some(serde_json::Value::Null));
    assert_eq!(
        category("refocus"),
        Some(serde_json::Value::Null),
        "a control beside PTZ"
    );
    assert_eq!(
        category("autofocus_state"),
        Some(serde_json::json!("diagnostic"))
    );
    let af = messages
        .iter()
        .find(|m| m.topic == "homeassistant/sensor/cam_e1zoom/autofocus_state/config")
        .unwrap();
    assert_eq!(
        af.payload["json_attributes_topic"],
        "camera/e1zoom/autofocus_attributes/state"
    );
    assert_eq!(
        af.payload["state_topic"],
        "camera/e1zoom/autofocus_state/state"
    );
    assert!(
        messages.iter().all(|m| !m.topic.contains("/focus/")),
        "focus/set is not advertised"
    );
    for m in &messages {
        assert!(m.retain);
        // one device, the camera; a setting's name leads with its section
        assert_eq!(
            m.payload["device"]["identifiers"],
            serde_json::json!(["cam_e1zoom"])
        );
        if m.topic.ends_with("/mic_volume/config") {
            assert_eq!(m.payload["name"], "Audio Mic volume");
        }
        if m.topic.ends_with("/ptz_ramp/config") {
            assert_eq!(m.payload["name"], "Pan & tilt Ramp");
        }
        assert_eq!(
            m.payload["availability_topic"],
            "camera/e1zoom/availability"
        );
        assert!(
            m.payload["unique_id"]
                .as_str()
                .unwrap()
                .starts_with("cam_e1zoom_")
        );
    }
    let zoom = messages
        .iter()
        .find(|m| m.topic == "homeassistant/number/cam_e1zoom/zoom/config")
        .unwrap();
    assert_eq!(zoom.payload["max"], 1024);
    assert_eq!(zoom.payload["command_topic"], "camera/e1zoom/zoom/set");
    let preset = messages
        .iter()
        .find(|m| m.payload["name"] == "Preset")
        .unwrap();
    assert_eq!(preset.payload["options"], serde_json::json!(["Desk"]));
    let temp = messages
        .iter()
        .find(|m| m.payload["name"] == "Temperature")
        .unwrap();
    assert_eq!(temp.payload["unit_of_measurement"], "°C");
    let empty = Config::parse("[mqtt]\nhost = 'localhost'").unwrap();
    assert_eq!(
        empty
            .discovery()
            .iter()
            .find(|m| m.topic == "homeassistant/select/cam_e1zoom/preset/config")
            .unwrap()
            .payload,
        serde_json::Value::Null
    );
}

use cam::mqtt::Command;
#[test]
fn commands_validate_topics_payloads_and_presets_before_hardware_access() {
    let cfg = Config::parse(CONFIG).unwrap();
    for (id, payload, expected) in [
        ("pan", "6195", Command::Pan(6195)),
        ("tilt", "650", Command::Tilt(650)),
        ("zoom", "1024", Command::Zoom(1024)),
        ("preset", "Desk", Command::Preset("Desk".into())),
        ("preset_save", "Desk", Command::SavePreset("Desk".into())),
        ("ir", "auto", Command::Setting("ir", "auto".into())),
        ("ir", "night", Command::Setting("ir", "night".into())),
        ("led", "OFF", Command::Setting("led", "OFF".into())),
        (
            "mic_volume",
            "55",
            Command::Setting("mic_volume", "55".into()),
        ),
        ("move", "left", Command::Move(cam::mqtt::Move::Left)),
        ("move", "stop", Command::Move(cam::mqtt::Move::Stop)),
        ("reboot", "PRESS", Command::Reboot),
        ("refocus", "PRESS", Command::Refocus),
        ("focus", "1705", Command::Focus(1705)),
        (
            "af_watch_hold",
            "60",
            Command::Setting("af_watch_hold", "60".into()),
        ),
    ] {
        let topic = format!("camera/e1zoom/{id}/set");
        assert_eq!(
            cfg.command(&topic, payload.as_bytes(), false).unwrap(),
            Some(expected)
        );
        assert!(
            cfg.command(&topic, payload.as_bytes(), true)
                .unwrap()
                .is_none()
        );
    }
    for (id, payload) in [
        ("pan", "7361"),
        ("tilt", "-1"),
        ("zoom", "1025"),
        ("zoom", "1.5"),
        ("preset", " "),
        ("preset_save", ""),
        ("ir", "dusk"),
        ("led", "yes"),
        ("reboot", "yes"),
        ("refocus", "ON"),
        ("focus", "1706"),
        ("af_watch_drop", "95"),
        ("move", "spin"),
        ("main_fps", "4"),
    ] {
        assert!(
            cfg.command(
                &format!("camera/e1zoom/{id}/set"),
                payload.as_bytes(),
                false
            )
            .is_err()
        );
    }
    assert!(
        cfg.command("other/pan/set", b"10", false)
            .unwrap()
            .is_none()
    );
    assert!(cfg.command("camera/e1zoom/led/set", &[255], false).is_err());
    // Any valid name reaches the hardware thread: a save creates the preset there.
    let none = Config::default();
    assert_eq!(
        none.command("camera/e1zoom/preset_save/set", b"Window", false),
        Ok(Some(Command::SavePreset("Window".into())))
    );
    assert_eq!(
        none.command("camera/e1zoom/preset/set", b"Missing", false),
        Ok(Some(Command::Preset("Missing".into())))
    );
    assert_eq!(
        none.command(
            "camera/e1zoom/motion/set",
            br#"{"v":1,"op":"remove_preset","name":"Desk"}"#,
            false
        ),
        Ok(Some(Command::RemovePreset("Desk".into())))
    );
}

#[test]
fn pipeline_health_requires_recent_frames_from_both_encoders() {
    let health = cam::mqtt::PipelineHealth::default();
    assert!(!health.healthy());
    health.frame(0);
    assert!(!health.healthy());
    health.frame(1);
    assert!(health.healthy());
}

#[test]
fn stop_is_a_nonretained_control_command() {
    let cfg = cam::mqtt::Config::parse("").unwrap();
    assert!(
        cfg.command("camera/e1zoom/stop/set", b"PRESS", false)
            .unwrap()
            .is_some()
    );
    assert!(
        cfg.command("camera/e1zoom/stop/set", b"PRESS", true)
            .unwrap()
            .is_none()
    );
    assert!(
        cfg.command("camera/e1zoom/stop/set", b"bad", false)
            .is_err()
    );
}

#[test]
fn discovery_inputs_are_bounded_before_building_packets() {
    let long_name = "x".repeat(65);
    let text = format!("[presets.{long_name}]\npan=0\ntilt=0\nzoom=0\n");
    assert!(Config::parse(&text).is_err());
    assert!(Config::parse(&" ".repeat(16 * 1024 + 1)).is_err());
}
