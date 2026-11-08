//! A talker's high-pass filter, ahead of its gain control: two second-order Butterworth sections
//! (Q 0.707, 12 dB per octave each, -6 dB at the corner) in integer arithmetic, since the speaker
//! thread may run on an ARM core without an FPU.
//!
//! Small horn speakers cannot move air below a few hundred Hz. The energy of a voice's fundamental
//! (100–250 Hz) only drives them into distortion, and the harmonics that distortion adds are not in the
//! echo canceller's reference, so they come back through the microphone as echo. Removing that band
//! before the gain control also lets the gain lift the band the speaker can reproduce.

/// Coefficient scale: Q24 keeps the poles (close to the unit circle at a low corner) accurate.
const SHIFT: u32 = 24;
/// Extra bits of the signal inside the filter: rounding there, not at the sample's own LSB, leaves no
/// limit cycle (a tick that never dies out) after the talker goes quiet.
const GUARD: u32 = 8;

#[derive(Clone, Copy)]
struct Section {
    b0: i64,
    b1: i64,
    b2: i64,
    a1: i64,
    a2: i64,
    x1: i64,
    x2: i64,
    y1: i64,
    y2: i64,
}

impl Section {
    fn new(rate: u32, hz: u32) -> Self {
        // RBJ cookbook high-pass, Q = 1/sqrt(2).
        let w = 2.0 * std::f64::consts::PI * f64::from(hz) / f64::from(rate);
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let q = |v: f64| (v / a0 * f64::from(1u32 << SHIFT)).round() as i64;
        Self {
            b0: q((1.0 + cos) / 2.0),
            b1: q(-(1.0 + cos)),
            b2: q((1.0 + cos) / 2.0),
            a1: q(-2.0 * cos),
            a2: q(1.0 - alpha),
            x1: 0,
            x2: 0,
            y1: 0,
            y2: 0,
        }
    }

    fn step(&mut self, x: i64) -> i64 {
        let acc = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        let y = (acc + (1 << (SHIFT - 1))) >> SHIFT;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

pub struct HighPass {
    sections: [Section; 2],
}

impl HighPass {
    /// `None` for a corner of 0 (no filter) or one at or above a quarter of the rate.
    pub fn new(rate: u32, hz: u32) -> Option<Self> {
        if hz == 0 || hz >= rate / 4 {
            return None;
        }
        let s = Section::new(rate, hz);
        Some(Self { sections: [s, s] })
    }

    pub fn process(&mut self, pcm: &[i16]) -> Vec<i16> {
        pcm.iter()
            .map(|&v| {
                let mut y = i64::from(v) << GUARD;
                for s in &mut self.sections {
                    y = s.step(y);
                }
                ((y + (1 << (GUARD - 1))) >> GUARD).clamp(i64::from(i16::MIN), i64::from(i16::MAX))
                    as i16
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, n: usize, amp: f64) -> Vec<i16> {
        (0..n)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin()) as i16
            })
            .collect()
    }

    fn rms(v: &[i16]) -> f64 {
        (v.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / v.len() as f64).sqrt()
    }

    /// Gain in dB of a steady sine after the filter has settled.
    fn gain_db(hz: f64) -> f64 {
        let input = sine(16_000, hz, 16_000, 10_000.0);
        let out = HighPass::new(16_000, 600).unwrap().process(&input);
        20.0 * (rms(&out[8_000..]) / rms(&input[8_000..])).log10()
    }

    #[test]
    fn a_voice_fundamental_is_cut_and_the_speech_band_passes() {
        assert!(gain_db(150.0) < -40.0, "150 Hz: {}", gain_db(150.0));
        assert!(gain_db(300.0) < -20.0, "300 Hz: {}", gain_db(300.0));
        assert!(
            (gain_db(600.0) + 6.0).abs() < 1.0,
            "600 Hz: {}",
            gain_db(600.0)
        );
        assert!(gain_db(1500.0).abs() < 1.0, "1500 Hz: {}", gain_db(1500.0));
        assert!(gain_db(3000.0).abs() < 0.5, "3000 Hz: {}", gain_db(3000.0));
    }

    #[test]
    fn full_scale_square_input_stays_in_range_and_settles() {
        let mut f = HighPass::new(16_000, 600).unwrap();
        let square: Vec<i16> = (0..16_000)
            .map(|i| {
                if (i / 40) % 2 == 0 {
                    i16::MAX
                } else {
                    i16::MIN
                }
            })
            .collect();
        let _ = f.process(&square);
        let quiet = f.process(&vec![0; 4_000]);
        assert!(
            quiet[3_000..].iter().all(|&v| v == 0),
            "rings after silence"
        );
    }

    #[test]
    fn no_corner_or_one_too_high_means_no_filter() {
        assert!(HighPass::new(16_000, 0).is_none());
        assert!(HighPass::new(8_000, 2_000).is_none());
        assert!(HighPass::new(8_000, 600).is_some());
    }
}
