//! Private production configuration. Parse errors deliberately omit input lines.
use serde::Deserialize;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

const MAX_BYTES: u64 = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub camera: Camera,
    pub mqtt: Mqtt,
    pub rtsp: Rtsp,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub onvif: Onvif,
    #[serde(default)]
    pub time: Time,
}
/// `[onvif]`: the ONVIF service (Profile S, discovery, snapshots), on by default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Onvif {
    /// HTTP port of the ONVIF services and `/snapshot.jpg`; 0 turns ONVIF off.
    #[serde(default = "onvif_port")]
    pub port: u16,
}
/// `[time]`: NTP servers the clock follows (none: the clock is left alone).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Time {
    #[serde(default)]
    pub ntp: Vec<String>,
}
fn onvif_port() -> u16 {
    80
}
impl Default for Onvif {
    fn default() -> Self {
        Self { port: onvif_port() }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    #[serde(default = "camera_id")]
    pub id: String,
    #[serde(default = "camera_name")]
    pub name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mqtt {
    pub host: String,
    #[serde(default = "mqtt_port")]
    pub port: u16,
    #[serde(default, alias = "user")]
    pub username: String,
    #[serde(default)]
    pub password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rtsp {
    pub username: String,
    pub password: String,
    #[serde(default = "rtsp_port")]
    pub port: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    #[serde(default = "enabled")]
    pub audio: bool,
    #[serde(default = "enabled")]
    pub watchdog: bool,
    /// Additional discovery must be explicitly authorized on the shared broker/HA.
    #[serde(default)]
    pub extended_controls: bool,
    /// Cold graph trials require an installed boot launcher and explicit opt-in.
    #[serde(default)]
    pub cold_controls: bool,
}
fn camera_id() -> String {
    "mi360".into()
}
fn camera_name() -> String {
    "Mi360".into()
}
fn mqtt_port() -> u16 {
    1883
}
fn rtsp_port() -> u16 {
    554
}
fn enabled() -> bool {
    true
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            id: camera_id(),
            name: camera_name(),
        }
    }
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            audio: true,
            watchdog: true,
            extended_controls: false,
            cold_controls: false,
        }
    }
}
impl Config {
    fn parse(text: &str) -> io::Result<Self> {
        if text.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("camera configuration exceeds16KiB"));
        }
        let config: Self =
            toml::from_str(text).map_err(|_| io::Error::other("invalid camera configuration"))?;
        let bounded = |text: &str, max: usize| {
            !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
        };
        if config.camera.id.is_empty()
            || config.camera.id.len() > 48
            || !config
                .camera
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            || !bounded(&config.camera.name, 64)
            || !bounded(&config.mqtt.host, 253)
            || config.mqtt.host.chars().any(char::is_whitespace)
            || config.mqtt.port == 0
            || config.mqtt.username.len() > 128
            || config.mqtt.password.len() > 256
            || config.mqtt.username.chars().any(char::is_control)
            || config.mqtt.password.chars().any(char::is_control)
            || !bounded(&config.rtsp.username, 128)
            || config.rtsp.username.contains(':')
            || !bounded(&config.rtsp.password, 128)
            || config.rtsp.port == 0
            || (config.runtime.cold_controls && !config.runtime.watchdog)
            || config.time.ntp.len() > 4
            || config
                .time
                .ntp
                .iter()
                .any(|server| !bounded(server, 253) || server.chars().any(char::is_whitespace))
        {
            return Err(io::Error::other(
                "invalid camera identity, broker or RTSP credentials",
            ));
        }
        Ok(config)
    }
}

pub fn load(path: &str) -> io::Result<Config> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configuration must be a root-owned0600 regular file, one link, at most16KiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::other("camera configuration is not UTF-8"))?;
    Config::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CONFIG: &str =
        "[mqtt]\nhost='127.0.0.1'\n[rtsp]\nusername='user'\npassword='private-test-value'\n";
    #[test]
    fn defaults_select_the_existing_device_namespace_and_audio() {
        let cfg = Config::parse(CONFIG).unwrap();
        assert_eq!(cfg.camera.id, "mi360");
        assert_eq!(cfg.rtsp.port, 554);
        assert!(cfg.runtime.audio && cfg.runtime.watchdog);
        assert!(!cfg.runtime.cold_controls);
    }
    #[test]
    fn unknown_inputs_and_other_namespaces_are_rejected_without_secret_context() {
        for text in [
            format!("{CONFIG}unknown='private-test-value'\n"),
            format!("[camera]\nid='bad/topic'\n{CONFIG}"),
            CONFIG.replace("host='127.0.0.1'", "host='bad host'"),
            CONFIG.replace("username='user'", "username='bad:user'"),
        ] {
            let err = Config::parse(&text).err().unwrap();
            assert!(!err.to_string().contains("private-test-value"));
        }
    }
    #[test]
    fn independent_camera_ids_are_validated_without_changing_the_default() {
        assert_eq!(Config::parse(CONFIG).unwrap().camera.id, "mi360");
        for id in ["mi360_a", "mi360_b"] {
            let cfg = Config::parse(&format!("[camera]\nid='{id}'\n{CONFIG}")).unwrap();
            assert_eq!(cfg.camera.id, id);
        }
        for id in [
            "",
            "bad/topic",
            "bad+topic",
            "bad#topic",
            "Upper",
            "bad space",
        ] {
            assert!(Config::parse(&format!("[camera]\nid='{id}'\n{CONFIG}")).is_err());
        }
    }

    #[test]
    fn cold_controls_are_explicit_and_require_the_boot_watchdog() {
        let text = format!("{CONFIG}[runtime]\ncold_controls=true\n");
        assert!(Config::parse(&text).unwrap().runtime.cold_controls);
        assert!(Config::parse(&(text + "watchdog=false\n")).is_err());
        assert!(Config::parse(&format!("{CONFIG}[runtime]\ncold_controls='true'\n")).is_err());
    }

    #[test]
    fn onvif_is_on_by_default_and_ntp_servers_are_bounded() {
        let cfg = Config::parse(CONFIG).unwrap();
        assert_eq!(cfg.onvif.port, 80);
        assert!(cfg.time.ntp.is_empty());
        let cfg = Config::parse(&format!(
            "{CONFIG}[onvif]\nport=0\n[time]\nntp=['ntp.example']\n"
        ))
        .unwrap();
        assert_eq!((cfg.onvif.port, cfg.time.ntp.len()), (0, 1));
        assert!(Config::parse(&format!("{CONFIG}[time]\nntp=['bad host']\n")).is_err());
        assert!(Config::parse(&format!("{CONFIG}[time]\nntp=['a','b','c','d','e']\n")).is_err());
    }
}
