//! The Opus codec seam. The transport frames, mixes and packetizes; an adapter crate supplies the codec.

/// Microphone samples per Opus frame: 20 ms at 48 kHz mono.
pub const MIC_FRAME: usize = 960;
/// Microphone RTP clock and capture rate.
pub const MIC_RATE: u32 = 48_000;
/// Speaker samples per 20 ms mixer block at the camera's speaker `rate` (Hz).
pub const fn speaker_frame(rate: u32) -> usize {
    rate as usize / 50
}
/// Largest decoded packet accepted from a talker at `rate`: 120 ms, the longest Opus packet.
pub const fn speaker_max(rate: u32) -> usize {
    rate as usize * 120 / 1000
}
/// Dynamic RTP payload type for Opus in both directions.
pub const PAYLOAD_TYPE: u8 = 111;

/// Encodes one 20 ms, 48 kHz mono frame into one Opus packet.
pub trait OpusEncode: Send {
    fn encode(&mut self, frame: &[i16; MIC_FRAME]) -> Option<Vec<u8>>;
}

/// Decodes one Opus packet (any frame size) into mono samples at the speaker rate it was created for.
pub trait OpusDecode: Send {
    fn decode(&mut self, payload: &[u8]) -> Option<Vec<i16>>;
}
