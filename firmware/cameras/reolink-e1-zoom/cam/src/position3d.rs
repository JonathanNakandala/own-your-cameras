//! Hikvision-style 3D positioning: a point or box on the picture becomes a pan/tilt/zoom target.
//!
//! The payload is what `master-camera-card` sends to its `position3d_service`
//! (`{start_x, start_y, end_x, end_y, operation}`): Hikvision's 0..=255 frame, x from the left, **y from
//! the bottom**. `start == end` centres that point and keeps the zoom; a box is centred and zoomed so it
//! fills the frame (`zoomto`) or so the current frame shrinks into it (`zoomout`).
//!
//! The E1 Zoom lens is far from a pinhole (strong barrel distortion at wide), so the map is measured
//! directly in motor steps: a feature `x` frame widths right of the centre is `a x + b x³` pan steps
//! away, with `(a, b)` per lens zoom position. Tilt uses the same map scaled by `tilt_ratio`, on the
//! vertical offset (also in frame widths). Measurement: `docs/hardware/position3d.md`.
use crate::{orientation::Orientation, ptz::Position};

pub use camera_control::position3d::{Operation, Request, SPAN};
/// Reolink streams preserve the full 4:3 sensor image.
const ASPECT: f64 = 0.75;

/// Where a request sends the camera. `zoom` is `None` for a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub position: Position,
    pub zoom: Option<u32>,
}

/// The lens-and-head map of one camera model, as measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Optics {
    /// `(zoom, a, b)`: pan steps from the centre to a feature `x` frame widths off it are `a x + b x³`.
    /// Zoom ascending; `a` must fall as zoom rises (zooming in narrows the view).
    pub rows: &'static [(u32, f64, f64)],
    /// Tilt steps per pan step for the same offset in the picture.
    pub tilt_ratio: f64,
    /// Under stock orientation (mirror and flip off), a point right of centre needs pan to grow.
    pub pan_right_grows: bool,
    /// Under stock orientation, a point above centre needs tilt to grow.
    pub tilt_up_grows: bool,
}

impl Optics {
    /// Reolink E1 Zoom, measured on real frames (`docs/hardware/position3d.md`).
    pub const E1_ZOOM: Self = Self {
        rows: &[
            (0, 1960.7, 553.4),
            (256, 1803.0, 385.2),
            (512, 1658.4, 217.7),
            (768, 1547.6, 127.3),
            (1024, 1462.6, 149.9),
        ],
        tilt_ratio: 0.91,
        pan_right_grows: true,
        tilt_up_grows: true,
    };

    /// `(a, b)` at `zoom`, linear between measured rows, clamped at the ends.
    pub fn coefficients(&self, zoom: u32) -> (f64, f64) {
        let rows = self.rows;
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return (0.0, 0.0);
        };
        if zoom <= first.0 {
            return (first.1, first.2);
        }
        for w in rows.windows(2) {
            let ((za, aa, ba), (zb, ab, bb)) = (w[0], w[1]);
            if zoom <= zb {
                let t = f64::from(zoom - za) / f64::from(zb - za);
                return (aa + t * (ab - aa), ba + t * (bb - ba));
            }
        }
        (last.1, last.2)
    }

    /// Pan steps from the centre to a feature `x` frame widths right of it, at `zoom`.
    pub fn steps(&self, x: f64, zoom: u32) -> f64 {
        let (a, b) = self.coefficients(zoom);
        a * x + b * x * x * x
    }

    /// The zoom whose central scale `a` is `scale`, clamped to the measured range.
    pub fn zoom_for(&self, scale: f64) -> u32 {
        let rows = self.rows;
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return 0;
        };
        if scale >= first.1 {
            return first.0;
        }
        for w in rows.windows(2) {
            let ((za, aa, _), (zb, ab, _)) = (w[0], w[1]);
            if scale >= ab {
                let t = (aa - scale) / (aa - ab);
                return za + (t * f64::from(zb - za)).round() as u32;
            }
        }
        last.0
    }

    /// The position `dx` frame widths right and `dy` frame widths up of `at`, under the picture's
    /// `orientation`, clamped to `travel`. The picture is the displayed one: mirror and flip reverse an axis.
    pub fn shift(
        &self,
        dx: f64,
        dy: f64,
        at: Position,
        zoom: u32,
        orientation: Orientation,
        travel: (u32, u32),
    ) -> Position {
        let sign = |grows: bool, reversed: bool| if grows ^ reversed { 1.0 } else { -1.0 };
        let pan = sign(self.pan_right_grows, orientation.mirror()) * self.steps(dx, zoom);
        let tilt =
            sign(self.tilt_up_grows, orientation.flip()) * self.tilt_ratio * self.steps(dy, zoom);
        let shift = |from: u32, by: f64, max: u32| {
            (f64::from(from) + by).round().clamp(0.0, f64::from(max)) as u32
        };
        Position {
            pan: shift(at.pan, pan, travel.0),
            tilt: shift(at.tilt, tilt, travel.1),
        }
    }

    /// [`Optics::shift`] for a move of `x` picture widths and `y` picture **heights** (ONVIF's field-of-view
    /// space: 1.0 is one whole picture), positive right and up.
    pub fn shift_fov(
        &self,
        x: f64,
        y: f64,
        at: Position,
        zoom: u32,
        orientation: Orientation,
        travel: (u32, u32),
    ) -> Position {
        self.shift(x, y * ASPECT, at, zoom, orientation, travel)
    }

    /// The target for `req` from the confirmed `at` position and lens `zoom`, under the picture's
    /// `orientation`, clamped to `travel` (pan max, tilt max) and `zoom_max`.
    pub fn target(
        &self,
        req: &Request,
        at: Position,
        zoom: u32,
        orientation: Orientation,
        travel: (u32, u32),
        zoom_max: u32,
    ) -> Target {
        let unit = |v: u8| f64::from(v) / f64::from(SPAN);
        let (cx, cy) = (
            (unit(req.start.0) + unit(req.end.0)) / 2.0,
            (unit(req.start.1) + unit(req.end.1)) / 2.0,
        );
        // Offsets from the centre in frame widths; y counts from the bottom, so +dy is up.
        let (dx, dy) = (cx - 0.5, (cy - 0.5) * ASPECT);
        let position = self.shift(dx, dy, at, zoom, orientation, travel);
        let zoom = (!req.is_point()).then(|| {
            let w = (unit(req.end.0) - unit(req.start.0)).abs();
            let h = (unit(req.end.1) - unit(req.start.1)).abs();
            // The box's larger share of the frame decides; a sliver counts as one coordinate.
            let share = w.max(h).max(1.0 / f64::from(SPAN));
            let (a, _) = self.coefficients(zoom);
            let wanted = match req.operation {
                Operation::ZoomTo => a * share,
                Operation::ZoomOut => a / share,
            };
            self.zoom_for(wanted).min(zoom_max)
        });
        Target { position, zoom }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round numbers: a pinhole-free map with a = 2000 at zoom 0 falling to 1000 at 1024.
    const OPTICS: Optics = Optics {
        rows: &[
            (0, 2000.0, 400.0),
            (512, 1500.0, 200.0),
            (1024, 1000.0, 0.0),
        ],
        tilt_ratio: 0.5,
        pan_right_grows: true,
        tilt_up_grows: true,
    };
    const STOCK: Orientation = Orientation::new(false, false);
    const AT: Position = Position {
        pan: 3000,
        tilt: 600,
    };

    fn req(start: (u8, u8), end: (u8, u8), operation: Operation) -> Request {
        Request {
            start,
            end,
            operation,
        }
    }
    fn point(x: u8, y: u8) -> Request {
        req((x, y), (x, y), Operation::ZoomTo)
    }
    fn go(r: Request, zoom: u32, o: Orientation) -> Target {
        OPTICS.target(&r, AT, zoom, o, (7360, 1240), 1024)
    }

    #[test]
    fn parses_the_cards_payload() {
        let r = Request::parse(
            r#"{"start_x":66,"start_y":189,"end_x":66,"end_y":189,"operation":"zoomto"}"#,
        )
        .unwrap();
        assert_eq!(r, point(66, 189));
        assert!(r.is_point());
        let out = Request::parse(
            r#"{"start_x":0,"start_y":0,"end_x":255,"end_y":255,"operation":"zoomout"}"#,
        )
        .unwrap();
        assert_eq!(out.operation, Operation::ZoomOut);
        assert_eq!(
            Request::parse(r#"{"start_x":1,"start_y":2,"end_x":3,"end_y":4}"#)
                .unwrap()
                .operation,
            Operation::ZoomTo
        );
    }

    #[test]
    fn refuses_anything_else() {
        for bad in [
            "",
            "128,128",
            r#"{"start_x":256,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":-1,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":1.5,"start_y":0,"end_x":0,"end_y":0}"#,
            r#"{"start_x":1,"start_y":0,"end_x":0}"#,
            r#"{"start_x":1,"start_y":0,"end_x":0,"end_y":0,"operation":"spin"}"#,
            r#"{"start_x":1,"start_y":0,"end_x":0,"end_y":0,"speed":3}"#,
        ] {
            assert!(Request::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_centre_is_no_move() {
        // 127.5 is the exact centre; 128 is half a coordinate right and up.
        let t = go(point(128, 128), 0, STOCK);
        assert!(t.position.pan.abs_diff(AT.pan) <= 4 && t.position.tilt.abs_diff(AT.tilt) <= 2);
        assert_eq!(t.zoom, None);
    }

    #[test]
    fn a_point_turns_by_the_measured_map_at_the_current_zoom() {
        // Right edge at zoom 0: 2000·0.5 + 400·0.125 = 1050 steps.
        assert_eq!(go(point(255, 128), 0, STOCK).position.pan, AT.pan + 1050);
        assert_eq!(go(point(0, 128), 0, STOCK).position.pan, AT.pan - 1050);
        // At zoom 1024 the same edge is 1000·0.5 = 500 steps.
        assert_eq!(go(point(255, 128), 1024, STOCK).position.pan, AT.pan + 500);
        // Rows interpolate: zoom 256 is a = 1750, b = 300 → 875 + 37.5.
        assert_eq!(go(point(255, 128), 256, STOCK).position.pan, AT.pan + 913);
        // Top edge: 0.375 width up → (2000·0.375 + 400·0.0527) · 0.5 = 385.5 tilt steps.
        assert_eq!(go(point(127, 255), 0, STOCK).position.tilt, AT.tilt + 386);
        assert_eq!(go(point(127, 0), 0, STOCK).position.tilt, AT.tilt - 386);
    }

    #[test]
    fn the_cards_tap_reaches_the_measured_map() {
        // The card's own test tap: 103 of 400 px wide, 52 of 200 px down → (66, 189).
        let t = Optics::E1_ZOOM.target(&point(66, 189), AT, 0, STOCK, (7360, 1240), 1024);
        assert!(
            t.position.pan < AT.pan,
            "left of centre turns toward lower pan"
        );
        assert!(t.position.tilt > AT.tilt, "above centre tilts up");
    }

    #[test]
    fn mirror_and_flip_reverse_their_axis() {
        let corner = point(255, 255);
        let stock = go(corner, 0, STOCK).position;
        let mirrored = go(corner, 0, Orientation::new(true, false)).position;
        let flipped = go(corner, 0, Orientation::new(false, true)).position;
        assert_eq!(mirrored.pan, AT.pan - (stock.pan - AT.pan));
        assert_eq!(mirrored.tilt, stock.tilt);
        assert_eq!(flipped.tilt, AT.tilt - (stock.tilt - AT.tilt));
        assert_eq!(flipped.pan, stock.pan);
    }

    #[test]
    fn targets_clamp_to_travel() {
        let edge = Position {
            pan: 10,
            tilt: 1230,
        };
        let t = OPTICS.target(&point(0, 255), edge, 0, STOCK, (7360, 1240), 1024);
        assert_eq!(t.position, Position { pan: 0, tilt: 1240 });
    }

    #[test]
    fn a_field_of_view_move_is_the_same_map_in_picture_units() {
        let go = |x, y, zoom, o| OPTICS.shift_fov(x, y, AT, zoom, o, (7360, 1240));
        // Half a picture right at zoom 0 is the right-edge point: 1050 steps.
        assert_eq!(go(0.5, 0.0, 0, STOCK).pan, AT.pan + 1050);
        // A whole picture height up is 0.75 widths: (2000·0.75 + 400·0.75³) · 0.5 tilt steps.
        let low = Position { pan: 3000, tilt: 0 };
        let up = OPTICS.shift_fov(0.0, 1.0, low, 0, STOCK, (7360, 1240));
        assert_eq!(
            up.tilt,
            ((2000.0 * 0.75 + 400.0 * 0.421875) * 0.5_f64).round() as u32
        );
        // Narrower at zoom 1024, and zero is no move.
        assert_eq!(go(0.5, 0.0, 1024, STOCK).pan, AT.pan + 500);
        assert_eq!(go(0.0, 0.0, 0, STOCK), AT);
        // Mirror reverses pan only, flip tilt only; travel clamps.
        assert_eq!(
            go(0.5, 0.0, 0, Orientation::new(true, false)).pan,
            AT.pan - 1050
        );
        assert_eq!(go(0.0, 0.5, 0, Orientation::new(false, true)).pan, AT.pan);
        assert!(go(0.0, 0.5, 0, Orientation::new(false, true)).tilt < AT.tilt);
        let edge = OPTICS.shift_fov(
            1.0,
            1.0,
            Position {
                pan: 7300,
                tilt: 1230,
            },
            0,
            STOCK,
            (7360, 1240),
        );
        assert_eq!(
            edge,
            Position {
                pan: 7360,
                tilt: 1240
            }
        );
    }

    #[test]
    fn a_box_is_centred_and_zoomed_to_fill_the_frame() {
        // A half-width box at zoom 0 (a = 2000) wants a = 996, past the last row → zoom 1024;
        // its centre is centred.
        let half = req((128, 64), (255, 191), Operation::ZoomTo);
        let t = go(half, 0, STOCK);
        assert_eq!(t.zoom, Some(1024));
        assert!(t.position.pan > AT.pan);
        // A three-quarter box (0.749) at zoom 0 wants a = 1498 → just past zoom 512.
        let most = req((32, 32), (223, 223), Operation::ZoomTo);
        assert_eq!(go(most, 0, STOCK).zoom, Some(514));
        // The taller share decides: full height → no change.
        let tall = req((120, 0), (130, 255), Operation::ZoomTo);
        assert_eq!(go(tall, 512, STOCK).zoom, Some(512));
    }

    #[test]
    fn zoomout_shrinks_the_frame_into_the_box_and_zoom_is_clamped() {
        let most = req((32, 32), (223, 223), Operation::ZoomOut);
        // At zoom 1024 (a = 1000) a 0.749 box wants a = 1335 → between rows 512 and 1024.
        assert_eq!(go(most, 1024, STOCK).zoom, Some(681));
        assert_eq!(go(most, 0, STOCK).zoom, Some(0));
        let tiny = req((127, 127), (128, 128), Operation::ZoomTo);
        assert_eq!(go(tiny, 512, STOCK).zoom, Some(1024));
        assert_eq!(
            OPTICS.target(&tiny, AT, 512, STOCK, (7360, 1240), 800).zoom,
            Some(800)
        );
    }

    /// `scripts/position3d/table.py` drives the on-camera acceptance; it must use this exact table.
    #[test]
    fn the_acceptance_script_uses_the_same_table() {
        let py = include_str!("../../scripts/position3d/table.py");
        let o = Optics::E1_ZOOM;
        let rows: Vec<String> = o
            .rows
            .iter()
            .map(|(z, a, b)| format!("({z}, {a:.1}, {b:.1})"))
            .collect();
        assert!(
            py.contains(&format!("TABLE = [{}]", rows.join(", "))),
            "table.py TABLE differs from Optics::E1_ZOOM"
        );
        assert!(py.contains(&format!("TILT_RATIO = {}", o.tilt_ratio)));
    }
}
