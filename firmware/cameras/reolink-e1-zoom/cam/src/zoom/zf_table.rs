//! The per-unit zoom→focus calibration (`ZFTable`) the factory writes into the `sp` partition (mtd7, only ever
//! read here), as the vendor `device` reads it (`Sp_get_zoomfocustable` @ 0x65b50) and turns it into focus-curve
//! rows (`Na_af_refresh_zf_info` @ 0xa5130). Layout: `docs/hardware/zoom-focus.md`.

use std::fmt;

use super::Row;

/// The read-only character device of the `sp` partition.
pub const SP_DEVICE: &str = "/dev/mtd7ro";

/// The lens variants whose factory behaviour is understood. The stock `device` treats variants 1 and 2 alike:
/// `Na_af_adapt_lens_focus_range` (0xa4658) sends both to the same return with the default windows, and the AF
/// parameter loader (0xa4e0c) takes `variant - 1 < 2` for the same parameter set. Variant 2 was seen on a unit
/// whose table starts `2000000,500dd`. Every other variant stays refused until its handling is read.
pub const SUPPORTED_LENS: [u32; 2] = [1, 2];

/// One calibrated point: at `zoom` steps the image is sharp at `focus` steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub zoom: u32,
    pub focus: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZfTable {
    /// Lens variant from the top byte of the first word (`& 3`), which picks the vendor's search spread.
    pub lens: u32,
    /// Focus backlash in steps (bits 16..23 of the second word), handed to the driver by `Na_af_set_backlash`.
    pub backlash: u32,
    /// Points in increasing zoom order.
    pub points: Vec<Point>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("no <ZFTable> element in the sp partition")]
    Missing,
    #[error("ZFTable value {0:?} is not hexadecimal")]
    NotHex(String),
    #[error("ZFTable has {0} values; it needs an even count of at least 4")]
    BadLength(usize),
    #[error("ZFTable point {0} is outside motor travel")]
    OutOfRange(usize),
    #[error("ZFTable zoom positions are not increasing at point {0}")]
    NotIncreasing(usize),
    #[error("unsupported factory lens calibration")]
    Unsupported,
}

impl ZfTable {
    /// Parses the `<ZFTable>` element out of the `sp` partition contents (the XML may sit among binary bytes).
    pub fn parse(sp: &[u8]) -> Result<Self, Error> {
        let text = String::from_utf8_lossy(sp);
        let start = text.find("<ZFTable>").ok_or(Error::Missing)? + "<ZFTable>".len();
        let end = text[start..].find("</ZFTable>").ok_or(Error::Missing)? + start;
        let words = text[start..end]
            .split(',')
            .map(|s| {
                let s = s.trim();
                u32::from_str_radix(s, 16).map_err(|_| Error::NotHex(s.to_string()))
            })
            .collect::<Result<Vec<u32>, _>>()?;
        if words.len() < 4 || words.len() % 2 != 0 {
            return Err(Error::BadLength(words.len()));
        }
        let points: Vec<Point> = words
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| Point {
                zoom: p[0] & 0xffff,
                focus: p[1] & 0xffff,
            })
            .collect();
        if let Some(i) = points
            .iter()
            .position(|p| p.zoom > crate::zoom::ZOOM_MAX || p.focus > crate::zoom::FOCUS_MAX)
        {
            return Err(Error::OutOfRange(i));
        }
        if let Some(i) = (1..points.len()).find(|&i| points[i].zoom <= points[i - 1].zoom) {
            return Err(Error::NotIncreasing(i));
        }
        Ok(Self {
            lens: (words[0] >> 24) & 3,
            backlash: (words[1] >> 16) & 0xff,
            points,
        })
    }

    /// The driver's focus-curve rows as the vendor installs them: one row per calibrated point, the sharp focus
    /// −144/+100 at zoom 0 and ±64 elsewhere, clamped to travel. Refuses a table this code does not understand.
    pub fn factory_rows(&self) -> Result<Vec<Row>, Error> {
        self.rows_with(|i, _| if i == 0 { (144, 100) } else { (64, 64) })
    }

    /// Rows whose window around each sharp point is `below(i, zoom)` steps down and `above` steps up.
    pub fn rows_with(
        &self,
        allowance: impl Fn(usize, u32) -> (u32, u32),
    ) -> Result<Vec<Row>, Error> {
        if !SUPPORTED_LENS.contains(&self.lens)
            || self.points.len() < 2
            || self.points.len() >= 50
            || self.backlash > 255
            || self.points[0].zoom != 0
            || self
                .points
                .iter()
                .any(|p| p.zoom > super::ZOOM_MAX || p.focus > super::FOCUS_MAX)
            || self.points.windows(2).any(|p| p[0].zoom >= p[1].zoom)
        {
            return Err(Error::Unsupported);
        }
        Ok(self
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (below, above) = allowance(i, p.zoom);
                Row {
                    zoom: p.zoom,
                    focus_min: p.focus.saturating_sub(below),
                    focus_max: p.focus.saturating_add(above).min(super::FOCUS_MAX),
                }
            })
            .collect())
    }

    /// The calibrated sharp focus at `zoom`: linear between the two neighbouring points, clamped to the ends.
    pub fn focus_at(&self, zoom: u32) -> u32 {
        let pts = &self.points;
        let first = pts[0];
        let last = pts[pts.len() - 1];
        if zoom <= first.zoom {
            return first.focus;
        }
        if zoom >= last.zoom {
            return last.focus;
        }
        let i = pts
            .iter()
            .position(|p| p.zoom >= zoom)
            .unwrap_or(pts.len() - 1);
        let (a, b) = (pts[i - 1], pts[i]);
        let (dz, t) = (i64::from(b.zoom - a.zoom), i64::from(zoom - a.zoom));
        let f = i64::from(a.focus) + (i64::from(b.focus) - i64::from(a.focus)) * t / dz;
        f as u32
    }
}

impl fmt::Display for ZfTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "zftable lens={} backlash={} points=",
            self.lens, self.backlash
        )?;
        for (i, p) in self.points.iter().enumerate() {
            write!(f, "{}{}:{}", if i == 0 { "" } else { "," }, p.zoom, p.focus)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera's own `sp` element (its body code replaced), wrapped in partition-like bytes.
    const SP: &[u8] = b"\xffCSPS<?xml version=\"1.0\"?>\n<StaticParameter>\n<BodyCode>000-00-0000-0000-0</BodyCode>\n\
<ZFTable>1000000,300e6,20,f3,40,125,60,15c,80,199,a0,1c4,c0,1fc,e0,229,100,254,120,27b,140,299,160,2ba,180,2dc,\
1a0,2fa,1c0,313,1e0,32e,200,345,220,35b,240,36f,260,37f,280,385,2a0,39f,2c0,3ad,2e0,3ba,300,3c6,320,3d1,340,3d9,\
360,3e1,380,3e9,3a0,3f0,3c0,3f5,3e0,3fa,400,3fe</ZFTable>\n<WifiSSID></WifiSSID>\n</StaticParameter>CEPS\x00\x00";

    #[test]
    fn parses_the_units_calibration() {
        let t = ZfTable::parse(SP).unwrap();
        assert_eq!((t.lens, t.backlash), (1, 3));
        assert_eq!(t.points.len(), 33);
        assert_eq!(
            t.points[0],
            Point {
                zoom: 0,
                focus: 230
            }
        );
        assert_eq!(
            t.points[1],
            Point {
                zoom: 32,
                focus: 243
            }
        );
        assert_eq!(
            t.points[32],
            Point {
                zoom: 1024,
                focus: 1022
            }
        );
    }

    /// Another unit's `sp` element, lens variant 2 and backlash 5 (its body code replaced).
    const SP_VARIANT_2: &[u8] = b"\xffCSPS<?xml version=\"1.0\"?>\n<StaticParameter>\n<BodyCode>000-00-0000-0000-0</BodyCode>\n\
<ZFTable>2000000,500dd,20,de,40,11f,60,15b,80,190,a0,1c4,c0,1ee,e0,21a,100,242,120,269,140,28c,160,2ae,180,2ce,\
1a0,2eb,1c0,305,1e0,321,200,339,220,351,240,363,260,377,280,387,2a0,396,2c0,3a2,2e0,3ae,300,3b9,320,3c5,340,3ce,\
360,3d6,380,3de,3a0,3e4,3c0,3ea,3e0,3ef,400,3f3</ZFTable>\n</StaticParameter>CEPS\x00\x00";

    #[test]
    fn lens_variant_2_gets_the_same_factory_windows_as_variant_1() {
        let t = ZfTable::parse(SP_VARIANT_2).unwrap();
        assert_eq!((t.lens, t.backlash), (2, 5));
        assert_eq!(t.points.len(), 33);
        let rows = t.factory_rows().unwrap();
        assert_eq!(rows.len(), 33);
        // sharp 221 at zoom 0: 221 - 144 ..= 221 + 100; every other row is sharp +- 64
        assert_eq!(
            (rows[0].zoom, rows[0].focus_min, rows[0].focus_max),
            (0, 77, 321)
        );
        assert_eq!(
            (rows[1].zoom, rows[1].focus_min, rows[1].focus_max),
            (32, 158, 286)
        );
        // the same table with the variant changed to 1 yields exactly the same rows
        let mut one = t.clone();
        one.lens = 1;
        assert_eq!(one.factory_rows().unwrap(), rows);
    }

    #[test]
    fn only_the_understood_lens_variants_are_accepted() {
        for lens in 0..4 {
            let mut t = ZfTable::parse(SP).unwrap();
            t.lens = lens;
            assert_eq!(
                t.factory_rows().is_ok(),
                SUPPORTED_LENS.contains(&lens),
                "lens variant {lens}"
            );
        }
    }

    #[test]
    fn focus_is_interpolated_between_points_and_clamped_past_the_ends() {
        let t = ZfTable::parse(SP).unwrap();
        assert_eq!(t.focus_at(0), 230);
        assert_eq!(t.focus_at(64), 293);
        // halfway between (32, 243) and (64, 293)
        assert_eq!(t.focus_at(48), 268);
        // past the last calibrated point (1024) up to the driver's 1148 the curve stays at the tele end
        assert_eq!(t.focus_at(1148), 1022);
    }

    #[test]
    fn rejects_broken_tables() {
        assert_eq!(ZfTable::parse(b"<Uid>1</Uid>"), Err(Error::Missing));
        assert_eq!(
            ZfTable::parse(b"<ZFTable>1,2,3</ZFTable>"),
            Err(Error::BadLength(3))
        );
        assert_eq!(
            ZfTable::parse(b"<ZFTable>0,e6,2g,f3</ZFTable>"),
            Err(Error::NotHex("2g".into()))
        );
        assert_eq!(
            ZfTable::parse(b"<ZFTable>0,e6,40,f3,20,f0</ZFTable>"),
            Err(Error::NotIncreasing(2))
        );
    }
}
