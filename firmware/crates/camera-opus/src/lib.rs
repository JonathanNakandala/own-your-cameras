//! libopus adapter for the camera transport: 48 kHz mono microphone encoder, mono speaker decoders at the
//! camera's speaker rate.
use camera_transport::microphone::Microphone;
use camera_transport::opus::{MIC_FRAME, MIC_RATE, OpusDecode, OpusEncode, speaker_max};
use camera_transport::rtsp::talk::Talkback;
use std::ffi::c_int;
use std::sync::Arc;

/// The microphone path both cameras run: one encoder, its packets shared by every listener.
pub fn microphone() -> std::io::Result<Microphone<Encoder>> {
    Encoder::new()
        .map(Microphone::new)
        .ok_or_else(|| std::io::Error::other("Opus encoder setup failed"))
}

/// Speaker input at `rate` Hz with one libopus decoder per talk session.
pub fn talkback(rate: u32) -> Arc<Talkback> {
    Talkback::new(
        Box::new(move || Decoder::new(rate).map(|d| Box::new(d) as Box<dyn OpusDecode>)),
        rate,
    )
}

#[repr(C)]
struct OpusEncoder {
    _private: [u8; 0],
}
#[repr(C)]
struct OpusDecoder {
    _private: [u8; 0],
}

const OPUS_OK: c_int = 0;
const OPUS_APPLICATION_AUDIO: c_int = 2049;
const OPUS_SET_BITRATE: c_int = 4002;
const OPUS_SET_COMPLEXITY: c_int = 4010;
/// Largest packet the encoder may produce (one frame; ~160 B are typical at 64 kbit/s).
const MAX_PACKET: usize = 1275;
/// Largest talker packet accepted: RFC 6716 allows up to 48 frames of 1,275 B in one packet; 120 ms of
/// 20 ms frames (6 × 1,275 B) covers every packet a talker can usefully send.
const MAX_TALK_PACKET: usize = 6 * 1275;

unsafe extern "C" {
    fn opus_encoder_create(
        fs: i32,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut OpusEncoder;
    fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    fn opus_encode(
        st: *mut OpusEncoder,
        pcm: *const i16,
        frame_size: c_int,
        data: *mut u8,
        max: i32,
    ) -> i32;
    fn opus_encoder_destroy(st: *mut OpusEncoder);
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decode(
        st: *mut OpusDecoder,
        data: *const u8,
        len: i32,
        pcm: *mut i16,
        frame_size: c_int,
        fec: c_int,
    ) -> c_int;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
}

/// 48 kHz mono, `OPUS_APPLICATION_AUDIO`, complexity 5, 64 kbit/s.
pub struct Encoder(*mut OpusEncoder);

// The state is owned exclusively by this value and libopus keeps no thread-local state.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new() -> Option<Self> {
        let mut error = 0;
        let st =
            unsafe { opus_encoder_create(MIC_RATE as i32, 1, OPUS_APPLICATION_AUDIO, &mut error) };
        if st.is_null() || error != OPUS_OK {
            return None;
        }
        let encoder = Self(st);
        let ok = unsafe {
            opus_encoder_ctl(st, OPUS_SET_BITRATE, 64_000 as c_int) == OPUS_OK
                && opus_encoder_ctl(st, OPUS_SET_COMPLEXITY, 5 as c_int) == OPUS_OK
        };
        ok.then_some(encoder)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.0) }
    }
}

impl OpusEncode for Encoder {
    fn encode(&mut self, frame: &[i16; MIC_FRAME]) -> Option<Vec<u8>> {
        let mut out = vec![0; MAX_PACKET];
        let n = unsafe {
            opus_encode(
                self.0,
                frame.as_ptr(),
                MIC_FRAME as c_int,
                out.as_mut_ptr(),
                MAX_PACKET as i32,
            )
        };
        let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
        out.truncate(n);
        Some(out)
    }
}

/// Mono decoder at the speaker's rate; one per talker.
pub struct Decoder {
    st: *mut OpusDecoder,
    max: usize,
}

unsafe impl Send for Decoder {}

impl Decoder {
    /// `rate` is one of libopus's 8, 12, 16, 24 or 48 kHz.
    pub fn new(rate: u32) -> Option<Self> {
        let mut error = 0;
        let st = unsafe { opus_decoder_create(rate as i32, 1, &mut error) };
        (!st.is_null() && error == OPUS_OK).then_some(Self {
            st,
            max: speaker_max(rate),
        })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.st) }
    }
}

impl OpusDecode for Decoder {
    fn decode(&mut self, payload: &[u8]) -> Option<Vec<i16>> {
        if payload.is_empty() || payload.len() > MAX_TALK_PACKET {
            return None;
        }
        let mut pcm = vec![0; self.max];
        let n = unsafe {
            opus_decode(
                self.st,
                payload.as_ptr(),
                payload.len() as i32,
                pcm.as_mut_ptr(),
                self.max as c_int,
                0,
            )
        };
        let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
        pcm.truncate(n);
        Some(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: f32, start: usize, len: usize) -> Vec<i16> {
        (start..start + len)
            .map(|i| (8000.0 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate).sin()) as i16)
            .collect()
    }

    #[test]
    fn a_tone_survives_the_roundtrip_at_the_configured_bitrate() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = Decoder::new(16_000).unwrap();
        let (mut bytes, mut decoded) = (0, Vec::new());
        for frame in 0..50 {
            let pcm: [i16; MIC_FRAME] = tone(1000.0, 48_000.0, frame * MIC_FRAME, MIC_FRAME)
                .try_into()
                .unwrap();
            let packet = encoder.encode(&pcm).unwrap();
            bytes += packet.len();
            let out = decoder.decode(&packet).unwrap();
            assert_eq!(out.len(), 320);
            decoded.extend(out);
        }
        // 64 kbit/s at 20 ms is 160 B a packet; allow the encoder's rate control some room.
        let average = bytes / 50;
        assert!((120..=200).contains(&average), "{average} B/packet");
        // Compare the second half with the reference tone at the best alignment (codec delay).
        let tail = &decoded[decoded.len() / 2..];
        let best = (0..320)
            .map(|lag| {
                let reference = tone(1000.0, 16_000.0, decoded.len() / 2 - lag, tail.len());
                let (signal, noise) =
                    tail.iter()
                        .zip(&reference)
                        .fold((0f64, 0f64), |(s, n), (&y, &x)| {
                            (
                                s + f64::from(x).powi(2),
                                n + (f64::from(y) - f64::from(x)).powi(2),
                            )
                        });
                10.0 * (signal / noise.max(1.0)).log10()
            })
            .fold(f64::MIN, f64::max);
        assert!(best > 20.0, "SNR {best:.1} dB");
    }

    #[test]
    fn garbage_and_empty_payloads_decode_to_nothing_or_bounded_audio() {
        let mut decoder = Decoder::new(16_000).unwrap();
        assert!(decoder.decode(&[]).is_none());
        assert!(decoder.decode(&[0; 8000]).is_none());
        for b in 0..=255u8 {
            if let Some(pcm) = decoder.decode(&[b, 0xff, 0x00, 0x13]) {
                assert!(pcm.len() <= speaker_max(16_000));
            }
        }
    }
}
