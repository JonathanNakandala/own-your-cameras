//! H.264 over RTP (RFC 6184): single NAL unit packets and FU-A fragments, packetization-mode 1. H.265 over RTP
//! (RFC 7798): single NAL unit packets and fragmentation units (type 49). Payloads borrow from the access unit;
//! only the 12-byte RTP header (+ FU bytes) is built per packet.

use crate::annexb::{Codec, nal_slices};

pub const CLOCK_HZ: u32 = 90_000;
const PT_H264: u8 = 96;

/// One RTP packet: a small header and a payload that points into the access unit.
#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    head: [u8; 15],
    head_len: usize,
    pub payload: &'a [u8],
}

impl Packet<'_> {
    pub fn head(&self) -> &[u8] {
        &self.head[..self.head_len]
    }

    pub fn len(&self) -> usize {
        self.head_len + self.payload.len()
    }
}

#[cfg(test)]
impl Packet<'_> {
    fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.len());
        v.extend_from_slice(self.head());
        v.extend_from_slice(self.payload);
        v
    }

    fn seq(&self) -> u16 {
        u16::from_be_bytes([self.head[2], self.head[3]])
    }
}

fn rtp_header(marker: bool, seq: u16, ts: u32, ssrc: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0] = 0x80;
    h[1] = PT_H264 | if marker { 0x80 } else { 0 };
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&ts.to_be_bytes());
    h[8..12].copy_from_slice(&ssrc.to_be_bytes());
    h
}

/// Per-stream RTP state: sequence number and SSRC. `max_payload` is the largest RTP payload (MTU less
/// IP/UDP/RTP headers).
pub struct Packetizer {
    ssrc: u32,
    seq: u16,
    max_payload: usize,
    codec: Codec,
}

impl Packetizer {
    pub fn new(ssrc: u32, first_seq: u16, max_payload: usize) -> Self {
        Self {
            ssrc,
            seq: first_seq,
            max_payload: max_payload.max(4),
            codec: Codec::H264,
        }
    }

    /// Packetizes `codec`'s access units (H.264 unless said otherwise).
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// Packets of one Annex B access unit; the last one carries the marker bit.
    pub fn packets<'a>(&mut self, annexb: &'a [u8], ts: u32) -> Vec<Packet<'a>> {
        let nals = nal_slices(annexb);
        let mut out = Vec::new();
        let last_nal = nals.len().saturating_sub(1);
        for (n, nal) in nals.iter().enumerate() {
            let is_last = n == last_nal;
            if nal.len() <= self.max_payload {
                out.push(self.packet(is_last, ts, &[], nal));
                continue;
            }
            if self.codec == Codec::H265 {
                // FU: the two NAL header bytes are replaced by a payload header (type 49, the layer and
                // temporal id kept) and an FU header holding the NAL type.
                let (hdr, body) = ([(nal[0] & 0x81) | (49 << 1), nal[1]], &nal[2..]);
                let kind = (nal[0] >> 1) & 0x3f;
                let chunk = self.max_payload - 3;
                let count = body.len().div_ceil(chunk);
                for (i, part) in body.chunks(chunk).enumerate() {
                    let mut fu = kind;
                    if i == 0 {
                        fu |= 0x80;
                    }
                    let last = i + 1 == count;
                    if last {
                        fu |= 0x40;
                    }
                    out.push(self.packet(is_last && last, ts, &[hdr[0], hdr[1], fu], part));
                }
                continue;
            }
            // FU-A: the NAL header byte is replaced by FU indicator + FU header.
            let (hdr, body) = (nal[0], &nal[1..]);
            let indicator = (hdr & 0xe0) | 28;
            let chunk = self.max_payload - 2;
            let count = body.len().div_ceil(chunk);
            for (i, part) in body.chunks(chunk).enumerate() {
                let mut fu = hdr & 0x1f;
                if i == 0 {
                    fu |= 0x80;
                }
                let last = i + 1 == count;
                if last {
                    fu |= 0x40;
                }
                out.push(self.packet(is_last && last, ts, &[indicator, fu], part));
            }
        }
        out
    }

    fn packet<'a>(&mut self, marker: bool, ts: u32, extra: &[u8], payload: &'a [u8]) -> Packet<'a> {
        let mut head = [0u8; 15];
        head[..12].copy_from_slice(&rtp_header(marker, self.seq, ts, self.ssrc));
        head[12..12 + extra.len()].copy_from_slice(extra);
        self.seq = self.seq.wrapping_add(1);
        Packet {
            head,
            head_len: 12 + extra.len(),
            payload,
        }
    }
}

/// RTP timestamp of a presentation time in microseconds.
pub fn timestamp(pts_us: u64) -> u32 {
    (pts_us.wrapping_mul(u64::from(CLOCK_HZ)) / 1_000_000) as u32
}

#[cfg(test)]
impl Packetizer {
    fn next_seq(&self) -> u16 {
        self.seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reassembles packets the way a receiver does, to Annex B with 4-byte start codes.
    fn depacketize(pkts: &[Packet]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in pkts {
            let b = p.to_vec();
            let pl = &b[12..];
            match pl[0] & 0x1f {
                28 => {
                    if pl[1] & 0x80 != 0 {
                        out.extend_from_slice(&[0, 0, 0, 1, (pl[0] & 0xe0) | (pl[1] & 0x1f)]);
                    }
                    out.extend_from_slice(&pl[2..]);
                }
                _ => {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(pl);
                }
            }
        }
        out
    }

    #[test]
    fn small_nals_go_single_with_marker_on_the_last() {
        let au = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 9, 9,
        ];
        let mut p = Packetizer::new(7, 100, 1400);
        let pk = p.packets(&au, 90_000);
        assert_eq!(pk.len(), 3);
        assert_eq!(pk[0].payload, &[0x67, 1, 2]);
        assert_eq!(pk[1].payload, &[0x68, 3]);
        assert_eq!(pk[2].payload, &[0x65, 9, 9]);
        let marks: Vec<bool> = pk.iter().map(|k| k.head()[1] & 0x80 != 0).collect();
        assert_eq!(marks, [false, false, true]);
        assert_eq!(
            pk.iter().map(|k| k.seq()).collect::<Vec<_>>(),
            [100, 101, 102]
        );
        assert_eq!(&pk[0].head()[4..8], &90_000u32.to_be_bytes());
        assert_eq!(&pk[0].head()[8..12], &7u32.to_be_bytes());
        assert_eq!(p.next_seq(), 103);
        assert_eq!(pk[0].head()[1] & 0x7f, 96);
    }

    #[test]
    fn large_nal_is_fragmented_with_start_and_end_bits_and_reassembles() {
        let body: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8 | 0x10).collect();
        let mut au = vec![0, 0, 0, 1, 0x65];
        au.extend_from_slice(&body);
        let mut p = Packetizer::new(1, 65535, 1000);
        let pk = p.packets(&au, 0);
        assert_eq!(pk.len(), 4); // 3000 / 998 -> 4
        assert!(
            pk.iter()
                .all(|k| k.payload.len() + 2 <= 1000 || k.head_len == 12)
        );
        let first = pk[0].to_vec();
        assert_eq!(first[12], (0x65 & 0xe0) | 28);
        assert_eq!(first[13], 0x80 | 5);
        assert_eq!(pk[3].to_vec()[13], 0x40 | 5);
        assert_eq!(pk[1].to_vec()[13], 5);
        assert_eq!(pk[3].head()[1] & 0x80, 0x80);
        assert_eq!(pk[2].head()[1] & 0x80, 0);
        // wraparound of the sequence number
        assert_eq!(pk[1].seq(), 0);
        assert_eq!(depacketize(&pk), au);
    }

    /// Reassembles H.265 packets to Annex B with 4-byte start codes.
    fn depacketize_h265(pkts: &[Packet]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in pkts {
            let b = p.to_vec();
            let pl = &b[12..];
            if (pl[0] >> 1) & 0x3f == 49 {
                if pl[2] & 0x80 != 0 {
                    out.extend_from_slice(&[
                        0,
                        0,
                        0,
                        1,
                        (pl[0] & 0x81) | ((pl[2] & 0x3f) << 1),
                        pl[1],
                    ]);
                }
                out.extend_from_slice(&pl[3..]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(pl);
            }
        }
        out
    }

    #[test]
    fn h265_small_nals_go_single_and_large_ones_are_fragmented_and_reassemble() {
        let body: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8 | 0x10).collect();
        // VPS (32), SPS (33), then an IDR_W_RADL (19) with temporal id 1 (low bits of the second byte).
        let mut au = vec![0, 0, 0, 1, 32 << 1, 1, 7, 7, 0, 0, 0, 1, 33 << 1, 1, 8];
        au.extend_from_slice(&[0, 0, 0, 1, 19 << 1, 2]);
        au.extend_from_slice(&body);
        let mut p = Packetizer::new(3, 10, 1000).codec(Codec::H265);
        let pk = p.packets(&au, 0);
        assert_eq!(pk.len(), 2 + 4); // 3000 / 997 -> 4 fragments
        assert_eq!(pk[0].payload, &[32 << 1, 1, 7, 7]);
        let first = pk[2].to_vec();
        assert_eq!(first[12] >> 1 & 0x3f, 49, "FU payload header type");
        assert_eq!(first[13], 2, "temporal id kept");
        assert_eq!(first[14], 0x80 | 19, "start bit and the NAL type");
        assert_eq!(pk[5].to_vec()[14], 0x40 | 19, "end bit");
        assert_eq!(pk[3].to_vec()[14], 19);
        assert!(pk.iter().all(|k| k.len() <= 12 + 1000 + 3));
        let marks: Vec<bool> = pk.iter().map(|k| k.head()[1] & 0x80 != 0).collect();
        assert_eq!(marks, [false, false, false, false, false, true]);
        assert_eq!(depacketize_h265(&pk), au);
    }

    #[test]
    fn payload_borrows_from_the_access_unit() {
        let au = [0, 0, 1, 0x41, 5, 6, 7];
        let mut p = Packetizer::new(1, 0, 1400);
        let pk = p.packets(&au, 0);
        assert_eq!(pk[0].payload.as_ptr(), au[3..].as_ptr());
    }

    #[test]
    fn timestamp_is_90khz() {
        assert_eq!(timestamp(1_000_000), 90_000);
        assert_eq!(timestamp(100_000), 9_000);
    }
}
