//! What the speaker plays next, 40 ms at a time: one-shot sounds (clips, tones, the deny tone) over
//! the looping ringback over talk. The speaker thread is the hardware adapter; this is its policy.
//!
//! - A one-shot plays to its end; others queue behind it in order (they are all short).
//! - A one-shot takes over from the ringback at the next frame and the ringback resumes after it.
//! - Stopping the ringback takes effect at the next frame.
//! - Talk (the backchannel mixer) plays only when nothing else does: never under the ringback.
use std::collections::VecDeque;
use std::sync::Arc;

pub type Samples = Arc<Vec<i16>>;

/// The next frame's source; the samples are exactly `len` long, zero-padded at a one-shot's end.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    OneShot(Vec<i16>),
    Ringback(Vec<i16>),
    /// Nothing scheduled: play talk if any.
    Talk,
}

#[derive(Debug, Default)]
pub struct Scheduler {
    ringback: Option<(Samples, usize)>,
    current: Option<(Samples, usize)>,
    queue: VecDeque<Samples>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts the loop from its beginning (`Some`) or stops it (`None`).
    pub fn ringback(&mut self, samples: Option<Samples>) {
        self.ringback = samples.filter(|s| !s.is_empty()).map(|s| (s, 0));
    }

    pub fn ringing(&self) -> bool {
        self.ringback.is_some()
    }

    pub fn play(&mut self, samples: Samples) {
        if !samples.is_empty() {
            self.queue.push_back(samples);
        }
    }

    /// True while a one-shot is playing or queued.
    pub fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    pub fn frame(&mut self, len: usize) -> Frame {
        if self.current.is_none() {
            self.current = self.queue.pop_front().map(|s| (s, 0));
        }
        if let Some((s, pos)) = self.current.as_mut() {
            let end = (*pos + len).min(s.len());
            let mut out = s[*pos..end].to_vec();
            out.resize(len, 0);
            *pos = end;
            if *pos >= s.len() {
                self.current = None;
            }
            return Frame::OneShot(out);
        }
        if let Some((s, pos)) = self.ringback.as_mut() {
            let mut out = Vec::with_capacity(len);
            while out.len() < len {
                let take = (len - out.len()).min(s.len() - *pos);
                out.extend_from_slice(&s[*pos..*pos + take]);
                *pos = (*pos + take) % s.len();
            }
            return Frame::Ringback(out);
        }
        Frame::Talk
    }
}

/// Notes `(Hz, ms)` at `rate`, a frequency of 0 being a rest, with 5 ms fades against clicks.
pub fn tone(notes: &[(f32, u32)], rate: u32, amplitude: f32) -> Vec<i16> {
    let fade = (rate / 200).max(1) as usize;
    let mut out = Vec::new();
    for &(hz, ms) in notes {
        let n = (rate as u64 * u64::from(ms) / 1000) as usize;
        for i in 0..n {
            let v = if hz <= 0.0 {
                0.0
            } else {
                let env = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
                let t = i as f32 / rate as f32;
                amplitude * env * (2.0 * std::f32::consts::PI * hz * t).sin()
            };
            out.push(v as i16);
        }
    }
    out
}

/// The mock's sounds: answered with the microphone on, declined, hang-up.
pub const ANSWERED: &[(f32, u32)] = &[(660.0, 120), (880.0, 180)];
pub const DECLINED: &[(f32, u32)] = &[(523.0, 180), (0.0, 80), (392.0, 300)];
pub const ENDED: &[(f32, u32)] = &[(440.0, 120), (0.0, 60), (440.0, 120)];

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[i16]) -> Samples {
        Arc::new(v.to_vec())
    }

    #[test]
    fn talk_plays_when_nothing_is_scheduled() {
        assert_eq!(Scheduler::new().frame(4), Frame::Talk);
    }

    #[test]
    fn the_ringback_loops_and_talk_never_plays_under_it() {
        let mut k = Scheduler::new();
        k.ringback(Some(s(&[1, 2, 3])));
        assert_eq!(k.frame(4), Frame::Ringback(vec![1, 2, 3, 1]));
        assert_eq!(k.frame(4), Frame::Ringback(vec![2, 3, 1, 2]));
        for _ in 0..100 {
            assert!(matches!(k.frame(4), Frame::Ringback(_)));
        }
    }

    #[test]
    fn stopping_the_ringback_takes_effect_at_the_next_frame() {
        let mut k = Scheduler::new();
        k.ringback(Some(s(&[1; 1000])));
        k.frame(640);
        k.ringback(None);
        assert_eq!(k.frame(640), Frame::Talk);
        assert!(!k.ringing());
    }

    #[test]
    fn a_one_shot_takes_over_from_the_ringback_at_once_and_it_resumes_after() {
        let mut k = Scheduler::new();
        k.ringback(Some(s(&[7, 8])));
        assert_eq!(k.frame(1), Frame::Ringback(vec![7]));
        k.play(s(&[1, 2, 3]));
        assert_eq!(k.frame(2), Frame::OneShot(vec![1, 2]));
        assert_eq!(k.frame(2), Frame::OneShot(vec![3, 0]));
        assert_eq!(k.frame(2), Frame::Ringback(vec![8, 7]));
    }

    #[test]
    fn one_shots_play_in_order_and_whole() {
        let mut k = Scheduler::new();
        k.play(s(&[1, 1, 1]));
        k.play(s(&[2]));
        assert!(k.busy());
        assert_eq!(k.frame(2), Frame::OneShot(vec![1, 1]));
        assert_eq!(k.frame(2), Frame::OneShot(vec![1, 0]));
        assert_eq!(k.frame(2), Frame::OneShot(vec![2, 0]));
        assert!(!k.busy());
        assert_eq!(k.frame(2), Frame::Talk);
    }

    #[test]
    fn empty_sounds_are_ignored() {
        let mut k = Scheduler::new();
        k.play(s(&[]));
        k.ringback(Some(s(&[])));
        assert_eq!(k.frame(2), Frame::Talk);
    }

    #[test]
    fn tones_have_the_mock_lengths_and_start_and_end_silent() {
        let t = tone(ANSWERED, 16_000, 8000.0);
        assert_eq!(t.len(), 16_000 * 300 / 1000);
        assert_eq!(t[0], 0);
        assert!(t[t.len() - 1].abs() < 200);
        assert!(t.iter().map(|v| v.abs()).max().unwrap() > 7000);
        assert_eq!(tone(DECLINED, 8_000, 1.0).len(), 8 * 560);
        let e = tone(ENDED, 16_000, 8000.0);
        assert!(
            e[16 * 130..16 * 170].iter().all(|&v| v == 0),
            "rest between the beeps"
        );
    }
}
