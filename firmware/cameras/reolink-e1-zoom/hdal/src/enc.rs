//! The video-processing and encoder half of the HDAL graph (capture → videoproc → videoenc), behind
//! [`EncBackend`] so `cam::pipeline` is unit-tested on the host with [`crate::fake::Fake`]. The capture
//! half and `common_*` stay in [`crate::Backend`].

use crate::{Dim, Error, PathId};

/// Largest access unit copied out of a ring (and accepted by the RTSP hub).
pub const MAX_AU_BYTES: usize = 7 * 1024 * 1024;
/// Largest encoder ring mapped: the main ring sized for 2592×1944 is about 9.4 MiB. The ring is
/// reserved HDAL memory, mapped, not copied; copies stay bounded by [`MAX_AU_BYTES`].
pub const MAX_RING_BYTES: usize = 16 * 1024 * 1024;
/// All rings together.
pub const MAX_RINGS_BYTES: usize = 20 * 1024 * 1024;
/// Largest JPEG a snapshot encoder is sized for.
pub const MAX_JPEG_BYTES: u32 = 1024 * 1024;

/// One H.264 Main-profile CBR stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncSpec {
    pub dim: Dim,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// I-frame period in frames.
    pub gop: u32,
}

/// One access unit as the encoder returned it: Annex B, packs concatenated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncFrame {
    /// Index into the path list given to [`EncBackend::videoenc_recv`].
    pub stream: usize,
    pub data: Vec<u8>,
    pub key: bool,
    pub pts_us: u64,
}

pub trait EncBackend {
    fn videoproc_init(&mut self) -> Result<(), Error>;
    /// Opens `HD_VIDEOPROC_0_CTRL`.
    fn videoproc_open_ctrl(&mut self) -> Result<PathId, Error>;
    /// `DEV_CONFIG`: the RAW-to-YUV pipe for a sensor frame of `cap_dim`.
    fn videoproc_set_dev_config(&mut self, ctrl: PathId, cap_dim: Dim) -> Result<(), Error>;
    /// Opens `HD_VIDEOPROC_0_IN_0` → `OUT_<out>`.
    fn videoproc_open_out(&mut self, out: u32) -> Result<PathId, Error>;
    /// `HD_VIDEOPROC_PARAM_OUT`: YUV420 of `dim` at `fps`.
    fn videoproc_set_out(&mut self, path: PathId, dim: Dim, fps: u32) -> Result<(), Error>;
    fn videoproc_start(&mut self, path: PathId) -> Result<(), Error>;
    fn videoproc_stop(&mut self, path: PathId) -> Result<(), Error>;
    fn videoproc_close(&mut self, path: PathId) -> Result<(), Error>;
    fn videoproc_uninit(&mut self) -> Result<(), Error>;

    fn videoenc_init(&mut self) -> Result<(), Error>;
    /// Opens `HD_VIDEOENC_0_IN_<idx>` → `OUT_<idx>`.
    fn videoenc_open(&mut self, idx: u32) -> Result<PathId, Error>;
    /// Path config (buffers, sized for `envelope`: the largest size and bitrate this path may later be
    /// retuned to), input, codec parameters and rate control of one encoder.
    fn videoenc_configure(
        &mut self,
        path: PathId,
        spec: &EncSpec,
        envelope: &EncSpec,
    ) -> Result<(), Error>;
    /// Input, codec parameters and rate control again, leaving the path config (and so the mapped
    /// bitstream ring) alone. `spec` must fit the configured envelope.
    fn videoenc_retune(&mut self, path: PathId, spec: &EncSpec) -> Result<(), Error>;
    fn videoenc_start(&mut self, path: PathId) -> Result<(), Error>;
    fn videoenc_stop(&mut self, path: PathId) -> Result<(), Error>;
    fn videoenc_close(&mut self, path: PathId) -> Result<(), Error>;
    fn videoenc_uninit(&mut self) -> Result<(), Error>;

    /// `videocap OUT_0 → videoproc IN_0`.
    fn bind_cap_proc(&mut self) -> Result<(), Error>;
    /// `videoproc OUT_<out> → videoenc IN_<enc>`.
    fn bind_proc_enc(&mut self, out: u32, enc: u32) -> Result<(), Error>;
    fn unbind_cap_proc(&mut self) -> Result<(), Error>;
    fn unbind_proc_enc(&mut self, out: u32) -> Result<(), Error>;

    /// Waits up to `wait_ms` for bitstreams on `paths`, copies every ready access unit out and releases it.
    fn videoenc_recv(&mut self, paths: &[PathId], wait_ms: u32) -> Result<Vec<EncFrame>, Error>;
    /// Monotonic milliseconds.
    fn now_ms(&mut self) -> u64;
    /// Path config, input and codec parameters of a JPEG encoder: one picture of `dim` per input frame at
    /// `quality` (1–100), each at most [`MAX_JPEG_BYTES`].
    fn videoenc_configure_jpeg(
        &mut self,
        path: PathId,
        dim: Dim,
        quality: u32,
    ) -> Result<(), Error>;
}
