//! Annex B elementary-stream writer: starts at the first key frame, and every key frame carries SPS and PPS.

use std::io::{self, Write};

use hdal::enc::EncFrame;

use camera_transport::annexb::nal_ranges;
#[cfg(test)]
pub use camera_transport::annexb::{nal_slices, nal_types};

/// The transport's name for an encoder path's codec.
pub fn transport_codec(codec: hdal::enc::Codec) -> camera_transport::annexb::Codec {
    match codec {
        hdal::enc::Codec::H264 => camera_transport::annexb::Codec::H264,
        hdal::enc::Codec::H265 => camera_transport::annexb::Codec::H265,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub frames: u64,
    pub key_frames: u64,
    pub bytes: u64,
}

pub struct AnnexB<W: Write> {
    out: W,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    started: bool,
    stats: Stats,
}

impl<W: Write> AnnexB<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            sps: None,
            pps: None,
            started: false,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Finish buffered writes explicitly; dropping a buffered sink discards its errors.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Writes `frame`, unless the stream has not seen a key frame yet.
    pub fn write(&mut self, frame: &EncFrame) -> io::Result<()> {
        let ranges = nal_ranges(&frame.data);
        for &(s, t, e) in &ranges {
            match t {
                7 => self.sps = Some(frame.data[s..e].to_vec()),
                8 => self.pps = Some(frame.data[s..e].to_vec()),
                _ => {}
            }
        }
        if !frame.key && !self.started {
            return Ok(());
        }
        if frame.key {
            self.started = true;
            let has = |t| ranges.iter().any(|r| r.1 == t);
            if !has(7)
                && let Some(sps) = &self.sps
            {
                self.out.write_all(sps)?;
                self.stats.bytes += sps.len() as u64;
            }
            if !has(8)
                && let Some(pps) = &self.pps
            {
                self.out.write_all(pps)?;
                self.stats.bytes += pps.len() as u64;
            }
            self.stats.key_frames += 1;
        }
        self.out.write_all(&frame.data)?;
        self.stats.bytes += frame.data.len() as u64;
        self.stats.frames += 1;
        Ok(())
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn adjacent_start_codes_skip_empty_nals_without_invalid_slices() {
        for data in [
            vec![0, 0, 1, 0, 0, 1, 0x65, 9],
            vec![0, 0, 1, 0, 0, 0, 1, 0x65, 9],
            vec![0, 0, 0, 1, 0, 0, 1, 0x65, 9],
        ] {
            assert_eq!(nal_slices(&data), [&[0x65, 9][..]]);
            assert_eq!(nal_types(&data), [5]);
        }
    }
}
