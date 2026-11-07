//! Hardware-neutral microphone path. Opus: 48 kHz mono capture blocks of any size → 20 ms Opus frames →
//! RTP (RFC 7587: dynamic PT 111, 48 kHz clock, one frame per packet). PCMU: 8 kHz G.711 µ-law blocks →
//! RTP (RFC 3551: static PT 0, 160 samples per 20 ms packet) for recorders that take no Opus.
//! Pure: no clock reads, no I/O.

use crate::opus::{MIC_FRAME, MIC_RATE, OpusEncode, PAYLOAD_TYPE};

const FRAME_US: u64 = MIC_FRAME as u64 * 1_000_000 / MIC_RATE as u64;
/// A capture block this far from where the previous one ended is a gap: the remainder is dropped.
const MAX_SKEW_US: u64 = 40_000;

/// The microphone codec an application publishes into its `AudioHub`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Opus48k,
    Pcmu8k,
}

impl Format {
    /// Duration of one RTP packet.
    pub const fn packet_us(self) -> u64 {
        match self {
            Self::Opus48k => FRAME_US,
            Self::Pcmu8k => PCMU_PACKET_US,
        }
    }
}

/// Track 1 of both RTSP paths. RFC 7587 requires `opus/48000/2` even for a mono stream.
pub fn sdp() -> String {
    sdp_for(Format::Opus48k)
}

/// Track 1 of both RTSP paths for the given codec.
pub fn sdp_for(format: Format) -> String {
    match format {
        Format::Opus48k => format!(
            "m=audio 0 RTP/AVP {pt}\r\na=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} sprop-stereo=0\r\n\
             a=control:trackID=1\r\na=recvonly\r\n",
            pt = PAYLOAD_TYPE
        ),
        Format::Pcmu8k => {
            "m=audio 0 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000/1\r\na=control:trackID=1\r\na=recvonly\r\n"
                .into()
        }
    }
}

/// 160 samples at 8 kHz.
pub const PCMU_PACKET_US: u64 = 20_000;
const PCMU_SAMPLES: usize = 160;

/// RTP for PCMU: a published block of any whole number of 20 ms packets is split into them, timestamps
/// advancing 160 per packet from the block's PTS.
pub struct PcmuPacketizer {
    ssrc: u32,
    sequence: u16,
    first: bool,
}

impl PcmuPacketizer {
    pub fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            sequence: 0,
            first: true,
        }
    }

    pub fn packets(&mut self, pts_us: u64, bytes: &[u8]) -> Vec<Vec<u8>> {
        if bytes.is_empty() || bytes.len() > 8191 {
            return Vec::new();
        }
        let timestamp = (pts_us.wrapping_mul(8000) / 1_000_000) as u32;
        bytes
            .chunks(PCMU_SAMPLES)
            .enumerate()
            .map(|(index, payload)| {
                let mut packet = Vec::with_capacity(12 + payload.len());
                packet.extend_from_slice(&[0x80, if self.first { 0x80 } else { 0 }]);
                packet.extend_from_slice(&self.sequence.to_be_bytes());
                packet.extend_from_slice(
                    &timestamp
                        .wrapping_add((index * PCMU_SAMPLES) as u32)
                        .to_be_bytes(),
                );
                packet.extend_from_slice(&self.ssrc.to_be_bytes());
                packet.extend_from_slice(payload);
                self.sequence = self.sequence.wrapping_add(1);
                self.first = false;
                packet
            })
            .collect()
    }
}

/// G.711 µ-law for one bounded 8 kHz mono PCM16 block (for example one 40 ms SDK frame).
pub fn encode_pcm8k_mono(samples: &[i16]) -> Option<Vec<u8>> {
    if samples.is_empty() || samples.len() > 1600 {
        return None;
    }
    Some(samples.iter().map(|&s| linear_to_mulaw(s)).collect())
}

fn linear_to_mulaw(sample: i16) -> u8 {
    let sign = if sample < 0 { 0x80 } else { 0 };
    let biased = i32::from(sample).abs().min(32635) + 132;
    let mut exponent = 7;
    let mut mask = 0x4000;
    while exponent > 0 && biased & mask == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = (biased >> (exponent + 3)) & 0x0f;
    !(sign | (exponent << 4) as u8 | mantissa as u8)
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

#[cfg(test)]
mod pcmu_tests {
    use super::*;

    #[test]
    fn a_40ms_block_becomes_two_paced_packets_with_contiguous_timestamps() {
        let mut pk = PcmuPacketizer::new(7);
        let packets = pk.packets(1_000_000, &[0xff; 320]);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0][1], 0x80, "marker on the first packet only");
        assert_eq!(packets[1][1], 0, "static payload type 0");
        assert_eq!(
            u32::from_be_bytes(packets[0][4..8].try_into().unwrap()),
            8000
        );
        assert_eq!(
            u32::from_be_bytes(packets[1][4..8].try_into().unwrap()),
            8160
        );
        assert_eq!(u16::from_be_bytes(packets[1][2..4].try_into().unwrap()), 1);
        assert_eq!(packets[1].len(), 12 + 160);
        assert!(pk.packets(0, &[]).is_empty());
    }

    #[test]
    fn mulaw_matches_the_itu_reference_points() {
        assert_eq!(
            encode_pcm8k_mono(&[0, 32767, -32768, 1000, -1000]).unwrap(),
            [0xff, 0x80, 0x00, 0xce, 0x4e]
        );
        assert!(encode_pcm8k_mono(&[]).is_none());
        assert!(encode_pcm8k_mono(&[0; 1601]).is_none());
    }
}
