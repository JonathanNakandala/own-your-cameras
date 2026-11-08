//! The lossless form the installer stores speaker clips in (`<name>.pcmd`): each s16le sample replaced
//! by its second-order prediction residual `x[i] - 2·x[i-1] + x[i-2]`, wrapping in 16 bits. Decoding
//! gives back the exact samples. The residuals compress about a fifth better under the payload's
//! LZMA than the raw samples, which matters on the small configuration partition.

/// The samples of a `.pcmd` clip; at most `max` of them.
pub fn decode(bytes: &[u8], max: usize) -> Vec<i16> {
    let mut out: Vec<i16> = Vec::with_capacity((bytes.len() / 2).min(max));
    for c in bytes.chunks_exact(2).take(max) {
        let r = i16::from_le_bytes([c[0], c[1]]);
        let n = out.len();
        let p1 = if n >= 1 { out[n - 1] } else { 0 };
        let p2 = if n >= 2 { out[n - 2] } else { 0 };
        out.push(r.wrapping_add(p1.wrapping_mul(2)).wrapping_sub(p2));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `full-scale.pcmd.hex` is `residual_encode` of `full-scale.pcm.hex` by the installer itself
    /// (`tools/ds-kd8003-ime1/install.py`), both as hex text: 400 samples of a 20000-amplitude sine,
    /// then `32767 -32768 32767 0 -32768 -32768 1 -1`. Regenerate after an encoder change:
    /// `python3 -B -c "import sys, pathlib, textwrap; sys.path.insert(0, 'tools/ds-kd8003-ime1'); import install;
    /// d = pathlib.Path('firmware/cameras/ds-kd8003-ime1/tests/fixtures/clip');
    /// e = install.residual_encode(bytes.fromhex((d / 'full-scale.pcm.hex').read_text()));
    /// (d / 'full-scale.pcmd.hex').write_text('\\n'.join(textwrap.wrap(e.hex(), 64)) + '\\n')"`
    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks_exact(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn decodes_the_installers_output_exactly_even_at_full_scale() {
        let pcm = &hex(include_str!("../tests/fixtures/clip/full-scale.pcm.hex"));
        let encoded = &hex(include_str!("../tests/fixtures/clip/full-scale.pcmd.hex"));
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(samples.len(), 408);
        assert_eq!(
            &samples[400..],
            [i16::MAX, i16::MIN, i16::MAX, 0, i16::MIN, i16::MIN, 1, -1]
        );
        assert_ne!(&pcm[..], &encoded[..]);
        assert_eq!(decode(encoded, usize::MAX), samples);
    }

    #[test]
    fn decode_stops_at_the_limit() {
        assert_eq!(decode(&[1; 100], 10).len(), 10);
        assert!(decode(&[], 10).is_empty());
    }
}
