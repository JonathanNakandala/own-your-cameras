//! Integer-ratio resampling to and from 8 kHz telephone audio, plus G.711 µ-law decoding.
//!
//! One rule, one owner: every camera decimates its microphone to 8 kHz here, and every PCMU talker is
//! interpolated to the speaker rate here. The filters are Hamming-windowed sinc low-passes cut at
//! 3.6 kHz, run in Q15 fixed point so a camera without an FPU pays only integer multiply-accumulates.
//! Pure: no clock reads, no I/O.

/// The telephone rate PCMU runs at.
pub const PCMU_RATE: u32 = 8000;
/// Low-pass corner (−6 dB). Measured response: −3.8 dB at 3.4 kHz, −13 dB at 4 kHz, −25 dB at 4.4 kHz,
/// ≥ 50 dB from 4.8 kHz; the 4.0–4.8 kHz band folds attenuated into 3.2–4.0 kHz, which a doorway or
/// room microphone barely carries.
const CUTOFF_HZ: f64 = 3600.0;
/// A block this far from where the previous one ended is a capture gap: filter history restarts.
const MAX_SKEW_US: u64 = 40_000;
const Q: u32 = 15;

/// Q15 low-pass taps for a filter running at `rate`, with DC gain `gain`.
fn lowpass(rate: u32, gain: f64) -> Vec<i32> {
    // Hamming: transition width ≈ 3.3 · rate / taps. 2.4 kHz keeps 5 kHz ≥ 30 dB down.
    let n = ((3.3 * f64::from(rate) / 2400.0).ceil() as usize) | 1;
    let m = (n - 1) as f64 / 2.0;
    let fc = CUTOFF_HZ / f64::from(rate);
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m;
            let sinc = if x == 0.0 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let window =
                0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
            sinc * window
        })
        .collect();
    let sum: f64 = raw.iter().sum();
    raw.iter()
        .map(|t| (t / sum * gain * f64::from(1u32 << Q)).round() as i32)
        .collect()
}

fn saturate(acc: i64) -> i16 {
    ((acc + (1 << (Q - 1))) >> Q).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

/// Streaming `rate` → 8 kHz decimation that keeps its filter history across capture blocks.
pub struct Decimator {
    rate: u32,
    factor: usize,
    taps: Vec<i32>,
    pending: Vec<i16>,
    /// Time of the first sample since the last restart, and input samples consumed since.
    origin_us: u64,
    consumed: u64,
}

impl Decimator {
    /// `None` unless `rate` is a whole multiple of 8 kHz.
    pub fn new(rate: u32) -> Option<Self> {
        if rate == 0 || !rate.is_multiple_of(PCMU_RATE) || rate > 192_000 {
            return None;
        }
        let factor = (rate / PCMU_RATE) as usize;
        let taps = if factor == 1 {
            vec![1 << Q]
        } else {
            lowpass(rate, 1.0)
        };
        Some(Self {
            rate,
            factor,
            taps,
            pending: Vec::new(),
            origin_us: 0,
            consumed: 0,
        })
    }

    fn time_of(&self, input_index: u64) -> u64 {
        self.origin_us + input_index * 1_000_000 / u64::from(self.rate)
    }

    /// Forgets the history, so the next block starts a new run.
    pub fn reset(&mut self) {
        self.pending.clear();
    }

    /// Feeds one capture block stamped with its first sample's time. Returns the 8 kHz samples now
    /// complete, stamped with the time of the first one (the filter centre's input time).
    pub fn push(&mut self, pts_us: u64, samples: &[i16]) -> Option<(u64, Vec<i16>)> {
        let expected = self.time_of(self.consumed + self.pending.len() as u64);
        if self.pending.is_empty() || expected.abs_diff(pts_us) > MAX_SKEW_US {
            self.pending.clear();
            self.origin_us = pts_us;
            self.consumed = 0;
        }
        self.pending.extend_from_slice(samples);
        let n = self.taps.len();
        let centre = (n as u64 - 1) / 2;
        let mut out = Vec::with_capacity(self.pending.len() / self.factor + 1);
        let mut at = 0;
        while at + n <= self.pending.len() {
            let acc: i64 = self.pending[at..at + n]
                .iter()
                .zip(&self.taps)
                .map(|(&x, &h)| i64::from(x) * i64::from(h))
                .sum();
            out.push(saturate(acc));
            at += self.factor;
        }
        if out.is_empty() {
            return None;
        }
        let first = self.time_of(self.consumed + centre);
        self.pending.drain(..at);
        self.consumed += at as u64;
        Some((first, out))
    }
}

/// Streaming 8 kHz → `rate` interpolation (zero-stuffing plus the low-pass, run polyphase).
pub struct Interpolator {
    factor: usize,
    /// `phases[p][j]` multiplies the input `j` samples back for output phase `p`.
    phases: Vec<Vec<i32>>,
    /// Newest input first.
    history: Vec<i16>,
}

impl Interpolator {
    /// `None` unless `rate` is a whole multiple of 8 kHz.
    pub fn new(rate: u32) -> Option<Self> {
        if rate == 0 || !rate.is_multiple_of(PCMU_RATE) || rate > 192_000 {
            return None;
        }
        let factor = (rate / PCMU_RATE) as usize;
        let phases = if factor == 1 {
            vec![vec![1 << Q]]
        } else {
            let taps = lowpass(rate, factor as f64);
            (0..factor)
                .map(|p| taps.iter().skip(p).step_by(factor).copied().collect())
                .collect()
        };
        let depth = phases.iter().map(Vec::len).max().unwrap_or(1);
        Some(Self {
            factor,
            phases,
            history: vec![0; depth],
        })
    }

    pub fn push(&mut self, samples: &[i16]) -> Vec<i16> {
        let mut out = Vec::with_capacity(samples.len() * self.factor);
        for &x in samples {
            self.history.rotate_right(1);
            self.history[0] = x;
            for phase in &self.phases {
                let acc: i64 = phase
                    .iter()
                    .zip(&self.history)
                    .map(|(&h, &x)| i64::from(h) * i64::from(x))
                    .sum();
                out.push(saturate(acc));
            }
        }
        out
    }
}

/// G.711 µ-law to linear PCM16 (the inverse of `microphone::encode_pcm8k_mono`).
pub fn mulaw_to_linear(byte: u8) -> i16 {
    let u = !byte;
    let exponent = (u >> 4) & 7;
    let mantissa = i32::from(u & 0x0f);
    let magnitude = (((mantissa << 3) + 132) << exponent) - 132;
    (if u & 0x80 != 0 { -magnitude } else { magnitude }) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<i16> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .map(|i| {
                (10_000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin())
                    as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f64 {
        // Skip the filter's start-up.
        let s = &samples[samples.len() / 4..];
        (s.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    fn decimate(rate: u32, hz: f64) -> f64 {
        let input = sine(rate, hz, 0.5);
        let mut d = Decimator::new(rate).unwrap();
        let mut out = Vec::new();
        // Odd-sized blocks, as capture hardware delivers them.
        let block = 1000;
        for (i, chunk) in input.chunks(block).enumerate() {
            let pts = i as u64 * block as u64 * 1_000_000 / u64::from(rate);
            if let Some((_, s)) = d.push(pts, chunk) {
                out.extend(s);
            }
        }
        let expected = input.len() / (rate / PCMU_RATE) as usize;
        assert!(out.len() + 40 >= expected && out.len() <= expected);
        20.0 * (rms(&out) / rms(&input)).log10()
    }

    #[test]
    fn decimating_48k_and_16k_passes_1_khz_and_attenuates_5_khz_by_30_db() {
        for rate in [48_000, 16_000] {
            let pass = decimate(rate, 1000.0);
            assert!(pass.abs() < 0.5, "{rate}: 1 kHz at {pass:.2} dB");
            let stop = decimate(rate, 5000.0);
            assert!(stop < -30.0, "{rate}: 5 kHz at {stop:.2} dB");
        }
        let same = decimate(8000, 1000.0);
        assert!(same.abs() < 0.01, "8 kHz passes through");
    }

    #[test]
    fn decimated_blocks_are_contiguous_in_time_and_a_gap_restarts_them() {
        let mut d = Decimator::new(48_000).unwrap();
        let (t0, a) = d.push(1_000_000, &[0; 1024]).unwrap();
        let (t1, _) = d.push(1_021_333, &[0; 1024]).unwrap();
        // Each 8 kHz sample is 125 µs.
        assert!(t1.abs_diff(t0 + a.len() as u64 * 125) < 2);
        let (t2, _) = d.push(5_000_000, &[0; 1024]).unwrap();
        assert!((5_000_000..5_001_000).contains(&t2));
        assert!(Decimator::new(44_100).is_none());
    }

    #[test]
    fn interpolating_8k_keeps_a_1_khz_level() {
        for rate in [8000, 16_000, 48_000] {
            let mut i = Interpolator::new(rate).unwrap();
            let input = sine(PCMU_RATE, 1000.0, 0.5);
            let out: Vec<i16> = input.chunks(160).flat_map(|c| i.push(c)).collect();
            assert_eq!(out.len(), input.len() * (rate / PCMU_RATE) as usize);
            let gain = 20.0 * (rms(&out) / rms(&input)).log10();
            assert!(gain.abs() < 0.5, "{rate}: {gain:.2} dB");
        }
    }

    #[test]
    fn mulaw_round_trips_within_its_quantisation_step() {
        use crate::microphone::encode_pcm8k_mono;
        assert_eq!(mulaw_to_linear(0xff), 0);
        assert_eq!(mulaw_to_linear(0x80), 32124);
        assert_eq!(mulaw_to_linear(0x00), -32124);
        for x in (-32000i16..32000).step_by(97) {
            let y = mulaw_to_linear(encode_pcm8k_mono(&[x]).unwrap()[0]);
            let step = (i32::from(x).abs() / 16).max(8);
            assert!((i32::from(x) - i32::from(y)).abs() <= step, "{x} → {y}");
        }
        for byte in 0..=255u8 {
            let y = mulaw_to_linear(byte);
            assert_eq!(encode_pcm8k_mono(&[y]).unwrap()[0] & 0x7f, byte & 0x7f);
        }
    }
}
