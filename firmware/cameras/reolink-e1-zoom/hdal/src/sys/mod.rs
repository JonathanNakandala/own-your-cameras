//! The real backend: `libhdal.so` through `hdal-sys`. Only built for the camera (armv7).
//!
//! Every call is a plain C function over structs that live on this stack for the call's duration.

// Bindgen's field types are plain integers; the casts keep the code right if a header changes a width, and
// the structs are filled field by field the way the SDK's own samples do.
#![allow(clippy::unnecessary_cast, clippy::field_reassign_with_default)]

mod audio;
pub mod ioctl_capture;
#[cfg(test)]
mod test_ffi;
#[cfg(test)]
mod tests;

#[cfg(not(test))]
use hdal_sys as h;
#[cfg(test)]
use test_ffi as h;

use crate::enc::{EncBackend, EncFrame, EncSpec};
use crate::frc_ratio;
use crate::{
    Backend, Dim, Error, MemPool, PathId, SensorConfig, Step, VideoCapSysCaps, VideoCapSysInfo,
};

/// Sensor 0 on the E1 Zoom, as the SDK's MIPI-CSI samples for NA51089 wire it. The kernel already has the
/// sensor driver loaded (`/mnt/src/sensor/sen_os05a10_56x.cfg`); this only tells HDAL where it sits.
const SENSOR_PINMUX: u32 = 0x220;
const SERIAL_IF_PINMUX: u32 = 0xF01;
const CMD_IF_PINMUX: u32 = 0x01;

/// A bitstream buffer of one encoder, mapped into this process.
struct EncMap {
    path: PathId,
    phy: usize,
    size: usize,
    va: *mut u8,
}

/// One HDAL session per process. Common init/uninit changes global kernel state;
/// recreating this object is not recovery. Callers must reboot after a session.
pub struct Sys {
    session: std::sync::Arc<std::sync::atomic::AtomicBool>,
    common: bool,
    terminal: bool,
    // common memory, capture, processing, encoder units
    initialized: [bool; 4],
    audio_cap: audio::State,
    audio_out: audio::State,
    speaker_enable: Option<audio::SpeakerEnable>,
    mic_gain: Option<audio::MicGain>,
    t0: std::time::Instant,
    cap_fps: u32,
    maps: Vec<EncMap>,
}

impl Sys {
    pub fn new() -> Self {
        Self {
            session: session_latch(),
            common: false,
            terminal: false,
            initialized: [false; 4],
            audio_cap: Default::default(),
            audio_out: Default::default(),
            speaker_enable: None,
            mic_gain: None,
            t0: std::time::Instant::now(),
            cap_fps: 30,
            maps: Vec::new(),
        }
    }

    /// Board amplifier control, supplied by camera wiring. Errors are negative errno values.
    /// Audio startup refuses to advertise a speaker without this physical output dependency.
    pub fn with_speaker_enable(
        mut self,
        enable: impl FnMut(bool) -> Result<(), i32> + Send + 'static,
    ) -> Self {
        self.speaker_enable = Some(Box::new(enable));
        self
    }

    /// The codec's record gain below HDAL ([`crate::codec`]), supplied by camera wiring and given
    /// `mic_volume` after every capture start. Errors are negative errno values.
    pub fn with_mic_gain(
        mut self,
        gain: impl FnMut(u8) -> Result<(), i32> + Send + 'static,
    ) -> Self {
        self.mic_gain = Some(Box::new(gain));
        self
    }
}

// Production shares one atomic across every thread. Each host driver fixture supplies
// its own shared atomic so unrelated simulated cameras remain isolated.
fn session_latch() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    #[cfg(test)]
    return h::DRIVER.with_borrow(|d| d.session.clone());
    #[cfg(not(test))]
    {
        static SESSION: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
            std::sync::OnceLock::new();
        SESSION.get_or_init(Default::default).clone()
    }
}

impl Sys {
    fn no_buffers(&self, step: Step) -> Result<(), Error> {
        if self.maps.is_empty() && !self.audio_cap.live() && !self.audio_out.live() {
            Ok(())
        } else {
            Err(Error {
                step,
                code: h::HD_ERR_IN_USE,
            })
        }
    }

    fn forward(&self, step: Step) -> Result<(), Error> {
        self.owned(step)?;
        if self.terminal {
            Err(Error {
                step,
                code: h::HD_ERR_TERM,
            })
        } else {
            Ok(())
        }
    }

    fn cleanup(&mut self, step: Step, rc: h::HD_RESULT) -> Result<(), Error> {
        let result = check(step, rc);
        self.terminal |= result.is_err();
        result
    }

    fn owned(&self, step: Step) -> Result<(), Error> {
        if self.common {
            Ok(())
        } else {
            Err(Error {
                step,
                code: h::HD_ERR_STATE,
            })
        }
    }
}

impl Default for Sys {
    fn default() -> Self {
        Self::new()
    }
}

fn check(step: Step, r: h::HD_RESULT) -> Result<(), Error> {
    if r as i32 == 0 {
        Ok(())
    } else {
        Err(Error {
            step,
            code: r as i32,
        })
    }
}

/// Reads the diagnostic clock through the same syscall capture as graph calls.
///
/// # Safety
/// `gettime` must be the pinned HDAL `UINT32 hd_gettime_ms(void)` export,
/// resolved from a library handle that remains live through this call.
pub unsafe fn read_time_ms(gettime: unsafe extern "C" fn() -> u32) -> Result<u32, i32> {
    // SAFETY: the caller guarantees the no-argument HDAL timer signature and lifetime.
    let (time, failure) = ioctl_capture::run(|| unsafe { gettime() });
    match failure {
        Some(failure) => Err(failure.code()),
        None => Ok(time),
    }
}

fn driver_name(name: &str) -> [h::ctypes::c_char; h::HD_VIDEOCAP_SEN_NAME_LEN as usize] {
    let mut out = [0; h::HD_VIDEOCAP_SEN_NAME_LEN as usize];
    // Keep the trailing NUL: HDAL reads this as a C string.
    for (d, s) in out
        .iter_mut()
        .zip(name.bytes().take(h::HD_VIDEOCAP_SEN_NAME_LEN as usize - 1))
    {
        *d = s as h::ctypes::c_char;
    }
    out
}

impl Backend for Sys {
    fn common_init(&mut self) -> Result<(), Error> {
        use std::sync::atomic::Ordering;
        // Reserve before the C call, including its failure path: failed init can have
        // touched global state. Never release the latch, even on Drop or clean uninit.
        if self.session.swap(true, Ordering::AcqRel) {
            return Err(Error {
                step: Step::CommonInit,
                code: h::HD_ERR_INIT,
            });
        }
        // SAFETY: exclusive process owner; vendor shutdown is still an operator precondition.
        check(
            Step::CommonInit,
            ioctl_capture::result(|| unsafe { h::hd_common_init(0) }),
        )?;
        self.common = true;
        Ok(())
    }

    fn common_mem_init(&mut self, pool: &MemPool) -> Result<(), Error> {
        self.forward(Step::CommonMemInit)?;
        if self.initialized[0] {
            return Err(Error {
                step: Step::CommonMemInit,
                code: h::HD_ERR_INIT,
            });
        }
        self.no_buffers(Step::CommonMemInit)?;
        let mut cfg = h::HD_COMMON_MEM_INIT_CONFIG::default();
        cfg.pool_info[0].ddr_id = h::DDR_ID0;
        cfg.pool_info[0].type_ = h::HD_COMMON_MEM_COMMON_POOL;
        cfg.pool_info[0].blk_size = pool.blk_size;
        cfg.pool_info[0].blk_cnt = pool.blk_cnt;
        // SAFETY: `cfg` is a valid, initialised config that outlives the call.
        check(
            Step::CommonMemInit,
            ioctl_capture::result(|| unsafe { h::hd_common_mem_init(&mut cfg) }),
        )?;
        self.initialized[0] = true;
        Ok(())
    }

    fn videocap_init(&mut self) -> Result<(), Error> {
        self.forward(Step::VideocapInit)?;
        if self.initialized[1] {
            return Err(Error {
                step: Step::VideocapInit,
                code: h::HD_ERR_INIT,
            });
        }
        // SAFETY: no arguments.
        check(
            Step::VideocapInit,
            ioctl_capture::result(|| unsafe { h::hd_videocap_init() }),
        )?;
        self.initialized[1] = true;
        Ok(())
    }

    fn videocap_open(&mut self) -> Result<PathId, Error> {
        self.forward(Step::VideocapOpen)?;
        let mut path: h::HD_PATH_ID = 0;
        // SAFETY: `path` is a valid out pointer for the call.
        check(
            Step::VideocapOpen,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_open(0, h::HD_VIDEOCAP_0_CTRL as _, &mut path)
            }),
        )?;
        Ok(PathId(path as u32))
    }

    fn videocap_set_sensor(&mut self, path: PathId, sensor: &SensorConfig) -> Result<(), Error> {
        self.forward(Step::VideocapSetSensor)?;
        let mut cfg = h::HD_VIDEOCAP_DRV_CONFIG::default();
        let dev = &mut cfg.sen_cfg.sen_dev;
        dev.driver_name = driver_name(&sensor.driver_name);
        dev.if_type = h::HD_COMMON_VIDEO_IN_MIPI_CSI;
        dev.pin_cfg.pinmux.sensor_pinmux = SENSOR_PINMUX;
        dev.pin_cfg.pinmux.serial_if_pinmux = SERIAL_IF_PINMUX;
        dev.pin_cfg.pinmux.cmd_if_pinmux = CMD_IF_PINMUX;
        dev.pin_cfg.clk_lane_sel = h::HD_VIDEOCAP_SEN_CLANE_SEL_CSI0_USE_C0;
        for (i, m) in dev.pin_cfg.sen_2_serial_pin_map.iter_mut().enumerate() {
            *m = if i < 2 {
                i as u32
            } else {
                h::HD_VIDEOCAP_SEN_IGNORE
            };
        }
        // SAFETY: `cfg` is a `HD_VIDEOCAP_DRV_CONFIG`, the struct this param id takes (abi/ table: 148 B).
        check(
            Step::VideocapSetSensor,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_set(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_DRV_CONFIG,
                    (&mut cfg as *mut h::HD_VIDEOCAP_DRV_CONFIG).cast(),
                )
            }),
        )?;
        // Without the 3A functions on the control path the ISP logs `force disable AE/AWB` every frame, the
        // exposure never adapts and the kernel AF never runs. The SDK samples set this right after the driver.
        let mut ctl = h::HD_VIDEOCAP_CTRL {
            func: h::HD_VIDEOCAP_FUNC_AE | h::HD_VIDEOCAP_FUNC_AWB | h::HD_VIDEOCAP_FUNC_AF,
        };
        // SAFETY: `ctl` is a `HD_VIDEOCAP_CTRL`, the struct this param id takes (abi/ table: 4 B).
        check(
            Step::VideocapSetSensor,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_set(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_CTRL,
                    (&mut ctl as *mut h::HD_VIDEOCAP_CTRL).cast(),
                )
            }),
        )
    }

    fn videocap_open_io(&mut self) -> Result<PathId, Error> {
        self.forward(Step::VideocapOpenIo)?;
        let mut path: h::HD_PATH_ID = 0;
        // SAFETY: `path` is a valid out pointer for the call.
        check(
            Step::VideocapOpenIo,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_open(
                    h::HD_VIDEOCAP_0_IN_0 as _,
                    h::HD_VIDEOCAP_0_OUT_0 as _,
                    &mut path,
                )
            }),
        )?;
        Ok(PathId(path as u32))
    }

    fn videocap_set_in(&mut self, path: PathId, dim: Dim, fps: u32) -> Result<(), Error> {
        self.forward(Step::VideocapSetIn)?;
        let mut cfg = h::HD_VIDEOCAP_IN::default();
        cfg.sen_mode = h::HD_VIDEOCAP_SEN_MODE_AUTO;
        // HD_VIDEO_FRC_RATIO(dst_fr, src_fr) = MAKE_UINT16_UINT16: dst in the high half.
        cfg.frc = (fps << 16) | 1;
        self.cap_fps = fps;
        cfg.dim.w = dim.w;
        cfg.dim.h = dim.h;
        cfg.pxlfmt = h::HD_VIDEO_PXLFMT_RAW10;
        cfg.out_frame_num = h::HD_VIDEOCAP_SEN_FRAME_NUM_1;
        // SAFETY: `cfg` is a `HD_VIDEOCAP_IN`, the struct this param id takes (abi/ table: 24 B).
        check(
            Step::VideocapSetIn,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_set(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_IN,
                    (&mut cfg as *mut h::HD_VIDEOCAP_IN).cast(),
                )
            }),
        )
    }

    fn videocap_set_out(&mut self, path: PathId, dim: Dim, depth: u32) -> Result<(), Error> {
        self.forward(Step::VideocapSetOut)?;
        let mut cfg = h::HD_VIDEOCAP_OUT::default();
        cfg.dim.w = dim.w;
        cfg.dim.h = dim.h;
        cfg.pxlfmt = h::HD_VIDEO_PXLFMT_RAW10;
        cfg.depth = depth;
        // SAFETY: `cfg` is a `HD_VIDEOCAP_OUT`, the struct this param id takes (abi/ table: 24 B).
        check(
            Step::VideocapSetOut,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_set(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_OUT,
                    (&mut cfg as *mut h::HD_VIDEOCAP_OUT).cast(),
                )
            }),
        )
    }

    fn videocap_start(&mut self, path: PathId) -> Result<(), Error> {
        self.forward(Step::VideocapStart)?;
        // SAFETY: plain id argument.
        check(
            Step::VideocapStart,
            ioctl_capture::result(|| unsafe { h::hd_videocap_start(path.0 as _) }),
        )
    }

    fn videocap_stop(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideocapStop)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::VideocapStop,
            ioctl_capture::result(|| unsafe { h::hd_videocap_stop(path.0 as _) }),
        )
    }

    fn sleep_ms(&mut self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(ms.into()));
    }

    fn videocap_get_syscaps(&mut self, path: PathId) -> Result<VideoCapSysCaps, Error> {
        self.forward(Step::VideocapGetSyscaps)?;
        let mut c = h::HD_VIDEOCAP_SYSCAPS::default();
        // SAFETY: `c` is a `HD_VIDEOCAP_SYSCAPS`, the struct this param id fills (abi/ table: 76 B).
        check(
            Step::VideocapGetSyscaps,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_get(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_SYSCAPS,
                    (&mut c as *mut h::HD_VIDEOCAP_SYSCAPS).cast(),
                )
            }),
        )?;
        Ok(VideoCapSysCaps {
            dev_id: c.dev_id as u32,
            chip_id: c.chip_id,
            max_in_count: c.max_in_count,
            max_out_count: c.max_out_count,
            max_dim: Dim {
                w: c.max_dim.w,
                h: c.max_dim.h,
            },
            max_frame_rate: c.max_frame_rate,
            max_w_scaleup: c.max_w_scaleup,
        })
    }

    fn videocap_get_sysinfo(&mut self, path: PathId) -> Result<VideoCapSysInfo, Error> {
        self.forward(Step::VideocapGetSysinfo)?;
        let mut i = h::HD_VIDEOCAP_SYSINFO::default();
        // SAFETY: `i` is a `HD_VIDEOCAP_SYSINFO`, the struct this param id fills (abi/ table: 64 B).
        check(
            Step::VideocapGetSysinfo,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_get(
                    path.0 as _,
                    h::HD_VIDEOCAP_PARAM_SYSINFO,
                    (&mut i as *mut h::HD_VIDEOCAP_SYSINFO).cast(),
                )
            }),
        )?;
        Ok(VideoCapSysInfo {
            dev_id: i.dev_id as u32,
            cur_fps: i.cur_fps,
            vd_count: i.vd_count,
            output_started: i.output_started != 0,
            cur_dim: Dim {
                w: i.cur_dim.w,
                h: i.cur_dim.h,
            },
        })
    }

    fn videocap_close(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideocapClose)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::VideocapClose,
            ioctl_capture::result(|| unsafe { h::hd_videocap_close(path.0 as _) }),
        )
    }

    fn videocap_uninit(&mut self) -> Result<(), Error> {
        self.owned(Step::VideocapUninit)?;
        // SAFETY: no arguments.
        let result = self.cleanup(
            Step::VideocapUninit,
            ioctl_capture::result(|| unsafe { h::hd_videocap_uninit() }),
        );
        self.initialized[1] = false;
        result
    }

    fn common_mem_uninit(&mut self) -> Result<(), Error> {
        self.owned(Step::CommonMemUninit)?;
        self.no_buffers(Step::CommonMemUninit)?;
        if self.initialized[1..].iter().any(|live| *live) {
            return Err(Error {
                step: Step::CommonMemUninit,
                code: h::HD_ERR_IN_USE,
            });
        }
        // SAFETY: no arguments.
        let result = self.cleanup(
            Step::CommonMemUninit,
            ioctl_capture::result(|| unsafe { h::hd_common_mem_uninit() }),
        );
        self.initialized[0] = false;
        result
    }

    fn common_uninit(&mut self) -> Result<(), Error> {
        self.owned(Step::CommonUninit)?;
        self.no_buffers(Step::CommonUninit)?;
        if self.initialized.iter().any(|live| *live) {
            return Err(Error {
                step: Step::CommonUninit,
                code: h::HD_ERR_IN_USE,
            });
        }
        // SAFETY: no arguments.
        let result = self.cleanup(
            Step::CommonUninit,
            ioctl_capture::result(|| unsafe { h::hd_common_uninit() }),
        );
        self.common = false;
        result
    }
}

const PROC_OUT: [u32; 3] = [
    h::HD_VIDEOPROC_0_OUT_0 as u32,
    h::HD_VIDEOPROC_0_OUT_1 as u32,
    h::HD_VIDEOPROC_0_OUT_2 as u32,
];
const ENC_IN: [u32; 3] = [
    h::HD_VIDEOENC_0_IN_0 as u32,
    h::HD_VIDEOENC_0_IN_1 as u32,
    h::HD_VIDEOENC_0_IN_2 as u32,
];
const ENC_OUT: [u32; 3] = [
    h::HD_VIDEOENC_0_OUT_0 as u32,
    h::HD_VIDEOENC_0_OUT_1 as u32,
    h::HD_VIDEOENC_0_OUT_2 as u32,
];
const NO_ID: u32 = 0xffff_ffff;

fn slot(table: &[u32; 3], i: u32, step: Step) -> Result<u32, Error> {
    table
        .get(i as usize)
        .copied()
        .ok_or(Error { step, code: -1 })
}

impl EncBackend for Sys {
    fn videoproc_init(&mut self) -> Result<(), Error> {
        self.forward(Step::VideoprocInit)?;
        if self.initialized[2] {
            return Err(Error {
                step: Step::VideoprocInit,
                code: h::HD_ERR_INIT,
            });
        }
        // SAFETY: no arguments.
        check(
            Step::VideoprocInit,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_init() }),
        )?;
        self.initialized[2] = true;
        Ok(())
    }

    fn videoproc_open_ctrl(&mut self) -> Result<PathId, Error> {
        self.forward(Step::VideoprocOpen)?;
        let mut path: h::HD_PATH_ID = 0;
        // SAFETY: `path` is a valid out pointer for the call.
        check(
            Step::VideoprocOpen,
            ioctl_capture::result(|| unsafe {
                h::hd_videoproc_open(0, h::HD_VIDEOPROC_0_CTRL as _, &mut path)
            }),
        )?;
        Ok(PathId(path as u32))
    }

    fn videoproc_set_dev_config(&mut self, ctrl: PathId, cap_dim: Dim) -> Result<(), Error> {
        self.forward(Step::VideoprocSetDevConfig)?;
        let mut cfg = h::HD_VIDEOPROC_DEV_CONFIG::default();
        cfg.pipe = h::HD_VIDEOPROC_PIPE_RAWALL;
        cfg.isp_id = 0;
        // the CTRL func below is capped by this maximum
        cfg.ctrl_max.func = h::HD_VIDEOPROC_FUNC_AF;
        cfg.in_max.dim.w = cap_dim.w;
        cfg.in_max.dim.h = cap_dim.h;
        cfg.in_max.pxlfmt = h::HD_VIDEO_PXLFMT_RAW10;
        cfg.in_max.frc = frc_ratio(1, 1);
        // SAFETY: `cfg` is a `HD_VIDEOPROC_DEV_CONFIG`, the struct this param id takes.
        check(
            Step::VideoprocSetDevConfig,
            ioctl_capture::result(|| unsafe {
                h::hd_videoproc_set(
                    ctrl.0 as _,
                    h::HD_VIDEOPROC_PARAM_DEV_CONFIG,
                    (&mut cfg as *mut h::HD_VIDEOPROC_DEV_CONFIG).cast(),
                )
            }),
        )?;
        // FUNC_AF turns on the VA (sharpness) statistics the kernel AF searches on.
        let mut ctl = h::HD_VIDEOPROC_CTRL {
            func: h::HD_VIDEOPROC_FUNC_AF,
            ..Default::default()
        };
        // SAFETY: `ctl` is a `HD_VIDEOPROC_CTRL`, the struct this param id takes.
        check(
            Step::VideoprocSetDevConfig,
            ioctl_capture::result(|| unsafe {
                h::hd_videoproc_set(
                    ctrl.0 as _,
                    h::HD_VIDEOPROC_PARAM_CTRL,
                    (&mut ctl as *mut h::HD_VIDEOPROC_CTRL).cast(),
                )
            }),
        )
    }

    fn videoproc_open_out(&mut self, out: u32) -> Result<PathId, Error> {
        self.forward(Step::VideoprocOpen)?;
        let out_id = slot(&PROC_OUT, out, Step::VideoprocOpen)?;
        let mut path: h::HD_PATH_ID = 0;
        // SAFETY: `path` is a valid out pointer for the call.
        check(
            Step::VideoprocOpen,
            ioctl_capture::result(|| unsafe {
                h::hd_videoproc_open(h::HD_VIDEOPROC_0_IN_0 as _, out_id as _, &mut path)
            }),
        )?;
        Ok(PathId(path as u32))
    }

    fn videoproc_set_out(&mut self, path: PathId, dim: Dim, fps: u32) -> Result<(), Error> {
        self.forward(Step::VideoprocSetOut)?;
        let mut cfg = h::HD_VIDEOPROC_OUT::default();
        cfg.dim.w = dim.w;
        cfg.dim.h = dim.h;
        cfg.pxlfmt = h::HD_VIDEO_PXLFMT_YUV420;
        cfg.dir = h::HD_VIDEO_DIR_NONE;
        cfg.frc = frc_ratio(fps, self.cap_fps);
        cfg.rect.w = dim.w;
        cfg.rect.h = dim.h;
        cfg.bg.w = dim.w;
        cfg.bg.h = dim.h;
        // SAFETY: `cfg` is a `HD_VIDEOPROC_OUT`, the struct this param id takes.
        check(
            Step::VideoprocSetOut,
            ioctl_capture::result(|| unsafe {
                h::hd_videoproc_set(
                    path.0 as _,
                    h::HD_VIDEOPROC_PARAM_OUT,
                    (&mut cfg as *mut h::HD_VIDEOPROC_OUT).cast(),
                )
            }),
        )
    }

    fn videoproc_start(&mut self, path: PathId) -> Result<(), Error> {
        self.forward(Step::VideoprocStart)?;
        // SAFETY: plain id argument.
        check(
            Step::VideoprocStart,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_start(path.0 as _) }),
        )
    }

    fn videoproc_stop(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideoprocStop)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::VideoprocStop,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_stop(path.0 as _) }),
        )
    }

    fn videoproc_close(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideoprocClose)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::VideoprocClose,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_close(path.0 as _) }),
        )
    }

    fn videoproc_uninit(&mut self) -> Result<(), Error> {
        self.owned(Step::VideoprocUninit)?;
        // SAFETY: no arguments.
        let result = self.cleanup(
            Step::VideoprocUninit,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_uninit() }),
        );
        self.initialized[2] = false;
        result
    }

    fn videoenc_init(&mut self) -> Result<(), Error> {
        self.forward(Step::VideoencInit)?;
        if self.initialized[3] {
            return Err(Error {
                step: Step::VideoencInit,
                code: h::HD_ERR_INIT,
            });
        }
        // SAFETY: no arguments.
        check(
            Step::VideoencInit,
            ioctl_capture::result(|| unsafe { h::hd_videoenc_init() }),
        )?;
        self.initialized[3] = true;
        Ok(())
    }

    fn videoenc_open(&mut self, idx: u32) -> Result<PathId, Error> {
        self.forward(Step::VideoencOpen)?;
        let in_id = slot(&ENC_IN, idx, Step::VideoencOpen)?;
        let out_id = slot(&ENC_OUT, idx, Step::VideoencOpen)?;
        let mut path: h::HD_PATH_ID = 0;
        // SAFETY: `path` is a valid out pointer for the call.
        check(
            Step::VideoencOpen,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_open(in_id as _, out_id as _, &mut path)
            }),
        )?;
        Ok(PathId(path as u32))
    }

    fn videoenc_configure(
        &mut self,
        path: PathId,
        spec: &EncSpec,
        envelope: &EncSpec,
    ) -> Result<(), Error> {
        self.forward(Step::VideoencConfigure)?;
        if self.maps.iter().any(|map| map.path == path) {
            return Err(Error {
                step: Step::VideoencConfigure,
                code: h::HD_ERR_IN_USE,
            });
        }
        let step = Step::VideoencConfigure;

        let mut pc = h::HD_VIDEOENC_PATH_CONFIG::default();
        pc.max_mem.codec_type = h::HD_CODEC_TYPE_H264;
        pc.max_mem.max_dim.w = envelope.dim.w.max(spec.dim.w);
        pc.max_mem.max_dim.h = envelope.dim.h.max(spec.dim.h);
        pc.max_mem.bitrate = envelope.bitrate_bps.max(spec.bitrate_bps);
        pc.max_mem.enc_buf_ms = 3000;
        pc.max_mem.svc_layer = h::HD_SVC_DISABLE;
        pc.isp_id = NO_ID;
        // SAFETY: `pc` is a `HD_VIDEOENC_PATH_CONFIG`, the struct this param id takes.
        check(
            step,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_set(
                    path.0 as _,
                    h::HD_VIDEOENC_PARAM_PATH_CONFIG,
                    (&mut pc as *mut h::HD_VIDEOENC_PATH_CONFIG).cast(),
                )
            }),
        )?;

        encoder_params(path, spec, step)
    }

    fn videoenc_retune(&mut self, path: PathId, spec: &EncSpec) -> Result<(), Error> {
        self.forward(Step::VideoencRetune)?;
        encoder_params(path, spec, Step::VideoencRetune)
    }

    fn videoenc_configure_jpeg(
        &mut self,
        path: PathId,
        dim: Dim,
        quality: u32,
    ) -> Result<(), Error> {
        let step = Step::VideoencConfigureJpeg;
        self.forward(step)?;
        if self.maps.iter().any(|map| map.path == path) {
            return Err(Error {
                step,
                code: h::HD_ERR_IN_USE,
            });
        }
        let mut pc = h::HD_VIDEOENC_PATH_CONFIG::default();
        pc.max_mem.codec_type = h::HD_CODEC_TYPE_JPEG;
        pc.max_mem.max_dim.w = dim.w;
        pc.max_mem.max_dim.h = dim.h;
        // The ring holds two of the largest pictures: `bitrate × enc_buf_ms` sizes it.
        pc.max_mem.bitrate = crate::enc::MAX_JPEG_BYTES * 8;
        pc.max_mem.enc_buf_ms = 2000;
        pc.max_mem.svc_layer = h::HD_SVC_DISABLE;
        pc.isp_id = NO_ID;
        // SAFETY: `pc` is a `HD_VIDEOENC_PATH_CONFIG`, the struct this param id takes.
        check(
            step,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_set(
                    path.0 as _,
                    h::HD_VIDEOENC_PARAM_PATH_CONFIG,
                    (&mut pc as *mut h::HD_VIDEOENC_PATH_CONFIG).cast(),
                )
            }),
        )?;
        let mut inp = h::HD_VIDEOENC_IN::default();
        inp.dim.w = dim.w;
        inp.dim.h = dim.h;
        inp.pxl_fmt = h::HD_VIDEO_PXLFMT_YUV420;
        inp.dir = h::HD_VIDEO_DIR_NONE;
        inp.frc = frc_ratio(1, 1);
        // SAFETY: `inp` is a `HD_VIDEOENC_IN`, the struct this param id takes.
        check(
            step,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_set(
                    path.0 as _,
                    h::HD_VIDEOENC_PARAM_IN,
                    (&mut inp as *mut h::HD_VIDEOENC_IN).cast(),
                )
            }),
        )?;
        let mut out = h::HD_VIDEOENC_OUT::default();
        out.codec_type = h::HD_CODEC_TYPE_JPEG;
        // SAFETY: the anonymous union is plain data; we write the jpeg member of a zeroed value.
        unsafe {
            let c = &mut out.__bindgen_anon_1.jpeg;
            c.retstart_interval = 0;
            c.image_quality = quality.clamp(1, 100);
        }
        // SAFETY: `out` is the 44-byte `HD_VIDEOENC_OUT` of HDAL 2.00 (abi/ table).
        check(
            step,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_set(
                    path.0 as _,
                    h::HD_VIDEOENC_PARAM_OUT_ENC_PARAM,
                    (&mut out as *mut h::HD_VIDEOENC_OUT).cast(),
                )
            }),
        )
    }

    fn videoenc_start(&mut self, path: PathId) -> Result<(), Error> {
        self.forward(Step::VideoencStart)?;
        // SAFETY: plain id argument.
        check(
            Step::VideoencStart,
            ioctl_capture::result(|| unsafe { h::hd_videoenc_start(path.0 as _) }),
        )
    }

    fn videoenc_stop(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideoencStop)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::VideoencStop,
            ioctl_capture::result(|| unsafe { h::hd_videoenc_stop(path.0 as _) }),
        )
    }

    fn videoenc_close(&mut self, path: PathId) -> Result<(), Error> {
        self.owned(Step::VideoencClose)?;
        let unmap = if let Some(i) = self.maps.iter().position(|m| m.path == path) {
            let m = self.maps.swap_remove(i);
            // SAFETY: `va`/`size` describe the owned mapping for this path.
            self.cleanup(
                Step::VideoencClose,
                ioctl_capture::result(|| unsafe {
                    h::hd_common_mem_munmap(m.va.cast(), m.size as u32)
                }),
            )
        } else {
            Ok(())
        };
        // SAFETY: plain id argument; attempted even if unmap failed.
        let close = self.cleanup(
            Step::VideoencClose,
            ioctl_capture::result(|| unsafe { h::hd_videoenc_close(path.0 as _) }),
        );
        finish(unmap, close, "videoenc_close")
    }

    fn videoenc_uninit(&mut self) -> Result<(), Error> {
        self.owned(Step::VideoencUninit)?;
        if !self.maps.is_empty() {
            return Err(Error {
                step: Step::VideoencUninit,
                code: h::HD_ERR_IN_USE,
            });
        }
        // SAFETY: no arguments.
        let result = self.cleanup(
            Step::VideoencUninit,
            ioctl_capture::result(|| unsafe { h::hd_videoenc_uninit() }),
        );
        self.initialized[3] = false;
        result
    }

    fn bind_cap_proc(&mut self) -> Result<(), Error> {
        self.forward(Step::BindCapProc)?;
        // SAFETY: plain id arguments.
        check(
            Step::BindCapProc,
            ioctl_capture::result(|| unsafe {
                h::hd_videocap_bind(h::HD_VIDEOCAP_0_OUT_0 as _, h::HD_VIDEOPROC_0_IN_0 as _)
            }),
        )
    }

    fn bind_proc_enc(&mut self, out: u32, enc: u32) -> Result<(), Error> {
        self.forward(Step::BindProcEnc)?;
        let o = slot(&PROC_OUT, out, Step::BindProcEnc)?;
        let i = slot(&ENC_IN, enc, Step::BindProcEnc)?;
        // SAFETY: plain id arguments.
        check(
            Step::BindProcEnc,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_bind(o as _, i as _) }),
        )
    }

    fn unbind_cap_proc(&mut self) -> Result<(), Error> {
        self.owned(Step::Unbind)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::Unbind,
            ioctl_capture::result(|| unsafe { h::hd_videocap_unbind(h::HD_VIDEOCAP_0_OUT_0 as _) }),
        )
    }

    fn unbind_proc_enc(&mut self, out: u32) -> Result<(), Error> {
        self.owned(Step::Unbind)?;
        let o = slot(&PROC_OUT, out, Step::Unbind)?;
        // SAFETY: plain id argument.
        self.cleanup(
            Step::Unbind,
            ioctl_capture::result(|| unsafe { h::hd_videoproc_unbind(o as _) }),
        )
    }

    fn videoenc_recv(&mut self, paths: &[PathId], wait_ms: u32) -> Result<Vec<EncFrame>, Error> {
        self.forward(Step::VideoencRecv)?;
        let step = Step::VideoencRecv;
        let wait_ms = i32::try_from(wait_ms).map_err(|_| Error {
            step,
            code: h::HD_ERR_INV,
        })?;
        let mut poll: Vec<h::HD_VIDEOENC_POLL_LIST> = paths
            .iter()
            .map(|p| {
                let mut e = h::HD_VIDEOENC_POLL_LIST::default();
                e.path_id = p.0 as _;
                e
            })
            .collect();
        // SAFETY: `poll` is a valid array of `poll.len()` entries for the call.
        let r = ioctl_capture::result(|| unsafe {
            h::hd_videoenc_poll_list(poll.as_mut_ptr(), poll.len() as u32, wait_ms)
        });
        if r as i32 == h::HD_ERR_TIMEDOUT as i32 {
            return Ok(Vec::new());
        }
        check(step, r)?;
        let mut out = Vec::new();
        for (stream, (p, ev)) in paths.iter().zip(&poll).enumerate() {
            if ev.revent.event == 0 {
                continue;
            }
            let map = self.map_for(*p)?;
            let mut bs = h::HD_VIDEOENC_BS::default();
            // SAFETY: `bs` is a valid `HD_VIDEOENC_BS` for the call.
            check(
                step,
                ioctl_capture::result(|| unsafe {
                    h::hd_videoenc_pull_out_buf(p.0 as _, &mut bs, 0)
                }),
            )?;
            let copied = copy_packs(&bs, map);
            // Release before reporting a bad pack: the encoder must get its buffer back either way.
            // SAFETY: `bs` is the descriptor `pull_out_buf` filled.
            let rel = check(
                step,
                ioctl_capture::result(|| unsafe {
                    h::hd_videoenc_release_out_buf(p.0 as _, &mut bs)
                }),
            );
            self.terminal |= rel.is_err();
            let data = finish(copied, rel, "videoenc_release_out_buf")?;
            let key = bs.frame_type == h::HD_FRAME_TYPE_IDR || bs.frame_type == h::HD_FRAME_TYPE_I;
            out.push(EncFrame {
                stream,
                data,
                key,
                pts_us: bs.timestamp,
            });
        }
        Ok(out)
    }

    fn now_ms(&mut self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }
}

impl Sys {
    /// The mapping of `path`'s bitstream buffer, made on first use.
    fn map_for(&mut self, path: PathId) -> Result<&EncMap, Error> {
        let step = Step::VideoencRecv;
        if let Some(i) = self.maps.iter().position(|m| m.path == path) {
            return Ok(&self.maps[i]);
        }
        let mut info = h::HD_VIDEOENC_BUFINFO::default();
        // SAFETY: `info` is a `HD_VIDEOENC_BUFINFO`, the struct this param id fills.
        check(
            step,
            ioctl_capture::result(|| unsafe {
                h::hd_videoenc_get(
                    path.0 as _,
                    h::HD_VIDEOENC_PARAM_BUFINFO,
                    (&mut info as *mut h::HD_VIDEOENC_BUFINFO).cast(),
                )
            }),
        )?;
        let (phy, size) = (
            info.buf_info.phy_addr as usize,
            info.buf_info.buf_size as usize,
        );
        // Ring mappings are bounded on their own; every copied AU is bounded by MAX_AU_BYTES.
        if size > crate::enc::MAX_RING_BYTES
            || self
                .maps
                .iter()
                .map(|m| m.size)
                .sum::<usize>()
                .checked_add(size)
                .is_none_or(|total| total > crate::enc::MAX_RINGS_BYTES)
        {
            return Err(Error { step, code: -22 });
        }
        eprintln!("HDAL encoder ring: path={} bytes={size}", path.0);
        let va = self.map_buffer(phy, size, step)?;
        self.maps.push(EncMap {
            path,
            phy,
            size,
            va: va.cast(),
        });
        Ok(&self.maps[self.maps.len() - 1])
    }
}

/// Input, codec and CBR rate control of one encoder path: everything but the path config, so a
/// running path's mapped bitstream ring stays valid.
fn encoder_params(path: PathId, spec: &EncSpec, step: Step) -> Result<(), Error> {
    let mut inp = h::HD_VIDEOENC_IN::default();
    inp.dim.w = spec.dim.w;
    inp.dim.h = spec.dim.h;
    inp.pxl_fmt = h::HD_VIDEO_PXLFMT_YUV420;
    inp.dir = h::HD_VIDEO_DIR_NONE;
    inp.frc = frc_ratio(1, 1);
    // SAFETY: `inp` is a `HD_VIDEOENC_IN`, the struct this param id takes.
    check(
        step,
        ioctl_capture::result(|| unsafe {
            h::hd_videoenc_set(
                path.0 as _,
                h::HD_VIDEOENC_PARAM_IN,
                (&mut inp as *mut h::HD_VIDEOENC_IN).cast(),
            )
        }),
    )?;

    let mut out = h::HD_VIDEOENC_OUT::default();
    out.codec_type = h::HD_CODEC_TYPE_H264;
    // SAFETY: the anonymous union is plain data; we write the h26x member of a zeroed value.
    unsafe {
        let c = &mut out.__bindgen_anon_1.h26x;
        c.gop_num = spec.gop;
        c.profile = h::HD_H264E_MAIN_PROFILE;
        c.level_idc = h::HD_H264E_LEVEL_4_1;
        c.svc_layer = h::HD_SVC_DISABLE;
        c.entropy_mode = h::HD_H264E_CABAC_CODING;
    }
    // SAFETY: `out` is the 44-byte `HD_VIDEOENC_OUT` of HDAL 2.00 (abi/ table).
    check(
        step,
        ioctl_capture::result(|| unsafe {
            h::hd_videoenc_set(
                path.0 as _,
                h::HD_VIDEOENC_PARAM_OUT_ENC_PARAM,
                (&mut out as *mut h::HD_VIDEOENC_OUT).cast(),
            )
        }),
    )?;

    let mut rc = h::HD_H26XENC_RATE_CONTROL::default();
    rc.rc_mode = h::HD_RC_MODE_CBR;
    // SAFETY: plain-data union; we write the cbr member of a zeroed value.
    unsafe {
        let c = &mut rc.__bindgen_anon_1.cbr;
        c.bitrate = spec.bitrate_bps;
        c.frame_rate_base = spec.fps;
        c.frame_rate_incr = 1;
        c.init_i_qp = 26;
        c.max_i_qp = 51;
        c.min_i_qp = 10;
        c.init_p_qp = 26;
        c.max_p_qp = 51;
        c.min_p_qp = 10;
    }
    // SAFETY: `rc` is a `HD_H26XENC_RATE_CONTROL`, the struct this param id takes.
    check(
        step,
        ioctl_capture::result(|| unsafe {
            h::hd_videoenc_set(
                path.0 as _,
                h::HD_VIDEOENC_PARAM_OUT_RATE_CONTROL,
                (&mut rc as *mut h::HD_H26XENC_RATE_CONTROL).cast(),
            )
        }),
    )
}

/// Bounds in the camera's 32-bit address space, even when tested on a 64-bit host.
fn region(phy: usize, size: usize, base: usize, len: usize) -> Option<std::ops::Range<usize>> {
    if size == 0 || size > i32::MAX as usize {
        return None;
    }
    u32::try_from(phy)
        .ok()?
        .checked_add(u32::try_from(size).ok()?)?;
    let off = phy.checked_sub(base)?;
    let end = off.checked_add(size)?;
    (end <= len).then_some(off..end)
}

/// Validate the whole descriptor before cache operations or any Rust slice construction.
fn copy_packs(bs: &h::HD_VIDEOENC_BS, map: &EncMap) -> Result<Vec<u8>, Error> {
    let invalid = Error {
        step: Step::VideoencRecv,
        code: -2,
    };
    let packs = bs
        .video_pack
        .get(..bs.pack_num as usize)
        .filter(|p| !p.is_empty())
        .ok_or(invalid)?;
    let ranges: Vec<_> = packs
        .iter()
        .map(|p| region(p.phy_addr as usize, p.size as usize, map.phy, map.size).ok_or(invalid))
        .collect::<Result<_, _>>()?;
    let total = ranges
        .iter()
        .try_fold(0usize, |sum, r| sum.checked_add(r.len()))
        .filter(|n| *n <= crate::enc::MAX_AU_BYTES)
        .ok_or(invalid)?;
    // Never duplicate overlapping packs, even if individually inside the ring mapping.
    for (i, a) in ranges.iter().enumerate() {
        if ranges[i + 1..]
            .iter()
            .any(|b| a.start < b.end && b.start < a.end)
        {
            return Err(invalid);
        }
    }
    let mut data = Vec::with_capacity(total);
    for (pack, range) in packs.iter().zip(ranges) {
        // SAFETY: the entire descriptor was checked against the live mapping before reading.
        let ptr = unsafe { map.va.add(range.start) };
        // SAFETY: checked mapped range; successful cache maintenance precedes DMA reads.
        check(
            Step::VideoencRecv,
            ioctl_capture::result(|| unsafe {
                h::hd_common_mem_flush_cache(ptr.cast(), pack.size)
            }),
        )?;
        // SAFETY: checked nonempty readable range within this mapping, bounded by isize::MAX.
        data.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, range.len()) });
    }
    Ok(data)
}

/// Both operations have already run. Preserve the first failure and report secondary cleanup errors.
fn finish<T>(
    primary: Result<T, Error>,
    cleanup: Result<(), Error>,
    operation: &str,
) -> Result<T, Error> {
    if let Err(e) = &cleanup {
        eprintln!("HDAL cleanup {operation}: {e}");
    }
    let value = primary?;
    cleanup?;
    Ok(value)
}

fn valid_extent(phy: usize, size: usize) -> bool {
    phy != 0 && region(phy, size, phy, size).is_some()
}
fn valid_pointer(va: *mut h::ctypes::c_void, size: usize) -> bool {
    !va.is_null() && va as usize != usize::MAX && (va as usize).checked_add(size).is_some()
}
impl Sys {
    fn map_buffer(&mut self, phy: usize, size: usize, step: Step) -> Result<*mut u8, Error> {
        let invalid = Error { step, code: -22 };
        if !valid_extent(phy, size) {
            return Err(invalid);
        }
        // SAFETY: nonempty, nonwrapping camera physical extent from BUFINFO.
        let (va, failure) = ioctl_capture::run(|| unsafe {
            h::hd_common_mem_mmap(h::HD_COMMON_MEM_MEM_TYPE_CACHE, phy as _, size as u32)
        });
        if let Some(failure) = failure {
            let primary = Err(Error {
                step,
                code: failure.code(),
            });
            if !va.is_null() && va as usize != usize::MAX {
                // SAFETY: release the returned mapping without using or dereferencing it.
                let cleanup = self.cleanup(
                    step,
                    ioctl_capture::result(|| unsafe { h::hd_common_mem_munmap(va, size as u32) }),
                );
                return finish(primary, cleanup, "failed mapping munmap");
            }
            return primary;
        }
        if va.is_null() || va as usize == usize::MAX {
            return Err(Error { step, code: -12 });
        }
        if !valid_pointer(va, size) {
            // SAFETY: release only the non-sentinel mapping returned by HDAL; never dereference it.
            let cleanup = self.cleanup(
                step,
                ioctl_capture::result(|| unsafe { h::hd_common_mem_munmap(va, size as u32) }),
            );
            return finish(Err(invalid), cleanup, "invalid mapping munmap");
        }
        Ok(va.cast())
    }
}
