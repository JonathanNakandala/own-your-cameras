//! Host fake of the HDAL backend: records the calls, can fail one step.

use crate::enc::{EncBackend, EncFrame, EncSpec};
use crate::{
    Backend, Dim, Error, MemPool, PathId, SensorConfig, Step, VideoCapSysCaps, VideoCapSysInfo,
};

/// Live knobs for tests that drive a running pipeline from another thread.
#[derive(Debug, Default)]
pub struct Control {
    /// Hold a hardware call after recording entry, until the test clears this step.
    pub hold_step: std::sync::Mutex<Option<Step>>,
    /// While set, `videoenc_recv` does not return (a stuck HDAL poll).
    pub block_recv: std::sync::atomic::AtomicBool,
    /// Bit `i` set: encoder `i` produces no access units.
    pub muted: std::sync::atomic::AtomicU32,
}

/// What the OS05A10 camera reports, so tests see realistic data.
#[derive(Debug)]
pub struct Fake {
    /// When set, `videoenc_recv` also paces itself (1 ms of real time) and honors the knobs.
    pub control: Option<std::sync::Arc<Control>>,
    pub audio_frames: std::collections::VecDeque<crate::audio::PcmFrame>,
    pub speaker_pcm: Vec<i16>,
    pub speaker_full: bool,
    /// Every capture tuning the fake was started or tuned with, in order.
    pub tunings: Vec<crate::audio::Tuning>,
    /// Speaker volumes, from start and every later change.
    pub speaker_volumes: Vec<u8>,
    pub calls: Vec<&'static str>,
    /// Optional shared hardware trace for lifecycle tests with concurrent device users.
    pub trace: Option<std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>>,
    pub sensor: Option<String>,
    pub pool: Option<MemPool>,
    pub input: Option<(Dim, u32)>,
    /// SYSINFO reports `cur_dim` 0x0 until it was read this many times.
    pub ready_after_polls: usize,
    pub slept_ms: u32,
    polls: usize,
    /// What `videoproc_set_out` was given, by output index.
    pub proc_out: Vec<(u32, Dim, u32)>,
    pub proc_dev_dim: Option<Dim>,
    pub cap_out_depth: Option<u32>,
    /// What `videoenc_configure` and `videoenc_retune` were given, by encoder index; the last one runs.
    pub enc_specs: Vec<(u32, EncSpec)>,
    /// Models the camera's measured limit: while encoder 0 runs above this rate, encoder 1 delivers
    /// nothing (the sub stream starves at main 20–30 fps).
    pub starve_sub_above_fps: Option<u32>,
    /// The path-config envelope `videoenc_configure` was given, by encoder index.
    pub envelopes: Vec<(u32, EncSpec)>,
    pub binds: Vec<(u32, u32)>,
    /// JPEG encoders configured: (index, size, quality).
    pub jpegs: Vec<(u32, Dim, u32)>,
    /// Encoder indexes running now.
    pub running: Vec<u32>,
    /// Virtual clock: `sleep_ms` and `videoenc_recv` advance it.
    pub clock_ms: u64,
    frames_made: [u32; 4],
    next_due_ms: [u64; 4],
    pub info: VideoCapSysInfo,
    pub caps: VideoCapSysCaps,
    failures: Vec<(Step, i32)>,
    /// `(step, n, code)`: only the n-th call (1-based) of `step` fails.
    nth_failures: Vec<(Step, usize, i32)>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            control: None,
            audio_frames: Default::default(),
            speaker_pcm: Vec::new(),
            speaker_full: false,
            tunings: Vec::new(),
            speaker_volumes: Vec::new(),
            calls: Vec::new(),
            trace: None,
            sensor: None,
            pool: None,
            input: None,
            ready_after_polls: 1,
            slept_ms: 0,
            polls: 0,
            proc_out: Vec::new(),
            proc_dev_dim: None,
            cap_out_depth: None,
            enc_specs: Vec::new(),
            envelopes: Vec::new(),
            starve_sub_above_fps: None,
            binds: Vec::new(),
            jpegs: Vec::new(),
            running: Vec::new(),
            clock_ms: 0,
            frames_made: [0; 4],
            next_due_ms: [0; 4],
            info: VideoCapSysInfo {
                dev_id: 0x2000,
                cur_fps: [0x0BB8_0064, 0, 0, 0, 0, 0, 0, 0],
                vd_count: 1234,
                output_started: true,
                cur_dim: Dim { w: 2592, h: 1944 },
            },
            caps: VideoCapSysCaps {
                dev_id: 0x2000,
                max_in_count: 1,
                max_out_count: 8,
                max_dim: Dim { w: 2592, h: 1944 },
                max_frame_rate: 30,
                ..Default::default()
            },
            failures: Vec::new(),
            nth_failures: Vec::new(),
        }
    }
}

impl Fake {
    /// A fake whose `step` returns `code`.
    pub fn failing(step: Step, code: i32) -> Self {
        Self {
            failures: vec![(step, code)],
            ..Self::default()
        }
    }

    /// Only the `n`-th call (1-based) of `step` returns `code`.
    pub fn failing_nth(mut self, step: Step, n: usize, code: i32) -> Self {
        self.nth_failures.push((step, n, code));
        self
    }

    /// Add an independent cleanup fault to an existing failure scenario.
    pub fn also_failing(mut self, step: Step, code: i32) -> Self {
        self.failures.push((step, code));
        self
    }

    fn enter(&mut self, step: Step) -> Result<(), Error> {
        self.calls.push(step.name());
        if let Some(trace) = &self.trace {
            trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(step.name());
        }
        if let Some(control) = &self.control {
            while *control
                .hold_step
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == Some(step)
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let calls = self.calls.iter().filter(|c| **c == step.name()).count();
        if let Some((_, _, code)) = self
            .nth_failures
            .iter()
            .find(|(s, n, _)| *s == step && *n == calls)
        {
            return Err(Error { step, code: *code });
        }
        match self.failures.iter().find(|(s, _)| *s == step) {
            Some((_, code)) => Err(Error { step, code: *code }),
            _ => Ok(()),
        }
    }
}

impl Backend for Fake {
    fn common_init(&mut self) -> Result<(), Error> {
        self.enter(Step::CommonInit)
    }
    fn common_mem_init(&mut self, pool: &MemPool) -> Result<(), Error> {
        self.enter(Step::CommonMemInit)?;
        self.pool = Some(*pool);
        Ok(())
    }
    fn videocap_init(&mut self) -> Result<(), Error> {
        self.enter(Step::VideocapInit)
    }
    fn videocap_open(&mut self) -> Result<PathId, Error> {
        self.enter(Step::VideocapOpen)?;
        Ok(PathId(0x2000_ffff))
    }
    fn videocap_set_sensor(&mut self, _: PathId, sensor: &SensorConfig) -> Result<(), Error> {
        self.enter(Step::VideocapSetSensor)?;
        self.sensor = Some(sensor.driver_name.clone());
        Ok(())
    }
    fn videocap_open_io(&mut self) -> Result<PathId, Error> {
        self.enter(Step::VideocapOpenIo)?;
        Ok(PathId(0x2000_0100))
    }
    fn videocap_set_in(&mut self, _: PathId, dim: Dim, fps: u32) -> Result<(), Error> {
        self.enter(Step::VideocapSetIn)?;
        self.input = Some((dim, fps));
        Ok(())
    }
    fn videocap_set_out(&mut self, _: PathId, _: Dim, depth: u32) -> Result<(), Error> {
        self.enter(Step::VideocapSetOut)?;
        self.cap_out_depth = Some(depth);
        Ok(())
    }
    fn videocap_start(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideocapStart)
    }
    fn videocap_stop(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideocapStop)
    }
    fn sleep_ms(&mut self, ms: u32) {
        self.slept_ms += ms;
        self.clock_ms += u64::from(ms);
    }
    fn videocap_get_syscaps(&mut self, _: PathId) -> Result<VideoCapSysCaps, Error> {
        self.enter(Step::VideocapGetSyscaps)?;
        Ok(self.caps)
    }
    fn videocap_get_sysinfo(&mut self, _: PathId) -> Result<VideoCapSysInfo, Error> {
        self.enter(Step::VideocapGetSysinfo)?;
        self.polls += 1;
        let mut info = self.info;
        if self.polls < self.ready_after_polls {
            info.cur_dim = Dim::default();
        }
        Ok(info)
    }
    fn videocap_close(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideocapClose)
    }
    fn videocap_uninit(&mut self) -> Result<(), Error> {
        self.enter(Step::VideocapUninit)
    }
    fn common_mem_uninit(&mut self) -> Result<(), Error> {
        self.enter(Step::CommonMemUninit)
    }
    fn common_uninit(&mut self) -> Result<(), Error> {
        self.enter(Step::CommonUninit)
    }
}

const PROC_CTRL: u32 = 0x3000_ffff;
const PROC_OUT: u32 = 0x3000_0000;
const ENC: u32 = 0x4000_0000;

fn nal(kind: u8, len: usize) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1, kind];
    v.resize(5 + len, 0xAA);
    v
}

impl EncBackend for Fake {
    fn videoproc_init(&mut self) -> Result<(), Error> {
        self.enter(Step::VideoprocInit)
    }
    fn videoproc_open_ctrl(&mut self) -> Result<PathId, Error> {
        self.enter(Step::VideoprocOpen)?;
        Ok(PathId(PROC_CTRL))
    }
    fn videoproc_set_dev_config(&mut self, _: PathId, cap_dim: Dim) -> Result<(), Error> {
        self.enter(Step::VideoprocSetDevConfig)?;
        self.proc_dev_dim = Some(cap_dim);
        Ok(())
    }
    fn videoproc_open_out(&mut self, out: u32) -> Result<PathId, Error> {
        self.enter(Step::VideoprocOpen)?;
        Ok(PathId(PROC_OUT + out))
    }
    fn videoproc_set_out(&mut self, path: PathId, dim: Dim, fps: u32) -> Result<(), Error> {
        self.enter(Step::VideoprocSetOut)?;
        self.proc_out.push((path.0 - PROC_OUT, dim, fps));
        Ok(())
    }
    fn videoproc_start(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideoprocStart)
    }
    fn videoproc_stop(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideoprocStop)
    }
    fn videoproc_close(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideoprocClose)
    }
    fn videoproc_uninit(&mut self) -> Result<(), Error> {
        self.enter(Step::VideoprocUninit)
    }
    fn videoenc_init(&mut self) -> Result<(), Error> {
        self.enter(Step::VideoencInit)
    }
    fn videoenc_open(&mut self, idx: u32) -> Result<PathId, Error> {
        self.enter(Step::VideoencOpen)?;
        Ok(PathId(ENC + idx))
    }
    fn videoenc_configure(
        &mut self,
        path: PathId,
        spec: &EncSpec,
        envelope: &EncSpec,
    ) -> Result<(), Error> {
        self.enter(Step::VideoencConfigure)?;
        self.enc_specs.push((path.0 - ENC, *spec));
        self.envelopes.push((path.0 - ENC, *envelope));
        Ok(())
    }
    fn videoenc_configure_jpeg(
        &mut self,
        path: PathId,
        dim: Dim,
        quality: u32,
    ) -> Result<(), Error> {
        self.enter(Step::VideoencConfigureJpeg)?;
        self.jpegs.push((path.0 - ENC, dim, quality));
        Ok(())
    }
    fn videoenc_retune(&mut self, path: PathId, spec: &EncSpec) -> Result<(), Error> {
        self.enter(Step::VideoencRetune)?;
        self.enc_specs.push((path.0 - ENC, *spec));
        Ok(())
    }
    fn videoenc_start(&mut self, path: PathId) -> Result<(), Error> {
        self.enter(Step::VideoencStart)?;
        // A (re)started encoder opens with a key frame, one frame period from now.
        let idx = (path.0 - ENC) as usize;
        self.frames_made[idx] = 0;
        self.next_due_ms[idx] = self.clock_ms;
        self.running.push(idx as u32);
        Ok(())
    }
    fn videoenc_stop(&mut self, path: PathId) -> Result<(), Error> {
        self.enter(Step::VideoencStop)?;
        self.running.retain(|i| *i != path.0 - ENC);
        Ok(())
    }
    fn videoenc_close(&mut self, _: PathId) -> Result<(), Error> {
        self.enter(Step::VideoencClose)
    }
    fn videoenc_uninit(&mut self) -> Result<(), Error> {
        self.enter(Step::VideoencUninit)
    }
    fn bind_cap_proc(&mut self) -> Result<(), Error> {
        self.enter(Step::BindCapProc)
    }
    fn bind_proc_enc(&mut self, out: u32, enc: u32) -> Result<(), Error> {
        self.enter(Step::BindProcEnc)?;
        self.binds.push((out, enc));
        Ok(())
    }
    fn unbind_cap_proc(&mut self) -> Result<(), Error> {
        self.enter(Step::Unbind)
    }
    fn unbind_proc_enc(&mut self, _: u32) -> Result<(), Error> {
        self.enter(Step::Unbind)
    }
    /// One access unit per stream whenever its frame period elapsed; key frames (SPS, PPS, IDR) every
    /// `gop` frames, P slices otherwise.
    fn videoenc_recv(&mut self, paths: &[PathId], wait_ms: u32) -> Result<Vec<EncFrame>, Error> {
        self.enter(Step::VideoencRecv)?;
        if let Some(control) = &self.control {
            use std::sync::atomic::Ordering::Acquire;
            while control.block_recv.load(Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.clock_ms += u64::from(wait_ms.min(20));
        let mut out = Vec::new();
        for (i, p) in paths.iter().enumerate() {
            let idx = (p.0 - ENC) as usize;
            if self.control.as_ref().is_some_and(|c| {
                c.muted.load(std::sync::atomic::Ordering::Acquire) & (1 << idx) != 0
            }) {
                continue;
            }
            if self.jpegs.iter().any(|(n, _, _)| *n as usize == idx) {
                if self.running.contains(&(idx as u32)) && self.clock_ms >= self.next_due_ms[idx] {
                    self.next_due_ms[idx] += 200;
                    out.push(EncFrame {
                        stream: i,
                        data: vec![0xff, 0xd8, 0xff, 0xe0, 0, 0, 0xff, 0xd9],
                        key: true,
                        pts_us: self.clock_ms * 1000,
                    });
                }
                continue;
            }
            let Some((_, spec)) = self.enc_specs.iter().rfind(|(n, _)| *n as usize == idx) else {
                continue;
            };
            let main_fps = self
                .enc_specs
                .iter()
                .rfind(|(n, _)| *n == 0)
                .map_or(0, |(_, s)| s.fps);
            if idx == 1
                && self
                    .starve_sub_above_fps
                    .is_some_and(|limit| main_fps > limit)
            {
                continue;
            }
            if self.clock_ms < self.next_due_ms[idx] {
                continue;
            }
            let n = self.frames_made[idx];
            let key = n.is_multiple_of(spec.gop);
            let period = u64::from(1000 / spec.fps);
            self.next_due_ms[idx] += period;
            self.frames_made[idx] += 1;
            let mut data = Vec::new();
            if key {
                data.extend(nal(0x67, 8));
                data.extend(nal(0x68, 4));
                data.extend(nal(0x65, 100));
            } else {
                data.extend(nal(0x41, 40));
            }
            out.push(EncFrame {
                stream: i,
                data,
                key,
                pts_us: self.clock_ms * 1000,
            });
        }
        Ok(out)
    }
    fn now_ms(&mut self) -> u64 {
        self.clock_ms
    }
}

impl crate::AudioCap for Fake {
    fn audio_cap_start(&mut self, tuning: &crate::audio::Tuning) -> Result<(), Error> {
        self.enter(Step::AudioCapStart)?;
        self.tunings.push(*tuning);
        Ok(())
    }
    fn audio_cap_recv(&mut self) -> Result<Option<crate::audio::PcmFrame>, Error> {
        self.enter(Step::AudioCapRecv)?;
        Ok(self.audio_frames.pop_front())
    }
    fn audio_cap_stop(&mut self) -> Result<(), Error> {
        self.enter(Step::AudioCapStop)
    }
}
impl crate::AudioOut for Fake {
    fn audio_out_start(&mut self, volume: u8) -> Result<(), Error> {
        self.enter(Step::AudioOutStart)?;
        self.speaker_volumes.push(volume);
        Ok(())
    }
    fn audio_out_volume(&mut self, volume: u8) -> Result<(), Error> {
        self.enter(Step::AudioOutVolume)?;
        self.speaker_volumes.push(volume);
        Ok(())
    }
    fn audio_out_push(&mut self, pcm: &[i16]) -> Result<bool, Error> {
        self.enter(Step::AudioOutPush)?;
        if self.speaker_full {
            return Ok(false);
        }
        self.speaker_pcm.extend_from_slice(pcm);
        Ok(true)
    }
    fn audio_out_stop(&mut self) -> Result<(), Error> {
        self.enter(Step::AudioOutStop)
    }
}
