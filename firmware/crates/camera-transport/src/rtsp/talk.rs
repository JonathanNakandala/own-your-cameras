//! Talkback from up to four RTSP backchannels at once, mixed for the speaker. Each talker sends Opus or
//! G.711 µ-law; the payload type of each packet picks its decoder.
//!
//! Each talker has its own decoder (at the speaker rate) and gain. Their packets arrive at independent times, so a pure
//! [`Mixer`] keeps one sample FIFO per talker and emits 20 ms blocks on its own real-time output clock:
//! taking "one packet per talker per call" would instead play two talkers one after another at double rate.

use super::agc::{Limiter, TalkGain};
use crate::opus::{OpusDecode, PAYLOAD_TYPE, speaker_frame, speaker_max};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Simultaneous talkers mixed into the speaker; the next backchannel SETUP is refused with 453.
pub const MAX_TALKERS: usize = 4;
/// A talker joins the mix (and rejoins after running dry) once this much audio is buffered. Wi-Fi delivery
/// to the cameras stalls for more than 40 ms now and then, which a 40 ms buffer turned into audible gaps.
const PREBUFFER_MS: usize = 100;
/// A talker's FIFO never holds more than this: older samples are dropped rather than played late. Room for
/// Surveillance Station's bursts (two 140 ms packets every 300 ms) behind their prebuffer.
const MAX_BUFFER_MS: usize = 800;
/// A gap between a talker's packets longer than this is a pause in speech, not delivery jitter.
pub(crate) const PAUSE: Duration = Duration::from_millis(800);
/// The longest PCMU packet accepted. Recorders pack more than an Opus frame: Surveillance Station sends
/// 128 ms (1,024 samples) per packet.
const PCMU_MAX_MS: usize = 1000;
const TICK: Duration = Duration::from_millis(20);
/// An output clock this far behind (a stalled hardware loop) restarts instead of bursting to catch up.
const MAX_LAG: Duration = Duration::from_millis(60);
/// A burst starts this far ahead of real time, so the speaker hardware's queue holds a lead that absorbs a
/// late turn of the hardware loop instead of running dry.
const LEAD: Duration = Duration::from_millis(40);

/// Produces one decoder per talk session.
pub type Decoders = Box<dyn Fn() -> Option<Box<dyn OpusDecode>> + Send + Sync>;

#[derive(Default)]
struct Fifo {
    samples: VecDeque<f32>,
    playing: bool,
    /// The session closed: its buffered audio plays out, then the slot is freed.
    closing: bool,
    stats: TalkStats,
    /// When audio last arrived, and the largest packet so far (in samples).
    last_push: Option<Instant>,
    largest: usize,
    /// The longest gap between packets so far, pauses excluded.
    max_gap: Duration,
    /// Extra buffering this talker earned by running dry: [`REBUFFER_STEP`] each time, up to
    /// [`REBUFFER_MAX_MS`], so jitter that emptied the buffer once does not empty it again.
    extra: usize,
}

const REBUFFER_STEP_MS: usize = 40;
const REBUFFER_MAX_MS: usize = 300;

/// What one talk session's audio went through, in speaker samples; logged when the session ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TalkStats {
    pub received: u64,
    pub played: u64,
    /// Output blocks this talker could only partly fill (zero-filled).
    pub underruns: u64,
    /// Times this talker ran dry and had to buffer again before rejoining.
    pub rebuffers: u64,
    /// Samples dropped because the talker was more than the latency cap ahead.
    pub overflow: u64,
    /// The longest wait between two packets (pauses in speech excluded), and how many waits exceeded
    /// [`LATE`]: delivery jitter as the camera saw it.
    pub max_gap_ms: u64,
    pub late: u64,
}

/// A wait between packets longer than this is late delivery (20 ms frames arrive every 20 ms).
const LATE: Duration = Duration::from_millis(60);

/// Pure mixing state: no clock reads, no I/O. Samples are full-scale `f32` after each talker's gain.
pub struct Mixer {
    talkers: [Option<Fifo>; MAX_TALKERS],
    next_tick: Option<Instant>,
    limiter: Limiter,
    frame: usize,
    prebuffer: usize,
    max_buffer: usize,
    lead: Duration,
    /// Holds each talker's buffer against clock drift by playing 1 % slower or faster.
    drift: bool,
    finished: Vec<TalkStats>,
}

/// The talker's longest delivery gap once a packet arrives at `now`; a pause in speech does not count.
fn gap_after(fifo: &Fifo, now: Instant) -> Duration {
    match fifo.last_push {
        Some(at) if now.saturating_duration_since(at) < PAUSE => {
            fifo.max_gap.max(now.saturating_duration_since(at))
        }
        _ => fifo.max_gap,
    }
}

impl Mixer {
    /// 20 ms output blocks at the speaker's `rate` (Hz).
    pub fn new(rate: u32) -> Self {
        Self {
            lead: LEAD,
            drift: true,
            ..Self::with_buffers(rate, PREBUFFER_MS, MAX_BUFFER_MS)
        }
    }

    /// As [`Mixer::new`] with explicit talker buffering (join threshold and latency cap, ms) and no start lead.
    pub fn with_buffers(rate: u32, prebuffer_ms: usize, max_buffer_ms: usize) -> Self {
        let ms = rate as usize / 1000;
        Self {
            talkers: Default::default(),
            next_tick: None,
            limiter: Limiter::new(rate),
            frame: speaker_frame(rate),
            prebuffer: prebuffer_ms * ms,
            max_buffer: max_buffer_ms * ms,
            lead: Duration::ZERO,
            drift: false,
            finished: Vec::new(),
        }
    }

    pub fn claim(&mut self) -> Option<usize> {
        let slot = self.talkers.iter().position(Option::is_none)?;
        self.talkers[slot] = Some(Fifo::default());
        Some(slot)
    }

    /// Ends a session. Audio it already sent still plays (cutting it mid-waveform clicks); the slot is freed
    /// and its stats returned once it is empty, here or from [`Mixer::take_finished`].
    pub fn release(&mut self, slot: usize) -> Option<TalkStats> {
        let entry = self.talkers.get_mut(slot)?;
        let fifo = entry.as_mut()?;
        if fifo.samples.is_empty() {
            return entry.take().map(|f| f.stats);
        }
        fifo.closing = true;
        fifo.playing = true;
        None
    }

    /// Stats of closed sessions whose remaining audio has finished playing.
    pub fn take_finished(&mut self) -> Vec<TalkStats> {
        std::mem::take(&mut self.finished)
    }

    fn free_drained(&mut self) {
        for entry in &mut self.talkers {
            if entry
                .as_ref()
                .is_some_and(|f| f.closing && f.samples.is_empty())
                && let Some(f) = entry.take()
            {
                self.finished.push(f.stats);
            }
        }
    }

    pub fn push(&mut self, slot: usize, samples: &[f32]) {
        self.push_at(slot, samples, Instant::now());
    }

    /// A talker's prebuffer: the base one, or for talkers that deliver in bursts (Surveillance Station sends
    /// two 140 ms PCMU packets every 300 ms) its longest packet or gap and half the base again, so the
    /// buffer outlasts the wait for the next burst.
    fn lead_samples(&self) -> usize {
        (self.lead.as_micros() * self.frame as u128 / TICK.as_micros()) as usize
    }

    fn prebuffer_of(&self, fifo: &Fifo) -> usize {
        // The output clock starts `lead` ahead of real time and takes that much at once, so the join
        // threshold covers it: the jitter cushion left is the whole base prebuffer, plus what this
        // talker earned by running dry.
        let lead = self.lead_samples();
        let base = self.prebuffer + fifo.extra + lead;
        let gap = (fifo.max_gap.as_micros() * self.frame as u128 / TICK.as_micros()) as usize;
        let need = fifo.largest.max(gap);
        if need <= self.prebuffer {
            return base;
        }
        // Never more than half the FIFO, so the next burst always fits behind it.
        (need + self.prebuffer / 2)
            .min(self.max_buffer / 2)
            .max(base)
    }

    pub fn push_at(&mut self, slot: usize, samples: &[f32], now: Instant) {
        let prebuffer = match self.talkers.get(slot) {
            Some(Some(fifo)) => self.prebuffer_of(&Fifo {
                largest: fifo.largest.max(samples.len()),
                max_gap: gap_after(fifo, now),
                extra: fifo.extra,
                ..Fifo::default()
            }),
            _ => return,
        };
        let Some(Some(fifo)) = self.talkers.get_mut(slot) else {
            return;
        };
        fifo.largest = fifo.largest.max(samples.len());
        if let Some(at) = fifo.last_push {
            let wait = now.saturating_duration_since(at);
            if wait < PAUSE {
                fifo.stats.max_gap_ms = fifo.stats.max_gap_ms.max(wait.as_millis() as u64);
                if wait > LATE {
                    fifo.stats.late += 1;
                }
            }
        }
        fifo.max_gap = gap_after(fifo, now);
        fifo.last_push = Some(now);
        fifo.samples.extend(samples);
        fifo.stats.received += samples.len() as u64;
        let excess = fifo.samples.len().saturating_sub(self.max_buffer);
        fifo.samples.drain(..excess);
        fifo.stats.overflow += excess as u64;
        if !fifo.playing && fifo.samples.len() >= prebuffer {
            fifo.playing = true;
        }
    }

    /// One 20 ms block when the output clock is due, else `None`. Silence (all FIFOs empty) stops the clock.
    pub fn next(&mut self, now: Instant) -> Option<Vec<i16>> {
        self.free_drained();
        if self.talkers.iter().flatten().all(|t| t.samples.is_empty()) {
            self.next_tick = None;
            for t in self.talkers.iter_mut().flatten() {
                t.playing = false;
            }
            return None;
        }
        // A remainder below the prebuffer that nothing follows plays out now rather than when the session
        // closes, which recorders do seconds after the talker let go.
        let quiet = Duration::from_micros(
            self.prebuffer as u64 * TICK.as_micros() as u64 / self.frame.max(1) as u64,
        );
        for t in self.talkers.iter_mut().flatten() {
            if !t.playing
                && !t.samples.is_empty()
                && t.last_push
                    .is_some_and(|at| now.saturating_duration_since(at) >= quiet)
            {
                t.playing = true;
            }
        }
        if !self.talkers.iter().flatten().any(|t| t.playing) {
            return None;
        }
        let due = match self.next_tick {
            Some(tick) if now.saturating_duration_since(tick) <= MAX_LAG => tick,
            Some(_) => now,
            None => now.checked_sub(self.lead).unwrap_or(now),
        };
        if now < due {
            return None;
        }
        self.next_tick = Some(due + TICK);
        let frame = self.frame;
        let mut mix = vec![0f32; frame];
        let drift = self.drift;
        let low = self.prebuffer / 2;
        let high = self.prebuffer * 2 + self.lead_samples();
        for fifo in self.talkers.iter_mut().flatten().filter(|t| t.playing) {
            // Clock drift between the sender and this output empties or fills the buffer slowly; play a
            // block from 1 % fewer or more samples (inaudible) to hold it instead of running dry.
            let level = fifo.samples.len();
            let want = if fifo.closing || !drift {
                frame
            } else if level < low + frame {
                frame - frame / 100
            } else if level > high + frame {
                frame + frame / 100
            } else {
                frame
            };
            if level >= want && want != frame {
                let src: Vec<f32> = fifo.samples.drain(..want).collect();
                for (i, out) in mix.iter_mut().enumerate() {
                    let pos = i as f32 * (want - 1) as f32 / (frame - 1) as f32;
                    let k = pos as usize;
                    let f = pos - k as f32;
                    let next = src[(k + 1).min(want - 1)];
                    *out += src[k] + (next - src[k]) * f;
                }
                fifo.stats.played += want as u64;
                if fifo.samples.is_empty() && !fifo.closing {
                    fifo.playing = false;
                    fifo.stats.rebuffers += 1;
                    let ms = frame / TICK.as_millis() as usize;
                    fifo.extra = (fifo.extra + REBUFFER_STEP_MS * ms).min(REBUFFER_MAX_MS * ms);
                }
                continue;
            }
            // An underrun is zero-filled; an emptied talker buffers again before it rejoins.
            let take = frame.min(fifo.samples.len());
            for (out, sample) in mix.iter_mut().zip(fifo.samples.drain(..take)) {
                *out += sample;
            }
            fifo.stats.played += take as u64;
            // A closed talker's last block is short by design: that is the end of its audio, not a loss.
            if take < frame && !fifo.closing {
                fifo.stats.underruns += 1;
            }
            if fifo.samples.is_empty() {
                fifo.playing = false;
                if !fifo.closing {
                    fifo.stats.rebuffers += 1;
                    let ms = self.frame / TICK.as_millis() as usize;
                    fifo.extra = (fifo.extra + REBUFFER_STEP_MS * ms).min(REBUFFER_MAX_MS * ms);
                }
            }
        }
        Some(self.limiter.process(&mix))
    }
}

/// Speaker input shared by both RTSP paths.
pub struct Talkback {
    decoders: Decoders,
    rate: u32,
    /// Corner of each talker's high-pass filter ahead of its gain control, 0 for none.
    high_pass_hz: u32,
    mixer: Mutex<Mixer>,
}

pub struct TalkLease {
    talk: Arc<Talkback>,
    slot: usize,
    decoder: TalkDecoder,
    filter: Option<super::highpass::HighPass>,
    gain: TalkGain,
    level: LevelStats,
    /// Packets refused (not RTP, or a payload type other than Opus or PCMU) and the last refused type.
    refused: u64,
    refused_pt: Option<u8>,
}

/// One talker's decoders, chosen per packet by payload type. PCMU is decoded and interpolated from
/// 8 kHz to the speaker rate; a speaker rate that is no multiple of 8 kHz plays no PCMU.
struct TalkDecoder {
    opus: Box<dyn OpusDecode>,
    pcmu: Option<crate::resample::Interpolator>,
}

impl TalkDecoder {
    fn new(opus: Box<dyn OpusDecode>, rate: u32) -> Self {
        Self {
            opus,
            pcmu: crate::resample::Interpolator::new(rate),
        }
    }

    fn decode(&mut self, codec: Codec, payload: &[u8]) -> Option<Vec<i16>> {
        match codec {
            Codec::Opus => self.opus.decode(payload),
            Codec::Pcmu => {
                let linear: Vec<i16> = payload
                    .iter()
                    .map(|&b| crate::resample::mulaw_to_linear(b))
                    .collect();
                Some(self.pcmu.as_mut()?.push(&linear))
            }
        }
    }
}

impl Talkback {
    /// `decoders` must produce mono samples at `rate`, the speaker's rate.
    pub fn new(decoders: Decoders, rate: u32) -> Arc<Self> {
        Self::with_high_pass(decoders, rate, 0)
    }

    /// As [`Talkback::new`], with every talker high-passed at `high_pass_hz` before its gain control
    /// (see [`super::highpass`]); 0 means no filter.
    pub fn with_high_pass(decoders: Decoders, rate: u32, high_pass_hz: u32) -> Arc<Self> {
        Arc::new(Self {
            decoders,
            rate,
            high_pass_hz,
            mixer: Mutex::new(Mixer::new(rate)),
        })
    }

    fn mixer(&self) -> std::sync::MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `None` when [`MAX_TALKERS`] sessions already talk or no decoder can be created.
    pub fn claim(self: &Arc<Self>) -> Option<TalkLease> {
        let slot = self.mixer().claim()?;
        let Some(decoder) = (self.decoders)() else {
            let _ = self.mixer().release(slot);
            return None;
        };
        Some(TalkLease {
            talk: Arc::clone(self),
            slot,
            decoder: TalkDecoder::new(decoder, self.rate),
            filter: super::highpass::HighPass::new(self.rate, self.high_pass_hz),
            gain: TalkGain::default(),
            level: LevelStats::default(),
            refused: 0,
            refused_pt: None,
        })
    }

    /// Backchannel sessions currently holding a talker slot.
    pub fn talkers(&self) -> usize {
        self.mixer().talkers.iter().flatten().count()
    }

    /// Called by the audio hardware loop at least every 20 ms; it never waits for a network client.
    pub fn next_pcm(&self, now: Instant) -> Option<Vec<i16>> {
        let (block, finished) = {
            let mut mixer = self.mixer();
            (mixer.next(now), mixer.take_finished())
        };
        for stats in finished {
            self.log(stats);
        }
        block
    }

    fn log(&self, s: TalkStats) {
        let ms = |samples: u64| samples * 1000 / u64::from(self.rate);
        eprintln!(
            "talk: session ended received_ms={} played_ms={} underruns={} rebuffers={} overflow_ms={} max_gap_ms={} late={}",
            ms(s.received),
            ms(s.played),
            s.underruns,
            s.rebuffers,
            ms(s.overflow),
            s.max_gap_ms,
            s.late
        );
    }
}

impl TalkLease {
    /// One RTP packet from this talker. Malformed packets and payload types other than Opus (111, 96) and
    /// PCMU (0) are ignored.
    pub fn push_rtp(&mut self, packet: &[u8]) {
        let Some((codec, payload)) = rtp_payload(packet) else {
            self.refused += 1;
            self.refused_pt = packet.get(1).map(|b| b & 127);
            return;
        };
        let Some(pcm) = self.decoder.decode(codec, payload) else {
            return;
        };
        let max = match codec {
            Codec::Opus => speaker_max(self.talk.rate),
            Codec::Pcmu => self.talk.rate as usize * PCMU_MAX_MS / 1000,
        };
        if pcm.is_empty() || pcm.len() > max {
            self.refused += 1;
            self.refused_pt = packet.get(1).map(|b| b & 127);
            return;
        }
        let before = rms(&pcm);
        let pcm = match &mut self.filter {
            Some(filter) => filter.process(&pcm),
            None => pcm,
        };
        let after = rms(&pcm);
        self.level.add(before, after);
        let samples = self.gain.process(&pcm);
        self.talk.mixer().push(self.slot, &samples);
    }
}

/// One talk session's input level, for the log line that says whether a voice arrived and how loud.
#[derive(Default)]
struct LevelStats {
    packets: u64,
    /// Packets the gain control counted as speech (after the filter).
    speech: u64,
    loudest: f32,
    sum_before: f32,
    sum_after: f32,
}

impl LevelStats {
    fn add(&mut self, before: f32, after: f32) {
        self.packets += 1;
        if TalkGain::is_speech(after) {
            self.speech += 1;
        }
        self.loudest = self.loudest.max(before);
        self.sum_before += before * before;
        self.sum_after += after * after;
    }

    fn mean(&self, sum: f32) -> f32 {
        (sum / self.packets as f32).sqrt()
    }
}

fn rms(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f32 = pcm.iter().map(|&s| (f32::from(s) / 32768.0).powi(2)).sum();
    (sum / pcm.len() as f32).sqrt()
}

fn dbfs(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

impl Drop for TalkLease {
    fn drop(&mut self) {
        if self.refused > 0 {
            eprintln!(
                "talk: refused {} packets (last payload type {:?}): not Opus (111, 96) or PCMU (0), or too long",
                self.refused, self.refused_pt
            );
        }
        if self.level.packets > 0 {
            let l = &self.level;
            eprintln!(
                "talk: input packets={} speech={} loudest_dbfs={:.0} mean_dbfs={:.0} after_filter_dbfs={:.0} gain_db={:.0}",
                l.packets,
                l.speech,
                dbfs(l.loudest),
                dbfs(l.mean(l.sum_before)),
                dbfs(l.mean(l.sum_after)),
                20.0 * self.gain.gain().log10()
            );
        }
        let stats = self.talk.mixer().release(self.slot);
        if let Some(stats) = stats {
            self.talk.log(stats);
        }
    }
}

/// The payload type ffmpeg gives its Opus output.
const FFMPEG_OPUS_PT: u8 = 96;

/// Static RTP payload type of G.711 µ-law (RFC 3551).
const PCMU_PT: u8 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Opus,
    Pcmu,
}

fn rtp_payload(packet: &[u8]) -> Option<(Codec, &[u8])> {
    // Opus is offered as 111, which WebRTC relays keep; ffmpeg (go2rtc's `ffmpeg:` sources) numbers its
    // Opus output 96 whatever was offered. Recorders talk PCMU, static type 0. Anything else is refused.
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let codec = match packet[1] & 127 {
        PAYLOAD_TYPE | FFMPEG_OPUS_PT => Codec::Opus,
        PCMU_PT => Codec::Pcmu,
        _ => return None,
    };
    let mut start = 12 + usize::from(packet[0] & 15) * 4;
    if packet[0] & 16 != 0 {
        let header = packet.get(start..start + 4)?;
        start += 4 + usize::from(u16::from_be_bytes([header[2], header[3]])) * 4;
    }
    let padding = if packet[0] & 32 != 0 {
        let n = usize::from(*packet.last()?);
        if n == 0 {
            return None;
        }
        n
    } else {
        0
    };
    let payload = packet.get(start..packet.len().checked_sub(padding)?)?;
    Some((codec, payload))
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// Fake codec: a payload's first byte becomes 20 ms of that constant level (× 16).
    struct Level;
    impl OpusDecode for Level {
        fn decode(&mut self, payload: &[u8]) -> Option<Vec<i16>> {
            (!payload.is_empty()).then(|| vec![i16::from(payload[0]) * 16; 320])
        }
    }

    /// 16 kHz, the Mi 360 speaker rate.
    pub(crate) fn talkback() -> Arc<Talkback> {
        Talkback::new(
            Box::new(|| Some(Box::new(Level) as Box<dyn OpusDecode>)),
            16_000,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::talkback as fake_talkback;

    /// 20 ms at 16 kHz.
    const F: usize = 320;

    fn rtp(level: u8) -> Vec<u8> {
        vec![0x80, PAYLOAD_TYPE, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, level]
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_sender_slower_than_the_output_clock_never_runs_dry() {
        // The sender delivers 0.5 % less than real time: without stretching, a 100 ms cushion runs dry
        // within 20 s; with 1 % stretching below half of it, it holds.
        let mut mixer = Mixer::new(16_000);
        let t0 = Instant::now();
        let a = mixer.claim().unwrap();
        let mut sent = 0f64;
        let mut t = Duration::ZERO;
        let mut next = t0;
        while t < Duration::from_secs(60) {
            let due = t.as_secs_f64() * 16_000.0 * 0.995;
            while sent + F as f64 <= due + 7.0 * F as f64 {
                mixer.push_at(a, &[0.1; F], t0 + t);
                sent += F as f64;
            }
            while next <= t0 + t {
                if mixer.next(t0 + t).is_none() {
                    break;
                }
                next += ms(20);
            }
            t += ms(5);
        }
        let stats = mixer.talkers[a].as_ref().unwrap().stats;
        assert!(stats.played > 16_000 * 59, "{stats:?}");
        assert_eq!(stats.rebuffers, 0, "{stats:?}");
        assert_eq!(stats.underruns, 0, "{stats:?}");
    }

    #[test]
    fn a_talker_that_ran_dry_buffers_40_ms_more_before_it_rejoins() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 800);
        let t0 = Instant::now();
        let a = mixer.claim().unwrap();
        mixer.push_at(a, &[0.1; 2 * F], t0);
        assert!(mixer.next(t0).is_some());
        assert!(
            mixer.next(t0 + ms(20)).is_some(),
            "this block empties the buffer: a rebuffer"
        );
        mixer.push_at(a, &[0.1; 2 * F], t0 + ms(30));
        assert!(mixer.next(t0 + ms(40)).is_none(), "40 ms no longer rejoins");
        mixer.push_at(a, &[0.1; 2 * F], t0 + ms(50));
        assert!(mixer.next(t0 + ms(60)).is_some(), "80 ms does");
    }

    #[test]
    fn a_single_talker_plays_after_the_prebuffer_at_real_time_cadence() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 100);
        let t0 = Instant::now();
        let a = mixer.claim().unwrap();
        mixer.push(a, &[0.1; F]);
        assert!(mixer.next(t0).is_none(), "20 ms is below the prebuffer");
        mixer.push(a, &[0.1; F]);
        mixer.push(a, &[0.1; F]);
        let first = mixer.next(t0).unwrap();
        assert_eq!(first.len(), F);
        assert!((f32::from(first[100]) / 32768.0 - 0.1).abs() < 1e-3);
        assert!(
            mixer.next(t0 + ms(5)).is_none(),
            "the next block is due 20 ms later"
        );
        assert!(mixer.next(t0 + ms(20)).is_some());
        assert!(mixer.next(t0 + ms(40)).is_some());
        assert!(
            mixer.next(t0 + ms(60)).is_none(),
            "all buffered audio played"
        );
    }

    #[test]
    fn two_talkers_are_summed_not_played_one_after_another() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 100);
        let t0 = Instant::now();
        let (a, b) = (mixer.claim().unwrap(), mixer.claim().unwrap());
        for _ in 0..3 {
            mixer.push(a, &[0.1; F]);
            mixer.push(b, &[0.2; F]);
        }
        let mut blocks = 0;
        for tick in 0..10 {
            if let Some(block) = mixer.next(t0 + ms(20 * tick)) {
                assert!((f32::from(block[100]) / 32768.0 - 0.3).abs() < 1e-3);
                blocks += 1;
            }
        }
        assert_eq!(blocks, 3, "60 ms from each talker is 60 ms of output");
    }

    #[test]
    fn the_sum_is_limited_below_full_scale() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 100);
        let t0 = Instant::now();
        let (a, b) = (mixer.claim().unwrap(), mixer.claim().unwrap());
        mixer.push(a, &[0.8; 640]);
        mixer.push(b, &[0.8; 640]);
        let block = mixer.next(t0).unwrap();
        assert!(block.iter().all(|&s| f32::from(s).abs() / 32768.0 <= 0.892));
    }

    #[test]
    fn a_late_talker_buffers_before_it_joins_and_underruns_are_silent() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 100);
        let t0 = Instant::now();
        let (a, b) = (mixer.claim().unwrap(), mixer.claim().unwrap());
        mixer.push(a, &[0.1; 1280]);
        mixer.push(b, &[0.2; F]);
        let block = mixer.next(t0).unwrap();
        assert!(
            (f32::from(block[100]) / 32768.0 - 0.1).abs() < 1e-3,
            "b is still buffering"
        );
        mixer.push(b, &[0.2; F]);
        let block = mixer.next(t0 + ms(20)).unwrap();
        assert!((f32::from(block[100]) / 32768.0 - 0.3).abs() < 1e-3);
        let mut solo = Mixer::with_buffers(16_000, 40, 100);
        let c = solo.claim().unwrap();
        // A packet longer than the prebuffer buffers more; with nothing after it, it plays out once the
        // talker has been quiet for a prebuffer's length (not when the session closes, seconds later).
        solo.push_at(c, &[0.1; 740], t0);
        assert!(
            solo.next(t0).is_none(),
            "a long packet buffers beyond the base prebuffer"
        );
        assert!(solo.next(t0 + ms(40)).is_some());
        assert!(solo.next(t0 + ms(60)).is_some());
        let block = solo.next(t0 + ms(80)).unwrap();
        assert!((f32::from(block[0]) / 32768.0 - 0.1).abs() < 1e-3);
        assert_eq!(block[200], 0, "a's underrun is zero-filled");
    }

    #[test]
    fn latency_is_capped_and_a_stalled_clock_restarts_without_a_burst() {
        let mut mixer = Mixer::with_buffers(16_000, 40, 100);
        let t0 = Instant::now();
        let a = mixer.claim().unwrap();
        for _ in 0..20 {
            mixer.push(a, &[0.1; F]);
        }
        let mut blocks = 0;
        while mixer.next(t0 + ms(20 * blocks)).is_some() {
            blocks += 1;
        }
        assert_eq!(blocks, 5, "only the newest 100 ms remain");
        for _ in 0..5 {
            mixer.push(a, &[0.1; F]);
        }
        let late = t0 + ms(1000);
        assert!(mixer.next(late).is_some());
        assert!(
            mixer.next(late).is_none(),
            "no catch-up burst after a stall"
        );
    }

    #[test]
    fn at_most_four_talk_and_a_closed_session_plays_out_then_frees_its_slot() {
        let talk = fake_talkback();
        let mut leases: Vec<_> = (0..MAX_TALKERS).map(|_| talk.claim().unwrap()).collect();
        assert!(talk.claim().is_none());
        assert_eq!(talk.talkers(), MAX_TALKERS);
        for _ in 0..6 {
            leases[0].push_rtp(&rtp(100));
        }
        drop(leases.remove(0));
        assert_eq!(talk.talkers(), MAX_TALKERS, "its last 120 ms still play");
        let t0 = Instant::now() + Duration::from_secs(1);
        let played = (0..10)
            .filter_map(|i| talk.next_pcm(t0 + ms(20 * i)))
            .count();
        assert_eq!(played, 6, "nothing it sent is cut off");
        assert_eq!(talk.talkers(), MAX_TALKERS - 1);
        assert!(talk.claim().is_some());
    }

    #[test]
    fn rtp_from_a_talker_reaches_the_speaker_with_gain() {
        let talk = fake_talkback();
        let mut lease = talk.claim().unwrap();
        for _ in 0..6 {
            lease.push_rtp(&rtp(100));
        }
        assert!(
            talk.next_pcm(Instant::now()).is_none(),
            "the talker is still within its 140 ms buffer (100 ms plus the 40 ms lead)"
        );
        lease.push_rtp(&rtp(100));
        let pcm = talk.next_pcm(Instant::now()).unwrap();
        assert_eq!(pcm.len(), F);
        assert!(pcm[319] > 1600, "the start gain raises a quiet talker");
    }

    #[test]
    fn malformed_rtp_and_other_payload_types_never_reach_the_speaker() {
        let talk = fake_talkback();
        let mut lease = talk.claim().unwrap();
        for packet in [
            vec![],
            vec![0x80, PAYLOAD_TYPE, 0, 0],
            vec![0x80, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff], // PCMA
            vec![0x90, PAYLOAD_TYPE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff], // short extension
            vec![0xa0, PAYLOAD_TYPE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100], // excessive padding
            vec![0x40, PAYLOAD_TYPE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], // RTP v1
        ] {
            for _ in 0..6 {
                lease.push_rtp(&packet);
            }
        }
        assert!(talk.next_pcm(Instant::now()).is_none());
        let csrc_ext_padding = [
            0xb1,
            PAYLOAD_TYPE,
            0,
            1,
            0,
            0,
            0,
            0,
            1,
            2,
            3,
            4,
            9,
            8,
            7,
            6,
            0xbe,
            0xde,
            0,
            1,
            1,
            2,
            3,
            4,
            100,
            0,
            2,
        ];
        assert_eq!(
            rtp_payload(&csrc_ext_padding),
            Some((Codec::Opus, &[100][..]))
        );
        // ffmpeg numbers the same Opus track 96; other dynamic types are not ours.
        assert_eq!(
            rtp_payload(&[0x80, 96, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 7]),
            Some((Codec::Opus, &[7][..]))
        );
        assert_eq!(
            rtp_payload(&[0x80, 0, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 7]),
            Some((Codec::Pcmu, &[7][..]))
        );
        assert_eq!(
            rtp_payload(&[0x80, 97, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 7]),
            None
        );
    }

    #[test]
    fn a_pcmu_talker_is_interpolated_to_the_speaker_rate_and_mixed_like_opus() {
        let talk = fake_talkback();
        let mut lease = talk.claim().unwrap();
        // 20 ms of a loud 1 kHz µ-law tone (8 samples per period at 8 kHz).
        let tone: Vec<u8> = (0..160)
            .map(|i| {
                let x = (12_000.0 * (std::f64::consts::PI * i as f64 / 4.0).sin()) as i16;
                crate::microphone::encode_pcm8k_mono(&[x]).unwrap()[0]
            })
            .collect();
        let mut packet = vec![0x80, 0, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4];
        packet.extend(&tone);
        for _ in 0..7 {
            lease.push_rtp(&packet);
        }
        let pcm = talk.next_pcm(Instant::now()).unwrap();
        assert_eq!(pcm.len(), F, "8 kHz in, 20 ms at the 16 kHz speaker out");
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 4000, "the tone reaches the speaker: {peak}");
    }

    #[test]
    fn a_48_khz_speaker_gets_960_sample_blocks_once_its_buffer_fills() {
        let mut mixer = Mixer::with_buffers(48_000, 40, 100);
        let t0 = Instant::now();
        let a = mixer.claim().unwrap();
        mixer.push(a, &[0.1; 960]);
        assert!(mixer.next(t0).is_none());
        mixer.push(a, &[0.1; 960]);
        assert_eq!(mixer.next(t0).unwrap().len(), 960);
    }

    #[test]
    fn a_burst_starts_40_ms_ahead_so_the_hardware_queue_has_a_lead() {
        let mut mixer = Mixer::new(16_000);
        let t0 = Instant::now() + Duration::from_secs(1);
        let a = mixer.claim().unwrap();
        mixer.push(a, &[0.1; 1600]);
        let at_once = std::iter::from_fn(|| mixer.next(t0)).count();
        assert_eq!(at_once, 3, "the due block plus 40 ms of lead");
        assert!(mixer.next(t0 + Duration::from_millis(19)).is_none());
        assert!(mixer.next(t0 + Duration::from_millis(20)).is_some());
    }
}
