//! The gate's live settings: the one list of its keys ([`SPECS`]), where they are stored, and the
//! one-time seed from an old `cam.conf`. Parsing, validation, discovery, the file and the docs table
//! derive from [`SPECS`]; [`crate::media`] applies the values.
use camera_transport::settings::{
    Codec, Disk, Kind, Reader, Spec, StreamKeys, StreamParams, Values,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The settings file, next to `cam.conf` on the config partition (the installer never replaces it).
pub const FILE: &str = "settings.json";

pub fn path(config_file: &Path) -> PathBuf {
    config_file.parent().unwrap_or(Path::new(".")).join(FILE)
}

/// Slots of the ISP keys are this plus the vendor key number, so a slot names its key.
pub const ISP_SLOT: u16 = 1000;

pub const RATE_CONTROL: &[&str] = &["CBR", "VBR"];
/// H.264 profiles in the bridge's numbering (0 Baseline, 1 Main, 2 High).
pub const PROFILES: &[&str] = &["baseline", "main", "high"];
/// Main stream sizes tested without a VI/ISP rebuild. 1920x1080 is VPSS channel 0; the others are
/// scaled on VPSS channel 2.
pub const MAIN_SIZES: &[&str] = &["1920x1080", "1280x720", "640x360"];
/// The only size the sub stream offers (VPSS channel 1).
pub const SUB_SIZE: (u32, u32) = (704, 480);
/// The AWB modes the vendor firmware labels (its mode table), as select options; the vendor number of
/// each is the same position in [`AWB_NUMBERS`]. Unlabelled modes (2, 9-13, 17) are not offered.
pub const AWB_MODES: [&str; 11] = [
    "manual",
    "auto",
    "locked",
    "outdoor",
    "indoor",
    "fluorescent",
    "sodium",
    "auto-track",
    "incandescent",
    "warm",
    "natural",
];
/// ISP key 82's value of each [`AWB_MODES`] option.
pub const AWB_NUMBERS: [u32; 11] = [0, 1, 3, 4, 5, 6, 7, 8, 14, 15, 16];
/// ISP key 82: white balance mode.
pub const ISP_AWB_MODE: u32 = 82;

const fn boxed(
    slot: u16,
    key: &'static str,
    name: &'static str,
    (min, max): (i32, i32),
    unit: Option<&'static str>,
    default: i32,
) -> Spec {
    let mut s = Spec::number(slot, key, name, (min, max), unit, default);
    s.kind = Kind::Number {
        min,
        max,
        unit,
        slider: false,
    };
    s
}

/// One ISP key as a number on its vendor range.
const fn isp(
    key: u16,
    id: &'static str,
    name: &'static str,
    range: (i32, i32),
    default: i32,
) -> Spec {
    Spec::number(ISP_SLOT + key, id, name, range, None, default).icon("mdi:camera-iris")
}

const fn isp_box(
    key: u16,
    id: &'static str,
    name: &'static str,
    range: (i32, i32),
    unit: Option<&'static str>,
    default: i32,
) -> Spec {
    boxed(ISP_SLOT + key, id, name, range, unit, default).icon("mdi:camera-iris")
}

const fn isp_switch(key: u16, id: &'static str, name: &'static str, on: bool) -> Spec {
    Spec::switch(ISP_SLOT + key, id, name, on).icon("mdi:camera-iris")
}

const fn isp_select(
    key: u16,
    id: &'static str,
    name: &'static str,
    options: &'static [&'static str],
    default: i32,
) -> Spec {
    Spec::select(ISP_SLOT + key, id, name, options, default).icon("mdi:camera-iris")
}

pub const OPEN_SECONDS: u16 = 1;
pub const CODEC_OUT_DB: u16 = 2;
pub const CLIP_DB: u16 = 3;
pub const TALK_DB: u16 = 4;
pub const MIC_GAIN_DB: u16 = 5;
/// The intercom call's settings: read where they are used (the call and the speaker), stored here
/// so they work without Home Assistant.
pub const CALL_RING_TIMEOUT: u16 = 6;
pub const CALL_RINGBACK_DB: u16 = 7;
pub const CALL_PANEL_RING_VOLUME: u16 = 8;
pub const CALL_MAX_TALK: u16 = 9;

/// Every live setting of the gate.
#[rustfmt::skip]
pub const SPECS: &[Spec] = &[
    Spec::number(OPEN_SECONDS, "open_seconds", "Door open time", (1, 120), Some("s"), 5).icon("mdi:timer-outline"),
    // The acodec's own ranges (SET_OUTPUT_VOL / SET_INPUT_VOL).
    Spec::number(CODEC_OUT_DB, "codec_out_db", "Speaker level", (-30, 6), Some("dB"), 0).icon("mdi:volume-high"),
    Spec::number(CLIP_DB, "clip_db", "Announcement gain", (-40, 12), Some("dB"), 0).icon("mdi:bullhorn"),
    Spec::number(TALK_DB, "talk_db", "Talk gain", (0, 18), Some("dB"), 6).icon("mdi:account-voice"),
    Spec::number(MIC_GAIN_DB, "mic_gain_db", "Microphone gain", (-5, 40), Some("dB"), 0).icon("mdi:microphone"),
    Spec::number(CALL_RING_TIMEOUT, "call_ring_timeout", "Call ring timeout", (15, 120), Some("s"), 45).icon("mdi:phone-ring"),
    Spec::number(CALL_RINGBACK_DB, "call_ringback_db", "Ringback level", (-40, 12), Some("dB"), 0).icon("mdi:phone-in-talk"),
    Spec::number(CALL_PANEL_RING_VOLUME, "call_panel_ring_volume", "Panel ring volume", (0, 100), Some("%"), 80).icon("mdi:bell-ring"),
    Spec::number(CALL_MAX_TALK, "call_max_talk", "Max talk time", (1, 10), Some("min"), 3).icon("mdi:timer-sand"),
    // Encoder ranges: the vendor's offer; the maxima of both streams together ran without a frame
    // shortfall on the panel (docs/home-assistant.md).
    Spec::select(20, "main_resolution", "Main resolution", MAIN_SIZES, 0).icon("mdi:video"),
    Spec::number(21, "main_fps", "Main frame rate", (1, 30), Some("fps"), 30).icon("mdi:video"),
    Spec::number(22, "main_kbps", "Main bitrate", (512, 4096), Some("kbit/s"), 4096).icon("mdi:video"),
    boxed(23, "main_gop", "Main keyframe interval", (1, 400), Some("frames"), 30).icon("mdi:video"),
    Spec::select(24, "main_rate_control", "Main rate control", RATE_CONTROL, 1).icon("mdi:video"),
    Spec::select(25, "main_profile", "Main profile", PROFILES, 0).icon("mdi:video"),
    Spec::number(31, "sub_fps", "Sub frame rate", (1, 30), Some("fps"), 15).icon("mdi:video-outline"),
    Spec::number(32, "sub_kbps", "Sub bitrate", (128, 2048), Some("kbit/s"), 1024).icon("mdi:video-outline"),
    boxed(33, "sub_gop", "Sub keyframe interval", (1, 400), Some("frames"), 30).icon("mdi:video-outline"),
    Spec::select(34, "sub_rate_control", "Sub rate control", RATE_CONTROL, 1).icon("mdi:video-outline"),
    Spec::select(35, "sub_profile", "Sub profile", PROFILES, 1).icon("mdi:video-outline"),
    // ISP keys (`ISP_SetKeyParam`), slot = ISP_SLOT + key, vendor ranges and defaults. Only the keys
    // seen to change the picture on the panel (docs/home-assistant.md, "Image settings").
    isp_switch(32, "isp_auto_exposure", "Auto exposure", true),
    isp(33, "isp_brightness", "Brightness", (0, 100), 50),
    isp_box(36, "isp_shutter_slowest", "Slowest shutter", (10, 40000), Some("µs"), 40000),
    isp(39, "isp_gain_limit", "Gain limit", (0, 100), 100),
    isp_box(40, "isp_manual_shutter", "Manual shutter", (10, 40000), Some("µs"), 40000),
    isp_select(82, "isp_awb_mode", "White balance mode", &AWB_MODES, 1),
    isp(84, "isp_saturation", "Saturation", (0, 100), 50),
    isp(144, "isp_contrast", "Contrast", (0, 100), 50),
    isp(145, "isp_sharpness", "Sharpness", (0, 100), 50),
    isp(148, "isp_nr_2d", "2D noise reduction", (0, 100), 50),
    isp(149, "isp_nr_3d", "3D noise reduction", (0, 100), 50),
    isp_switch(153, "isp_hlc", "Highlight compensation", false),
];

/// A setting's Home Assistant entity; the ring timeout moves in 5 s steps.
pub fn entity(spec: &Spec) -> camera_transport::mqtt::Entity {
    let mut e = spec.entity();
    if spec.slot == CALL_RING_TIMEOUT
        && let camera_transport::mqtt::Domain::Number { step, .. } = &mut e.domain
    {
        *step = 5;
    }
    e
}

/// ISP keys applied at every start with their stored value, never seeded from the ISP: AWB mode 1
/// is what keeps the image from turning magenta; saturation may read 0 from a monochrome night.
pub const ISP_FORCED: &[u32] = &[82, 84];
/// Saturation: monochrome at night is applied over it.
pub const ISP_SATURATION: u32 = 84;

/// The vendor key of an ISP slot.
pub fn isp_key(slot: u16) -> Option<u32> {
    slot.checked_sub(ISP_SLOT).map(u32::from)
}

/// Setting value to the key's raw value: the AWB mode's vendor number, every other key as is.
pub fn isp_raw(key: u32, value: i32) -> u32 {
    if key == ISP_AWB_MODE {
        let index = usize::try_from(value).unwrap_or(usize::MAX);
        return AWB_NUMBERS.get(index).copied().unwrap_or(1);
    }
    value as u32
}

/// The key's raw value to a setting value; an AWB mode without a label reads as none.
pub fn isp_value(key: u32, raw: u32) -> Option<i32> {
    if key == ISP_AWB_MODE {
        return AWB_NUMBERS
            .iter()
            .position(|&n| n == raw)
            .and_then(|i| i32::try_from(i).ok());
    }
    i32::try_from(raw).ok()
}

/// What a setting drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Read where it is used (the strike pulse).
    Door,
    /// acodec output (true) or input level.
    Codec {
        output: bool,
    },
    /// Clip and talk gains: the speaker thread.
    Speaker,
    /// An encoder channel: 0 main, 1 sub.
    Stream(usize),
    Isp(u32),
}

pub fn target(slot: u16) -> Option<Target> {
    Some(match slot {
        OPEN_SECONDS | CALL_RING_TIMEOUT..=CALL_MAX_TALK => Target::Door,
        CODEC_OUT_DB => Target::Codec { output: true },
        MIC_GAIN_DB => Target::Codec { output: false },
        CLIP_DB | TALK_DB => Target::Speaker,
        20..=29 => Target::Stream(0),
        30..=39 => Target::Stream(1),
        s => Target::Isp(isp_key(s)?),
    })
}

/// The setting keys of each stream, for [`Reader::stream`].
pub const STREAM_KEYS: [StreamKeys; 2] = [
    StreamKeys {
        resolution: Some("main_resolution"),
        fps: Some("main_fps"),
        kbps: Some("main_kbps"),
        gop: Some("main_gop"),
        profile: Some("main_profile"),
    },
    StreamKeys {
        resolution: None,
        fps: Some("sub_fps"),
        kbps: Some("sub_kbps"),
        gop: Some("sub_gop"),
        profile: Some("sub_profile"),
    },
];

/// Current parameters of stream `chn` (0 main, 1 sub) for another owner (ONVIF); the sub stream's
/// fixed size is filled in.
pub fn stream_params(reader: &Reader<'_>, chn: usize) -> StreamParams {
    let mut p = reader.stream(&STREAM_KEYS[chn.min(1)]);
    if chn == 1 {
        p.width = Some(SUB_SIZE.0);
        p.height = Some(SUB_SIZE.1);
    }
    p
}

/// `cam.conf` keys that became live settings. An old file's values seed the settings once.
pub const DEPRECATED: &[&str] = &[
    "open_seconds",
    "codec_out_db",
    "clip_db",
    "talk_db",
    "mic_gain_db",
];

/// Seeds the values from an old `cam.conf`'s removed keys, only when no settings file existed yet.
/// Returns one log line per key: seeded, ignored (settings already exist) or invalid (default kept).
pub fn seed<D: Disk, C: Codec>(
    values: &mut Values<'_, D, C>,
    legacy: &[(String, String)],
    now: Duration,
) -> Vec<String> {
    let first = values.first_run();
    legacy
        .iter()
        .map(|(key, text)| {
            let head = format!("cam.conf: {key} is deprecated, it is a live setting now");
            if !first {
                return format!("{head}; ignored, the settings file wins");
            }
            let Some(spec) = values.spec_by_key(key) else {
                return format!("{head}; unknown, ignored");
            };
            match spec.parse(text).and_then(|v| values.put(spec.slot, v, now)) {
                Ok(_) => format!(
                    "{head}; seeded {}",
                    spec.render(values.get(spec.slot).unwrap_or(spec.default))
                ),
                Err(e) => format!("{head}; {e}, default {} kept", spec.render(spec.default)),
            }
        })
        .collect()
}

/// The settings table of `docs/home-assistant.md`, derived from [`SPECS`] (a test keeps the doc
/// equal to it).
pub fn doc_table() -> String {
    let mut out = String::from("| Entity id | Name | Values | Default |\n|---|---|---|---|\n");
    for s in SPECS {
        let values = match s.kind {
            Kind::Number { min, max, unit, .. } => {
                format!(
                    "{min}..{max}{}",
                    unit.map(|u| format!(" {u}")).unwrap_or_default()
                )
            }
            Kind::Switch => "`ON`/`OFF`".into(),
            Kind::Select(options) => options
                .iter()
                .map(|o| format!("`{o}`"))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let note = match target(s.slot) {
            Some(Target::Isp(key)) => format!(" (ISP key {key})"),
            _ => String::new(),
        };
        out += &format!(
            "| `{}` | {}{note} | {values} | {} |\n",
            s.key,
            s.name,
            s.render(s.default)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::settings::{JsonCodec, check_catalog};
    use std::io;

    #[derive(Default)]
    pub(crate) struct Mem(pub Option<Vec<u8>>);
    impl Disk for Mem {
        fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(self.0.clone())
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.0 = Some(bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn the_spec_list_is_valid_and_every_slot_has_a_target() {
        check_catalog(SPECS).unwrap();
        for s in SPECS {
            assert!(target(s.slot).is_some(), "{} has no target", s.key);
            if let Some(Target::Isp(key)) = target(s.slot) {
                assert!(s.key.starts_with("isp_"), "{}", s.key);
                assert!(key < 1000);
            }
        }
        assert!(
            SPECS.len() + 8 + 12 <= 96,
            "settings, door entities and sensors fit the entity limit"
        );
    }

    #[test]
    fn the_removed_cam_conf_keys_keep_their_ranges_and_defaults() {
        let want = [
            ("open_seconds", 1, 120, 5),
            ("codec_out_db", -30, 6, 0),
            ("clip_db", -40, 12, 0),
            ("talk_db", 0, 18, 6),
            ("mic_gain_db", -5, 40, 0),
        ];
        for (key, lo, hi, default) in want {
            let s = SPECS.iter().find(|s| s.key == key).unwrap();
            assert!(
                matches!(s.kind, Kind::Number { min, max, .. } if min == lo && max == hi),
                "{key}"
            );
            assert_eq!(s.default, default, "{key}");
            assert!(DEPRECATED.contains(&key));
        }
    }

    #[test]
    fn isp_slots_name_their_keys() {
        assert_eq!(target(1082), Some(Target::Isp(82)));
        assert_eq!(isp_raw(84, 30), 30);
        assert_eq!(isp_value(84, 30), Some(30));
        let awb = SPECS.iter().find(|s| s.key == "isp_awb_mode").unwrap();
        assert_eq!(awb.default, 1, "the magenta fix is the default");
        assert_eq!(AWB_MODES.len(), AWB_NUMBERS.len());
        assert!(AWB_MODES.iter().all(|m| !m.starts_with("mode ")));
        let n = |label: &str| AWB_MODES.iter().position(|m| *m == label).unwrap() as i32;
        assert_eq!(isp_raw(82, n("auto")), 1);
        assert_eq!(isp_raw(82, n("manual")), 0);
        assert_eq!(isp_raw(82, n("incandescent")), 14);
        assert_eq!(isp_raw(82, n("natural")), 16);
        assert_eq!(isp_value(82, 15), Some(n("warm")));
        assert_eq!(isp_value(82, 2), None, "unlabelled");
        assert_eq!(isp_value(82, 17), None);
        for i in 0..AWB_MODES.len() as i32 {
            assert_eq!(isp_value(82, isp_raw(82, i)), Some(i));
        }
    }

    #[test]
    fn the_docs_table_is_the_spec_list() {
        let doc = include_str!("../docs/home-assistant.md");
        let (begin, end) = ("<!-- settings:begin -->\n", "<!-- settings:end -->");
        let start = doc.find(begin).expect("begin marker") + begin.len();
        let stop = doc.find(end).expect("end marker");
        assert!(
            doc[start..stop] == doc_table(),
            "docs/home-assistant.md is out of date; its table is:\n{}",
            doc_table()
        );
    }

    #[test]
    fn the_settings_file_sits_next_to_cam_conf() {
        assert_eq!(
            path(Path::new("/home/config/custom/cam.conf")),
            PathBuf::from("/home/config/custom/settings.json")
        );
    }

    fn legacy(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_old_cam_conf_seeds_only_the_first_run_and_bad_values_keep_defaults() {
        let (mut v, problems) = Values::load(SPECS, Mem::default(), JsonCodec);
        assert!(problems.is_empty() && v.first_run());
        let log = seed(
            &mut v,
            &legacy(&[
                ("open_seconds", "9"),
                ("talk_db", "99"),
                ("clip_db", "loud"),
                ("mic_gain_db", "20"),
            ]),
            Duration::ZERO,
        );
        assert_eq!(v.value("open_seconds"), Some(9));
        assert_eq!(v.value("talk_db"), Some(6), "out of range: default");
        assert_eq!(v.value("clip_db"), Some(0), "not a number: default");
        assert_eq!(v.value("mic_gain_db"), Some(20));
        assert!(log.iter().all(|l| l.contains("deprecated")));
        assert!(log[1].contains("out of range") && log[2].contains("not a number"));
        v.flush().unwrap();

        // Settings exist now: a later boot with the same cam.conf changes nothing.
        let disk = Mem(v.disk().0.clone());
        let (mut again, _) = Values::load(SPECS, disk, JsonCodec);
        assert!(!again.first_run());
        let log = seed(
            &mut again,
            &legacy(&[("open_seconds", "30")]),
            Duration::ZERO,
        );
        assert_eq!(again.value("open_seconds"), Some(9));
        assert!(log[0].contains("ignored"));
        assert!(!again.dirty());
    }

    #[test]
    fn the_sub_stream_reports_its_fixed_size() {
        let (v, _) = Values::load(SPECS, Mem::default(), JsonCodec);
        let main = stream_params(&v.reader(), 0);
        assert_eq!(
            (main.width, main.height, main.fps, main.kbps),
            (Some(1920), Some(1080), Some(30), Some(4096))
        );
        assert_eq!(main.profile.as_deref(), Some("baseline"));
        let sub = stream_params(&v.reader(), 1);
        assert_eq!(
            (sub.width, sub.height, sub.fps, sub.gop),
            (Some(704), Some(480), Some(15), Some(30))
        );
    }
}
