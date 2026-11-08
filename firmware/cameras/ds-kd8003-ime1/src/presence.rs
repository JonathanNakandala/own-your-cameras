//! The microphone's presence boost: a peaking filter at 2.7 kHz (Q 0.9) that lifts the consonant band of the
//! narrowband voice before it is decimated to 8 kHz. The gate's FM2018 runs at 8 kHz and cuts everything above
//! about 3.7 kHz (measured 2026-11-08), so speech from the gate sounds dull; the band just below the cut is where
//! intelligibility lives. Integer arithmetic: the gate's ARM926 has no FPU.

/// Centre of the boost.
const CENTRE_HZ: f64 = 2700.0;
const Q: f64 = 0.9;
/// Coefficient scale and guard bits for the signal inside the filter (no limit cycle after silence).
const SHIFT: u32 = 24;
const GUARD: u32 = 8;

pub struct Presence {
    rate: u32,
    db: i32,
    c: [i64; 5],
    z: [i64; 4],
}

impl Presence {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            db: 0,
            c: [0; 5],
            z: [0; 4],
        }
    }

    /// The boost in dB (0 = off); a change keeps the filter history.
    pub fn set(&mut self, db: i32) {
        if db == self.db {
            return;
        }
        self.db = db;
        if db == 0 || f64::from(self.rate) <= 2.0 * CENTRE_HZ {
            return;
        }
        let a = 10f64.powf(f64::from(db) / 40.0);
        let w = 2.0 * std::f64::consts::PI * CENTRE_HZ / f64::from(self.rate);
        let alpha = w.sin() / (2.0 * Q);
        let cos = w.cos();
        let a0 = 1.0 + alpha / a;
        let q = |v: f64| (v / a0 * f64::from(1u32 << SHIFT)).round() as i64;
        self.c = [
            q(1.0 + alpha * a),
            q(-2.0 * cos),
            q(1.0 - alpha * a),
            q(-2.0 * cos),
            q(1.0 - alpha / a),
        ];
    }

    pub fn db(&self) -> i32 {
        self.db
    }

    /// Filters in place; samples beyond full scale saturate.
    pub fn process(&mut self, pcm: &mut [i16]) {
        if self.db == 0 || f64::from(self.rate) <= 2.0 * CENTRE_HZ {
            return;
        }
        let [b0, b1, b2, a1, a2] = self.c;
        for s in pcm.iter_mut() {
            let x = i64::from(*s) << GUARD;
            let [x1, x2, y1, y2] = self.z;
            let acc = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            let y = (acc + (1 << (SHIFT - 1))) >> SHIFT;
            self.z = [x, x1, y, y1];
            *s = ((y + (1 << (GUARD - 1))) >> GUARD).clamp(i64::from(i16::MIN), i64::from(i16::MAX))
                as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_db(db: i32, hz: f64) -> f64 {
        let rate = 16_000u32;
        let mut p = Presence::new(rate);
        p.set(db);
        let mut x: Vec<i16> = (0..rate as usize)
            .map(|i| {
                (8000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin())
                    as i16
            })
            .collect();
        let input = x.clone();
        p.process(&mut x);
        let rms = |v: &[i16]| {
            (v[8000..].iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / 8000.0).sqrt()
        };
        20.0 * (rms(&x) / rms(&input)).log10()
    }

    #[test]
    fn the_consonant_band_is_lifted_by_the_setting_and_the_voice_body_is_left_alone() {
        assert!(
            (gain_db(6, 2700.0) - 6.0).abs() < 0.3,
            "{}",
            gain_db(6, 2700.0)
        );
        assert!(
            (gain_db(3, 2700.0) - 3.0).abs() < 0.3,
            "{}",
            gain_db(3, 2700.0)
        );
        assert!(gain_db(6, 500.0).abs() < 0.6, "{}", gain_db(6, 500.0));
        assert!(gain_db(6, 200.0).abs() < 0.3, "{}", gain_db(6, 200.0));
    }

    #[test]
    fn zero_is_bit_exact() {
        let mut p = Presence::new(16_000);
        let mut x: Vec<i16> = (0..1000).map(|i| (i * 37 % 2000 - 1000) as i16).collect();
        let y = x.clone();
        p.process(&mut x);
        assert_eq!(x, y);
        p.set(4);
        p.set(0);
        p.process(&mut x);
        assert_eq!(x, y);
    }

    #[test]
    fn silence_after_a_loud_burst_settles_to_zero() {
        let mut p = Presence::new(16_000);
        p.set(6);
        let mut loud: Vec<i16> = (0..4000)
            .map(|i| if (i / 3) % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect();
        p.process(&mut loud);
        let mut quiet = vec![0i16; 4000];
        p.process(&mut quiet);
        assert!(quiet[3000..].iter().all(|&v| v == 0));
    }
}
