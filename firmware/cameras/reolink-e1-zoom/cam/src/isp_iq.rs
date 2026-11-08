//! ISP picture controls: the image-quality and exposure items the stock `device` (or the kernel UI
//! table) offers, each with its pinned payload (`docs/hardware/isp-iq.md`, `docs/hdal-abi.md`).
//! Applied on the hardware thread through the one `/dev/nvt_isp` owner, always with readback: the
//! kernel's set path logs a rejected value and still returns 0.

use crate::settings::{Group, Setting, Spec};
use crate::zoom::{Error, Isp, WORDS};

/// Which `vendor_isp_*` family carries the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Iq,
    Ae,
}

/// How a stock `0_isp.cfg` value (0..=255 UI scale) becomes the item's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FromVendor {
    /// The stock per-sensor curve then `·200/255` (handlers @ 0xc5cfc and siblings, curve 0xc5378):
    /// `v = ui ≤ 127 ? ui·base/128 : base + (256−base)·(ui−128)/128`. `base` is this unit's table
    /// row, fitted to the stock ioctls captured on the camera (`docs/hardware/isp-iq.md`).
    Level200 { base: i64 },
    /// UI 128 = 0°: `t = v > 127 ? v − 128 : v + 128`, then `t * 360 / 255` (@ 0xc5e84).
    Hue,
    /// `antiflick`: 1 → 60 Hz (1), anything else → 50 Hz (0) (@ 0xc6680).
    Flicker,
}

/// One established item: `{isp id, value}`, 8 bytes in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// Settings key (`isp_<name>`).
    pub key: &'static str,
    pub name: &'static str,
    pub unit: Unit,
    pub item: u32,
    /// The kernel's accepted range (its UI table, `max` exclusive there).
    pub range: (i64, i64),
    /// Option names when the value is an enumeration (index = value).
    pub options: Option<&'static [&'static str]>,
    pub default: i64,
    /// `0_isp.cfg` attribute that seeds the first-run value.
    pub stock: Option<(&'static str, FromVendor)>,
}

const fn level(
    key: &'static str,
    name: &'static str,
    item: u32,
    stock: Option<(&'static str, FromVendor)>,
) -> Item {
    Item {
        key,
        name,
        unit: Unit::Iq,
        item,
        range: (0, 200),
        options: None,
        default: 100,
        stock,
    }
}

const fn curve(base: i64) -> FromVendor {
    FromVendor::Level200 { base }
}

/// The items established on the camera (set, readback and a measured frame effect, `docs/hardware/isp-iq.md`).
pub const ITEMS: &[Item] = &[
    level(
        "isp_brightness",
        "Brightness",
        14,
        Some(("brightness", curve(116))),
    ),
    level(
        "isp_contrast",
        "Contrast",
        13,
        Some(("contrast", curve(129))),
    ),
    level(
        "isp_saturation",
        "Saturation",
        12,
        Some(("saturation", curve(128))),
    ),
    level(
        "isp_sharpness",
        "Sharpness",
        11,
        Some(("sharpen", curve(77))),
    ),
    Item {
        range: (0, 360),
        default: 0,
        ..level("isp_hue", "Hue", 20, Some(("hue", FromVendor::Hue)))
    },
    level("isp_noise_reduction", "Noise reduction (2D)", 10, None),
    Item {
        unit: Unit::Ae,
        range: (0, 32),
        default: 16,
        ..level("isp_exposure", "Exposure compensation", 14, None)
    },
    Item {
        unit: Unit::Ae,
        range: (0, 1),
        options: Some(&["50 Hz", "60 Hz"]),
        default: 0,
        ..level(
            "isp_anti_flicker",
            "Anti-flicker",
            12,
            Some(("antiflick", FromVendor::Flicker)),
        )
    },
];

impl Item {
    /// The setting of this item; slots are assigned by `settings::specs`.
    pub fn setting(&self) -> Setting {
        let (min, max) = (self.range.0 as i32, self.range.1 as i32);
        let spec = match self.options {
            Some(o) => Spec::select(0, self.key, self.name, o, self.default as i32),
            None => Spec::number(
                0,
                self.key,
                self.name,
                (min, max),
                None,
                self.default as i32,
            ),
        };
        Setting {
            stock: self.stock.map(|(attr, _)| attr),
            ..Setting::new(spec, Group::Picture)
        }
    }

    fn convert(&self, raw: i64) -> i64 {
        match self.stock.map(|(_, f)| f) {
            Some(FromVendor::Level200 { base }) => {
                let ui = raw.clamp(0, 255);
                let v = if ui <= 127 {
                    ui * base / 128
                } else {
                    base + (256 - base) * (ui - 128) / 128
                };
                v.clamp(0, 255) * 200 / 255
            }
            Some(FromVendor::Hue) => {
                let raw = raw.clamp(0, 255);
                let t = if raw > 127 { raw - 128 } else { raw + 128 };
                t * 360 / 255
            }
            Some(FromVendor::Flicker) => i64::from(raw == 1),
            None => raw,
        }
    }
}

/// Maps a stock `0_isp.cfg` value of `key` to the item's own scale (identity for other keys).
pub fn from_vendor(key: &str, raw: i64) -> i64 {
    ITEMS
        .iter()
        .find(|i| i.key == key)
        .map_or(raw, |i| i.convert(raw))
}

/// Sets `key` to `value` and reads it back; a mismatch is an error (the kernel ignores values it
/// rejects) and the caller keeps the old value.
pub fn apply(isp: &mut impl Isp, key: &str, value: i64) -> Result<i64, Error> {
    let item = ITEMS
        .iter()
        .find(|i| i.key == key)
        .ok_or_else(|| Error::Ioctl(format!("unknown picture control {key}")))?;
    if !(item.range.0..=item.range.1).contains(&value) {
        return Err(Error::Ioctl(format!("{key}: {value} out of range")));
    }
    let mut words = Box::new([0u32; WORDS]);
    words[1] = value as u32;
    let mut back = Box::new([0u32; WORDS]);
    match item.unit {
        Unit::Iq => {
            isp.set_iq(item.item, &words)?;
            isp.get_iq(item.item, &mut back)?;
        }
        Unit::Ae => {
            isp.set_ae(item.item, &words)?;
            isp.get_ae(item.item, &mut back)?;
        }
    }
    if back[1] != words[1] {
        return Err(Error::Ioctl(format!("{key}: readback {}", back[1])));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom::fake::FakeIsp;

    #[test]
    fn a_set_writes_the_isp_id_and_level_and_is_read_back() {
        let mut isp = FakeIsp::new(0, 0);
        assert_eq!(apply(&mut isp, "isp_brightness", 150), Ok(150));
        assert_eq!(isp.iq[&14][..2], [0, 150]);
        assert_eq!(apply(&mut isp, "isp_anti_flicker", 1), Ok(1));
        assert_eq!(isp.iq[&1012][..2], [0, 1], "AE item 12");
    }

    #[test]
    fn out_of_range_is_rejected_before_any_ioctl() {
        let mut isp = FakeIsp::new(0, 0);
        assert!(apply(&mut isp, "isp_contrast", 201).is_err());
        assert!(apply(&mut isp, "isp_hue", -1).is_err());
        assert!(apply(&mut isp, "nope", 1).is_err());
        assert!(isp.iq.is_empty());
    }

    #[test]
    fn a_readback_mismatch_is_an_error() {
        let mut isp = FakeIsp::new(0, 0);
        isp.iq_readback.insert(13, vec![0, 100]);
        assert!(apply(&mut isp, "isp_contrast", 180).is_err());
    }

    #[test]
    fn stock_ui_values_map_like_the_captured_vendor_ioctls() {
        // (UI value, level the stock `device` wrote) from an ioctl capture.
        for (key, ui, level) in [
            ("isp_brightness", 200, 152),
            ("isp_brightness", 30, 21),
            ("isp_brightness", 128, 90),
            ("isp_contrast", 200, 156),
            ("isp_contrast", 30, 23),
            ("isp_contrast", 128, 101),
            ("isp_saturation", 200, 156),
            ("isp_saturation", 128, 100),
            ("isp_sharpness", 128, 60),
        ] {
            assert_eq!(from_vendor(key, ui), level, "{key} {ui}");
        }
        assert_eq!(from_vendor("isp_hue", 128), 0);
        assert_eq!(from_vendor("isp_hue", 0), 180);
        assert_eq!(from_vendor("isp_anti_flicker", 3), 0);
        assert_eq!(from_vendor("isp_anti_flicker", 1), 1);
        assert_eq!(from_vendor("mic_volume", 77), 77);
    }

    #[test]
    fn the_camera_stock_file_seeds_what_stock_applies() {
        let stock = crate::settings::StockDefaults::from_vendor(
            r#"<isp_config_t><channel brightness="128" saturation="128" contrast="128" hue="128" sharpen="128" antiflick="3" mirror="0" flip="0" /></isp_config_t>"#,
        );
        let s = crate::settings::Settings::defaults(&stock);
        for (k, v) in [
            ("isp_brightness", 90),
            ("isp_contrast", 101),
            ("isp_saturation", 100),
            ("isp_sharpness", 60),
        ] {
            assert_eq!(s.picture(k), Some(v), "{k}");
        }
        assert_eq!(s.picture("isp_hue"), Some(0));
        assert_eq!(s.get("isp_anti_flicker"), Some(0));
    }
}
