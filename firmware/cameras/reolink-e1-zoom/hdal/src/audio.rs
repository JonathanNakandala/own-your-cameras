//! Audio shares the video graph's common HDAL/memory lifetime. Start the speaker first so capture's
//! AEC has a loopback source; stop both before common memory is released.
use crate::Error;

/// The codec rate of both directions. Capture and playback share one codec clock: with the speaker opened at
/// 16 kHz, a 48 kHz capture delivers frames at the 16 kHz rate, so both run at 48 kHz.
pub const RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    /// Time of the first sample.
    pub pts_us: u64,
    /// Signed 16-bit, [`RATE`] mono; 1,024 samples per driver frame.
    pub samples: Vec<i16>,
}

/// Acoustic echo cancellation as `HD_AUDIOCAP_AEC` carries it, without the buffer-sizing fields
/// (filter length, frame size, loopback channel) that stay fixed inside the `aec_max` envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aec {
    pub enabled: bool,
    pub leak_estimate: bool,
    /// 25..=99.
    pub leak_value: i32,
    /// dB, -60..=-3.
    pub noise_level: i32,
    /// dB, -60..=-30.
    pub echo_level: i32,
    /// 0..=1000.
    pub notch_radius: i32,
}

/// Audio noise reduction as `HD_AUDIOCAP_ANR` carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anr {
    pub enabled: bool,
    pub suppress_level: i32,
    /// Hz.
    pub hpf_cutoff: i32,
    pub bias_sensitive: i32,
}

/// Everything about the microphone and speaker that may change while the graph runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    pub aec: Aec,
    pub anr: Anr,
    /// Capture gain, 0..=100 (`HD_AUDIOCAP_PARAM_VOLUME`, ctrl path).
    pub mic_volume: u8,
    /// Playback gain, 0..=100 (`HD_AUDIOOUT_PARAM_VOLUME`, ctrl path).
    pub speaker_volume: u8,
}

impl Tuning {
    /// AEC and ANR on with the stock parameters, speaker at 100. Mic volume is the stock `device`'s 80 (`dvr.xml` `default_ai_volume`).
    /// These are also the `DEV_CONFIG` `aec_max`/`anr_max` envelope.
    pub const fn stock() -> Self {
        Self {
            aec: Aec {
                enabled: true,
                leak_estimate: false,
                leak_value: 99,
                noise_level: -60,
                echo_level: -60,
                notch_radius: 992,
            },
            anr: Anr {
                enabled: true,
                suppress_level: 30,
                hpf_cutoff: 160,
                bias_sensitive: 9,
            },
            mic_volume: 80,
            speaker_volume: 100,
        }
    }
}

pub trait AudioCap {
    /// Atomic startup with `tuning` (speaker volume excepted): on failure release everything
    /// acquired by this call.
    fn audio_cap_start(&mut self, tuning: &Tuning) -> Result<(), Error>;
    /// Nonblocking; copies and releases at most one raw capture frame.
    fn audio_cap_recv(&mut self) -> Result<Option<PcmFrame>, Error>;
    /// Attempt every cleanup step even when one fails.
    fn audio_cap_stop(&mut self) -> Result<(), Error>;
}

pub trait AudioOut {
    /// Atomic startup within an already initialised common HDAL/memory context.
    fn audio_out_start(&mut self, volume: u8) -> Result<(), Error>;
    /// Sets the playback gain (0..=100) on the running speaker.
    fn audio_out_volume(&mut self, volume: u8) -> Result<(), Error>;
    /// Signed 16-bit, [`RATE`] mono, at most 1,024 samples. Never waits for speaker space.
    /// `false` means the bounded hardware buffer is full and this block was dropped.
    fn audio_out_push(&mut self, pcm: &[i16]) -> Result<bool, Error>;
    fn audio_out_stop(&mut self) -> Result<(), Error>;
}
