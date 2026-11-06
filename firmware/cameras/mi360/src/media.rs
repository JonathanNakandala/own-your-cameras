//! One main-thread owner for the shared IMP video/System and optional PCM audio.
//! Provider qualification and stock exclusion precede initialization. SDK calls
//! may block internally; the outer process supervisor owns deadline escalation.
pub use crate::h264::VideoMetadata;
use std::io;

/// Exact fixed-width O32 record in sdk/imp_bridge.h. Bitrates are kbit/s.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RustVideoSpec {
    pub main_width: u32,
    pub main_height: u32,
    pub sub_width: u32,
    pub sub_height: u32,
    pub main_profile: u32,
    pub sub_profile: u32,
    pub main_fps: u32,
    pub sub_fps: u32,
    pub main_bitrate: u32,
    pub sub_bitrate: u32,
}

impl Default for RustVideoSpec {
    fn default() -> Self {
        Self {
            main_width: 1920,
            main_height: 1080,
            sub_width: 640,
            sub_height: 360,
            main_profile: 66,
            sub_profile: 66,
            main_fps: 20,
            sub_fps: 20,
            main_bitrate: 800,
            sub_bitrate: 240,
        }
    }
}

const _: () = assert!(std::mem::size_of::<RustVideoSpec>() == 40);
const _: () = assert!(std::mem::align_of::<RustVideoSpec>() == 4);

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "mips", feature = "imp-video")
))]
impl RustVideoSpec {
    fn expected(self, channel: usize) -> Option<VideoMetadata> {
        match channel {
            0 => Some(VideoMetadata {
                width: self.main_width,
                height: self.main_height,
                profile_idc: self.main_profile,
            }),
            1 => Some(VideoMetadata {
                width: self.sub_width,
                height: self.sub_height,
                profile_idc: self.sub_profile,
            }),
            _ => None,
        }
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "mips", feature = "imp-video")
))]
struct MetadataState {
    requested: RustVideoSpec,
    actual: [Option<VideoMetadata>; 2],
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "mips", feature = "imp-video")
))]
impl MetadataState {
    fn new(requested: RustVideoSpec) -> Self {
        Self {
            requested,
            actual: [None, None],
        }
    }

    // Returns false until this channel has produced its own matching SPS.
    fn observe(&mut self, channel: usize, data: &[u8]) -> io::Result<bool> {
        let expected = self.requested.expected(channel).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "video channel must be0 or1")
        })?;
        if let Some(actual) = crate::h264::metadata(data)? {
            if actual != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "channel{channel} encoded SPS differs from requested video spec: actual{actual:?}, requested{expected:?}"
                    ),
                ));
            }
            self.actual[channel] = Some(actual);
        }
        Ok(self.actual[channel].is_some())
    }
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
}

/// 40 ms of 48 kHz mono microphone samples.
pub struct AudioFrame {
    pub samples: [i16; 1920],
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlResult {
    Applied,
    Pending,
    Unverified,
}

#[cfg(all(target_os = "linux", target_arch = "mips", feature = "imp-video"))]
mod target {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void};
    use std::fs::File;
    use std::marker::PhantomData;
    use std::rc::Rc;

    const VIDEO_CAPACITY: usize = 1024 * 1024;
    /// 40 ms at 48 kHz (capture) and at 16 kHz (playback), PCM16 mono.
    const CAPTURE_BYTES: usize = 3840;
    const PLAY_BYTES: usize = 1280;
    const ERROR_CAPACITY: usize = 512;

    unsafe extern "C" {
        fn t31_controls_get(
            sdk: *mut c_void,
            control: u32,
            value: *mut i32,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_controls_set(
            sdk: *mut c_void,
            control: u32,
            value: i32,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_tuning_get(
            sdk: *mut c_void,
            control: u32,
            value: *mut i32,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_tuning_set(
            sdk: *mut c_void,
            control: u32,
            value: i32,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_video_open_spec(
            handle: *mut *mut c_void,
            spec: *const RustVideoSpec,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_video_sdk(handle: *mut c_void) -> *mut c_void;
        fn t31_video_next_channel(
            handle: *mut c_void,
            channel: c_uint,
            data: *mut u8,
            capacity: u32,
            length: *mut u32,
            timestamp: *mut i64,
            error: *mut c_char,
            error_capacity: u32,
        ) -> c_int;
        fn t31_video_night(
            handle: *mut c_void,
            night: c_int,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_video_close(handle: *mut c_void, error: *mut c_char, cap: u32) -> c_int;
        fn t31_audio_open(
            handle: *mut *mut c_void,
            sdk: *mut c_void,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_audio_next(
            handle: *mut c_void,
            data: *mut u8,
            capacity: u32,
            length: *mut u32,
            timestamp: *mut i64,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_audio_play(
            handle: *mut c_void,
            data: *const u8,
            length: u32,
            error: *mut c_char,
            cap: u32,
        ) -> c_int;
        fn t31_audio_close(handle: *mut *mut c_void, error: *mut c_char, cap: u32) -> c_int;
    }

    fn diagnostic(operation: &str, code: c_int, bytes: &[u8]) -> String {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let detail = String::from_utf8_lossy(&bytes[..end]);
        format!("{operation} failed ({code}): {detail}")
    }

    /// Keep on its opening thread; all C entry points are serialized by this owner.
    pub struct Media {
        video: *mut c_void,
        audio: *mut c_void,
        lock: Option<File>,
        video_scratch: Vec<u8>,
        last_video: [Option<u64>; 2],
        metadata: MetadataState,
        last_audio: Option<u64>,
        fatal: bool,
        retain_video: bool,
        closed: bool,
        close_failure: Option<String>,
        _main_thread: PhantomData<Rc<()>>,
    }

    impl Media {
        pub fn control_get(&mut self, control: u32) -> io::Result<i32> {
            self.ready()?;
            let mut value = 0_i32;
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: called only by the serialized media owner while its SDK/System are alive.
            let code = unsafe {
                let get = if control >= 30 {
                    t31_tuning_get
                } else {
                    t31_controls_get
                };
                get(
                    t31_video_sdk(self.video),
                    control,
                    &mut value,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            if code == 0 {
                Ok(value)
            } else {
                Err(io::Error::other(diagnostic("control read", code, &error)))
            }
        }

        pub fn control_set(&mut self, control: u32, value: i32) -> io::Result<ControlResult> {
            self.ready()?;
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: the borrowed adapter neither creates endpoints nor initializes/unloads providers.
            let code = unsafe {
                let set = if control >= 30 {
                    t31_tuning_set
                } else {
                    t31_controls_set
                };
                set(
                    t31_video_sdk(self.video),
                    control,
                    value,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            match code {
                0 => Ok(ControlResult::Applied),
                1 => Ok(ControlResult::Pending),
                2 => Ok(ControlResult::Unverified),
                _ => Err(io::Error::other(diagnostic("control write", code, &error))),
            }
        }

        pub fn open(audio: bool) -> io::Result<Self> {
            Self::open_spec(audio, RustVideoSpec::default())
        }

        pub fn open_spec(audio: bool, spec: RustVideoSpec) -> io::Result<Self> {
            let lock = crate::capture::exclude_stock()?;
            crate::sdk::verify_library()?;
            let mut scratch = Vec::new();
            scratch
                .try_reserve_exact(VIDEO_CAPACITY)
                .map_err(io::Error::other)?;
            scratch.resize(VIDEO_CAPACITY, 0);
            let mut media = Self {
                video: std::ptr::null_mut(),
                audio: std::ptr::null_mut(),
                lock: Some(lock),
                video_scratch: scratch,
                last_video: [None, None],
                metadata: MetadataState::new(spec),
                last_audio: None,
                fatal: false,
                retain_video: false,
                closed: false,
                close_failure: None,
                _main_thread: PhantomData,
            };
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: sole process media owner; the shim owns all vendor records.
            let code = unsafe {
                t31_video_open_spec(
                    &mut media.video,
                    &spec,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            if code != 0 || media.video.is_null() {
                let message = diagnostic("dual video open", code, &error);
                if message.contains("POISONED") || !media.video.is_null() {
                    media.retain_video = true;
                    media.fatal = true;
                }
                return Err(io::Error::other(message));
            }
            if audio {
                // SAFETY: borrows this live video's already qualified SDK/System.
                let sdk = unsafe { t31_video_sdk(media.video) };
                if sdk.is_null() {
                    return Err(media.audio_failure("cannot borrow the shared SDK handle".into()));
                }
                error.fill(0);
                // SAFETY: borrowed live SDK handle; the adapter writes only the NULL-initialised owner.
                let code = unsafe {
                    t31_audio_open(
                        &mut media.audio,
                        sdk,
                        error.as_mut_ptr().cast(),
                        error.len() as u32,
                    )
                };
                if code != 0 || media.audio.is_null() {
                    return Err(media.audio_failure(diagnostic("audio open", code, &error)));
                }
            }
            Ok(media)
        }

        fn ready(&self) -> io::Result<()> {
            if self.closed || self.video.is_null() {
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "media session is closed",
                ))
            } else if self.fatal {
                Err(io::Error::other(
                    "media session failed; close and supervise recovery",
                ))
            } else {
                Ok(())
            }
        }

        fn audio_failure(&mut self, message: String) -> io::Error {
            self.fatal = true;
            self.retain_video = true;
            io::Error::other(format!(
                "{message}; shared video/System retained; clean boot required before reuse"
            ))
        }

        /// Actual parsed SPS observation, never a cached SDK setting.
        pub fn video_metadata(&self, channel: usize) -> Option<VideoMetadata> {
            self.metadata.actual.get(channel).copied().flatten()
        }

        pub fn next_video(&mut self, channel: usize) -> io::Result<Option<EncodedFrame>> {
            self.ready()?;
            if channel >= 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "video channel must be0 or1",
                ));
            }
            let mut length = 0_u32;
            let mut timestamp = 0_i64;
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: scratch and outputs are valid disjoint allocations; the
            // C shim finishes/releases its SDK borrow before returning.
            let code = unsafe {
                t31_video_next_channel(
                    self.video,
                    channel as c_uint,
                    self.video_scratch.as_mut_ptr(),
                    self.video_scratch.len() as u32,
                    &mut length,
                    &mut timestamp,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            if code == 1 {
                return Ok(None);
            }
            if code != 0 {
                self.fatal = true;
                return Err(io::Error::other(diagnostic("video next", code, &error)));
            }
            if length == 0 || length as usize > self.video_scratch.len() || timestamp < 0 {
                self.fatal = true;
                self.retain_video = true;
                return Err(io::Error::other(
                    "invalid copied video result; clean boot required",
                ));
            }
            let stamp = timestamp as u64;
            if self.last_video[channel].is_some_and(|last| stamp < last) {
                self.fatal = true;
                return Err(io::Error::other("video timestamp regressed"));
            }
            let bytes = &self.video_scratch[..length as usize];
            let qualified = match self.metadata.observe(channel, bytes) {
                Ok(qualified) => qualified,
                Err(error) => {
                    self.fatal = true;
                    return Err(error);
                }
            };
            self.last_video[channel] = Some(stamp);
            if !qualified {
                // Do not publish, count healthy frames, or claim applied settings
                // before an SPS from this actual channel establishes its format.
                return Ok(None);
            }
            let mut data = Vec::new();
            data.try_reserve_exact(length as usize)
                .map_err(io::Error::other)?;
            data.extend_from_slice(&self.video_scratch[..length as usize]);
            Ok(Some(EncodedFrame {
                data,
                timestamp_us: stamp,
            }))
        }

        pub fn next_audio(&mut self) -> io::Result<Option<AudioFrame>> {
            self.ready()?;
            if self.audio.is_null() {
                return Ok(None);
            }
            let mut pcm = [0_u8; CAPTURE_BYTES];
            let mut length = 0_u32;
            let mut timestamp = 0_i64;
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: exact copied-PCM interface; no SDK pointer leaves C.
            let code = unsafe {
                t31_audio_next(
                    self.audio,
                    pcm.as_mut_ptr(),
                    pcm.len() as u32,
                    &mut length,
                    &mut timestamp,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            if code == 1 {
                return Ok(None);
            }
            if code != 0 {
                return Err(self.audio_failure(diagnostic("audio next", code, &error)));
            }
            if length as usize != CAPTURE_BYTES || timestamp < 0 {
                return Err(self.audio_failure("invalid copied PCM result".into()));
            }
            let stamp = timestamp as u64;
            if self.last_audio.is_some_and(|last| stamp < last) {
                return Err(self.audio_failure("audio timestamp regressed".into()));
            }
            let mut samples = [0_i16; CAPTURE_BYTES / 2];
            for (sample, bytes) in samples.iter_mut().zip(pcm.chunks_exact(2)) {
                *sample = i16::from_le_bytes([bytes[0], bytes[1]]);
            }
            self.last_audio = Some(stamp);
            Ok(Some(AudioFrame {
                samples,
                timestamp_us: stamp,
            }))
        }

        /// Returns false for a full SDK queue. Caller owns pacing/expiry and retries.
        pub fn play(&mut self, samples: &[i16]) -> io::Result<bool> {
            self.ready()?;
            if samples.len() != 640 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "play needs exactly640 PCM samples",
                ));
            }
            if self.audio.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "audio was not opened",
                ));
            }
            let mut pcm = [0_u8; PLAY_BYTES];
            for (bytes, sample) in pcm.chunks_exact_mut(2).zip(samples) {
                bytes.copy_from_slice(&sample.to_le_bytes());
            }
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: C copies the complete supplied block before returning.
            let code = unsafe {
                t31_audio_play(
                    self.audio,
                    pcm.as_ptr(),
                    pcm.len() as u32,
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            match code {
                0 => Ok(true),
                1 => Ok(false),
                _ => Err(self.audio_failure(diagnostic("audio play", code, &error))),
            }
        }

        pub fn set_night(&mut self, night: bool) -> io::Result<()> {
            self.ready()?;
            let mut error = [0_u8; ERROR_CAPACITY];
            // SAFETY: serialized live ISP owner; physical IR controls are separate.
            let code = unsafe {
                t31_video_night(
                    self.video,
                    c_int::from(night),
                    error.as_mut_ptr().cast(),
                    error.len() as u32,
                )
            };
            if code != 0 {
                self.fatal = true;
                return Err(io::Error::other(diagnostic("ISP night", code, &error)));
            }
            Ok(())
        }

        pub fn close(&mut self) -> io::Result<()> {
            if self.closed {
                return self
                    .close_failure
                    .as_ref()
                    .map_or(Ok(()), |m| Err(io::Error::other(m.clone())));
            }
            self.closed = true;
            let mut failures = Vec::new();
            if !self.audio.is_null() {
                let mut error = [0_u8; ERROR_CAPACITY];
                // C consumes its handle once, including a poisoned failure.
                let code = unsafe {
                    t31_audio_close(
                        &mut self.audio,
                        error.as_mut_ptr().cast(),
                        error.len() as u32,
                    )
                };
                if code != 0 || !self.audio.is_null() {
                    self.retain_video = true;
                    failures.push(diagnostic("audio close", code, &error));
                }
            }
            if self.retain_video {
                failures
                    .push("shared video/System retained; clean boot required before reuse".into());
            } else if !self.video.is_null() {
                let pointer = std::mem::replace(&mut self.video, std::ptr::null_mut());
                let mut error = [0_u8; ERROR_CAPACITY];
                let code = unsafe {
                    t31_video_close(pointer, error.as_mut_ptr().cast(), error.len() as u32)
                };
                if code != 0 {
                    self.retain_video = true;
                    failures.push(diagnostic("video close", code, &error));
                }
            }
            if self.retain_video {
                // Keep the same inode locked until process exit, including when
                // this Rust wrapper drops; SDK worker ownership may remain live.
                if let Some(lock) = self.lock.take() {
                    std::mem::forget(lock);
                }
            } else {
                self.lock.take();
            }
            if failures.is_empty() {
                Ok(())
            } else {
                let message = failures.join("; ");
                self.close_failure = Some(message.clone());
                Err(io::Error::other(message))
            }
        }
    }

    impl Drop for Media {
        fn drop(&mut self) {
            if !self.closed {
                if let Err(error) = self.close() {
                    eprintln!("Media cleanup: {error}");
                }
            }
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "mips", feature = "imp-video"))]
pub use target::Media;

#[cfg(not(all(target_os = "linux", target_arch = "mips", feature = "imp-video")))]
pub struct Media {
    _private: (),
}

#[cfg(not(all(target_os = "linux", target_arch = "mips", feature = "imp-video")))]
impl Media {
    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "IMP media requires the MIPS camera build",
        ))
    }
    pub fn open(audio: bool) -> io::Result<Self> {
        Self::open_spec(audio, RustVideoSpec::default())
    }
    pub fn open_spec(_: bool, _: RustVideoSpec) -> io::Result<Self> {
        Self::unsupported()
    }
    pub fn video_metadata(&self, _: usize) -> Option<VideoMetadata> {
        None
    }
    pub fn control_get(&mut self, _: u32) -> io::Result<i32> {
        Self::unsupported()
    }
    pub fn control_set(&mut self, _: u32, _: i32) -> io::Result<ControlResult> {
        Self::unsupported()
    }
    pub fn next_video(&mut self, _: usize) -> io::Result<Option<EncodedFrame>> {
        Self::unsupported()
    }
    pub fn next_audio(&mut self) -> io::Result<Option<AudioFrame>> {
        Self::unsupported()
    }
    pub fn play(&mut self, _: &[i16]) -> io::Result<bool> {
        Self::unsupported()
    }
    pub fn set_night(&mut self, _: bool) -> io::Result<()> {
        Self::unsupported()
    }
    pub fn close(&mut self) -> io::Result<()> {
        Self::unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::tests::{REAL_MAIN, REAL_SUB, annexb};

    #[test]
    fn video_spec_matches_all_ten_fixed_width_ffi_offsets() {
        assert_eq!(std::mem::size_of::<RustVideoSpec>(), 40);
        assert_eq!(std::mem::align_of::<RustVideoSpec>(), 4);
        assert_eq!(
            [
                std::mem::offset_of!(RustVideoSpec, main_width),
                std::mem::offset_of!(RustVideoSpec, main_height),
                std::mem::offset_of!(RustVideoSpec, sub_width),
                std::mem::offset_of!(RustVideoSpec, sub_height),
                std::mem::offset_of!(RustVideoSpec, main_profile),
                std::mem::offset_of!(RustVideoSpec, sub_profile),
                std::mem::offset_of!(RustVideoSpec, main_fps),
                std::mem::offset_of!(RustVideoSpec, sub_fps),
                std::mem::offset_of!(RustVideoSpec, main_bitrate),
                std::mem::offset_of!(RustVideoSpec, sub_bitrate),
            ],
            [0, 4, 8, 12, 16, 20, 24, 28, 32, 36]
        );
        let spec = RustVideoSpec::default();
        assert_eq!(
            (
                spec.main_width,
                spec.main_height,
                spec.sub_width,
                spec.sub_height
            ),
            (1920, 1080, 640, 360)
        );
        assert_eq!(
            (
                spec.main_profile,
                spec.sub_profile,
                spec.main_fps,
                spec.sub_fps,
                spec.main_bitrate,
                spec.sub_bitrate
            ),
            (66, 66, 20, 20, 800, 240)
        );
    }

    #[test]
    fn metadata_requires_real_sps_per_channel_and_rejects_mismatch() {
        let mut state = MetadataState::new(RustVideoSpec::default());
        let no_sps = [0, 0, 1, 0x65, 0x80];
        assert!(!state.observe(0, &no_sps).unwrap());
        assert_eq!(state.actual, [None, None]);
        assert!(state.observe(0, &annexb(REAL_MAIN)).unwrap());
        assert!(!state.observe(1, &no_sps).unwrap());
        assert!(state.observe(1, &annexb(REAL_SUB)).unwrap());
        assert!(state.observe(0, &no_sps).unwrap());
        assert!(state.observe(0, &annexb(REAL_SUB)).is_err());
        assert!(state.observe(2, &annexb(REAL_SUB)).is_err());
        let mut wrong_profile = MetadataState::new(RustVideoSpec {
            main_profile: 100,
            ..RustVideoSpec::default()
        });
        assert!(wrong_profile.observe(0, &annexb(REAL_MAIN)).is_err());
        assert_eq!(
            wrong_profile.actual[0], None,
            "cached requested profile must never count as observed"
        );
        assert!(state.observe(0, &[0, 0, 1, 0x67]).is_err());
    }

    #[cfg(not(all(target_os = "linux", target_arch = "mips", feature = "imp-video")))]
    #[test]
    fn host_media_keeps_unsupported_semantics_and_unknown_metadata() {
        for result in [
            Media::open(false),
            Media::open_spec(true, RustVideoSpec::default()),
        ] {
            assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Unsupported);
        }
        let media = Media { _private: () };
        assert_eq!(media.video_metadata(0), None);
        assert_eq!(media.video_metadata(2), None);
    }
}
