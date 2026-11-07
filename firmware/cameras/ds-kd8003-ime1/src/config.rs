//! `key=value` configuration (one file, mode 0600, read once at startup) and the PIN table.
//! No value is logged by this module; the caller decides what leaves the device.
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Where the startup hook keeps owner files on the persistent config partition.
pub const DEFAULT_BASE: &str = "/home/config/custom";
/// RAM directory the supervisor unpacks the payload into.
pub const DEFAULT_RUNTIME: &str = "/home/app/custom/native";
/// Written next to the configuration file once a boot ran healthily; the supervisor looks for it there.
pub const HEALTH_MARKER: &str = "native.ok";

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_user: String,
    pub mqtt_password: String,
    /// Topic prefix every state/command topic hangs under (`[A-Za-z0-9_/-]`).
    pub mqtt_prefix: String,
    /// Home Assistant device node id (discovery topic and unique_id prefix, `[A-Za-z0-9_-]`).
    pub mqtt_node: String,
    /// Device name Home Assistant shows and prefixes entity names with.
    pub device_name: String,
    pub rtsp_user: String,
    pub rtsp_password: String,
    pub rtsp_port: u16,
    /// Strike relay pulse for an authorized opening, seconds.
    pub open_seconds: u32,
    /// DAC output level applied at start, dB.
    pub codec_out_db: i32,
    /// Digital gain applied to clips, dB (negative attenuates).
    pub clip_db: i32,
    /// Extra gain on talkback after the mixer's AGC/limiter, dB (0..18); clipped, loudness over purity.
    pub talk_db: i32,
    pub mic_gain_db: i32,
    /// 16 raw bytes: the keypad bootstrap key.
    pub keypad_key: PathBuf,
    pub pins: PathBuf,
    /// Directory of `<name>.pcm` clips: raw s16le mono 16 kHz.
    pub clips: PathBuf,
    /// The health marker, derived from the configuration file's directory.
    pub health: PathBuf,
    /// Diagnostic HTTP port (0 disables; off by default).
    pub http_port: u16,
    /// ONVIF HTTP port (0 disables; on by default).
    pub onvif_port: u16,
    /// NTP servers the clock follows (none: left alone).
    pub ntp: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mqtt_host: String::new(),
            mqtt_port: 1883,
            mqtt_user: String::new(),
            mqtt_password: String::new(),
            mqtt_prefix: "ds-kd8003-ime1".into(),
            mqtt_node: "ds_kd8003_ime1".into(),
            device_name: "DS-KD8003-IME1".into(),
            rtsp_user: String::new(),
            rtsp_password: String::new(),
            rtsp_port: 554,
            open_seconds: 5,
            codec_out_db: 0,
            clip_db: 0,
            talk_db: 6,
            mic_gain_db: 0,
            keypad_key: format!("{DEFAULT_BASE}/keypad.key").into(),
            pins: format!("{DEFAULT_BASE}/pins.conf").into(),
            clips: format!("{DEFAULT_RUNTIME}/clips").into(),
            health: format!("{DEFAULT_BASE}/{HEALTH_MARKER}").into(),
            http_port: 0,
            onvif_port: 80,
            ntp: Vec::new(),
        }
    }
}

/// Redacting: passwords never reach a log through `{:?}`.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Config(mqtt {}:{} as {}, prefix {}, node {}, rtsp {}:{}, open {} s)",
            self.mqtt_host,
            self.mqtt_port,
            self.mqtt_user,
            self.mqtt_prefix,
            self.mqtt_node,
            self.rtsp_user,
            self.rtsp_port,
            self.open_seconds
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Io,
    Line(usize),
    Unknown(String),
    Range(&'static str),
}

fn topic_chars(s: &str, slash: bool) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.ends_with('/')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (slash && b == b'/'))
}

impl Config {
    /// Reads the file; the health marker lives in the same directory.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let mut c = Self::parse(&std::fs::read_to_string(path).map_err(|_| Error::Io)?)?;
        c.health = path.parent().unwrap_or(Path::new(".")).join(HEALTH_MARKER);
        Ok(c)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut c = Self::default();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(Error::Line(index + 1))?;
            let (key, value) = (key.trim(), value.trim().to_string());
            let number = |name| value.parse::<i64>().map_err(|_| Error::Range(name));
            match key {
                "mqtt_host" => c.mqtt_host = value,
                "mqtt_port" => c.mqtt_port = port(number("mqtt_port")?, "mqtt_port")?,
                "mqtt_user" => c.mqtt_user = value,
                "mqtt_password" => c.mqtt_password = value,
                "mqtt_prefix" => c.mqtt_prefix = value,
                "mqtt_node" => c.mqtt_node = value,
                "device_name" => c.device_name = value,
                "rtsp_user" => c.rtsp_user = value,
                "rtsp_password" => c.rtsp_password = value,
                "rtsp_port" => c.rtsp_port = port(number("rtsp_port")?, "rtsp_port")?,
                "http_port" => {
                    let v = number("http_port")?;
                    c.http_port = u16::try_from(v).map_err(|_| Error::Range("http_port"))?;
                }
                "onvif_port" => {
                    let v = number("onvif_port")?;
                    c.onvif_port = u16::try_from(v).map_err(|_| Error::Range("onvif_port"))?;
                }
                "ntp" => {
                    c.ntp = value
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                    if c.ntp.len() > 4
                        || c.ntp
                            .iter()
                            .any(|s| s.len() > 253 || s.contains(char::is_whitespace))
                    {
                        return Err(Error::Range("ntp"));
                    }
                }
                "open_seconds" => {
                    let v = number("open_seconds")?;
                    if !(1..=120).contains(&v) {
                        return Err(Error::Range("open_seconds"));
                    }
                    c.open_seconds = v as u32;
                }
                "codec_out_db" => {
                    c.codec_out_db = bounded(number("codec_out_db")?, -30, 6, "codec_out_db")?
                }
                "clip_db" => c.clip_db = bounded(number("clip_db")?, -40, 12, "clip_db")?,
                "talk_db" => c.talk_db = bounded(number("talk_db")?, 0, 18, "talk_db")?,
                "mic_gain_db" => {
                    c.mic_gain_db = bounded(number("mic_gain_db")?, 0, 40, "mic_gain_db")?
                }
                "keypad_key" => c.keypad_key = value.into(),
                "pins" => c.pins = value.into(),
                "clips" => c.clips = value.into(),
                other => return Err(Error::Unknown(other.into())),
            }
        }
        if c.mqtt_host.is_empty() {
            return Err(Error::Range("mqtt_host"));
        }
        if !topic_chars(&c.mqtt_prefix, true) || !topic_chars(&c.mqtt_node, false) {
            return Err(Error::Range("mqtt_prefix"));
        }
        // It lands in a JSON string unescaped: printable ASCII without quotes or backslashes.
        if c.device_name.is_empty()
            || !c
                .device_name
                .bytes()
                .all(|b| (b' '..=b'~').contains(&b) && b != b'"' && b != b'\\')
        {
            return Err(Error::Range("device_name"));
        }
        if c.rtsp_user.is_empty() || c.rtsp_password.is_empty() {
            return Err(Error::Range("rtsp_user"));
        }
        Ok(c)
    }
}

fn port(v: i64, name: &'static str) -> Result<u16, Error> {
    u16::try_from(v)
        .ok()
        .filter(|p| *p != 0)
        .ok_or(Error::Range(name))
}

fn bounded(v: i64, min: i64, max: i64, name: &'static str) -> Result<i32, Error> {
    if (min..=max).contains(&v) {
        Ok(v as i32)
    } else {
        Err(Error::Range(name))
    }
}

/// One authorized code: the digits, the person it belongs to and the clip to play.
pub struct Credential {
    pub pin: Zeroizing<String>,
    pub name: String,
    pub clip: String,
}

/// `<digits> <name> [clip]` per line; `#` starts a comment. Names and clip names are `[A-Za-z0-9_-]`.
/// No code may contain another: codes open without `#` as soon as their digits end the typed run, so
/// the longer one could never be told apart from the shorter one inside it.
pub fn parse_pins(text: &str) -> Result<Vec<Credential>, Error> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(pin), Some(name)) = (fields.next(), fields.next()) else {
            return Err(Error::Line(index + 1));
        };
        let clip = fields.next().unwrap_or("door_opened");
        if !(crate::pin::MIN_DIGITS..=crate::pin::MAX_DIGITS).contains(&pin.len())
            || !pin.bytes().all(|b| b.is_ascii_digit())
            || !topic_chars(name, false)
            || !topic_chars(clip, false)
            || fields.next().is_some()
        {
            return Err(Error::Line(index + 1));
        }
        if out
            .iter()
            .any(|c: &Credential| c.pin.contains(pin) || pin.contains(c.pin.as_str()))
        {
            return Err(Error::Line(index + 1));
        }
        out.push(Credential {
            pin: Zeroizing::new(pin.to_string()),
            name: name.to_string(),
            clip: clip.to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "mqtt_host=mqtt.example\nmqtt_user=u\nmqtt_password=p\nrtsp_user=admin\nrtsp_password=secret\nopen_seconds=7\nclip_db=-6\n";

    #[test]
    fn parses_overrides_and_keeps_defaults() {
        let c = Config::parse(GOOD).unwrap();
        assert_eq!(c.open_seconds, 7);
        assert_eq!(c.clip_db, -6);
        assert_eq!(c.mqtt_port, 1883);
        assert_eq!(c.mqtt_prefix, "ds-kd8003-ime1");
        assert_eq!(c.device_name, "DS-KD8003-IME1");
        assert_eq!(c.rtsp_port, 554);
        assert_eq!(c.http_port, 0, "diagnostics are off unless asked for");
        assert_eq!(c.health, PathBuf::from("/home/config/custom/native.ok"));
    }

    #[test]
    fn rejects_unknown_keys_bad_lines_and_ranges() {
        assert_eq!(
            Config::parse(&format!("{GOOD}bogus=1\n")),
            Err(Error::Unknown("bogus".into()))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}novalue\n")),
            Err(Error::Line(8))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}open_seconds=0\n")),
            Err(Error::Range("open_seconds"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}mqtt_port=70000\n")),
            Err(Error::Range("mqtt_port"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}mqtt_prefix=a b\n")),
            Err(Error::Range("mqtt_prefix"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}mqtt_node=x/y\n")),
            Err(Error::Range("mqtt_prefix"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}health=/x\n")),
            Err(Error::Unknown("health".into())),
            "the marker path is derived, never configured"
        );
        assert_eq!(
            Config::parse("rtsp_user=a\nrtsp_password=b\n"),
            Err(Error::Range("mqtt_host"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}device_name=a\"b\n")),
            Err(Error::Range("device_name"))
        );
        assert_eq!(
            Config::parse(&format!("{GOOD}device_name=Front door\n")).map(|c| c.device_name),
            Ok("Front door".into())
        );
        assert_eq!(
            Config::parse("mqtt_host=h\n"),
            Err(Error::Range("rtsp_user"))
        );
    }

    #[test]
    fn pin_table_validates_each_line() {
        let pins = parse_pins("# household\n1234 resident resident\n56789 guest\n").unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[1].clip, "door_opened");
        assert_eq!(parse_pins("123 short\n"), Err(Error::Line(1)));
        assert_eq!(parse_pins("12a4 letters\n"), Err(Error::Line(1)));
        assert_eq!(parse_pins("1234 bad/name\n"), Err(Error::Line(1)));
        assert_eq!(parse_pins("1234 a\n1234 b\n"), Err(Error::Line(2)));
        assert_eq!(
            parse_pins("1234 a\n912345 b\n"),
            Err(Error::Line(2)),
            "typing the longer code would open the shorter one"
        );
        assert_eq!(parse_pins("56781234 a\n1234 b\n"), Err(Error::Line(2)));
        assert!(
            parse_pins("1234 a\n2345 b\n").is_ok(),
            "overlap without containment is fine"
        );
        assert_eq!(parse_pins("1234\n"), Err(Error::Line(1)));
    }

    impl PartialEq for Credential {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
        }
    }
    impl std::fmt::Debug for Credential {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Credential({})", self.name)
        }
    }
}
