//! Mi 360's setting catalog and its hardware mapping. Values in force and their persistence are
//! `camera_transport::settings::Values` (sparse, so only hardware-confirmed values are saved);
//! probing, readback and rollback are `setting_service`.
use camera_transport::settings::Spec;

pub const SPEED: u16 = 7;
pub const START_SPEED: u16 = 27;
pub const RAMP_UP: u16 = 28;
pub const RAMP_DOWN: u16 = 29;
pub const STOCK_HV: i32 = 3;
/// The settings document, `{"version":1,"values":{…}}` (`JsonCodec`).
pub const PATH: &str = "/mnt/data/mi360/settings.json";

const fn n(
    slot: u16,
    key: &'static str,
    name: &'static str,
    range: (i32, i32),
    default: i32,
) -> Spec {
    Spec::number(slot, key, name, range, None, default)
}
const fn u(
    slot: u16,
    key: &'static str,
    name: &'static str,
    range: (i32, i32),
    unit: &'static str,
    default: i32,
) -> Spec {
    Spec::number(slot, key, name, range, Some(unit), default)
}

/// The only list of Mi 360's setting keys. Defaults are placeholders for the read API (which does
/// not say whether a slot is set): nothing is applied or saved that the hardware did not report or
/// confirm.
pub const SPECS: &[Spec] = &[
    Spec::switch(0, "mirror", "Mirror", false),
    Spec::switch(1, "flip", "Flip", false),
    n(2, "brightness", "Brightness", (0, 255), 128),
    n(3, "contrast", "Contrast", (0, 255), 128),
    n(4, "saturation", "Saturation", (0, 255), 128),
    n(5, "sharpness", "Sharpness", (0, 255), 128),
    Spec::select(
        6,
        "anti_flicker",
        "Anti-flicker",
        &["off", "50hz", "60hz"],
        0,
    ),
    u(SPEED, "ptz_speed", "PTZ speed", (100, 900), "steps/s", 200),
    n(8, "mic_volume", "Microphone level", (-30, 120), 0),
    n(9, "speaker_volume", "Speaker level", (-30, 120), 0),
    u(10, "main_fps_target", "Main target FPS", (1, 20), "fps", 20),
    u(11, "sub_fps_target", "Sub target FPS", (1, 20), "fps", 20),
    u(
        12,
        "main_bitrate_target",
        "Main target bitrate",
        (32, 800),
        "kbit/s",
        800,
    ),
    u(
        13,
        "sub_bitrate_target",
        "Sub target bitrate",
        (32, 240),
        "kbit/s",
        240,
    ),
    u(14, "main_gop", "Main GOP duration", (1, 15), "s", 1),
    u(15, "sub_gop", "Sub GOP duration", (1, 15), "s", 1),
    n(
        16,
        "main_idr_multiplier",
        "Main keyframe refresh multiplier",
        (1, 3),
        1,
    ),
    n(
        17,
        "sub_idr_multiplier",
        "Sub keyframe refresh multiplier",
        (1, 3),
        1,
    ),
    n(18, "hue", "Hue", (0, 255), 128),
    n(
        19,
        "ae_compensation",
        "Exposure compensation",
        (0, 255),
        128,
    ),
    n(
        20,
        "highlight_suppression",
        "Highlight suppression",
        (0, 10),
        0,
    ),
    n(
        21,
        "defect_pixel_correction",
        "Defect pixel correction strength",
        (0, 255),
        128,
    ),
    n(
        22,
        "dynamic_range_strength",
        "Dynamic range correction strength",
        (0, 255),
        128,
    ),
    Spec::select(
        23,
        "white_balance",
        "White balance",
        &[
            "auto",
            "daylight",
            "cloudy",
            "incandescent",
            "fluorescent",
            "twilight",
            "shade",
            "warm_fluorescent",
        ],
        0,
    ),
    Spec::switch(
        24,
        "dynamic_range_correction",
        "Dynamic range correction",
        false,
    ),
    Spec::switch(
        25,
        "spatial_noise_reduction",
        "Spatial noise reduction",
        false,
    ),
    Spec::switch(
        26,
        "temporal_luma_noise_reduction",
        "Temporal luma noise reduction",
        false,
    ),
    u(
        START_SPEED,
        "ptz_start_speed",
        "PTZ start speed",
        (100, 900),
        "steps/s",
        100,
    ),
    u(
        RAMP_UP,
        "ptz_ramp_up",
        "PTZ acceleration time",
        (0, 3000),
        "ms",
        250,
    ),
    u(
        RAMP_DOWN,
        "ptz_ramp_down",
        "PTZ deceleration time",
        (0, 3000),
        "ms",
        150,
    ),
];

pub fn spec(slot: u16) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.slot == slot)
}

/// The SDK control behind a slot; `None` for a software setting held by the runtime.
pub const fn hardware(slot: u16) -> Option<u32> {
    match slot {
        0 | 1 => Some(0),
        2..=6 => Some(slot as u32 - 1),
        8 | 9 => Some(slot as u32 + 2),
        10..=17 => Some(slot as u32 + 10),
        18..=26 => Some(slot as u32 + 12),
        _ => None,
    }
}

/// The slot behind an SDK control (mirror for the shared orientation control).
pub fn slot_of(control: u32) -> Option<u16> {
    SPECS
        .iter()
        .map(|s| s.slot)
        .find(|s| hardware(*s) == Some(control))
}

/// Only present when the runtime has audio.
pub const fn audio(slot: u16) -> bool {
    matches!(slot, 8 | 9)
}

/// The logical value of a hardware reading.
pub fn decode(slot: u16, hardware: i32) -> i32 {
    match slot {
        0 | 1 => ((hardware ^ STOCK_HV) >> slot) & 1,
        23 => match hardware {
            0 => 0,
            2..=8 => hardware - 1,
            _ => -1,
        },
        _ => hardware,
    }
}

/// The hardware value for `value`; `current` keeps the other orientation axis.
pub fn encode(slot: u16, value: i32, current: i32) -> i32 {
    match slot {
        0 | 1 => {
            let bit = 1 << slot;
            let logical = current ^ STOCK_HV;
            ((logical & !bit) | (value << slot)) ^ STOCK_HV
        }
        23 => {
            if value == 0 {
                0
            } else {
                value + 1
            }
        }
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::settings::{Disk, JsonCodec, Values, check_catalog};

    #[test]
    fn the_catalog_is_valid_and_keeps_the_sdk_control_of_every_slot() {
        check_catalog(SPECS).unwrap();
        let controls: Vec<_> = SPECS.iter().map(|s| (s.slot, hardware(s.slot))).collect();
        let expected: Vec<(u16, Option<u32>)> = vec![
            (0, Some(0)),
            (1, Some(0)),
            (2, Some(1)),
            (3, Some(2)),
            (4, Some(3)),
            (5, Some(4)),
            (6, Some(5)),
            (7, None),
            (8, Some(10)),
            (9, Some(11)),
            (10, Some(20)),
            (11, Some(21)),
            (12, Some(22)),
            (13, Some(23)),
            (14, Some(24)),
            (15, Some(25)),
            (16, Some(26)),
            (17, Some(27)),
            (18, Some(30)),
            (19, Some(31)),
            (20, Some(32)),
            (21, Some(33)),
            (22, Some(34)),
            (23, Some(35)),
            (24, Some(36)),
            (25, Some(37)),
            (26, Some(38)),
            (27, None),
            (28, None),
            (29, None),
        ];
        assert_eq!(controls, expected);
        assert_eq!(slot_of(24), Some(14));
        assert_eq!(slot_of(0), Some(0));
    }
    #[test]
    fn logical_orientation_matches_stock_and_preserves_the_other_axis() {
        for raw in 0..4 {
            for axis in 0..2 {
                for value in 0..2 {
                    let updated = encode(axis, value, raw);
                    assert_eq!(decode(axis, updated), value);
                    let other = 1 - axis;
                    assert_eq!(decode(other, updated), decode(other, raw));
                }
            }
        }
        assert_eq!(decode(0, 3), 0);
        assert_eq!(decode(1, 3), 0);
    }

    struct Memory(Option<Vec<u8>>);
    impl Disk for Memory {
        fn read(&mut self) -> std::io::Result<Option<Vec<u8>>> {
            Ok(self.0.clone())
        }
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.0 = Some(bytes.to_vec());
            Ok(())
        }
    }

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/settings-2026-11-07/"
    );

    #[test]
    fn existing_settings_files_keep_loading_and_round_trip_byte_for_byte() {
        let text = std::fs::read(format!("{FIXTURES}settings.json")).unwrap();
        let (values, problems) = Values::load(SPECS, Memory(Some(text.clone())), JsonCodec);
        assert!(problems.is_empty(), "{problems:?}");
        let mut values = values.sparse();
        let set: Vec<_> = SPECS
            .iter()
            .filter(|s| values.is_set(s.slot))
            .map(|s| s.key)
            .collect();
        assert_eq!(
            set,
            [
                "mirror",
                "flip",
                "brightness",
                "white_balance",
                "ptz_ramp_up"
            ]
        );
        assert_eq!(values.value("white_balance"), Some(2));
        values.put(2, 130, std::time::Duration::ZERO).unwrap();
        values.put(2, 129, std::time::Duration::ZERO).unwrap();
        values.flush().unwrap();
        assert_eq!(values.disk().0.as_deref(), Some(&text[..]));
    }
    #[test]
    fn a_partly_invalid_file_keeps_its_valid_fields_and_names_the_others() {
        let text = std::fs::read(format!("{FIXTURES}settings-invalid-fields.json")).unwrap();
        let (values, problems) = Values::load(SPECS, Memory(Some(text)), JsonCodec);
        let values = values.sparse();
        let named: Vec<_> = problems.iter().filter_map(|p| p.key.as_deref()).collect();
        assert_eq!(named, ["brightness", "main_fps_target"]);
        assert!(values.is_set(3) && !values.is_set(2) && !values.is_set(10));
    }
}
