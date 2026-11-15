//! The videocap probe: init, memory pool, sensor, one SYSINFO read, and clean-up in reverse on any failure.

use std::fmt::Write as _;

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    pub sensor: SensorConfig,
    pub dim: Dim,
    pub fps: u32,
    pub pool: MemPool,
}

impl ProbeConfig {
    /// The E1 Zoom: OS05A10 at 2592x1944; three blocks sized at 1.5 bytes/pixel,
    /// inside the ~47.5 MiB `nvtmpp` area. The probe starts RAW capture to read SYSINFO.
    pub fn os05a10() -> Self {
        Self {
            sensor: SensorConfig {
                driver_name: "nvt_sen_os05a10".into(),
            },
            dim: Dim { w: 2592, h: 1944 },
            fps: 30,
            pool: MemPool {
                blk_size: 2592 * 1944 * 3 / 2,
                blk_cnt: 3,
            },
        }
    }
}

/// What the probe read, plus clean-up calls that failed (the data is valid regardless). SYSCAPS needs the
/// sensor mode attributes, which the kernel only has once a capture path runs: its failure is reported,
/// not fatal (the camera answered SYSINFO).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutput {
    pub sensor: String,
    pub caps: Result<VideoCapSysCaps, Error>,
    pub info: VideoCapSysInfo,
    pub cleanup_errors: Vec<Error>,
}

/// A failed probe retains both the initiating failure and every cleanup failure.
#[derive(Debug)]
pub struct ProbeError {
    pub primary: Error,
    pub cleanup: Vec<Error>,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.primary)?;
        for error in &self.cleanup {
            write!(f, "; cleanup: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

impl ProbeOutput {
    /// One `key=value` per line, for `grep` in the device test.
    pub fn report(&self) -> String {
        let mut r = String::new();
        let i = &self.info;
        let _ = writeln!(r, "sensor={}", self.sensor);
        let _ = writeln!(r, "cur_dim={}x{}", i.cur_dim.w, i.cur_dim.h);
        let _ = writeln!(r, "cur_fps={:.2}", i.fps(0));
        let _ = writeln!(r, "cur_fps_raw={:#010x}", i.cur_fps[0]);
        let _ = writeln!(r, "vd_count={}", i.vd_count);
        let _ = writeln!(r, "output_started={}", i.output_started);
        let _ = writeln!(r, "dev_id={:#x}", i.dev_id);
        match &self.caps {
            Ok(c) => {
                let _ = writeln!(r, "caps_chip_id={:#x}", c.chip_id);
                let _ = writeln!(
                    r,
                    "caps_max_in={} max_out={}",
                    c.max_in_count, c.max_out_count
                );
                let _ = writeln!(r, "caps_max_dim={}x{}", c.max_dim.w, c.max_dim.h);
                let _ = writeln!(r, "caps_max_fps={:.2}", frc_to_fps(c.max_frame_rate));
            }
            Err(e) => {
                let _ = writeln!(r, "caps_error={e}");
            }
        }
        if self.cleanup_errors.is_empty() {
            r.push_str("cleanup=ok\n");
        } else {
            for e in &self.cleanup_errors {
                let _ = writeln!(r, "cleanup_error={e}");
            }
        }
        r
    }
}

/// Undoes the steps that succeeded, newest first. Failures are collected, never stop the unwind.
struct Unwind<'a, B: Backend> {
    hw: &'a mut B,
    path: Option<PathId>,
    io: Option<PathId>,
    started: bool,
    videocap: bool,
    mem: bool,
    common: bool,
    errors: Vec<Error>,
}

impl<B: Backend> Unwind<'_, B> {
    fn run(mut self) -> Vec<Error> {
        let mut note = |r: Result<(), Error>| {
            if let Err(e) = r {
                self.errors.push(e);
            }
        };
        if let (true, Some(p)) = (self.started, self.io) {
            note(self.hw.videocap_stop(p));
        }
        if let Some(p) = self.io.take() {
            note(self.hw.videocap_close(p));
        }
        if let Some(p) = self.path.take() {
            note(self.hw.videocap_close(p));
        }
        if self.videocap {
            note(self.hw.videocap_uninit());
        }
        if self.mem {
            note(self.hw.common_mem_uninit());
        }
        if self.common {
            note(self.hw.common_uninit());
        }
        self.errors
    }
}

const SYSINFO_POLLS: usize = 20;
const SYSINFO_POLL_MS: u32 = 100;

/// Init, open the capture control path, set the sensor, read SYSCAPS and SYSINFO, tear everything down.
/// On a failing step the steps already done are undone and the error returned.
pub fn probe_videocap<B: Backend>(
    hw: &mut B,
    cfg: &ProbeConfig,
) -> Result<ProbeOutput, ProbeError> {
    let mut u = Unwind {
        hw,
        path: None,
        io: None,
        started: false,
        videocap: false,
        mem: false,
        common: false,
        errors: Vec::new(),
    };
    let read = (|| {
        u.hw.common_init()?;
        u.common = true;
        u.hw.common_mem_init(&[cfg.pool])?;
        u.mem = true;
        u.hw.videocap_init()?;
        u.videocap = true;
        let path = u.hw.videocap_open()?;
        u.path = Some(path);
        u.hw.videocap_set_sensor(path, &cfg.sensor)?;
        let io = u.hw.videocap_open_io()?;
        u.io = Some(io);
        u.hw.videocap_set_in(io, cfg.dim, cfg.fps)?;
        u.hw.videocap_set_out(io, cfg.dim, 1)?;
        u.hw.videocap_start(io)?;
        u.started = true;
        // The output size only shows once frames flow: poll for it.
        let mut info = u.hw.videocap_get_sysinfo(path)?;
        for _ in 1..SYSINFO_POLLS {
            if info.cur_dim.w != 0 {
                break;
            }
            u.hw.sleep_ms(SYSINFO_POLL_MS);
            info = u.hw.videocap_get_sysinfo(path)?;
        }
        let caps = u.hw.videocap_get_syscaps(path);
        Ok((caps, info))
    })();
    let cleanup_errors = u.run();
    let (caps, info) = match read {
        Ok(data) => data,
        Err(primary) => {
            return Err(ProbeError {
                primary,
                cleanup: cleanup_errors,
            });
        }
    };
    Ok(ProbeOutput {
        sensor: cfg.sensor.driver_name.clone(),
        caps,
        info,
        cleanup_errors,
    })
}

#[cfg(test)]
mod tests;
