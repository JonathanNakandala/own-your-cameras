//! The keypad UART (`/dev/ttyS1`): apply the stock termios profile exactly and restore it on exit.
use std::fs::File;
use std::os::unix::io::AsRawFd;

const TCGETS: libc::c_ulong = 0x5401;
const TCSETS: libc::c_ulong = 0x5402;

/// The stock application's TCGETS image for the keypad bus, applied verbatim.
pub const STOCK36: [u8; 36] = [
    0, 0, 0, 0, 4, 0, 0, 0, 190, 12, 0, 0, 32, 138, 0, 0, 0, 3, 28, 127, 21, 4, 0, 1, 0, 17, 19,
    26, 0, 18, 15, 23, 22, 0, 0, 0,
];

pub struct Saved {
    initial: [u8; 36],
}

fn get(fd: i32) -> Result<[u8; 36], &'static str> {
    let mut aligned = [0u32; 9];
    // SAFETY: TCGETS writes one struct termios (36 bytes on this ARM kernel) into aligned storage.
    if unsafe { libc::ioctl(fd, TCGETS, aligned.as_mut_ptr()) } != 0 {
        return Err("uart_tcgets");
    }
    let mut raw = [0u8; 36];
    for (i, word) in aligned.into_iter().enumerate() {
        raw[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    Ok(raw)
}

fn set(fd: i32, raw: &[u8; 36]) -> Result<(), &'static str> {
    let mut aligned = [0u32; 9];
    for (i, word) in aligned.iter_mut().enumerate() {
        *word = u32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap());
    }
    // SAFETY: TCSETS reads one struct termios from aligned storage.
    if unsafe { libc::ioctl(fd, TCSETS, aligned.as_ptr()) } != 0 {
        return Err("uart_tcsets");
    }
    Ok(())
}

/// Applies the stock profile and verifies it read back exactly; on failure the original is restored.
pub fn configure(uart: &File) -> Result<Saved, &'static str> {
    let fd = uart.as_raw_fd();
    let initial = get(fd)?;
    let applied = set(fd, &STOCK36).and_then(|()| get(fd));
    match applied {
        Ok(actual) if actual == STOCK36 => Ok(Saved { initial }),
        Ok(_) => {
            let _ = set(fd, &initial);
            Err("uart_profile_mismatch")
        }
        Err(e) => {
            let _ = set(fd, &initial);
            Err(e)
        }
    }
}

pub fn restore(uart: &File, saved: &Saved) -> Result<(), &'static str> {
    let fd = uart.as_raw_fd();
    set(fd, &saved.initial)?;
    if get(fd)? != saved.initial {
        return Err("uart_restore_mismatch");
    }
    Ok(())
}
