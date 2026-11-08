//! SNTP (RFC 4330) so a camera keeps real time: ONVIF authentication refuses clients whose timestamps are
//! more than minutes off the camera clock, and event times are only useful when true. The packet rules
//! are pure; [`Keeper`] queries the configured servers and steps the system clock.

use std::io;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds from 1900-01-01 (NTP era 0) to 1970-01-01.
const NTP_UNIX: u64 = 2_208_988_800;
/// A clock this close is left alone: stepping it would only add jitter.
const STEP_THRESHOLD: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(3);
/// How often the clock is checked once it is in sync, and the retry while no server answers.
const PERIOD: Duration = Duration::from_secs(3600);
const RETRY: Duration = Duration::from_secs(60);

fn to_ntp(t: SystemTime) -> u64 {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = (d.as_secs() + NTP_UNIX) & 0xffff_ffff;
    let frac = (u64::from(d.subsec_nanos()) << 32) / 1_000_000_000;
    secs << 32 | frac
}

/// NTP timestamps are seconds since 1900 modulo 2^32; era 1 starts in 2036. A timestamp below the
/// 1970 offset is read as era 1.
fn from_ntp(ts: u64) -> SystemTime {
    let secs = ts >> 32;
    let secs = if secs < NTP_UNIX {
        secs + (1 << 32)
    } else {
        secs
    };
    let nanos = ((ts & 0xffff_ffff) * 1_000_000_000) >> 32;
    UNIX_EPOCH + Duration::new(secs - NTP_UNIX, nanos as u32)
}

/// A client request sent at `sent`: version 4, mode 3, the send time as transmit timestamp.
pub fn request(sent: SystemTime) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0] = 0b00_100_011;
    p[40..48].copy_from_slice(&to_ntp(sent).to_be_bytes());
    p
}

/// The clock offset (server minus local, in signed nanoseconds) from a reply to the request sent at `sent`
/// and received at `received`; `None` for anything but a valid, synchronised server reply to it.
pub fn offset(reply: &[u8], sent: SystemTime, received: SystemTime) -> Option<i128> {
    if reply.len() < 48 {
        return None;
    }
    let ts = |at: usize| u64::from_be_bytes(reply[at..at + 8].try_into().unwrap_or_default());
    let mode = reply[0] & 7;
    let leap = reply[0] >> 6;
    let stratum = reply[1];
    // Server mode, not "alarm" (unsynchronised), a real stratum, the request's own timestamp echoed.
    if mode != 4
        || leap == 3
        || !(1..=15).contains(&stratum)
        || ts(24) != to_ntp(sent)
        || ts(40) == 0
    {
        return None;
    }
    let nanos = |t: SystemTime| -> i128 {
        t.duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as i128)
    };
    let (t1, t2, t3, t4) = (
        nanos(sent),
        nanos(from_ntp(ts(32))),
        nanos(from_ntp(ts(40))),
        nanos(received),
    );
    Some(((t2 - t1) + (t3 - t4)) / 2)
}

/// One query to `server` (`host` or `host:port`, port 123 by default).
pub fn query(server: &str) -> io::Result<i128> {
    let target = if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:123")
    };
    let addr = target
        .to_socket_addrs()?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| io::Error::other("no IPv4 address"))?;
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.connect(addr)?;
    let sent = SystemTime::now();
    socket.send(&request(sent))?;
    let mut buf = [0u8; 128];
    let n = socket.recv(&mut buf)?;
    offset(&buf[..n], sent, SystemTime::now()).ok_or_else(|| io::Error::other("invalid NTP reply"))
}

fn step(offset: i128) -> io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128);
    let target = now + offset;
    if target <= 0 {
        return Err(io::Error::other("refusing a clock before 1970"));
    }
    let ts = libc::timespec {
        tv_sec: (target / 1_000_000_000) as libc::time_t,
        tv_nsec: (target % 1_000_000_000) as _,
    };
    // SAFETY: a valid timespec on the stack; CLOCK_REALTIME needs root, which the camera runtime has.
    if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Keeps the system clock on the first server that answers, checking every hour (every minute until one
/// answers). Monotonic clocks, which every timeout and pacer here uses, are not affected by a step.
pub struct Keeper {
    pub servers: Vec<String>,
    pub log: fn(String),
}

impl Keeper {
    /// One round: the first valid answer is applied. Returns whether the clock is now in sync.
    pub fn sync(&self) -> bool {
        for server in &self.servers {
            match query(server) {
                Ok(offset) if offset.unsigned_abs() > STEP_THRESHOLD.as_nanos() => {
                    match step(offset) {
                        Ok(()) => {
                            (self.log)(format!(
                                "time: clock stepped {:+.3} s from {server}",
                                offset as f64 / 1e9
                            ));
                            return true;
                        }
                        Err(e) => (self.log)(format!("time: cannot set the clock: {e}")),
                    }
                }
                Ok(_) => return true,
                Err(e) => (self.log)(format!("time: {server}: {e}")),
            }
        }
        false
    }

    /// One [`Keeper::sync`] round whose result `synced` then carries.
    pub fn round(&self, synced: &AtomicBool) -> bool {
        let ok = self.sync();
        synced.store(ok, Ordering::Relaxed);
        ok
    }

    /// Runs [`Keeper::sync`] until `stop`, on its own thread.
    pub fn spawn(self, stop: Arc<AtomicBool>) -> io::Result<std::thread::JoinHandle<()>> {
        self.spawn_synced(stop, Arc::new(AtomicBool::new(false)))
    }

    /// [`Keeper::spawn`], with `synced` true while the last round put or found the clock in sync.
    pub fn spawn_synced(
        self,
        stop: Arc<AtomicBool>,
        synced: Arc<AtomicBool>,
    ) -> io::Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("time".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let wait = if self.round(&synced) { PERIOD } else { RETRY };
                    let until = std::time::Instant::now() + wait;
                    while !stop.load(Ordering::Relaxed) && std::time::Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn reply(sent: SystemTime, server_rx: SystemTime, server_tx: SystemTime) -> [u8; 48] {
        let mut p = [0u8; 48];
        p[0] = 0b00_100_100;
        p[1] = 2;
        p[24..32].copy_from_slice(&to_ntp(sent).to_be_bytes());
        p[32..40].copy_from_slice(&to_ntp(server_rx).to_be_bytes());
        p[40..48].copy_from_slice(&to_ntp(server_tx).to_be_bytes());
        p
    }

    #[test]
    fn timestamps_round_trip_across_the_2036_era() {
        for t in [at(0), at(1_794_052_800), at(2_085_978_496 + 10)] {
            let back = from_ntp(to_ntp(t));
            let d = back.duration_since(t).unwrap_or_else(|e| e.duration());
            assert!(d < Duration::from_micros(1), "{t:?}");
        }
    }

    #[test]
    fn the_offset_is_the_mean_of_both_legs() {
        // A camera seven years slow: sent at its 2019, the server answers in 2026, 2 ms round trip.
        let sent = at(1_562_159_298);
        let server = at(1_794_052_800);
        let r = reply(sent, server, server + Duration::from_millis(1));
        let offset = offset(&r, sent, sent + Duration::from_millis(2)).unwrap();
        let expected = (1_794_052_800 - 1_562_159_298) as i128 * 1_000_000_000;
        assert!((offset - expected).abs() < 1_000_000, "{offset}");
    }

    #[test]
    fn foreign_unsynchronised_or_short_replies_are_refused() {
        let sent = at(1_000_000_000);
        let good = reply(sent, sent, sent);
        assert!(offset(&good, sent, sent).is_some());
        assert!(offset(&good[..47], sent, sent).is_none());
        let mut client_mode = good;
        client_mode[0] = 0b00_100_011;
        assert!(offset(&client_mode, sent, sent).is_none());
        let mut alarm = good;
        alarm[0] |= 0b11 << 6;
        assert!(offset(&alarm, sent, sent).is_none());
        let mut stratum0 = good;
        stratum0[1] = 0;
        assert!(offset(&stratum0, sent, sent).is_none());
        let other = reply(at(5), sent, sent);
        assert!(
            offset(&other, sent, sent).is_none(),
            "not an answer to our request"
        );
        let r = request(sent);
        assert_eq!(r[0], 0x23);
        assert_eq!(
            u64::from_be_bytes(r[40..48].try_into().unwrap()),
            to_ntp(sent)
        );
    }

    /// A loopback server answering with the reply fixture: its clock is ours, so nothing is stepped.
    fn loopback_server() -> String {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let mut buf = [0u8; 48];
            if let Ok((_, from)) = socket.recv_from(&mut buf) {
                let now = SystemTime::now();
                let mut answer = reply(now, now, now);
                answer[24..32].copy_from_slice(&buf[40..48]);
                let _ = socket.send_to(&answer, from);
            }
        });
        addr
    }

    #[test]
    fn the_synced_flag_follows_the_last_round() {
        let synced = AtomicBool::new(false);
        let keeper = Keeper {
            servers: vec![loopback_server()],
            log: |_| {},
        };
        assert!(keeper.round(&synced));
        assert!(synced.load(Ordering::Relaxed));
        let keeper = Keeper {
            servers: vec![],
            log: |_| {},
        };
        assert!(!keeper.round(&synced));
        assert!(!synced.load(Ordering::Relaxed));
    }
}
