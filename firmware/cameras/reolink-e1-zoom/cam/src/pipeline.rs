//! The capture → videoproc → two H.264 encoders graph: built from a config, run, and
//! torn down in reverse whatever fails. All hardware access goes through the `hdal` backend traits.

use std::io::Write;

use hdal::audio::Tuning;
use hdal::enc::{EncBackend, EncFrame, EncSpec};
use hdal::{Backend, Dim, Error, MemPool, PathId, ProbeConfig, Step};

use crate::annexb::{AnnexB, Stats};

/// The 4:3 full-sensor-frame sizes the main stream may use (never a crop: videoproc scales the whole
/// 2592×1944 frame).
pub const MAIN_SIZES: [Dim; 3] = [
    Dim { w: 2592, h: 1944 },
    Dim { w: 1920, h: 1440 },
    Dim { w: 1280, h: 960 },
];
/// Top of the main bitrate range; the encoder ring is sized for it at startup so a raise never re-opens.
pub const MAIN_MAX_BITRATE: u32 = 8_192_000;

/// Main and sub, in `streams` order: proc output `i` feeds encoder `i`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineConfig {
    pub cap: ProbeConfig,
    pub streams: [EncSpec; 2],
    /// Largest size and bitrate each encoder may later be retuned to (its path config).
    pub envelopes: [EncSpec; 2],
}

/// Main-stream parameters that may change while the graph runs. The GOP follows the frame rate (one
/// key frame per second).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainStream {
    pub dim: Dim,
    pub bitrate_bps: u32,
    pub fps: u32,
}

impl MainStream {
    pub fn spec(&self) -> EncSpec {
        EncSpec {
            dim: self.dim,
            fps: self.fps,
            bitrate_bps: self.bitrate_bps,
            gop: self.fps,
        }
    }

    pub fn of(spec: &EncSpec) -> Self {
        Self {
            dim: spec.dim,
            bitrate_bps: spec.bitrate_bps,
            fps: spec.fps,
        }
    }
}

/// A request to the media thread; absent parts stay as they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaTuning {
    pub audio: Option<Tuning>,
    pub main: Option<MainStream>,
}

/// What runs after startup or a request. `refused` says why the requested values are not the applied ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tuned {
    Audio {
        applied: Tuning,
        refused: Option<Error>,
    },
    Main {
        applied: MainStream,
        refused: Option<Error>,
        /// How long the main path was stopped (0 at startup).
        restart_ms: u64,
    },
}

/// The settings owner's side of the media thread: persisted values at startup, then latest-wins requests.
pub trait Tuner {
    /// Values to build with; absent parts use the config's stock values.
    fn initial(&mut self) -> MediaTuning;
    /// Polled once per media-loop iteration.
    fn next(&mut self) -> Option<MediaTuning>;
    fn report(&mut self, tuned: Tuned);
}

/// Stock values throughout, for the dump, the diagnostic serve and tests that do not tune.
pub struct Untuned;

impl Tuner for Untuned {
    fn initial(&mut self) -> MediaTuning {
        MediaTuning::default()
    }
    fn next(&mut self) -> Option<MediaTuning> {
        None
    }
    fn report(&mut self, _: Tuned) {}
}

impl PipelineConfig {
    /// The vendor's encoders (`0_0_enc.cfg`, `0_1_enc.cfg`) without its 16:9 crop: main 1920×1440@15
    /// 1024 kbps (the full 4:3 frame, scaled), sub 640×480@10 512 kbps, one-second GOPs, from the full
    /// 2592×1944@30 sensor frame.
    pub fn e1_zoom() -> Self {
        let mut cap = ProbeConfig::os05a10();
        // The camera's `nvtmpp` area is ~47.5 MiB. See docs/hdal-abi.md.
        cap.pool = MemPool {
            blk_size: 2592 * 1944 * 3 / 2,
            blk_cnt: 4,
        };
        let main = EncSpec {
            dim: MAIN_SIZES[1],
            fps: 15,
            bitrate_bps: 1_024_000,
            gop: 15,
        };
        let sub = EncSpec {
            dim: Dim { w: 640, h: 480 },
            fps: 10,
            bitrate_bps: 512_000,
            gop: 10,
        };
        Self {
            cap,
            streams: [main, sub],
            envelopes: [
                EncSpec {
                    dim: MAIN_SIZES[0],
                    bitrate_bps: MAIN_MAX_BITRATE,
                    ..main
                },
                sub,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DumpReport {
    pub elapsed_ms: u64,
    pub streams: [Stats; 2],
}

/// The initiating error and every later teardown failure, in attempted order.
#[derive(Debug)]
pub struct PipelineError {
    pub primary: Error,
    pub cleanup: Vec<Error>,
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.primary)?;
        for error in &self.cleanup {
            write!(f, "; cleanup: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

fn complete<T>(
    result: Result<T, Error>,
    cleanup: impl IntoIterator<Item = Error>,
) -> Result<T, PipelineError> {
    let mut cleanup = cleanup.into_iter();
    let primary = match result {
        Err(error) => error,
        Ok(value) => match cleanup.next() {
            Some(error) => error,
            None => return Ok(value),
        },
    };
    Err(PipelineError {
        primary,
        cleanup: cleanup.collect(),
    })
}

/// Everything that was brought up, so a failure at any point unwinds exactly that.
#[derive(Default)]
struct Graph {
    common: bool,
    mem: bool,
    cap_init: bool,
    proc_init: bool,
    enc_init: bool,
    cap_ctrl: Option<PathId>,
    cap_io: Option<PathId>,
    proc_ctrl: Option<PathId>,
    proc_out: Vec<PathId>,
    enc: Vec<PathId>,
    cap_proc_bound: bool,
    /// Proc outputs bound to an encoder.
    enc_bound: Vec<u32>,
    cap_started: bool,
    proc_started: Vec<PathId>,
    enc_started: Vec<PathId>,
    /// What the main encoder runs.
    main: Option<EncSpec>,
    /// Why the requested main stream was replaced by the stock one at build.
    main_refused: Option<Error>,
}

impl Graph {
    /// `main` replaces the stock main stream; if the driver refuses it, the stock one is used instead
    /// (recorded in `main_refused`), so a persisted value can never stop the graph from coming up.
    fn build<B: Backend + EncBackend>(
        &mut self,
        hw: &mut B,
        cfg: &PipelineConfig,
        main: Option<&EncSpec>,
        keep_going: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Error> {
        // Record each successful acquisition before observing cancellation again.
        // An in-flight driver call cannot be interrupted; no later call starts after refusal.
        macro_rules! step {
            ($call:expr) => {{
                if !keep_going() {
                    return Ok(false);
                }
                $call?
            }};
        }
        step!(hw.common_init());
        self.common = true;
        step!(hw.common_mem_init(&cfg.cap.pool));
        self.mem = true;
        step!(hw.videocap_init());
        self.cap_init = true;
        step!(hw.videoproc_init());
        self.proc_init = true;
        step!(hw.videoenc_init());
        self.enc_init = true;

        let ctrl = step!(hw.videocap_open());
        self.cap_ctrl = Some(ctrl);
        step!(hw.videocap_set_sensor(ctrl, &cfg.cap.sensor));
        let io = step!(hw.videocap_open_io());
        self.cap_io = Some(io);
        step!(hw.videocap_set_in(io, cfg.cap.dim, cfg.cap.fps));
        step!(hw.videocap_set_out(io, cfg.cap.dim, 0));

        let pctrl = step!(hw.videoproc_open_ctrl());
        self.proc_ctrl = Some(pctrl);
        step!(hw.videoproc_set_dev_config(pctrl, cfg.cap.dim));
        let mut streams = cfg.streams;
        if let Some(m) = main {
            streams[0] = *m;
        }
        for i in 0..streams.len() {
            let s = streams[i];
            let p = step!(hw.videoproc_open_out(i as u32));
            self.proc_out.push(p);
            if !keep_going() {
                return Ok(false);
            }
            match hw.videoproc_set_out(p, s.dim, s.fps) {
                Err(e) if i == 0 && streams[0] != cfg.streams[0] => {
                    self.main_refused = Some(e);
                    streams[0] = cfg.streams[0];
                    step!(hw.videoproc_set_out(p, streams[0].dim, streams[0].fps));
                }
                r => r?,
            }
        }
        for i in 0..streams.len() {
            let s = streams[i];
            let e = step!(hw.videoenc_open(i as u32));
            self.enc.push(e);
            if !keep_going() {
                return Ok(false);
            }
            match hw.videoenc_configure(e, &s, &cfg.envelopes[i]) {
                Err(err) if i == 0 && streams[0] != cfg.streams[0] => {
                    self.main_refused = Some(err);
                    streams[0] = cfg.streams[0];
                    step!(hw.videoproc_set_out(self.proc_out[0], streams[0].dim, streams[0].fps));
                    step!(hw.videoenc_configure(e, &streams[0], &cfg.envelopes[0]));
                }
                r => r?,
            }
        }
        self.main = Some(streams[0]);

        step!(hw.bind_cap_proc());
        self.cap_proc_bound = true;
        for i in 0..cfg.streams.len() as u32 {
            step!(hw.bind_proc_enc(i, i));
            self.enc_bound.push(i);
        }

        // Downstream first, so no frame is produced before its consumer runs.
        for e in self.enc.clone() {
            step!(hw.videoenc_start(e));
            self.enc_started.push(e);
        }
        for p in self.proc_out.clone() {
            step!(hw.videoproc_start(p));
            self.proc_started.push(p);
        }
        step!(hw.videocap_start(io));
        self.cap_started = true;
        Ok(true)
    }

    /// Restarts the main path alone with `spec` (videoproc output size/rate and encoder parameters),
    /// leaving capture, the sub stream and audio running; the first frame after it is a key frame with
    /// the new SPS/PPS. A refused value restores the previous parameters and returns the refusal; an
    /// error while restoring (the path may be left stopped) is the outer error.
    fn retune_main<B: Backend + EncBackend>(
        &mut self,
        hw: &mut B,
        spec: &EncSpec,
    ) -> Result<Option<Error>, Error> {
        let (Some(&p), Some(&e), Some(old)) = (self.proc_out.first(), self.enc.first(), self.main)
        else {
            return Err(Error {
                step: Step::VideoencRetune,
                code: -22,
            });
        };
        if self.proc_started.contains(&p) {
            hw.videoproc_stop(p)?;
            self.proc_started.retain(|x| *x != p);
        }
        if self.enc_started.contains(&e) {
            hw.videoenc_stop(e)?;
            self.enc_started.retain(|x| *x != e);
        }
        let apply = |hw: &mut B, s: &EncSpec| {
            hw.videoproc_set_out(p, s.dim, s.fps)?;
            hw.videoenc_retune(e, s)
        };
        let refused = match apply(hw, spec) {
            Ok(()) => {
                self.main = Some(*spec);
                None
            }
            Err(refusal) => {
                apply(hw, &old)?;
                Some(refusal)
            }
        };
        hw.videoenc_start(e)?;
        self.enc_started.push(e);
        hw.videoproc_start(p)?;
        self.proc_started.push(p);
        Ok(refused)
    }

    /// Reverse of `build`, only for what was done. Failures are collected; they never stop the unwind.
    fn teardown<B: Backend + EncBackend>(mut self, hw: &mut B) -> Vec<Error> {
        let mut errs = Vec::new();
        let mut note = |r: Result<(), Error>| {
            if let Err(e) = r {
                errs.push(e);
            }
        };
        if let (true, Some(io)) = (self.cap_started, self.cap_io) {
            note(hw.videocap_stop(io));
        }
        for p in self.proc_started.drain(..) {
            note(hw.videoproc_stop(p));
        }
        for e in self.enc_started.drain(..) {
            note(hw.videoenc_stop(e));
        }
        if self.cap_proc_bound {
            note(hw.unbind_cap_proc());
        }
        for o in self.enc_bound.drain(..) {
            note(hw.unbind_proc_enc(o));
        }
        for e in self.enc.drain(..) {
            note(hw.videoenc_close(e));
        }
        for p in self.proc_out.drain(..) {
            note(hw.videoproc_close(p));
        }
        if let Some(p) = self.proc_ctrl.take() {
            note(hw.videoproc_close(p));
        }
        if let Some(p) = self.cap_io.take() {
            note(hw.videocap_close(p));
        }
        if let Some(p) = self.cap_ctrl.take() {
            note(hw.videocap_close(p));
        }
        if self.enc_init {
            note(hw.videoenc_uninit());
        }
        if self.proc_init {
            note(hw.videoproc_uninit());
        }
        if self.cap_init {
            note(hw.videocap_uninit());
        }
        if self.mem {
            note(hw.common_mem_uninit());
        }
        if self.common {
            note(hw.common_uninit());
        }
        errs
    }
}

const RECV_WAIT_MS: u32 = 100;
/// After a main-stream change, either stream silent this long reverts the change: the encoders cannot
/// sustain it (the sub stream starves at main 20–30 fps), and a stream silent for 5 s resets
/// the camera through the watchdog's encoder budgets.
pub const STALL_REVERT_MS: u64 = 2000;
/// How long a main-stream change stays on probation.
pub const PROBATION_MS: u64 = 10_000;
/// The refusal a reverted main-stream change reports (`ETIMEDOUT`: a stream stopped delivering).
pub const STALLED: i32 = -110;

/// Builds the graph, writes both streams as Annex B for `seconds`, tears it down and flushes both sinks.
/// Write/flush errors use `videoenc_recv` with code -5 (EIO). Cleanup and both flushes are attempted
/// even after an earlier failure; flushing happens after hardware teardown.
pub fn dump<B, W>(
    hw: &mut B,
    cfg: &PipelineConfig,
    seconds: u32,
    sinks: &mut [AnnexB<W>; 2],
) -> Result<DumpReport, PipelineError>
where
    B: Backend + EncBackend,
    W: Write,
{
    let mut g = Graph::default();
    let ran = (|| {
        g.build(hw, cfg, None, &mut || true)?;
        let start = hw.now_ms();
        let end = start + u64::from(seconds) * 1000;
        let paths = g.enc.clone();
        while hw.now_ms() < end {
            for f in hw.videoenc_recv(&paths, RECV_WAIT_MS)? {
                sinks[f.stream].write(&f).map_err(|_| Error {
                    step: Step::VideoencRecv,
                    code: -5,
                })?;
            }
        }
        Ok(hw.now_ms() - start)
    })();
    let mut cleanup = g.teardown(hw);
    for sink in sinks.iter_mut() {
        if sink.flush().is_err() {
            cleanup.push(Error {
                step: Step::VideoencRecv,
                code: -5,
            });
        }
    }
    let elapsed_ms = complete(ran, cleanup)?;
    Ok(DumpReport {
        elapsed_ms,
        streams: [sinks[0].stats(), sinks[1].stats()],
    })
}

/// Builds the graph and hands every encoded access unit to `on_frame` until `keep_going` returns false (it
/// is polled before startup operations and between receive batches), then tears the graph down.
pub fn stream<B>(
    hw: &mut B,
    cfg: &PipelineConfig,
    keep_going: &mut dyn FnMut() -> bool,
    on_frame: &mut dyn FnMut(EncFrame),
) -> Result<(), PipelineError>
where
    B: Backend + EncBackend,
{
    let mut g = Graph::default();
    let ran = (|| {
        if !g.build(hw, cfg, None, keep_going)? {
            return Ok(());
        }
        let paths = g.enc.clone();
        while keep_going() {
            for f in hw.videoenc_recv(&paths, RECV_WAIT_MS)? {
                on_frame(f);
            }
        }
        Ok(())
    })();
    let cleanup = g.teardown(hw);
    complete(ran, cleanup)
}

/// Applies `want`: speaker volume live on the running output; everything else by restarting the
/// capture path alone (stop, start with the new tuning), which leaves video and the speaker running.
/// The driver only takes AEC/ANR/volume at capture start: live sets were accepted but had no audible
/// effect on the camera (`docs/hardware/audio.md`). A refused start falls back to `current`; if even that fails
/// the capture is down and it is a media error.
fn retune_audio<B: hdal::AudioCap + hdal::AudioOut>(
    hw: &mut B,
    current: Tuning,
    want: Tuning,
    cap: &mut bool,
    tuner: &mut dyn Tuner,
) -> Result<Tuning, Error> {
    let mut applied = current;
    let mut refused = None;
    if want.speaker_volume != current.speaker_volume {
        match hw.audio_out_volume(want.speaker_volume) {
            Ok(()) => applied.speaker_volume = want.speaker_volume,
            Err(e) => refused = Some(e),
        }
    }
    let capture = Tuning {
        speaker_volume: applied.speaker_volume,
        ..want
    };
    let running = Tuning {
        speaker_volume: applied.speaker_volume,
        ..current
    };
    if capture != running {
        *cap = false;
        hw.audio_cap_stop()?;
        match hw.audio_cap_start(&capture) {
            Ok(()) => applied = capture,
            Err(e) => {
                refused = Some(e);
                hw.audio_cap_start(&running)?;
                applied = running;
            }
        }
        *cap = true;
    }
    tuner.report(Tuned::Audio { applied, refused });
    Ok(applied)
}

/// Samples per speaker push: the driver's frame size (`frame_sample_max`).
const SPEAKER_FRAME: usize = 1024;
/// Samples waiting for a full hardware buffer, at most (200 ms at 48 kHz).
const SPEAKER_BACKLOG: usize = 9600;

/// Capture frames and speaker blocks taken per loop turn at most (the capture ring holds ten frames).
const AUDIO_BLOCKS_PER_TURN: usize = 10;

/// Successful submissions are counters, not evidence of physical audibility or echo cancellation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AudioReport {
    pub mic_frames: u64,
    pub mic_samples: u64,
    pub speaker_samples: u64,
    pub speaker_dropped_samples: u64,
}

/// Starts real audio before `on_ready` can advertise it. Hardware polling is bounded to 10 ms for
/// video and nonblocking raw microphone receives (until the ring is empty) per iteration, so talkback stays
/// inside its 200 ms cap.
/// `keep_going` is also checked before each startup operation and before `on_ready`.
/// `on_stopping` must stop and join every service that can access hardware. It runs on normal exit,
/// startup failure and receive/push failure, before any audio/video teardown, even if `on_ready` was
/// never called. It must be safe with no workers and must not panic. A stuck join leaves the graph intact,
/// for the armed production watchdog to reset the boot.
///
/// `tuner` supplies persisted values for startup (a refused one falls back to stock, never failing the
/// start) and is polled once per loop iteration for live changes, which are applied here, on the media
/// thread, between frames. Every outcome is reported back to it.
#[allow(clippy::too_many_arguments)]
pub fn stream_with_audio<B>(
    hw: &mut B,
    cfg: &PipelineConfig,
    on_ready: &mut dyn FnMut(),
    on_stopping: &mut dyn FnMut(),
    keep_going: &mut dyn FnMut() -> bool,
    on_video: &mut dyn FnMut(EncFrame),
    on_audio: &mut dyn FnMut(hdal::audio::PcmFrame),
    next_pcm: &mut dyn FnMut() -> Option<Vec<i16>>,
    tuner: &mut dyn Tuner,
) -> Result<AudioReport, PipelineError>
where
    B: Backend + EncBackend + hdal::AudioCap + hdal::AudioOut,
{
    let mut g = Graph::default();
    let mut report = AudioReport::default();
    let mut speaker: Vec<i16> = Vec::with_capacity(SPEAKER_BACKLOG + SPEAKER_FRAME);
    let (mut speaker_last, mut burst_pushed, mut burst_dropped) = (0u64, 0u64, 0u64);
    let (mut cap, mut out) = (false, false);
    let initial = tuner.initial();
    let ran = (|| {
        let main = initial.main.map(|m| m.spec());
        if !g.build(hw, cfg, main.as_ref(), keep_going)? || !keep_going() {
            return Ok(());
        }
        if let Some(applied) = g.main {
            tuner.report(Tuned::Main {
                applied: MainStream::of(&applied),
                refused: g.main_refused.take(),
                restart_ms: 0,
            });
        }
        let mut tuning = initial.audio.unwrap_or_else(Tuning::stock);
        let mut refused = None;
        if let Err(e) = hw.audio_out_start(tuning.speaker_volume) {
            if tuning.speaker_volume == Tuning::stock().speaker_volume {
                return Err(e);
            }
            refused = Some(e);
            tuning.speaker_volume = Tuning::stock().speaker_volume;
            hw.audio_out_start(tuning.speaker_volume)?;
        }
        out = true;
        if !keep_going() {
            return Ok(());
        }
        if let Err(e) = hw.audio_cap_start(&tuning) {
            let stock = Tuning {
                speaker_volume: tuning.speaker_volume,
                ..Tuning::stock()
            };
            if tuning == stock {
                return Err(e);
            }
            refused = Some(e);
            tuning = stock;
            hw.audio_cap_start(&tuning)?;
        }
        cap = true;
        tuner.report(Tuned::Audio {
            applied: tuning,
            refused,
        });
        if !keep_going() {
            return Ok(());
        }
        on_ready();
        // A main stream other than stock, at startup or after a change, is on probation: the stream
        // it replaced comes back if either stream goes silent.
        let start = hw.now_ms();
        let mut last = [start; 2];
        let mut probation = g
            .main
            .filter(|m| *m != cfg.streams[0])
            .map(|_| (start, cfg.streams[0]));
        while keep_going() {
            for f in hw.videoenc_recv(&g.enc, 10)? {
                if let Some(t) = last.get_mut(f.stream) {
                    *t = hw.now_ms();
                }
                on_video(f);
            }
            if let Some((since, fallback)) = probation {
                let now = hw.now_ms();
                let silent = last
                    .iter()
                    .any(|t| now.saturating_sub((*t).max(since)) > STALL_REVERT_MS);
                if silent {
                    let started = hw.now_ms();
                    let refused = g.retune_main(hw, &fallback)?;
                    probation = None;
                    last = [hw.now_ms(); 2];
                    tuner.report(Tuned::Main {
                        applied: MainStream::of(&g.main.unwrap_or(fallback)),
                        refused: Some(refused.unwrap_or(Error {
                            step: Step::VideoencRecv,
                            code: STALLED,
                        })),
                        restart_ms: hw.now_ms() - started,
                    });
                } else if now.saturating_sub(since) > PROBATION_MS {
                    probation = None;
                }
            }
            if let Some(request) = tuner.next() {
                if let Some(want) = request.audio {
                    tuning = retune_audio(hw, tuning, want, &mut cap, tuner)?;
                }
                if let Some(want) = request.main
                    && g.main.map(|m| MainStream::of(&m)) != Some(want)
                {
                    let started = hw.now_ms();
                    let previous = g.main.unwrap_or(cfg.streams[0]);
                    let refused = g.retune_main(hw, &want.spec())?;
                    if refused.is_none() {
                        probation = Some((hw.now_ms(), previous));
                        last = [hw.now_ms(); 2];
                    }
                    tuner.report(Tuned::Main {
                        applied: MainStream::of(&g.main.unwrap_or(cfg.streams[0])),
                        refused,
                        restart_ms: hw.now_ms() - started,
                    });
                }
            }
            // Raw frames arrive every 21.3 ms, faster than a loop turn can take: drain what the driver's
            // ten-frame ring holds, or it overruns and drops whole stretches of microphone audio.
            for _ in 0..AUDIO_BLOCKS_PER_TURN {
                let Some(f) = hw.audio_cap_recv()? else { break };
                report.mic_frames += 1;
                report.mic_samples += f.samples.len() as u64;
                on_audio(f);
            }
            // The talkback mixer emits a 20 ms block whenever its clock is due; take every due block.
            for _ in 0..AUDIO_BLOCKS_PER_TURN {
                let Some(pcm) = next_pcm() else { break };
                speaker.extend(pcm);
                speaker_last = hw.now_ms();
            }
            // The driver plays whole 1,024-sample frames: shorter pushes lose samples at every frame
            // boundary. A burst's last partial frame is padded once the mixer has gone quiet.
            if !speaker.len().is_multiple_of(SPEAKER_FRAME)
                && hw.now_ms().saturating_sub(speaker_last) > 40
            {
                speaker.resize(speaker.len().next_multiple_of(SPEAKER_FRAME), 0);
            }
            while speaker.len() >= SPEAKER_FRAME {
                if !hw.audio_out_push(&speaker[..SPEAKER_FRAME])? {
                    break;
                }
                report.speaker_samples += SPEAKER_FRAME as u64;
                burst_pushed += SPEAKER_FRAME as u64;
                speaker.drain(..SPEAKER_FRAME);
            }
            // A full hardware buffer keeps at most 200 ms waiting; older samples are dropped.
            let excess = speaker.len().saturating_sub(SPEAKER_BACKLOG);
            if excess > 0 {
                speaker.drain(..excess);
                report.speaker_dropped_samples += excess as u64;
                burst_dropped += excess as u64;
            }
            if burst_pushed + burst_dropped > 0 && hw.now_ms().saturating_sub(speaker_last) > 1000 {
                eprintln!(
                    "speaker: burst ended played_ms={} dropped_ms={}",
                    burst_pushed / 48,
                    burst_dropped / 48
                );
                (burst_pushed, burst_dropped) = (0, 0);
            }
        }
        Ok(())
    })();
    on_stopping();
    let cap_cleanup = if cap { hw.audio_cap_stop() } else { Ok(()) };
    let out_cleanup = if out { hw.audio_out_stop() } else { Ok(()) };
    let cleanup = g.teardown(hw);
    complete(
        ran,
        cap_cleanup
            .err()
            .into_iter()
            .chain(out_cleanup.err())
            .chain(cleanup),
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests;
