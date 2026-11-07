//! Home Assistant MQTT device discovery: one retained device document with every component. Unique
//! ids hang under the configured node id and topics under the configured prefix, so an installation
//! that set both can keep the entities it already has.
//!
//! Topics under `<prefix>`: `availability` (retained JSON string `"online"`/`"offline"`), `door/state`
//! (retained JSON string `"open"`/`"closed"`: open while the strike is released or the reed is open),
//! `reed` (retained, the contact alone), `lock/state` (JSON attributes with `lock_actuator`), `call_button` (`press`/`release` events), `access` (JSON
//! `{"event_type":"granted","name":...}`), `door/command` (incoming). Every payload is JSON because the
//! discovery templates read `value_json`.

/// Availability payloads (JSON strings).
pub const ONLINE: &str = "\"online\"";
pub const OFFLINE: &str = "\"offline\"";

/// `door/state` and `reed` payload: open or closed.
pub fn door_state(open: bool) -> &'static str {
    if open { "\"open\"" } else { "\"closed\"" }
}

pub struct Topics {
    pub availability: String,
    pub door_state: String,
    pub lock_state: String,
    /// Reed contact on the leaf: `"open"` / `"closed"`, retained.
    pub reed: String,
    pub call_button: String,
    pub access: String,
    pub door_command: String,
    pub discovery: String,
}

impl Topics {
    pub fn new(prefix: &str, node: &str) -> Self {
        Self {
            availability: format!("{prefix}/availability"),
            door_state: format!("{prefix}/door/state"),
            lock_state: format!("{prefix}/lock/state"),
            reed: format!("{prefix}/reed"),
            call_button: format!("{prefix}/call_button"),
            access: format!("{prefix}/access"),
            door_command: format!("{prefix}/door/command"),
            discovery: format!("homeassistant/device/{node}/config"),
        }
    }
}

/// The retained device-discovery document.
/// `name` is the device name Home Assistant shows; the configuration restricts it to JSON-safe ASCII.
pub fn payload(node: &str, prefix: &str, name: &str, version: &str) -> String {
    let t = Topics::new(prefix, node);
    let avail = format!(
        r#"[{{"topic":"{}","value_template":"{{{{ value_json }}}}"}}]"#,
        t.availability
    );
    let open = r#"{\"command\":\"open\"}"#;
    let quiet = r#"{\"command\":\"open_quiet\"}"#;
    let close = r#"{\"command\":\"close\"}"#;
    format!(
        concat!(
            r#"{{"device":{{"identifiers":["{node}"],"name":"{name}","manufacturer":"Hikvision","model":"DS-KD8003-IME1","sw_version":"{version}"}},"#,
            r#""origin":{{"name":"ds-kd8003-ime1-cam","sw_version":"{version}"}},"#,
            r#""availability":{avail},"qos":1,"components":{{"#,
            r#""gate":{{"platform":"cover","unique_id":"{node}_gate","name":null,"device_class":"gate","#,
            r#""state_topic":"{door_state}","value_template":"{{{{ value_json }}}}","state_open":"open","state_closed":"closed","#,
            r#""json_attributes_topic":"{lock_state}","command_topic":"{door_command}","#,
            r#""payload_open":"{open}","payload_close":"{close}","payload_stop":null,"optimistic":false,"retain":false}},"#,
            r#""open":{{"platform":"button","unique_id":"{node}_open","name":"Open with announcement","icon":"mdi:gate-open","#,
            r#""command_topic":"{door_command}","payload_press":"{open}","retain":false}},"#,
            r#""quiet":{{"platform":"button","unique_id":"{node}_open_quiet","name":"Open quietly","icon":"mdi:volume-off","#,
            r#""command_topic":"{door_command}","payload_press":"{quiet}","retain":false}},"#,
            r#""close":{{"platform":"button","unique_id":"{node}_close","name":"Close now","icon":"mdi:gate","#,
            r#""command_topic":"{door_command}","payload_press":"{close}","retain":false}},"#,
            r#""reed":{{"platform":"binary_sensor","unique_id":"{node}_reed","name":"Door","device_class":"door","#,
            r#""state_topic":"{reed}","value_template":"{{{{ value_json }}}}","payload_on":"open","payload_off":"closed"}},"#,
            r#""lock_actuator":{{"platform":"binary_sensor","unique_id":"{node}_lock_actuator","name":"Lock actuator","device_class":"lock","#,
            r#""state_topic":"{lock_state}","payload_on":"unlocked","payload_off":"locked","#,
            r#""value_template":"{{{{ 'None' if value_json.lock_actuator_unlocked is none else value_json.lock_actuator }}}}"}},"#,
            r#""doorbell":{{"platform":"event","unique_id":"{node}_doorbell","name":"Doorbell","device_class":"doorbell","#,
            r#""state_topic":"{call_button}","event_types":["press","release"]}},"#,
            r#""access":{{"platform":"event","unique_id":"{node}_access","name":"Access","icon":"mdi:dialpad","#,
            r#""state_topic":"{access}","event_types":["granted","denied","remote"]}}"#,
            r#"}}}}"#
        ),
        node = node,
        name = name,
        version = version,
        avail = avail,
        door_state = t.door_state,
        lock_state = t.lock_state,
        reed = t.reed,
        door_command = t.door_command,
        call_button = t.call_button,
        access = t.access,
        open = open,
        quiet = quiet,
        close = close,
    )
}

/// `door/command` payloads the application acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorCommand {
    Open,
    OpenQuiet,
    /// Release the strike now instead of waiting for the pulse to run out.
    Close,
}

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

/// `lock/state` attributes.
pub fn lock_state(unlocked: bool) -> String {
    format!(
        r#"{{"lock_actuator":"{}","lock_actuator_unlocked":{unlocked}}}"#,
        if unlocked { "unlocked" } else { "locked" }
    )
}

/// `call_button` and `access` event payloads (Home Assistant `event` platform).
pub fn event(event_type: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!(r#"{{"event_type":"{event_type}","name":"{n}"}}"#),
        None => format!(r#"{{"event_type":"{event_type}"}}"#),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_valid_json_under_the_configured_identities() {
        let doc: serde_json::Value = serde_json::from_str(&payload(
            "ds_kd8003_ime1",
            "ds-kd8003-ime1",
            "Front gate",
            "0.1.0",
        ))
        .unwrap();
        assert_eq!(doc["device"]["identifiers"][0], "ds_kd8003_ime1");
        assert_eq!(doc["device"]["name"], "Front gate");
        let c = &doc["components"];
        assert_eq!(c["gate"]["unique_id"], "ds_kd8003_ime1_gate");
        assert_eq!(c["gate"]["command_topic"], "ds-kd8003-ime1/door/command");
        assert_eq!(c["gate"]["payload_open"], r#"{"command":"open"}"#);
        assert_eq!(c["gate"]["payload_close"], r#"{"command":"close"}"#);
        assert_eq!(c["close"]["payload_press"], r#"{"command":"close"}"#);
        assert_eq!(c["open"]["unique_id"], "ds_kd8003_ime1_open");
        assert_eq!(c["quiet"]["payload_press"], r#"{"command":"open_quiet"}"#);
        assert_eq!(c["doorbell"]["state_topic"], "ds-kd8003-ime1/call_button");
        assert_eq!(c["lock_actuator"]["payload_on"], "unlocked");
        assert_eq!(c["reed"]["state_topic"], "ds-kd8003-ime1/reed");
        assert_eq!(c["reed"]["device_class"], "door");
        assert_eq!(c["access"]["event_types"][0], "granted");
        assert_eq!(
            doc["availability"][0]["topic"],
            "ds-kd8003-ime1/availability"
        );
    }

    #[test]
    fn commands_accept_bare_quoted_and_json_payloads() {
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
    }

    #[test]
    fn state_payloads_are_json() {
        let v: serde_json::Value = serde_json::from_str(&lock_state(true)).unwrap();
        assert_eq!(v["lock_actuator"], "unlocked");
        assert_eq!(v["lock_actuator_unlocked"], true);
        let e: serde_json::Value =
            serde_json::from_str(&event("granted", Some("resident"))).unwrap();
        assert_eq!(e["name"], "resident");
        let e: serde_json::Value = serde_json::from_str(&event("press", None)).unwrap();
        assert_eq!(e["event_type"], "press");
        assert_eq!(serde_json::from_str::<String>(ONLINE).unwrap(), "online");
        assert_eq!(
            serde_json::from_str::<String>(door_state(true)).unwrap(),
            "open"
        );
    }
}
