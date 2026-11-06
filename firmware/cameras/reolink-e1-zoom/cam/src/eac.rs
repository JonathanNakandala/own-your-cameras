//! `/dev/mem` adapter for the codec record gain ([`hdal::codec`]): ALC and noise gate off, PGA from
//! `mic_volume`, then the codec's load latch. Mapped per call; it runs once per capture start.
use std::io;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use hdal::codec::{AD_DYNAMICS, AD_GAIN, BASE, LOAD, ad_dynamics, ad_gain};

const PAGE: usize = 4096;
/// The kernel's own `eac_set_load` waits without a bound; the latch clears within a sample period.
const LOAD_TIMEOUT: Duration = Duration::from_millis(20);

pub fn set_record_gain(volume: u8) -> io::Result<()> {
    let fd = crate::hardware_fd::open_rw("/dev/mem")?;
    // SAFETY: one page of the EAC register block, shared with the kernel driver; unmapped below.
    let va = unsafe {
        // 64-bit offset: the block lies above 2 GiB, beyond a 32-bit `off_t`.
        libc::mmap64(
            std::ptr::null_mut(),
            PAGE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            BASE as libc::off64_t,
        )
    };
    if va == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    let reg = |offset: usize| va.cast::<u8>().wrapping_add(offset).cast::<u32>();
    // SAFETY: aligned registers inside the mapped page; volatile because the codec owns them. The media
    // thread calls this between HDAL calls, so the driver is not changing them concurrently.
    let latched = unsafe {
        reg(AD_DYNAMICS).write_volatile(ad_dynamics(reg(AD_DYNAMICS).read_volatile()));
        reg(AD_GAIN).write_volatile(ad_gain(reg(AD_GAIN).read_volatile(), volume));
        reg(LOAD).write_volatile(1);
        let start = Instant::now();
        loop {
            if reg(LOAD).read_volatile() & 1 == 0 {
                break true;
            }
            if start.elapsed() > LOAD_TIMEOUT {
                break false;
            }
            std::thread::sleep(Duration::from_micros(100));
        }
    };
    // SAFETY: the mapping made above, no longer referenced.
    unsafe { libc::munmap(va, PAGE) };
    if latched {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::TimedOut, "codec load latch"))
    }
}
