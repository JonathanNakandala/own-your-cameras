//! The DS-KD-KP keypad over RS-485 (`/dev/ttyS1`, direction via `/dev/DDM/misc`): registration,
//! bootstrap-key version check, fresh session key, then a 100 ms poll loop that reports the entry
//! buffer. Digits only ever travel to the PIN engine through a zeroizing channel. Frame shapes and
//! validation live in `ds_kd8003_ime1_cam::keypad_frames` so they are tested on the host.
use super::Device;
use super::uart;
use ds_kd8003_ime1_cam::keypad_frames::{MODULE, entry_text, hj, validate_frame};
use keypad_protocol::{Key, key_update, open_response, seal};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const DIRECTION_IOCTL: libc::c_ulong = 0x40084d03;
type Result<T> = std::result::Result<T, &'static str>;

/// What the session reports upward: the keypad's current entry text (validated ASCII graphic), or idle.
pub enum Report {
    Text(Zeroizing<Vec<u8>>),
    Phase(&'static str),
}

struct Bus {
    uart: File,
    ddm: File,
}

impl Bus {
    fn direction(&self, transmit: bool) -> Result<()> {
        let mut arg = [0u8; 8];
        arg[1] = u8::from(transmit);
        // SAFETY: the DDM misc driver reads 8 bytes for the RS-485 direction request.
        if unsafe { libc::ioctl(self.ddm.as_raw_fd(), DIRECTION_IOCTL, arg.as_mut_ptr()) } < 0 {
            return Err("direction");
        }
        Ok(())
    }

    fn ready(&self, events: i16, end: Instant) -> Result<()> {
        let now = Instant::now();
        if now >= end {
            return Err("uart_timeout");
        }
        let ms = end.duration_since(now).as_millis().clamp(1, 1000) as i32;
        let mut p = PollFd {
            fd: self.uart.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut p as *mut PollFd as *mut libc::pollfd, 1, ms) };
        if r == 0 {
            return Err("uart_timeout");
        }
        if r != 1 || p.revents & events == 0 || p.revents & 0x38 != 0 {
            return Err("uart_wait");
        }
        Ok(())
    }

    /// One request, one bounded reply (1 s).
    fn exchange(
        &mut self,
        request: &[u8],
        hj_reply: bool,
        expected: Option<u8>,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let end = Instant::now() + Duration::from_millis(1000);
        self.direction(true)?;
        let transmit = (|| {
            // SAFETY: TCIOFLUSH on our own descriptor.
            if unsafe { libc::tcflush(self.uart.as_raw_fd(), libc::TCIOFLUSH) } != 0 {
                return Err("flush");
            }
            let mut offset = 0;
            while offset < request.len() {
                self.ready(libc::POLLOUT, end)?;
                match self.uart.write(&request[offset..]) {
                    Ok(n) if n > 0 => offset += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    _ => return Err("uart_write"),
                }
            }
            // SAFETY: drain our own descriptor.
            if unsafe { libc::tcdrain(self.uart.as_raw_fd()) } != 0 {
                return Err("drain");
            }
            Ok(())
        })();
        let receive = self.direction(false);
        transmit?;
        receive?;
        let mut frame = Zeroizing::new(Vec::with_capacity(267));
        let mut want = None;
        loop {
            if let Err(error) = self.ready(libc::POLLIN, end) {
                return Err(match (error, frame.is_empty()) {
                    ("uart_timeout", true) => "uart_no_response",
                    ("uart_timeout", false) => "uart_partial_timeout",
                    _ => error,
                });
            }
            let mut byte = [0];
            match self.uart.read(&mut byte) {
                Ok(1) => frame.push(byte[0]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                _ => return Err("uart_read"),
            }
            if frame.len() == 1 && frame[0] != 0x42 {
                return Err("uart_noise");
            }
            if frame.len() >= 9 && want.is_none() {
                let n = if hj_reply {
                    frame[8] as usize + 12
                } else {
                    u16::from_le_bytes([frame[5], frame[6]]) as usize + 10
                };
                if !(11..=267).contains(&n) {
                    return Err("response_extent");
                }
                want = Some(n);
            }
            if want == Some(frame.len()) {
                validate_frame(&frame, hj_reply, expected.unwrap_or(frame[4]))?;
                return Ok(frame);
            }
        }
    }
}

/// Registration → info → notify → bootstrap version check → key update → idle under the new key.
fn negotiate(bus: &mut Bus, seed: &mut [u8; 16], pending: &mut [u8; 32]) -> Result<(Key, u8)> {
    drop(bus.exchange(&hj(0x12, 0), true, Some(0x92))?);
    let info = bus.exchange(&hj(0, 0xee), true, Some(0x80))?;
    if info[8] == 0x21 || info.len() <= 45 || info[42] != 1 || info[45] != 33 {
        return Err("info_capability_unqualified");
    }
    drop(info);
    let current = Key::take(seed);
    let notify = bus.exchange(&hj(0x66, 0xb0), true, Some(0xe6))?;
    if notify.len() <= 10 || notify[10] != 1 {
        return Err("notify_status");
    }
    drop(notify);
    // The stock application pauses 500 ms, then queries the version under the bootstrap key before
    // any key update; up to four attempts 50 ms apart.
    std::thread::sleep(Duration::from_millis(500));
    let mut verified_sequence = None;
    for sequence in 1..=4u8 {
        if sequence > 1 {
            std::thread::sleep(Duration::from_millis(50));
        }
        let version =
            seal(&current, MODULE, 0x10, sequence, &[0x22]).map_err(|_| "version_encoding")?;
        let response = match bus.exchange(version.as_bytes(), false, None) {
            Ok(r) => r,
            Err("uart_no_response") => continue,
            Err(e) => return Err(e),
        };
        let message = open_response(&current, MODULE, response[4], &response)
            .map_err(|_| "version_decrypt")?;
        if message.sequence() != sequence || message.body().len() < 12 || message.body()[0] != 0x22
        {
            return Err("version_unqualified");
        }
        verified_sequence = Some(sequence);
        break;
    }
    let update_sequence = verified_sequence.ok_or("bootstrap_version_unanswered")? + 1;
    let update = key_update(&current, MODULE, update_sequence, 16, pending)
        .map_err(|_| "key_update_encoding")?;
    let response = bus.exchange(update.as_bytes(), false, None)?;
    let message =
        open_response(&current, MODULE, response[4], &response).map_err(|_| "update_decrypt")?;
    if message.sequence() != update_sequence || message.body() != [0x77] {
        return Err("update_ack_unqualified");
    }
    drop(message);
    drop(response);
    let mut new = [0u8; 16];
    new.copy_from_slice(&pending[..16]);
    pending.zeroize();
    let next = Key::take(&mut new);
    let request =
        seal(&next, MODULE, 0x30, update_sequence + 1, &[0x30, 1]).map_err(|_| "idle_encoding")?;
    let response = bus.exchange(request.as_bytes(), false, Some(0x30))?;
    let reply = open_response(&next, MODULE, 0x30, &response).map_err(|_| "idle_decrypt")?;
    if reply.sequence() != update_sequence + 1 || reply.body() != [0xc2] {
        return Err("idle_not_confirmed");
    }
    Ok((next, update_sequence + 2))
}

/// Poll reply bodies: `c2` idle, or `c4 21 <len> 00 <text…>` padded to 36 bytes.
fn uart_unowned() -> Result<()> {
    let target = std::fs::metadata("/dev/ttyS1")
        .map_err(|_| "uart_metadata")?
        .rdev();
    for entry in std::fs::read_dir("/proc").map_err(|_| "uart_owners")? {
        let entry = entry.map_err(|_| "uart_owners")?;
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(m) = std::fs::metadata(fd.path())
                && m.rdev() == target
            {
                return Err("uart_already_owned");
            }
        }
    }
    Ok(())
}

/// 16 raw bytes, regular file, mode 0600 or tighter.
fn read_key(path: &std::path::Path) -> Result<Zeroizing<[u8; 16]>> {
    let meta = std::fs::metadata(path).map_err(|_| "key_missing")?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 || meta.len() != 16 {
        return Err("key_file_shape");
    }
    let mut seed = Zeroizing::new([0u8; 16]);
    File::open(path)
        .and_then(|mut f| f.read_exact(seed.as_mut()))
        .map_err(|_| "key_read")?;
    Ok(seed)
}

fn session(g: &Device, key_path: &std::path::Path, tx: &mpsc::Sender<Report>) -> Result<()> {
    uart_unowned()?;
    let mut seed = read_key(key_path)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let uart = options.open("/dev/ttyS1").map_err(|_| "uart_open")?;
    let ddm = options.open("/dev/DDM/misc").map_err(|_| "ddm_open")?;
    let saved = uart::configure(&uart)?;
    let mut bus = Bus { uart, ddm };
    let result = (|| {
        let mut pending = Zeroizing::new([0u8; 32]);
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(pending.as_mut()))
            .map_err(|_| "random")?;
        let _ = tx.send(Report::Phase("negotiating"));
        let (key, mut sequence) = negotiate(&mut bus, &mut seed, &mut pending)?;
        let _ = tx.send(Report::Phase("ready"));
        let mut failures = 0u32;
        while !g.stop.load(Ordering::Relaxed) {
            let request =
                seal(&key, MODULE, 0x30, sequence, &[0x30, 1]).map_err(|_| "poll_encoding")?;
            let outcome = bus
                .exchange(request.as_bytes(), false, Some(0x30))
                .and_then(|frame| {
                    open_response(&key, MODULE, 0x30, &frame).map_err(|_| "poll_decrypt")
                })
                .and_then(|reply| {
                    if reply.sequence() == sequence {
                        entry_text(reply.body())
                    } else {
                        Err("poll_sequence")
                    }
                });
            match outcome {
                Ok(text) => {
                    failures = 0;
                    g.st.lock().unwrap().keypad_polls += 1;
                    if let Some(t) = text {
                        let _ = tx.send(Report::Text(t));
                    } else {
                        let _ = tx.send(Report::Text(Zeroizing::new(Vec::new())));
                    }
                }
                // One missed poll is a glitch; three in a row mean the bus is gone and the session restarts.
                Err(e) => {
                    failures += 1;
                    if failures >= 3 {
                        return Err(e);
                    }
                }
            }
            sequence = sequence.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })();
    let _ = bus.direction(false);
    let _ = uart::restore(&bus.uart, &saved);
    result
}

/// Runs keypad sessions for the life of the application, re-negotiating after a failure.
pub fn start(
    g: Arc<Device>,
    key_path: std::path::PathBuf,
    tx: mpsc::Sender<Report>,
) -> std::thread::JoinHandle<()> {
    super::spawn_named("keypad", move || {
        let mut backoff = 2u64;
        while !g.stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            match session(&g, &key_path, &tx) {
                Ok(()) => return,
                Err(e) => {
                    g.log(format!("keypad: session ended: {e}"));
                    let _ = tx.send(Report::Phase("down"));
                    g.st.lock().unwrap().keypad_restarts += 1;
                }
            }
            backoff = if started.elapsed() > Duration::from_secs(60) {
                2
            } else {
                (backoff * 2).min(60)
            };
            let end = Instant::now() + Duration::from_secs(backoff);
            while Instant::now() < end && !g.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    })
}
