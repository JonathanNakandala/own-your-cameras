//! Camera HDAL 2.00 audio. Owned resources unwind in reverse on every startup/receive error.
//! Constants and speaker copy ownership: docs/hardware/audio.md. No common init/uninit here.
use super::h;
use super::{Sys, check, finish, region, valid_extent, valid_pointer};
use crate::audio::{Aec, Anr, AudioCap, AudioOut, PcmFrame, RATE, Tuning};
use crate::{Error, Step};

struct Api {
    init: unsafe extern "C" fn() -> h::HD_RESULT,
    uninit: unsafe extern "C" fn() -> h::HD_RESULT,
    open: unsafe extern "C" fn(h::HD_IN_ID, h::HD_OUT_ID, *mut h::HD_PATH_ID) -> h::HD_RESULT,
    close: unsafe extern "C" fn(h::HD_PATH_ID) -> h::HD_RESULT,
    start: unsafe extern "C" fn(h::HD_PATH_ID) -> h::HD_RESULT,
    stop: unsafe extern "C" fn(h::HD_PATH_ID) -> h::HD_RESULT,
    set: unsafe extern "C" fn(h::HD_PATH_ID, u32, *mut h::ctypes::c_void) -> h::HD_RESULT,
}
static CAP: Api = Api {
    init: h::hd_audiocap_init,
    uninit: h::hd_audiocap_uninit,
    open: h::hd_audiocap_open,
    close: h::hd_audiocap_close,
    start: h::hd_audiocap_start,
    stop: h::hd_audiocap_stop,
    set: h::hd_audiocap_set,
};
static OUT: Api = Api {
    init: h::hd_audioout_init,
    uninit: h::hd_audioout_uninit,
    open: h::hd_audioout_open,
    close: h::hd_audioout_close,
    start: h::hd_audioout_start,
    stop: h::hd_audioout_stop,
    set: h::hd_audioout_set,
};

#[derive(Clone, Copy)]
struct Mapping {
    phy: usize,
    va: *mut u8,
    size: usize,
}
enum Release {
    Init(&'static Api),
    Path(&'static Api, h::HD_PATH_ID),
    Started(&'static Api, h::HD_PATH_ID),
    Map(Mapping),
    Alloc(Mapping),
}
pub(super) type SpeakerEnable = Box<dyn FnMut(bool) -> Result<(), i32> + Send>;
pub(super) type MicGain = Box<dyn FnMut(u8) -> Result<(), i32> + Send>;
impl Release {
    fn name(&self) -> &'static str {
        match self {
            Self::Init(_) => "uninit",
            Self::Path(_, _) => "close",
            Self::Started(_, _) => "stop",
            Self::Map(_) => "munmap",
            Self::Alloc(_) => "mem_free",
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    failed_cleanup: bool,
    amplifier: Option<SpeakerEnable>,
    resources: Vec<Release>,
    ctrl: h::HD_PATH_ID,
    io: h::HD_PATH_ID,
    map: Option<Mapping>,
}
impl State {
    pub(super) fn live(&self) -> bool {
        !self.resources.is_empty()
    }
    fn init(&mut self, api: &'static Api, step: Step) -> Result<(), Error> {
        // SAFETY: common HDAL is owned by the calling video graph; vendor device is stopped.
        check(
            step,
            super::ioctl_capture::result(|| unsafe { (api.init)() }),
        )?;
        self.resources.push(Release::Init(api));
        Ok(())
    }
    fn open(
        &mut self,
        api: &'static Api,
        input: u32,
        output: u32,
        step: Step,
    ) -> Result<u32, Error> {
        let mut path = 0;
        // SAFETY: valid path out pointer; the selected audio unit is initialised.
        check(
            step,
            super::ioctl_capture::result(|| unsafe { (api.open)(input, output, &mut path) }),
        )?;
        self.resources.push(Release::Path(api, path));
        Ok(path)
    }
    fn start(&mut self, api: &'static Api, path: u32, step: Step) -> Result<(), Error> {
        // SAFETY: path has been opened and configured by this state.
        check(
            step,
            super::ioctl_capture::result(|| unsafe { (api.start)(path) }),
        )?;
        self.resources.push(Release::Started(api, path));
        Ok(())
    }
    fn close(&mut self, step: Step) -> Result<(), Error> {
        let mut first = None;
        if let Some(enable) = self.amplifier.as_mut()
            && let Err(code) = enable(false)
        {
            self.failed_cleanup = true;
            first = Some(Error { step, code });
        }
        while let Some(r) = self.resources.pop() {
            // SAFETY: each successful acquisition appends exactly one matching release; reverse
            // traversal stops paths before closing them and uninitialises only opened units.
            let rc = super::ioctl_capture::result(|| unsafe {
                match r {
                    Release::Init(api) => (api.uninit)(),
                    Release::Path(api, path) => (api.close)(path),
                    Release::Started(api, path) => (api.stop)(path),
                    Release::Map(m) => h::hd_common_mem_munmap(m.va.cast(), m.size as u32),
                    Release::Alloc(m) => h::hd_common_mem_free(m.phy as _, m.va.cast()),
                }
            });
            if let Err(e) = check(step, rc) {
                eprintln!("HDAL audio cleanup {}: {e}", r.name());
                self.failed_cleanup = true;
                first.get_or_insert(e);
            }
        }
        self.map = None;
        first.map_or(Ok(()), Err)
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Err(e) = self.close(Step::AudioCapStop) {
            eprintln!("audio unwind: {e}");
        }
    }
}
fn set<T>(api: &Api, path: u32, param: u32, value: &mut T, step: Step) -> Result<(), Error> {
    // SAFETY: call sites pair each exact ABI-verified param id with its struct; value lives through call.
    check(
        step,
        super::ioctl_capture::result(|| unsafe {
            (api.set)(path, param, (value as *mut T).cast()) as i32
        }) as _,
    )
}
/// Filter length, frame size and loopback channel size the AEC buffers; they never change.
fn hd_aec(a: &Aec) -> h::HD_AUDIOCAP_AEC {
    h::HD_AUDIOCAP_AEC {
        enabled: a.enabled.into(),
        leak_estimate_enabled: a.leak_estimate.into(),
        leak_estimate_value: a.leak_value,
        noise_cancel_level: a.noise_level,
        echo_cancel_level: a.echo_level,
        filter_length: 1024,
        frame_size: 128,
        notch_radius: a.notch_radius,
        lb_channel: h::HD_AUDIOCAP_LB_CH_LEFT,
    }
}
fn hd_anr(a: &Anr) -> h::HD_AUDIOCAP_ANR {
    h::HD_AUDIOCAP_ANR {
        enabled: a.enabled.into(),
        suppress_level: a.suppress_level,
        hpf_cut_off_freq: a.hpf_cutoff,
        bias_sensitive: a.bias_sensitive,
    }
}
/// HDAL capture volume, fixed: see `audio_cap_start`.
const HDAL_MIC_VOLUME: u8 = 100;
fn volume(v: u8) -> u32 {
    u32::from(v.min(100))
}
impl AudioCap for Sys {
    fn audio_cap_start(&mut self, tuning: &Tuning) -> Result<(), Error> {
        self.forward(Step::AudioCapStart)?;
        let step = Step::AudioCapStart;
        if self.audio_cap.failed_cleanup || !self.audio_cap.resources.is_empty() {
            return Err(Error { step, code: -16 });
        }
        let mut s = State::default();
        let result = (|| {
            s.init(&CAP, step)?;
            let ctrl = s.open(&CAP, 0, h::HD_AUDIOCAP_0_CTRL, step)?;
            let mut cfg = h::HD_AUDIOCAP_DEV_CONFIG::default();
            // Raw 48 kHz capture, pulled from the capture unit itself: no audio encoder unit.
            cfg.in_max = h::HD_AUDIOCAP_IN {
                sample_rate: RATE,
                sample_bit: 16,
                mode: 1,
                frame_sample: 1024,
            };
            cfg.out_max.sample_rate = RATE;
            cfg.frame_num_max = 10;
            // The envelope stays stock: a disabled maximum could forbid enabling later.
            cfg.aec_max = hd_aec(&Tuning::stock().aec);
            cfg.anr_max = hd_anr(&Tuning::stock().anr);
            set(&CAP, ctrl, h::HD_AUDIOCAP_PARAM_DEV_CONFIG, &mut cfg, step)?;
            set(
                &CAP,
                ctrl,
                h::HD_AUDIOCAP_PARAM_DRV_CONFIG,
                &mut h::HD_AUDIOCAP_DRV_CONFIG::default(),
                step,
            )?;
            s.ctrl = ctrl;
            s.io = s.open(&CAP, h::HD_AUDIOCAP_0_IN_0, h::HD_AUDIOCAP_0_OUT_0, step)?;
            set(&CAP, s.io, h::HD_AUDIOCAP_PARAM_IN, &mut cfg.in_max, step)?;
            set(&CAP, s.io, h::HD_AUDIOCAP_PARAM_OUT, &mut cfg.out_max, step)?;
            set(
                &CAP,
                s.io,
                h::HD_AUDIOCAP_PARAM_OUT_AEC,
                &mut hd_aec(&tuning.aec),
                step,
            )?;
            set(
                &CAP,
                s.io,
                h::HD_AUDIOCAP_PARAM_OUT_ANR,
                &mut hd_anr(&tuning.anr),
                step,
            )?;
            s.start(&CAP, s.io, step)?;
            // Stock sets the volume last, on the started capture (item 0x19001 after the AEC/ANR sets and
            // the path start). Without the codec's ALC it scales the capture too; `mic_volume` drives the
            // analog PGA instead, so this stays at the 100 the level was chosen with.
            // Ctrl-path VOLUME reads one word (libhdal hd_audiocap_set @ 0xdddc, kflow item 0x19001).
            set(
                &CAP,
                s.ctrl,
                h::HD_AUDIOCAP_PARAM_VOLUME,
                &mut h::HD_AUDIOCAP_VOLUME {
                    volume: volume(HDAL_MIC_VOLUME),
                },
                step,
            )?;
            // HDAL's volume does not reach the codec, and the start re-enables the codec's ALC and noise
            // gate: set the fixed analog gain last.
            if let Some(gain) = self.mic_gain.as_mut() {
                gain(tuning.mic_volume).map_err(|code| Error { step, code })?;
            }

            // The ctrl path reports the page-aligned raw ring that every pulled frame lies in.
            let mut info = h::HD_AUDIOCAP_BUFINFO::default();
            // SAFETY: exact BUFINFO id and output struct, filled before mapping.
            check(
                step,
                super::ioctl_capture::result(|| unsafe {
                    h::hd_audiocap_get(
                        s.ctrl,
                        h::HD_AUDIOCAP_PARAM_BUFINFO,
                        (&mut info as *mut h::HD_AUDIOCAP_BUFINFO).cast(),
                    )
                }),
            )?;
            let phy = info.buf_info.phy_addr as usize;
            let size = info.buf_info.buf_size as usize;
            let va = self.map_buffer(phy, size, step)?;
            let map = Mapping {
                phy,
                size,
                va: va.cast(),
            };
            s.resources.push(Release::Map(map));
            s.map = Some(map);
            Ok(())
        })();
        let result = if result.is_err() {
            let cleanup = s.close(step);
            finish(result, cleanup, "audio cap startup unwind")
        } else {
            result
        };
        self.terminal |= s.failed_cleanup;
        self.audio_cap = s;
        result
    }
    fn audio_cap_recv(&mut self) -> Result<Option<PcmFrame>, Error> {
        self.forward(Step::AudioCapRecv)?;
        let step = Step::AudioCapRecv;
        let s = &self.audio_cap;
        let map = s.map.ok_or(Error { step, code: -22 })?;
        let mut frame = h::HD_AUDIO_FRAME::default();
        // SAFETY: valid descriptor; nonblocking pull on the running capture path owned by this state.
        let rc = super::ioctl_capture::result(|| unsafe {
            h::hd_audiocap_pull_out_buf(s.io, &mut frame, 0)
        });
        if matches!(rc as i32, h::HD_ERR_TIMEDOUT | h::HD_ERR_UNDERRUN) {
            return Ok(None);
        }
        check(step, rc)?;
        let copied = (|| {
            let invalid = Error { step, code: -22 };
            let range = region(
                frame.phy_addr[0] as usize,
                frame.size as usize,
                map.phy,
                map.size,
            )
            .ok_or(invalid)?;
            if frame.sample_rate != RATE
                || frame.bit_width != 16
                || frame.sound_mode != 1
                || !frame.size.is_multiple_of(2)
                || frame.size > 8192
            {
                return Err(invalid);
            }
            // SAFETY: checked nonempty range in the live cached mapping; DMA wrote these bytes.
            let ptr = unsafe { map.va.add(range.start) };
            // SAFETY: same mapped range; cache maintenance must succeed before CPU access.
            check(
                step,
                super::ioctl_capture::result(|| unsafe {
                    h::hd_common_mem_flush_cache(ptr.cast(), frame.size)
                }),
            )?;
            // SAFETY: checked readable range; copy before releasing the pulled descriptor.
            let bytes = unsafe { std::slice::from_raw_parts(ptr, range.len()) };
            Ok(bytes
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>())
        })();
        // SAFETY: release the exact pulled descriptor even on bad bounds or cache failure.
        let release = check(
            step,
            super::ioctl_capture::result(|| unsafe {
                h::hd_audiocap_release_out_buf(s.io, &mut frame)
            }),
        );
        self.terminal |= release.is_err();
        let samples = finish(copied, release, "audiocap_release_out_buf")?;
        Ok(Some(PcmFrame {
            pts_us: frame.timestamp,
            samples,
        }))
    }
    fn audio_cap_stop(&mut self) -> Result<(), Error> {
        self.owned(Step::AudioCapStop)?;
        let result = self.audio_cap.close(Step::AudioCapStop);
        self.terminal |= result.is_err();
        result
    }
}
impl AudioOut for Sys {
    fn audio_out_start(&mut self, volume_level: u8) -> Result<(), Error> {
        self.forward(Step::AudioOutStart)?;
        let step = Step::AudioOutStart;
        if self.audio_out.failed_cleanup || !self.audio_out.resources.is_empty() {
            return Err(Error { step, code: -16 });
        }
        let mut s = State::default();
        let result = (|| {
            s.init(&OUT, step)?;
            let ctrl = s.open(&OUT, 0, h::HD_AUDIOOUT_0_CTRL, step)?;
            let mut cfg = h::HD_AUDIOOUT_DEV_CONFIG::default();
            cfg.out_max = h::HD_AUDIOOUT_OUT {
                sample_rate: RATE,
                sample_bit: 16,
                mode: 1,
            };
            cfg.frame_sample_max = 1024;
            cfg.frame_num_max = 8;
            cfg.in_max.sample_rate = RATE;
            set(&OUT, ctrl, h::HD_AUDIOOUT_PARAM_DEV_CONFIG, &mut cfg, step)?;
            set(
                &OUT,
                ctrl,
                h::HD_AUDIOOUT_PARAM_DRV_CONFIG,
                &mut h::HD_AUDIOOUT_DRV_CONFIG::default(),
                step,
            )?;
            set(
                &OUT,
                ctrl,
                h::HD_AUDIOOUT_PARAM_VOLUME,
                &mut h::HD_AUDIOOUT_VOLUME {
                    volume: volume(volume_level),
                },
                step,
            )?;
            s.ctrl = ctrl;
            s.io = s.open(&OUT, h::HD_AUDIOOUT_0_IN_0, h::HD_AUDIOOUT_0_OUT_0, step)?;
            set(&OUT, s.io, h::HD_AUDIOOUT_PARAM_OUT, &mut cfg.out_max, step)?;
            set(&OUT, s.io, h::HD_AUDIOOUT_PARAM_IN, &mut cfg.in_max, step)?;
            s.start(&OUT, s.io, step)?;
            let (mut phy, mut va) = (0, std::ptr::null_mut());
            // SAFETY: valid output pointers; alloc gives a physical HDAL-owned staging buffer plus mapping.
            check(
                step,
                super::ioctl_capture::result(|| unsafe {
                    h::hd_common_mem_alloc(
                        c"cam.speaker".as_ptr().cast_mut().cast(),
                        &mut phy,
                        &mut va,
                        2048,
                        h::DDR_ID0,
                    )
                }),
            )?;
            let map = Mapping {
                phy: phy as usize,
                va: va.cast(),
                size: 2048,
            };
            // A failed allocation has no ownership to release. Even a success status must not expose
            // sentinel pointers to Rust or pass them to mem_free (the camera ABI never returns these on success).
            if va.is_null() || va as usize == usize::MAX {
                return Err(Error { step, code: -12 });
            }
            s.resources.push(Release::Alloc(map));
            if !valid_extent(map.phy, map.size)
                || !valid_pointer(va, map.size)
                || !(va as usize).is_multiple_of(std::mem::align_of::<i16>())
            {
                return Err(Error { step, code: -22 });
            }
            s.map = Some(map);
            // HDAL drives the codec, but the E1 board's external PA is separately gated.
            // Retain cleanup ownership before either write, including partial enable failure.
            s.amplifier = self.speaker_enable.take();
            let enable = s.amplifier.as_mut().ok_or(Error { step, code: -19 })?;
            enable(false)
                .and_then(|()| enable(true))
                .map_err(|code| Error { step, code })?;
            Ok(())
        })();
        let result = if result.is_err() {
            let cleanup = s.close(step);
            if let Some(enable) = s.amplifier.take() {
                self.speaker_enable = Some(enable);
            }
            finish(result, cleanup, "audio out startup unwind")
        } else {
            result
        };
        self.terminal |= s.failed_cleanup;
        self.audio_out = s;
        result
    }
    fn audio_out_volume(&mut self, level: u8) -> Result<(), Error> {
        self.forward(Step::AudioOutVolume)?;
        let step = Step::AudioOutVolume;
        if self.audio_out.map.is_none() {
            return Err(Error { step, code: -22 });
        }
        set(
            &OUT,
            self.audio_out.ctrl,
            h::HD_AUDIOOUT_PARAM_VOLUME,
            &mut h::HD_AUDIOOUT_VOLUME {
                volume: volume(level),
            },
            step,
        )?;
        // The driver applies a ctrl-path volume only when the running io path is started again, as
        // the vendor setter does; the path is already owned and released as started, so no new resource.
        let io = self.audio_out.io;
        // SAFETY: io was opened and started by audio_out_start and stays open while map is set.
        check(
            step,
            super::ioctl_capture::result(|| unsafe { (OUT.start)(io) }),
        )
    }
    fn audio_out_push(&mut self, pcm: &[i16]) -> Result<bool, Error> {
        self.forward(Step::AudioOutPush)?;
        let step = Step::AudioOutPush;
        let map = self.audio_out.map.ok_or(Error { step, code: -22 })?;
        if pcm.len() > 1024 {
            return Err(Error { step, code: -22 });
        }
        if pcm.is_empty() {
            return Ok(true);
        }
        let size = std::mem::size_of_val(pcm) as u32;
        // SAFETY: the mapped, aligned 2,048-byte staging buffer fits at most 1,024 i16 samples.
        unsafe { std::ptr::copy_nonoverlapping(pcm.as_ptr(), map.va.cast(), pcm.len()) };
        // SAFETY: flush the written bytes so the kernel's physical-address mapping sees them.
        check(
            step,
            super::ioctl_capture::result(|| unsafe {
                h::hd_common_mem_flush_cache(map.va.cast(), size)
            }),
        )?;
        let mut frame = h::HD_AUDIO_FRAME::default();
        frame.sign = u32::from_le_bytes(*b"AFRM");
        frame.size = size;
        frame.phy_addr[0] = map.phy as _;
        frame.bit_width = 16;
        frame.sound_mode = 1;
        frame.sample_rate = RATE;
        // SAFETY: frame points into HDAL physical memory. The exact kernel's wavstudio push copies into
        // its own playback ring synchronously (docs/hardware/audio.md); staging can be reused after this returns.
        let rc = super::ioctl_capture::result(|| unsafe {
            h::hd_audioout_push_in_buf(self.audio_out.io, &mut frame, 0)
        });
        if matches!(rc as i32, h::HD_ERR_TIMEDOUT | h::HD_ERR_OVERRUN) {
            return Ok(false);
        }
        check(step, rc)?;
        Ok(true)
    }
    fn audio_out_stop(&mut self) -> Result<(), Error> {
        self.owned(Step::AudioOutStop)?;
        let result = self.audio_out.close(Step::AudioOutStop);
        if let Some(enable) = self.audio_out.amplifier.take() {
            self.speaker_enable = Some(enable);
        }
        self.terminal |= result.is_err();
        result
    }
}
