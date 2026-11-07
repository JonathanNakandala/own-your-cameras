//! Keypad bus frames that need no hardware: the plaintext "HJ" requests, reply validation and the
//! poll-reply body shapes. The RS-485 session itself lives in the ARM-only runtime.
use zeroize::Zeroizing;

pub const MODULE: u16 = 1;

pub fn crc(bytes: &[u8]) -> u16 {
    keypad_protocol::crc16_ibm(bytes)
}

/// Plaintext request: registration (`0x12`), info (`0x00`, marker `0xee`), notify (`0x66`, marker
/// `0xb0`). The notification carries a zero tail instead of a CRC, as the stock application sends it.
pub fn hj(command: u8, marker: u8) -> [u8; 11] {
    let mut f = [0x24, 0, 1, 0, command, 0xfe, 1, marker, 3, 0, 0];
    if command != 0x66 {
        let c = crc(&f[..9]);
        f[9..].copy_from_slice(&c.to_le_bytes());
    }
    f
}

/// Bounds, routing, opcode, terminator and CRC of one reply. The stock notification acknowledgement
/// (`0xe6`, body length 2, status 1) is the one frame whose tail is not a CRC.
pub fn validate_frame(f: &[u8], hj_reply: bool, expected: u8) -> Result<(), &'static str> {
    let notification_tail_waived =
        hj_reply && expected == 0xe6 && f.len() == 14 && f[4] == 0xe6 && f[8] == 2 && f[10] == 1;
    if f.len() < 11
        || f[0] != 0x42
        || (!hj_reply && f[1] != 0)
        || u16::from_le_bytes([f[2], f[3]]) != MODULE
        || f[4] != expected
        || f[f.len() - 3] != 3
        || (crc(&f[..f.len() - 2]) != u16::from_le_bytes([f[f.len() - 2], f[f.len() - 1]])
            && !notification_tail_waived)
    {
        return Err("response_frame");
    }
    let extent = if hj_reply {
        f[8] as usize + 12
    } else {
        u16::from_le_bytes([f[5], f[6]]) as usize + 10
    };
    if f.len() != extent {
        return Err("response_extent");
    }
    Ok(())
}

/// Poll reply bodies: `c2` idle, or `c4 21 <len> 00 <text…>` padded to 36 bytes.
pub fn entry_text(body: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
    if body == [0xc2] {
        return Ok(None);
    }
    if body.len() != 36 || body[..2] != [0xc4, 0x21] || body[2] > 32 || body[3] != 0 {
        return Err("snapshot_shape");
    }
    let n = body[2] as usize;
    let current = &body[4..4 + n];
    if !current.iter().all(|c| c.is_ascii_graphic()) || body[4 + n..].iter().any(|c| *c != 0) {
        return Err("snapshot_characters_or_padding");
    }
    Ok(Some(Zeroizing::new(current.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(text: &[u8]) -> Vec<u8> {
        let mut b = vec![0xc4, 0x21, text.len() as u8, 0];
        b.extend_from_slice(text);
        b.resize(36, 0);
        b
    }

    #[test]
    fn poll_bodies_are_idle_text_or_refused() {
        assert_eq!(entry_text(&[0xc2]).unwrap(), None);
        assert_eq!(
            entry_text(&snapshot(b"#12")).unwrap().unwrap().as_slice(),
            b"#12"
        );
        assert_eq!(entry_text(&snapshot(b"")).unwrap().unwrap().len(), 0);
        assert!(entry_text(&snapshot(b"\n")).is_err());
        assert!(entry_text(&[0xc4]).is_err());
        let mut bad = snapshot(b"12");
        bad[35] = 1;
        assert!(entry_text(&bad).is_err());
    }

    #[test]
    fn frames_follow_the_stock_encoding() {
        assert_eq!(
            hj(0x12, 0),
            [0x24, 0, 1, 0, 0x12, 0xfe, 1, 0, 3, 0x23, 0xeb]
        );
        assert_eq!(
            hj(0x66, 0xb0),
            [0x24, 0, 1, 0, 0x66, 0xfe, 1, 0xb0, 3, 0, 0]
        );
        let mut notify = vec![0x42, 113, 1, 0, 0xe6, 0, 0, 0, 2, 0, 1, 3, 0, 0];
        assert!(
            validate_frame(&notify, true, 0xe6).is_ok(),
            "zero tail accepted on the exact notify profile"
        );
        notify[10] = 0;
        assert!(validate_frame(&notify, true, 0xe6).is_err());
        let mut f = vec![0x42, 0, 1, 0, 0x30, 1, 0, 0xc2, 3];
        let c = crc(&f);
        f.extend_from_slice(&c.to_le_bytes());
        assert!(validate_frame(&f, false, 0x30).is_ok());
        f[7] ^= 1;
        assert!(validate_frame(&f, false, 0x30).is_err());
        assert!(validate_frame(&[0; 12], true, 0x92).is_err());
    }
}
