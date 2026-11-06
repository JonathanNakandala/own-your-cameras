//! Latest-wins hand-off between the settings owner (the `mqtt-hardware` thread) and the media thread,
//! which alone may call HDAL. The owner posts requests; the media loop takes at most one merged request
//! per iteration and reports what actually runs.

use std::sync::Mutex;

use crate::pipeline::{MainStream, MediaTuning, Tuned, Tuner};
use hdal::audio::Tuning;

#[derive(Debug, Default)]
struct State {
    initial: MediaTuning,
    pending: MediaTuning,
    audio: Option<Tuning>,
    main: Option<MainStream>,
    /// Refusals not yet seen by the owner.
    refusals: Vec<String>,
    restarts: Vec<u64>,
    /// Bumped on every report, so the owner republishes states the moment something was applied.
    reports: u64,
}

#[derive(Debug, Default)]
pub struct Handoff(Mutex<State>, Mutex<std::collections::VecDeque<f32>>);

/// Microphone levels kept for homing by ear: about 2 s at one level per 21 ms capture frame.
const LEVELS_KEPT: usize = 100;

/// [`crate::ptz::Listener`] over the media thread's microphone levels.
pub struct Ears(std::sync::Arc<Handoff>);

impl crate::ptz::Listener for Ears {
    fn heard(&mut self) -> Result<Vec<f32>, String> {
        Ok(self.0.levels().drain(..).collect())
    }
}

impl Handoff {
    /// `initial` is what the graph is built with (the persisted settings).
    pub fn new(initial: MediaTuning) -> Self {
        Self(
            Mutex::new(State {
                initial,
                ..State::default()
            }),
            Mutex::new(std::collections::VecDeque::with_capacity(LEVELS_KEPT)),
        )
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn levels(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<f32>> {
        self.1.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What homing by ear listens to.
    pub fn ears(self: &std::sync::Arc<Self>) -> Ears {
        Ears(std::sync::Arc::clone(self))
    }

    pub fn request_audio(&self, t: Tuning) {
        self.state().pending.audio = Some(t);
    }

    pub fn request_main(&self, m: MainStream) {
        self.state().pending.main = Some(m);
    }

    /// What runs now, once the media thread reported it.
    pub fn applied(&self) -> (Option<Tuning>, Option<MainStream>) {
        let s = self.state();
        (s.audio, s.main)
    }

    /// Requests the media thread refused (or replaced with stock at startup) since the last call.
    pub fn take_refusals(&self) -> Vec<String> {
        std::mem::take(&mut self.state().refusals)
    }

    /// Number of reports so far.
    pub fn reports(&self) -> u64 {
        self.state().reports
    }

    /// Main-path restart durations (ms) since the last call.
    pub fn take_restarts(&self) -> Vec<u64> {
        std::mem::take(&mut self.state().restarts)
    }
}

impl Tuner for &Handoff {
    /// The newest [`LEVELS_KEPT`] levels wait for the listener.
    fn hear(&mut self, dbfs: f32) {
        let mut l = self.levels();
        if l.len() >= LEVELS_KEPT {
            l.pop_front();
        }
        l.push_back(dbfs);
    }

    fn initial(&mut self) -> MediaTuning {
        self.state().initial
    }

    fn next(&mut self) -> Option<MediaTuning> {
        let mut s = self.state();
        let p = std::mem::take(&mut s.pending);
        (p != MediaTuning::default()).then_some(p)
    }

    fn report(&mut self, tuned: Tuned) {
        let mut s = self.state();
        s.reports += 1;
        match tuned {
            Tuned::Audio { applied, refused } => {
                s.audio = Some(applied);
                if let Some(e) = refused {
                    s.refusals.push(format!("audio: {e}"));
                }
            }
            Tuned::Main {
                applied,
                refused,
                restart_ms,
            } => {
                s.main = Some(applied);
                if restart_ms > 0 {
                    s.restarts.push(restart_ms);
                }
                if let Some(e) = refused {
                    s.refusals.push(format!("main stream: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_merge_latest_wins_and_are_taken_once() {
        let h = Handoff::new(MediaTuning::default());
        let mut t = &h;
        assert_eq!(t.next(), None);
        let mut a = Tuning::stock();
        h.request_audio(a);
        a.mic_volume = 10;
        h.request_audio(a);
        let m = MainStream {
            dim: hdal::Dim { w: 1280, h: 960 },
            bitrate_bps: 512_000,
            fps: 10,
        };
        h.request_main(m);
        assert_eq!(
            t.next(),
            Some(MediaTuning {
                audio: Some(a),
                main: Some(m)
            })
        );
        assert_eq!(t.next(), None);
    }

    #[test]
    fn reports_are_what_the_owner_reads_back() {
        let h = Handoff::new(MediaTuning::default());
        let mut t = &h;
        t.report(Tuned::Audio {
            applied: Tuning::stock(),
            refused: Some(hdal::Error {
                step: hdal::Step::AudioCapStart,
                code: -1,
            }),
        });
        assert_eq!(h.applied().0, Some(Tuning::stock()));
        assert_eq!(h.take_refusals().len(), 1);
        assert!(h.take_refusals().is_empty());
    }
}

#[cfg(test)]
mod ear_tests {
    use super::*;
    use crate::ptz::Listener;

    #[test]
    fn levels_are_heard_once_and_only_the_newest_are_kept() {
        let h = std::sync::Arc::new(Handoff::new(MediaTuning::default()));
        let mut ears = h.ears();
        assert!(ears.heard().unwrap().is_empty());
        let mut t = &*h;
        for i in 0..(LEVELS_KEPT + 3) {
            t.hear(i as f32);
        }
        let heard = ears.heard().unwrap();
        assert_eq!(heard.len(), LEVELS_KEPT);
        assert_eq!(heard[0], 3.0);
        assert!(ears.heard().unwrap().is_empty());
    }
}
