//! The strike pulse policy: when the strike relocks, how long the pulse is, and when a publication is
//! due. Monotonic milliseconds in, decisions out; the runtime owns the hardware and the retries.

/// The strike never stays energized longer than this whatever the configuration says.
pub const MAX_PULSE_MS: u64 = 120_000;

/// What a publication needs about the running pulse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub pulse_ms: u64,
    pub remaining_ms: u64,
}

#[derive(Debug, Default)]
pub struct Strike {
    deadline_ms: Option<u64>,
    pulse_ms: u64,
    generation: u64,
}

impl Strike {
    pub fn new() -> Self {
        Self::default()
    }
    /// Start or restart a pulse of `open_s` (clamped to 1..=120 s) at `now_ms`. A restart runs from now
    /// with the current length, so it can also shorten a pulse.
    pub fn open(&mut self, now_ms: u64, open_s: u32) {
        self.pulse_ms = (u64::from(open_s) * 1000).clamp(1000, MAX_PULSE_MS);
        self.deadline_ms = Some(now_ms + self.pulse_ms);
        self.generation += 1;
    }
    /// "Close now" or a failed open: the running pulse is due immediately. Nothing when locked.
    pub fn close_now(&mut self) {
        if self.deadline_ms.is_some() {
            self.deadline_ms = Some(0);
            self.generation += 1;
        }
    }
    /// True while a pulse runs and its deadline has passed: the runtime releases, retrying, until
    /// [`Strike::released`].
    pub fn due(&self, now_ms: u64) -> bool {
        self.deadline_ms.is_some_and(|d| now_ms >= d)
    }
    /// The hardware confirmed the release.
    pub fn released(&mut self) {
        if self.deadline_ms.take().is_some() {
            self.generation += 1;
        }
    }
    /// `Some` while a pulse is running; `None` when locked.
    pub fn pulse(&self, now_ms: u64) -> Option<Pulse> {
        self.deadline_ms.map(|d| Pulse {
            pulse_ms: self.pulse_ms,
            remaining_ms: d.saturating_sub(now_ms),
        })
    }
    /// Changes on every open, restart, close and release: the runtime republishes when it changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_from_locked_runs_the_configured_pulse() {
        let mut s = Strike::new();
        assert_eq!(s.pulse(0), None);
        s.open(1_000, 120);
        assert_eq!(
            s.pulse(1_000),
            Some(Pulse {
                pulse_ms: 120_000,
                remaining_ms: 120_000
            })
        );
        assert!(!s.due(120_999));
        assert!(s.due(121_000));
    }

    #[test]
    fn a_restart_moves_the_deadline_and_bumps_the_generation() {
        let mut s = Strike::new();
        s.open(0, 10);
        let g = s.generation();
        s.open(4_000, 10);
        assert!(s.generation() > g);
        assert_eq!(s.pulse(4_000).unwrap().remaining_ms, 10_000);
        assert!(!s.due(10_000));
    }

    #[test]
    fn a_restart_after_open_seconds_was_lowered_uses_the_new_value() {
        let mut s = Strike::new();
        s.open(0, 120);
        s.open(1_000, 5);
        assert_eq!(
            s.pulse(1_000),
            Some(Pulse {
                pulse_ms: 5_000,
                remaining_ms: 5_000
            })
        );
        assert!(s.due(6_000));
    }

    #[test]
    fn the_length_is_clamped_to_1_to_120_seconds() {
        let mut s = Strike::new();
        s.open(0, 0);
        assert_eq!(s.pulse(0).unwrap().pulse_ms, 1_000);
        s.open(0, 600);
        assert_eq!(s.pulse(0).unwrap().pulse_ms, MAX_PULSE_MS);
    }

    #[test]
    fn close_now_is_due_immediately_and_stays_due_until_released() {
        let mut s = Strike::new();
        s.open(5_000, 120);
        let g = s.generation();
        s.close_now();
        assert!(s.generation() > g);
        assert!(s.due(5_000));
        assert!(s.due(5_050), "a failed release is retried");
        let g = s.generation();
        s.released();
        assert!(s.generation() > g);
        assert!(!s.due(5_100));
        assert_eq!(s.pulse(5_100), None);
    }

    #[test]
    fn close_when_locked_changes_nothing() {
        let mut s = Strike::new();
        s.close_now();
        s.released();
        assert_eq!((s.generation(), s.pulse(0), s.due(0)), (0, None, false));
    }

    #[test]
    fn remaining_saturates_at_zero_past_the_deadline() {
        let mut s = Strike::new();
        s.open(0, 1);
        assert_eq!(s.pulse(9_999).unwrap().remaining_ms, 0);
        assert!(s.due(9_999));
    }
}
