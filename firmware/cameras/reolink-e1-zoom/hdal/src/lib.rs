//! Safe HDAL wrapper: every hardware call sits behind [`Backend`], so the order of calls and the
//! clean-up on failure are unit-tested on the host with [`fake::Fake`].
//!
//! HDAL's `hd_common_init`/`uninit` act on process-global kernel state: no call may be made while the vendor
//! `device` runs. Callers stop the vendor stack first and reboot afterwards.

pub mod audio;
pub mod codec;
pub use audio::{AudioCap, AudioOut};
pub mod enc;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
mod probe;
pub use probe::{ProbeConfig, ProbeError, ProbeOutput, probe_videocap};
#[cfg(any(target_arch = "arm", test))]
pub mod sys;

/// An HDAL path id returned by `hd_*_open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathId(pub u32);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dim {
    pub w: u32,
    pub h: u32,
}

/// One block pool for `hd_common_mem_init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemPool {
    pub blk_size: u32,
    pub blk_cnt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorConfig {
    pub driver_name: String,
}

/// `HD_VIDEOCAP_SYSINFO`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoCapSysInfo {
    pub dev_id: u32,
    pub cur_fps: [u32; 8],
    pub vd_count: u64,
    pub output_started: bool,
    pub cur_dim: Dim,
}

impl VideoCapSysInfo {
    /// Frame rate of output `i`: HDAL packs numerator in the high and denominator in the low 16 bits.
    pub fn fps(&self, i: usize) -> f32 {
        frc_to_fps(self.cur_fps[i])
    }
}

/// A packed `HD_VIDEO_FRC` (numerator high, denominator low) as frames per second.
pub fn frc_to_fps(v: u32) -> f32 {
    let (num, den) = (v >> 16, v & 0xffff);
    if den == 0 {
        0.0
    } else {
        num as f32 / den as f32
    }
}

/// `HD_VIDEO_FRC_RATIO(dst, src)`: destination in the high half, reduced by the common factor. HDAL's
/// frame-rate controller drops an extra frame about every tenth output for an unreduced 10:30 but is exact
/// for 1:3 (measured on the camera).
pub fn frc_ratio(dst: u32, src: u32) -> u32 {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(dst, src).max(1);
    ((dst / g) << 16) | ((src / g) & 0xffff)
}

/// `HD_VIDEOCAP_SYSCAPS`, the fields worth printing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoCapSysCaps {
    pub dev_id: u32,
    pub chip_id: u32,
    pub max_in_count: u32,
    pub max_out_count: u32,
    pub max_dim: Dim,
    pub max_frame_rate: u32,
    pub max_w_scaleup: u32,
}

/// The HDAL calls of the probe, in forward order then clean-up order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    CommonInit,
    CommonMemInit,
    VideocapInit,
    VideocapOpen,
    VideocapSetSensor,
    VideocapOpenIo,
    VideocapSetIn,
    VideocapSetOut,
    VideocapStart,
    VideocapStop,
    VideocapGetSyscaps,
    VideocapGetSysinfo,
    VideocapClose,
    VideocapUninit,
    CommonMemUninit,
    CommonUninit,
    VideoprocInit,
    VideoprocOpen,
    VideoprocSetDevConfig,
    VideoprocSetOut,
    VideoprocStart,
    VideoprocStop,
    VideoprocClose,
    VideoprocUninit,
    VideoencInit,
    VideoencOpen,
    VideoencConfigure,
    VideoencRetune,
    VideoencStart,
    VideoencStop,
    VideoencClose,
    VideoencUninit,
    VideoencRecv,
    VideoencConfigureJpeg,
    BindCapProc,
    BindProcEnc,
    Unbind,
    AudioCapStart,
    AudioCapRecv,
    AudioCapStop,
    AudioOutStart,
    AudioOutVolume,
    AudioOutPush,
    AudioOutStop,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Step::CommonInit => "common_init",
            Step::CommonMemInit => "common_mem_init",
            Step::VideocapInit => "videocap_init",
            Step::VideocapOpen => "videocap_open",
            Step::VideocapSetSensor => "videocap_set_sensor",
            Step::VideocapOpenIo => "videocap_open_io",
            Step::VideocapSetIn => "videocap_set_in",
            Step::VideocapSetOut => "videocap_set_out",
            Step::VideocapStart => "videocap_start",
            Step::VideocapStop => "videocap_stop",
            Step::VideocapGetSyscaps => "videocap_get_syscaps",
            Step::VideocapGetSysinfo => "videocap_get_sysinfo",
            Step::VideocapClose => "videocap_close",
            Step::VideocapUninit => "videocap_uninit",
            Step::CommonMemUninit => "common_mem_uninit",
            Step::CommonUninit => "common_uninit",
            Step::VideoprocInit => "videoproc_init",
            Step::VideoprocOpen => "videoproc_open",
            Step::VideoprocSetDevConfig => "videoproc_set_dev_config",
            Step::VideoprocSetOut => "videoproc_set_out",
            Step::VideoprocStart => "videoproc_start",
            Step::VideoprocStop => "videoproc_stop",
            Step::VideoprocClose => "videoproc_close",
            Step::VideoprocUninit => "videoproc_uninit",
            Step::VideoencInit => "videoenc_init",
            Step::VideoencOpen => "videoenc_open",
            Step::VideoencConfigure => "videoenc_configure",
            Step::VideoencRetune => "videoenc_retune",
            Step::VideoencStart => "videoenc_start",
            Step::VideoencStop => "videoenc_stop",
            Step::VideoencClose => "videoenc_close",
            Step::VideoencUninit => "videoenc_uninit",
            Step::VideoencRecv => "videoenc_recv",
            Step::VideoencConfigureJpeg => "videoenc_configure_jpeg",
            Step::BindCapProc => "bind_cap_proc",
            Step::BindProcEnc => "bind_proc_enc",
            Step::Unbind => "unbind",
            Step::AudioCapStart => "audio_cap_start",
            Step::AudioCapRecv => "audio_cap_recv",
            Step::AudioCapStop => "audio_cap_stop",
            Step::AudioOutStart => "audio_out_start",
            Step::AudioOutVolume => "audio_out_volume",
            Step::AudioOutPush => "audio_out_push",
            Step::AudioOutStop => "audio_out_stop",
        }
    }
}

/// An HDAL call that returned a non-zero `HD_RESULT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("hd_{} failed: HD_RESULT {code}", step.name())]
/// Syscall failures captured below HDAL use code `-65536 - errno`.
/// HDAL status values are otherwise retained unchanged.
pub struct Error {
    pub step: Step,
    pub code: i32,
}

/// The HDAL calls the probe needs. The real one is `sys::Sys`.
pub trait Backend {
    fn common_init(&mut self) -> Result<(), Error>;
    fn common_mem_init(&mut self, pool: &MemPool) -> Result<(), Error>;
    fn videocap_init(&mut self) -> Result<(), Error>;
    /// Opens the control path (`HD_VIDEOCAP_0_CTRL`).
    fn videocap_open(&mut self) -> Result<PathId, Error>;
    fn videocap_set_sensor(&mut self, path: PathId, sensor: &SensorConfig) -> Result<(), Error>;
    /// Opens the input 0 / output 0 path (`HD_VIDEOCAP_0_IN_0`, `HD_VIDEOCAP_0_OUT_0`).
    fn videocap_open_io(&mut self) -> Result<PathId, Error>;
    /// `HD_VIDEOCAP_PARAM_IN` with automatic sensor-mode selection for `dim` at `fps`.
    fn videocap_set_in(&mut self, path: PathId, dim: Dim, fps: u32) -> Result<(), Error>;
    /// `HD_VIDEOCAP_PARAM_OUT`: raw output of `dim`, queue depth 1.
    /// `depth` 0 hands frames straight to the bound consumer; above 0 they queue for `pull_out_buf`.
    fn videocap_set_out(&mut self, path: PathId, dim: Dim, depth: u32) -> Result<(), Error>;
    fn videocap_start(&mut self, path: PathId) -> Result<(), Error>;
    fn videocap_stop(&mut self, path: PathId) -> Result<(), Error>;
    fn videocap_get_syscaps(&mut self, path: PathId) -> Result<VideoCapSysCaps, Error>;
    /// Pause between SYSINFO polls.
    fn sleep_ms(&mut self, ms: u32);
    fn videocap_get_sysinfo(&mut self, path: PathId) -> Result<VideoCapSysInfo, Error>;
    fn videocap_close(&mut self, path: PathId) -> Result<(), Error>;
    fn videocap_uninit(&mut self) -> Result<(), Error>;
    fn common_mem_uninit(&mut self) -> Result<(), Error>;
    fn common_uninit(&mut self) -> Result<(), Error>;
}

#[cfg(test)]
mod frc_tests {
    use super::frc_ratio;

    #[test]
    fn ratios_are_reduced_with_the_destination_in_the_high_half() {
        assert_eq!(frc_ratio(10, 30), (1 << 16) | 3);
        assert_eq!(frc_ratio(15, 30), (1 << 16) | 2);
        assert_eq!(frc_ratio(1, 1), (1 << 16) | 1);
        assert_eq!(frc_ratio(30, 30), (1 << 16) | 1);
    }
}
