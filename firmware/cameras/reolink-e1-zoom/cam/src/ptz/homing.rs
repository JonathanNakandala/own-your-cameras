//! End-stop detection by ear for homing (`docs/hardware/ptz.md`).
//!
//! The MS32006 reports nothing about the head, but the microphone sits in the same body as the motors: a
//! stepper driven into its stop loses sync and rattles, steadily louder than it runs (measured on one E1 Zoom:
//! quiet room −46 dBFS, running motor −41…−38, pan at its stop −27, tilt at its stop −33). The seam is
//! [`Listener`]: a stream of microphone levels. This module owns the decision rule; `Ptz::home` owns the
//! motion. Without a listener, or when nothing is heard, the axis simply runs its full travel, as it always did.

#[cfg(test)]
#[path = "homing_levels.rs"]
mod homing_levels;

/// Microphone levels, one per capture frame (about 21 ms at 48 kHz), in dBFS RMS. `heard` returns every
/// level captured since the previous call; an error means there is no audio at all.
pub trait Listener {
    fn heard(&mut self) -> Result<Vec<f32>, String>;
}

/// The stall is judged on the median of this many latest frames: 32 × 21 ms ≈ 0.7 s, so a knock or a
/// syllable (shorter than half of it) cannot pass, and the head grinds well under a second, not seconds.
pub const RECENT_FRAMES: usize = 32;
/// Rule A: the recent median this far above the quiet room is the stall. Tilt is the tight one: running
/// −39…−38 (frames up to −35), its stall −33; pan runs at −41 and stalls at −27.
pub const RISE_OVER_QUIET_DB: f32 = 11.0;
/// Rule B: the recent median this far above the running motor is the stall (tilt +6, pan +14). It also
/// covers a capture that reads loud right after start, when rule A cannot be trusted.
pub const RISE_OVER_MOTOR_DB: f32 = 4.0;
/// A "quiet" room louder than this is not quiet (seen once at a cold boot: −29 dBFS); rule A is then off.
pub const QUIET_MAX_DBFS: f32 = -40.0;
/// Frames after the start of the drive that make up the running-motor level: the first 15 (0.3 s) are the
/// kick itself, the next 32 (0.7 s) the motor running.
const MOTOR_FRAMES: std::ops::Range<usize> = 15..47;
/// Quiet frames needed before homing starts; fewer means the microphone is not delivering.
pub const MIN_QUIET_FRAMES: usize = 5;

/// RMS level of a PCM frame in dBFS; silence is −100.
pub fn level_dbfs(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s) / 32768.0;
            v * v
        })
        .sum();
    let rms = (sum / samples.len() as f64).sqrt();
    (20.0 * rms.max(1e-5).log10()) as f32
}

fn median(v: impl Iterator<Item = f32>) -> Option<f32> {
    let mut sorted: Vec<f32> = v.collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(sorted[sorted.len() / 2])
}

/// Hears the stall of one axis: created on the quiet head, fed every level from the first kick on.
#[derive(Debug, Clone, PartialEq)]
pub struct Detector {
    /// Median of the quiet frames, whatever it was.
    pub quiet_dbfs: f32,
    motor: Vec<f32>,
    recent: std::collections::VecDeque<f32>,
    seen: usize,
}

impl Detector {
    /// `None` when there are too few quiet frames to trust the microphone.
    pub fn listening_to(quiet: &[f32]) -> Option<Self> {
        if quiet.len() < MIN_QUIET_FRAMES {
            return None;
        }
        Some(Self {
            quiet_dbfs: median(quiet.iter().copied())?,
            motor: Vec::new(),
            recent: std::collections::VecDeque::with_capacity(RECENT_FRAMES),
            seen: 0,
        })
    }

    /// The running motor's level, once heard.
    pub fn motor_dbfs(&self) -> Option<f32> {
        (self.motor.len() >= MOTOR_FRAMES.len() / 2)
            .then(|| median(self.motor.iter().copied()))
            .flatten()
    }

    /// Feeds newly heard levels; `true` once the stall has been heard.
    pub fn stalled(&mut self, levels: &[f32]) -> bool {
        for &l in levels {
            self.seen += 1;
            if MOTOR_FRAMES.contains(&self.seen) {
                self.motor.push(l);
            }
            if self.recent.len() == RECENT_FRAMES {
                self.recent.pop_front();
            }
            self.recent.push_back(l);
            if self.recent.len() < RECENT_FRAMES {
                continue;
            }
            let Some(now) = median(self.recent.iter().copied()) else {
                continue;
            };
            if self.quiet_dbfs <= QUIET_MAX_DBFS && now >= self.quiet_dbfs + RISE_OVER_QUIET_DB {
                return true;
            }
            if let Some(motor) = self.motor_dbfs()
                && now >= motor + RISE_OVER_MOTOR_DB
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::homing_levels::{PAN_HOME, PAN_HOME_FIRST, TILT_HOME};
    use super::*;

    #[test]
    fn level_of_silence_and_of_a_full_scale_square() {
        assert_eq!(level_dbfs(&[0; 1024]), -100.0);
        let full: Vec<i16> = (0..1024)
            .map(|i| if i % 2 == 0 { 32767 } else { -32767 })
            .collect();
        assert!((level_dbfs(&full)).abs() < 0.01);
        let half: Vec<i16> = (0..1024)
            .map(|i| if i % 2 == 0 { 16384 } else { -16384 })
            .collect();
        assert!((level_dbfs(&half) + 6.02).abs() < 0.05);
    }

    /// Frame at which the detector, created on `quiet`, first reports the stall while fed `drive`.
    fn heard_at(quiet: &[f32], drive: &[f32]) -> Option<usize> {
        let mut d = Detector::listening_to(quiet).unwrap();
        drive.iter().position(|l| d.stalled(&[*l]))
    }

    // TILT_HOME frames: quiet 0..23, running 23..78, grinding 79..131, quiet after.
    const TILT_QUIET: std::ops::Range<usize> = 0..23;
    const TILT_STOP: usize = 79;

    #[test]
    fn tilt_is_heard_at_its_bottom_stop_within_half_a_second_and_not_while_running() {
        let at = heard_at(&TILT_HOME[TILT_QUIET], &TILT_HOME[23..]).unwrap() + 23;
        assert!(
            (TILT_STOP..TILT_STOP + RECENT_FRAMES).contains(&at),
            "heard at frame {at}"
        );
    }

    #[test]
    fn pan_is_heard_at_its_stop_in_both_recordings() {
        // PAN_HOME: running from frame 0, the stop at 58
        let at = heard_at(&TILT_HOME[TILT_QUIET], PAN_HOME).unwrap();
        assert!(
            (58..58 + RECENT_FRAMES).contains(&at),
            "heard at frame {at}"
        );
        // PAN_HOME_FIRST: running from frame 0, the stop at 26
        let at = heard_at(&TILT_HOME[TILT_QUIET], PAN_HOME_FIRST).unwrap();
        assert!(
            (26..26 + RECENT_FRAMES).contains(&at),
            "heard at frame {at}"
        );
    }

    #[test]
    fn a_capture_reading_loud_at_boot_still_hears_the_stall_against_the_running_motor() {
        let at = heard_at(&[-29.0; 8], &TILT_HOME[23..]).unwrap() + 23;
        assert!(
            (TILT_STOP..TILT_STOP + RECENT_FRAMES).contains(&at),
            "heard at frame {at}"
        );
    }

    #[test]
    fn a_knock_while_running_is_not_the_stall() {
        let mut drive: Vec<f32> = TILT_HOME[23..70].to_vec();
        drive.extend([-20.0; 10]);
        drive.extend(&TILT_HOME[23..70]);
        assert_eq!(heard_at(&TILT_HOME[TILT_QUIET], &drive), None);
    }

    #[test]
    fn running_right_from_the_stop_is_heard_by_the_quiet_rule() {
        let at = heard_at(&TILT_HOME[TILT_QUIET], &TILT_HOME[TILT_STOP..]).unwrap();
        assert!(at < RECENT_FRAMES, "heard at frame {at}");
    }

    #[test]
    fn too_few_quiet_frames_mean_no_microphone() {
        assert!(Detector::listening_to(&[-45.0; 4]).is_none());
        assert!(Detector::listening_to(&[]).is_none());
    }
}
