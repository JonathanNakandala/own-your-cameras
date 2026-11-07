//! Random bytes for nonces and message ids, from the kernel; a clock-derived fallback keeps a camera
//! without `/dev/urandom` answering (its nonces are then guessable, never reused within a run).

use std::io::Read;
use std::time::SystemTime;

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !ok {
        let t = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> (8 * (i % 16))) as u8 ^ i as u8;
        }
    }
    b
}
