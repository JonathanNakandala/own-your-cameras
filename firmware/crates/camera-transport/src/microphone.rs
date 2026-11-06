//! Hardware-neutral microphone path: 48 kHz mono capture blocks of any size → 20 ms Opus frames → RTP
//! (RFC 7587: dynamic PT 111, 48 kHz clock, one frame per packet). Pure: no clock reads, no I/O.

use crate::opus::{MIC_FRAME, MIC_RATE, OpusEncode, PAYLOAD_TYPE};

const FRAME_US: u64 = MIC_FRAME as u64 * 1_000_000 / MIC_RATE as u64;
/// A capture block this far from where the previous one ended is a gap: the remainder is dropped.
const MAX_SKEW_US: u64 = 40_000;

/// Track 1 of both RTSP paths. RFC 7587 requires `opus/48000/2` even for a mono stream.
pub fn sdp() -> String {
    format!(
        "m=audio 0 RTP/AVP {pt}\r\na=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} sprop-stereo=0\r\n\
         a=control:trackID=1\r\na=recvonly\r\n",
        pt = PAYLOAD_TYPE
    )
}

/// Regroups capture blocks into exact 960-sample frames, each stamped with its first sample's time.
#[derive(Default)]
pub struct Framer {
    pending: Vec<i16>,
    /// Time of `pending[0]`.
    start_us: u64,
}

impl Framer {
    pub fn push(&mut self, pts_us: u64, samples: &[i16]) -> Vec<(u64, Box<[i16; MIC_FRAME]>)> {
        let expected = self.start_us + self.pending.len() as u64 * 1_000_000 / u64::from(MIC_RATE);
        if self.pending.is_empty() || expected.abs_diff(pts_us) > MAX_SKEW_US {
            self.pending.clear();
            self.start_us = pts_us;
        }
        self.pending.extend_from_slice(samples);
        let mut frames = Vec::new();
        while self.pending.len() >= MIC_FRAME {
            let frame: Box<[i16; MIC_FRAME]> = self
                .pending
                .drain(..MIC_FRAME)
                .collect::<Vec<_>>()
                .try_into()
                .ok()
                .unwrap();
            frames.push((self.start_us, frame));
            self.start_us += FRAME_US;
        }
        frames
    }
}

/// The capture path an application runs: framing plus the codec, yielding `(pts_us, opus packet)`.
pub struct Microphone<E: OpusEncode> {
    framer: Framer,
    encoder: E,
}

impl<E: OpusEncode> Microphone<E> {
    pub fn new(encoder: E) -> Self {
        Self {
            framer: Framer::default(),
            encoder,
        }
    }

    pub fn push(&mut self, pts_us: u64, samples: &[i16]) -> Vec<(u64, Vec<u8>)> {
        self.framer
            .push(pts_us, samples)
            .into_iter()
            .filter_map(|(pts, frame)| Some((pts, self.encoder.encode(&frame)?)))
            .collect()
    }
}

/// RTP for one Opus packet per 20 ms frame.
pub struct Packetizer {
    ssrc: u32,
    sequence: u16,
    first: bool,
    /// The previous packet's PTS and RTP timestamp: contiguous frames advance by exactly one frame.
    last: Option<(u64, u32)>,
}

impl Packetizer {
    pub fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            sequence: 0,
            first: true,
            last: None,
        }
    }

    pub fn packet(&mut self, pts_us: u64, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.is_empty() || payload.len() > 1275 {
            return None;
        }
        let timestamp = match self.last {
            // Microsecond PTS round differently from sample counts; never let that jitter the RTP clock.
            Some((pts, ts)) if pts_us.abs_diff(pts + FRAME_US) < FRAME_US / 2 => {
                ts.wrapping_add(MIC_FRAME as u32)
            }
            _ => (pts_us.wrapping_mul(u64::from(MIC_RATE)) / 1_000_000) as u32,
        };
        self.last = Some((pts_us, timestamp));
        let mut packet = Vec::with_capacity(12 + payload.len());
        packet.extend_from_slice(&[0x80, PAYLOAD_TYPE | if self.first { 0x80 } else { 0 }]);
        packet.extend_from_slice(&self.sequence.to_be_bytes());
        packet.extend_from_slice(&timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());
        packet.extend_from_slice(payload);
        self.sequence = self.sequence.wrapping_add(1);
        self.first = false;
        Some(packet)
    }
}

/// Pure pacing policy. The caller supplies monotonic microseconds and waits outside this module. Small
/// scheduling jitter preserves the original cadence; deadlines more than 40 ms late reset instead of
/// accumulating catch-up work.
#[derive(Default)]
pub struct Pacer {
    next: Option<u64>,
}

impl Pacer {
    pub fn deadline(&mut self, now_us: u64, duration_us: u64) -> Option<u64> {
        if !(1..=120_000).contains(&duration_us) {
            return None;
        }
        let due = match self.next {
            Some(next) if now_us.saturating_sub(next) <= 40_000 => next,
            _ => now_us,
        };
        self.next = Some(due.checked_add(duration_us)?);
        Some(due.max(now_us))
    }
}

/// Duration of one microphone RTP packet.
pub const PACKET_US: u64 = FRAME_US;

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake codec: the frame's first sample, as two bytes.
    struct First;
    impl OpusEncode for First {
        fn encode(&mut self, frame: &[i16; MIC_FRAME]) -> Option<Vec<u8>> {
            Some(frame[0].to_be_bytes().to_vec())
        }
    }

    #[test]
    fn sdp_is_rfc7587_opus_mono() {
        let sdp = sdp();
        assert!(sdp.contains("m=audio 0 RTP/AVP 111\r\n"));
        assert!(sdp.contains("a=rtpmap:111 opus/48000/2\r\n"));
        assert!(sdp.contains("a=fmtp:111 sprop-stereo=0\r\n"));
        assert!(sdp.contains("a=control:trackID=1\r\n"));
    }

    #[test]
    fn capture_blocks_of_any_size_become_exact_frames_stamped_by_their_first_sample() {
        let mut mic = Microphone::new(First);
        let block = |start: i16| (start..start + 1024).collect::<Vec<i16>>();
        assert_eq!(
            mic.push(1_000_000, &block(0)),
            [(1_000_000, 0i16.to_be_bytes().to_vec())]
        );
        // 64 samples carried over: the next frame starts at sample 960 of the stream.
        let next = mic.push(1_021_333, &block(1024));
        assert_eq!(next, [(1_020_000, 960i16.to_be_bytes().to_vec())]);
        let (mut frames, mut start) = (0, 2048);
        for i in 2..30 {
            frames += mic.push(1_000_000 + i * 21_333, &block(start)).len();
            start += 1024;
        }
        assert_eq!(frames, (30 * 1024 / 960) - 2);
    }

    #[test]
    fn a_capture_gap_restarts_framing_at_the_new_time() {
        let mut framer = Framer::default();
        assert!(framer.push(0, &[1; 500]).is_empty());
        let frames = framer.push(500_000, &[2; 960]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, 500_000);
        assert!(frames[0].1.iter().all(|&s| s == 2));
    }

    #[test]
    fn rtp_has_dynamic_payload_type_48_khz_clock_and_marker_on_the_first_packet() {
        let mut packetizer = Packetizer::new(0x01020304);
        assert_eq!(
            packetizer.packet(1_000_000, &[0xaa, 0xbb]).unwrap(),
            [
                0x80,
                0x80 | 111,
                0,
                0,
                0,
                0,
                0xbb,
                0x80,
                1,
                2,
                3,
                4,
                0xaa,
                0xbb
            ]
        );
        let second = packetizer.packet(1_020_001, &[0xcc]).unwrap();
        assert_eq!(second[1], 111);
        assert_eq!(&second[2..4], &[0, 1]);
        assert_eq!(
            u32::from_be_bytes(second[4..8].try_into().unwrap()),
            48_960,
            "rounding never jitters"
        );
        let after_gap = packetizer.packet(2_000_000, &[0xdd]).unwrap();
        assert_eq!(
            u32::from_be_bytes(after_gap[4..8].try_into().unwrap()),
            96_000
        );
        assert!(packetizer.packet(0, &[]).is_none());
        assert!(packetizer.packet(0, &[0; 1276]).is_none());
    }

    #[test]
    fn paced_packets_keep_twenty_ms_and_late_senders_never_catch_up() {
        let mut pacer = Pacer::default();
        assert_eq!(pacer.deadline(100_000, 20_000), Some(100_000));
        assert_eq!(pacer.deadline(100_000, 20_000), Some(120_000));
        assert_eq!(pacer.deadline(140_000, 20_000), Some(140_000));
        assert_eq!(pacer.deadline(140_000, 20_000), Some(160_000));
        assert_eq!(pacer.deadline(300_000, 20_000), Some(300_000));
        assert_eq!(pacer.deadline(300_000, 0), None);
        assert_eq!(pacer.deadline(300_000, 120_001), None);
        let mut overflow = Pacer::default();
        assert_eq!(overflow.deadline(u64::MAX - 1, 20_000), None);
        let mut jitter = Pacer::default();
        assert_eq!(jitter.deadline(100_000, 20_000), Some(100_000));
        assert_eq!(jitter.deadline(121_000, 20_000), Some(121_000));
        assert_eq!(jitter.deadline(121_000, 20_000), Some(140_000));
    }
}
