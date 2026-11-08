//! Mi 360's MQTT vocabulary on the shared layer: its candidate entity catalog and its command parser.
//! The runtime removes capabilities its hardware does not qualify before handing the catalog over.
use camera_transport::mqtt::{self, Category, Domain, Entity, Mode, ptz};
use camera_transport::telemetry;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrMode {
    Day,
    Night,
    Auto,
}

/// Mi 360's own commands beside the shared Setting/Stop/Release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Ptz(ptz::Ptz),
    IrMode(IrMode),
    IrLight(bool),
    Led(bool),
    Reboot,
    MicVolume(u8),
    SpeakerVolume(u8),
}

pub type Command = mqtt::Command<App>;

/// A plain entity; sensors are diagnostic and measured when they have a unit.
pub fn entity(id: &str, name: &str, domain: Domain, unit: Option<&'static str>) -> Entity {
    let sensor = matches!(domain, Domain::Sensor);
    let mut e = Entity::new(id, name, domain);
    if sensor {
        e = e.category(Category::Diagnostic);
        if unit.is_some() {
            e = e.state_class("measurement");
        }
    }
    e.unit = unit;
    e
}

/// Candidate catalog. Position limits come from the hardware owner's one source.
pub fn catalog(pan_max: i32, tilt_max: i32) -> Vec<Entity> {
    let number = |max| Domain::number(0, max, Mode::Slider);
    let mut result = vec![
        entity("pan", "Pan", number(pan_max), None),
        entity("tilt", "Tilt", number(tilt_max), None),
        entity("stop", "Stop", Domain::Button, None),
        entity("ir_mode", "IR mode", Domain::select(["day", "night"]), None),
        entity("ir_lamp", "IR lamp", Domain::Switch, None),
        entity("status_led", "Status LED", Domain::Switch, None),
        entity("mic_volume", "Microphone volume", number(100), Some("%")),
        entity("speaker_volume", "Speaker volume", number(100), Some("%")),
        entity("reboot", "Reboot", Domain::Button, None).device_class("restart"),
        entity(
            "pipeline_health",
            "Pipeline health",
            Domain::BinarySensor,
            None,
        )
        .category(Category::Diagnostic),
    ];
    result.extend(telemetry().map(telemetry::Sensor::entity));
    result
}

/// Mi 360's sensors of the shared catalog (no temperature source).
const TELEMETRY: &[&str] = &[
    "cpu_busy",
    "memory_available",
    "daemon_memory",
    "load",
    "uptime",
    "main_fps_actual",
    "main_bitrate_actual",
    "sub_fps_actual",
    "sub_bitrate_actual",
    "wifi_signal",
];
/// Sensor ids of earlier releases that no longer exist; their discovery is cleared.
/// `ae_luma` and `exposure_us` became `isp_luma` and `isp_exposure_us`.
pub const RETIRED_SENSORS: &[&str] = &[
    "main_fps",
    "sub_fps",
    "main_bitrate",
    "sub_bitrate",
    "ae_luma",
    "exposure_us",
];

pub fn telemetry() -> impl Iterator<Item = &'static telemetry::Sensor> {
    telemetry::SENSORS
        .iter()
        .filter(|s| TELEMETRY.contains(&s.id))
}

/// ISP read-outs on the shared catalog, by tuning control; offered only with extended controls and
/// only when the control reads at startup.
pub const ISP_TELEMETRY: &[(u32, &str)] = &[(50, "isp_luma"), (51, "isp_exposure_us")];

pub fn isp_telemetry() -> impl Iterator<Item = (u32, &'static telemetry::Sensor)> {
    ISP_TELEMETRY
        .iter()
        .filter_map(|&(control, id)| telemetry::sensor_by_id(id).map(|sensor| (control, sensor)))
}

/// Commands accepted only for capabilities present in the final catalog.
pub struct Parser {
    syntax: ptz::Syntax,
    ids: BTreeSet<String>,
    ir_modes: Vec<String>,
    volume_max: i32,
}

impl Parser {
    pub fn new(entities: &[Entity], position3d: bool) -> Self {
        let max = |id: &str| {
            entities
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| match e.domain {
                    Domain::Number { max, .. } => Some(max),
                    _ => None,
                })
        };
        let ir_modes = entities
            .iter()
            .find(|e| e.id == "ir_mode")
            .and_then(|e| match &e.domain {
                Domain::Select { options } => Some(options.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Self {
            syntax: ptz::Syntax {
                pan: max("pan"),
                tilt: max("tilt"),
                zoom: None,
                relative: ptz::Relative::Topic { limit: 256 },
                fractions: true,
                vectors: true,
                move_stop_releases: false,
                position3d,
            },
            ids: entities.iter().map(|e| e.id.clone()).collect(),
            ir_modes,
            volume_max: 100,
        }
    }
}

impl mqtt::Parser for Parser {
    type App = App;
    fn parse(&self, id: &str, raw: &str) -> Result<Option<Command>, String> {
        let json = matches!(id, "position3d" | "motion");
        let text = raw.trim();
        // JSON carries four coordinates plus an operation; scalars stay short.
        if !json && (raw.len() > 64 || text.is_empty() || text.chars().any(char::is_control)) {
            return Err(format!("{id}: invalid payload"));
        }
        if let Some(result) = ptz::parse(&self.syntax, id, raw) {
            return result.map(|c| Some(c.map(App::Ptz)));
        }
        if !self.ids.contains(id) {
            return Ok(None);
        }
        let switch = || match text {
            "ON" => Ok(true),
            "OFF" => Ok(false),
            _ => Err(format!("{id}: expected ON or OFF")),
        };
        let press = |app| {
            if text == "PRESS" {
                Ok(Some(mqtt::Command::App(app)))
            } else {
                Err(format!("{id}: expected PRESS"))
            }
        };
        let volume = || {
            text.strip_suffix(".0")
                .unwrap_or(text)
                .parse::<i32>()
                .ok()
                .filter(|v| (0..=self.volume_max).contains(v))
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| format!("{id}: volume outside range"))
        };
        let app = match id {
            "home" => return press(App::Ptz(ptz::Ptz::Home)),
            "reboot" => return press(App::Reboot),
            "mic_volume" => App::MicVolume(volume()?),
            "speaker_volume" => App::SpeakerVolume(volume()?),
            "ir_lamp" => App::IrLight(switch()?),
            "status_led" => App::Led(switch()?),
            "ir_mode" if self.ir_modes.iter().any(|m| m == text) => App::IrMode(match text {
                "day" => IrMode::Day,
                "night" => IrMode::Night,
                "auto" => IrMode::Auto,
                _ => return Err(format!("{id}: unknown mode")),
            }),
            "ir_mode" => return Err(format!("{id}: unknown mode")),
            _ => return Ok(None),
        };
        Ok(Some(mqtt::Command::App(app)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::mqtt::Parser as _;
    use camera_transport::mqtt::{Direction, Position, Ptz};
    use camera_transport::position3d;

    fn parser(position3d: bool) -> Parser {
        Parser::new(&catalog(2404, 713), position3d)
    }
    fn ok(p: &Parser, id: &str, text: &str) -> Option<Command> {
        p.parse(id, text).ok().flatten()
    }

    #[test]
    fn discovery_keeps_mi360_topics_unique_ids_and_entity_ids() {
        let mut entities = catalog(2404, 713);
        entities.retain(|e| {
            ["pan", "stop", "pipeline_health", "reboot", "cpu_busy"].contains(&e.id.as_str())
        });
        let mut cfg = mqtt::Config::new(
            mqtt::Broker {
                host: "broker".into(),
                port: 1883,
                username: String::new(),
                password: String::new(),
            },
            "cam_mi360",
            "camera/mi360",
            mqtt::Device {
                name: "Mi360".into(),
                manufacturer: "Xiaomi".into(),
                model: "MJSXJ10CM".into(),
                sw_version: Some("test".into()),
            },
            entities,
        );
        cfg.entity_id_prefix = Some("mi360".into());
        let out = cfg.discovery().unwrap();
        let json = |i: usize| serde_json::from_slice::<serde_json::Value>(&out[i].payload).unwrap();
        assert_eq!(out[0].topic, "homeassistant/number/cam_mi360/pan/config");
        let ids: Vec<_> = (0..out.len())
            .map(|i| json(i)["unique_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            ids,
            [
                "cam_mi360_pan",
                "cam_mi360_stop",
                "cam_mi360_reboot",
                "cam_mi360_pipeline_health",
                "cam_mi360_cpu_busy"
            ]
        );
        assert_eq!(json(0)["mode"], "slider");
        assert_eq!(json(0)["max"], 2404);
        assert_eq!(json(2)["device_class"], "restart");
        let health = json(3);
        assert_eq!(
            health["default_entity_id"],
            "binary_sensor.mi360_pipeline_health"
        );
        assert_eq!(health["state_topic"], "camera/mi360/pipeline_health/state");
        assert_eq!(health["entity_category"], "diagnostic");
        assert_eq!(json(4)["state_class"], "measurement");
        assert_eq!(json(4)["unit_of_measurement"], "%");
    }

    #[test]
    fn telemetry_uses_the_shared_catalog_and_secondary_sensors_start_disabled() {
        assert_eq!(
            telemetry().count(),
            TELEMETRY.len(),
            "every id is in the catalog"
        );
        let entities = catalog(2404, 713);
        for id in TELEMETRY {
            let e = entities.iter().find(|e| e.id == *id).unwrap();
            assert_eq!(e.category, Some(Category::Diagnostic));
            assert_eq!(e.state_class, Some("measurement"));
            let secondary = [
                "load",
                "daemon_memory",
                "sub_fps_actual",
                "sub_bitrate_actual",
            ];
            assert_eq!(e.enabled_by_default, !secondary.contains(id), "{id}");
        }
        for id in RETIRED_SENSORS {
            assert!(entities.iter().all(|e| e.id != *id), "{id} is retired");
        }
    }

    #[test]
    fn isp_read_outs_are_secondary_diagnostic_sensors_of_the_shared_catalog() {
        let ids: Vec<_> = isp_telemetry().map(|(c, s)| (c, s.id)).collect();
        assert_eq!(ids, [(50, "isp_luma"), (51, "isp_exposure_us")]);
        for (_, s) in isp_telemetry() {
            let e = s.entity();
            assert_eq!(e.category, Some(Category::Diagnostic));
            assert!(!e.enabled_by_default, "{}", s.id);
            assert!(!RETIRED_SENSORS.contains(&s.id));
        }
        assert_eq!(
            telemetry::sensor_by_id("isp_exposure_us").unwrap().unit,
            Some("µs")
        );
    }

    #[test]
    fn commands_check_real_capabilities_and_limits() {
        let p = parser(false);
        assert_eq!(
            ok(&p, "pan", "2404"),
            Some(Command::App(App::Ptz(Ptz::Pan(Position::Absolute(2404)))))
        );
        assert_eq!(
            ok(&p, "tilt", "713.0"),
            Some(Command::App(App::Ptz(Ptz::Tilt(Position::Absolute(713)))))
        );
        assert_eq!(
            ok(&p, "pan_relative", "-20"),
            Some(Command::App(App::Ptz(Ptz::Pan(Position::Relative(-20)))))
        );
        assert_eq!(ok(&p, "stop", "PRESS"), Some(Command::Stop));
        assert_eq!(ok(&p, "move", "stop"), Some(Command::Stop));
        assert_eq!(ok(&p, "move", "release"), Some(Command::Release));
        assert_eq!(
            ok(&p, "move", r#"{"pan":-1,"tilt":1}"#),
            Some(Command::App(App::Ptz(Ptz::Move(Direction::UpLeft))))
        );
        assert_eq!(
            ok(&p, "ir_mode", "night"),
            Some(Command::App(App::IrMode(IrMode::Night)))
        );
        assert_eq!(
            ok(&p, "status_led", "OFF"),
            Some(Command::App(App::Led(false)))
        );
        assert_eq!(
            ok(&p, "speaker_volume", "10"),
            Some(Command::App(App::SpeakerVolume(10)))
        );
        assert_eq!(ok(&p, "reboot", "PRESS"), Some(Command::App(App::Reboot)));
        for (id, text) in [
            ("pan", "2405"),
            ("tilt", "-1"),
            ("ir_mode", "auto"),
            ("reboot", "ON"),
            ("pan_relative", "257"),
            ("home", "PRESS"),
            (
                "position3d",
                r#"{"start_x":0,"start_y":255,"end_x":0,"end_y":255}"#,
            ),
            ("pan", "1\0"),
        ] {
            assert_eq!(ok(&p, id, text), None, "{id} {text}");
        }
        assert!(p.parse("pan", &"1".repeat(65)).is_err(), "scalar budget");
        let mut entities = catalog(2404, 713);
        entities.retain(|e| !matches!(e.id.as_str(), "pan" | "tilt" | "speaker_volume"));
        let without = Parser::new(&entities, false);
        assert_eq!(ok(&without, "move", "release"), None, "no axes, no moves");
        assert_eq!(
            ok(&without, "speaker_volume", "10"),
            None,
            "omitted capability"
        );
    }

    #[test]
    fn position3d_only_when_calibrated_and_bounded() {
        let p = parser(true);
        let point = r#"{"start_x":0,"start_y":255,"end_x":0,"end_y":255}"#;
        assert!(
            matches!(ok(&p, "position3d", point), Some(Command::App(App::Ptz(Ptz::Position3d(r)))) if r.is_point())
        );
        let boxed = r#"{"start_x":0,"start_y":0,"end_x":255,"end_y":255,"operation":"zoomout"}"#;
        assert_eq!(
            ok(&p, "position3d", boxed),
            Some(Command::App(App::Ptz(Ptz::Position3d(
                position3d::Request {
                    start: (0, 0),
                    end: (255, 255),
                    operation: position3d::Operation::ZoomOut
                }
            ))))
        );
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"start_x":256,"start_y":0,"end_x":0,"end_y":0}"#,
        ] {
            assert_eq!(ok(&p, "position3d", bad), None, "{bad}");
        }
        let copied = ok(&p, "position3d", point).unwrap();
        let again = copied;
        assert_eq!(copied, again, "Command remains Copy");
    }
}
