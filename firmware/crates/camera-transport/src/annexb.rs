//! Annex B elementary-stream writer: starts at the first key frame, and every key frame carries SPS and PPS.

/// The video codec of an Annex B stream. H.264 is the default of every camera; the Reolink E1 Zoom can also
/// encode H.265.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Codec {
    #[default]
    H264,
    H265,
}

impl Codec {
    /// `nal_unit_type` of a NAL whose first byte is `header`: the low five bits in H.264, bits 1 to 6 in H.265.
    pub const fn nal_type(self, header: u8) -> u8 {
        match self {
            Self::H264 => header & 0x1f,
            Self::H265 => (header >> 1) & 0x3f,
        }
    }

    /// A NAL type that starts a picture a viewer can begin decoding at: an IDR picture in H.264, any IRAP picture
    /// (IDR, CRA, BLA: types 16 to 21) in H.265.
    pub const fn is_random_access(self, nal_type: u8) -> bool {
        match self {
            Self::H264 => nal_type == 5,
            Self::H265 => nal_type >= 16 && nal_type <= 21,
        }
    }

    /// Which of `[VPS, SPS, PPS]` a parameter set NAL type is, if it is one.
    pub const fn param_slot(self, nal_type: u8) -> Option<usize> {
        match (self, nal_type) {
            (Self::H264, 7) | (Self::H265, 33) => Some(1),
            (Self::H264, 8) | (Self::H265, 34) => Some(2),
            (Self::H265, 32) => Some(0),
            _ => None,
        }
    }

    /// The `[VPS, SPS, PPS]` slots a decoder needs before the first picture.
    pub const fn required_params(self) -> &'static [usize] {
        match self {
            Self::H264 => &[1, 2],
            Self::H265 => &[0, 1, 2],
        }
    }
}

/// NAL unit types (`nal_unit_type`, low five bits) of every NAL in `data`, in order.
pub fn nal_types(data: &[u8]) -> Vec<u8> {
    nal_ranges(data).into_iter().map(|(_, t, _)| t).collect()
}

/// Like [`nal_types`] for the given codec's header layout.
pub fn nal_types_of(codec: Codec, data: &[u8]) -> Vec<u8> {
    nal_slices(data)
        .into_iter()
        .map(|nal| codec.nal_type(nal[0]))
        .collect()
}

/// `(start of the start code, nal type, end)` of each NAL.
pub fn nal_ranges(data: &[u8]) -> Vec<(usize, u8, usize)> {
    let mut starts = Vec::new(); // (start-code offset, payload offset)
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let sc = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            starts.push((sc, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (n, &(sc, payload)) in starts.iter().enumerate() {
        if payload >= data.len() {
            continue;
        }
        let end = starts.get(n + 1).map_or(data.len(), |s| s.0);
        if payload >= end {
            continue; // Adjacent start codes contain no NAL header or payload.
        }
        out.push((sc, data[payload] & 0x1f, end));
    }
    out
}

/// Each NAL of `data` without its start code (trailing zero bytes before the next start code trimmed).
pub fn nal_slices(data: &[u8]) -> Vec<&[u8]> {
    nal_ranges(data)
        .into_iter()
        .map(|(sc, _, end)| {
            let skip = if data[sc..].starts_with(&[0, 0, 0, 1]) {
                4
            } else {
                3
            };
            &data[sc + skip..end]
        })
        .collect()
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
