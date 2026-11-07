//! Snapshot policy: on demand and coalesced. Requests within the freshness window share one JPEG,
//! concurrent requests wait for the one encode in flight, and nothing older than the window is ever
//! served. Pure: the time is passed in; the adapter blocks waiters and runs the encode.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Requests this close together share one encoded frame.
pub const FRESH: Duration = Duration::from_secs(1);
/// How long a request waits for an encode before it gets a 503.
pub const DEADLINE: Duration = Duration::from_secs(2);

pub type Jpeg = Arc<Vec<u8>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// A fresh image is at hand.
    Serve(Jpeg),
    /// The caller runs the one encode, then reports it with [`SnapshotCache::finished`].
    Encode,
    /// Another request is encoding: wait for it.
    Wait,
}

#[derive(Default)]
pub struct SnapshotCache {
    last: Option<(Instant, Jpeg)>,
    encoding: bool,
    /// Every encode started, for the camera's diagnostics.
    encodes: u64,
}

impl SnapshotCache {
    fn fresh(&self, now: Instant) -> Option<Jpeg> {
        self.last
            .as_ref()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= FRESH)
            .map(|(_, jpeg)| Arc::clone(jpeg))
    }

    pub fn plan(&mut self, now: Instant) -> Plan {
        if let Some(jpeg) = self.fresh(now) {
            return Plan::Serve(jpeg);
        }
        if self.encoding {
            return Plan::Wait;
        }
        self.encoding = true;
        self.encodes += 1;
        Plan::Encode
    }

    /// The encode ended at `now`; `None` when it failed (waiters then get no image).
    pub fn finished(&mut self, now: Instant, jpeg: Option<Vec<u8>>) {
        self.encoding = false;
        if let Some(jpeg) = jpeg {
            self.last = Some((now, Arc::new(jpeg)));
        }
    }

    /// What a waiter woken at `now` gets: the fresh image, or `None` to keep waiting (or give up).
    pub fn ready(&self, now: Instant) -> Option<Jpeg> {
        self.fresh(now)
    }

    pub fn busy(&self) -> bool {
        self.encoding
    }

    pub fn encodes(&self) -> u64 {
        self.encodes
    }
}

/// Snapshot requests waiting for the thread that owns the encoder: request threads block in
/// [`Waiters::wait`]; the owner polls [`Waiters::wanted`] and answers everyone waiting at once.
#[derive(Debug, Default)]
pub struct Waiters(std::sync::Mutex<Vec<Waiter>>);

/// One caller: its deadline, and where its answer goes.
type Waiter = (Instant, std::sync::mpsc::SyncSender<Result<Jpeg, String>>);

impl Waiters {
    /// The callers still waiting at `now`; those whose deadline passed are dropped.
    fn waiting(&self, now: Instant) -> std::sync::MutexGuard<'_, Vec<Waiter>> {
        let mut waiting = self.0.lock().unwrap_or_else(|e| e.into_inner());
        waiting.retain(|(deadline, _)| *deadline > now);
        waiting
    }

    /// The next JPEG the owner answers with, or an error once `deadline` passes.
    pub fn wait(&self, deadline: Instant) -> Result<Vec<u8>, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.waiting(Instant::now()).push((deadline, tx));
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result.map(|jpeg| jpeg.as_ref().clone()),
            Err(_) => Err("snapshot: the encoder did not answer in time".into()),
        }
    }

    /// Someone is still waiting.
    pub fn wanted(&self) -> bool {
        !self.waiting(Instant::now()).is_empty()
    }

    /// One answer for everyone waiting now.
    pub fn answer(&self, result: Result<Vec<u8>, String>) {
        let result = result.map(Arc::new);
        for (_, reply) in self.waiting(Instant::now()).drain(..) {
            // A caller that gave up has dropped its receiver.
            let _ = reply.try_send(result.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_requests_share_one_encode_and_the_image_stays_fresh_for_one_second() {
        let t0 = Instant::now();
        let mut cache = SnapshotCache::default();
        assert_eq!(cache.plan(t0), Plan::Encode);
        for _ in 0..9 {
            assert_eq!(cache.plan(t0), Plan::Wait);
        }
        cache.finished(t0 + Duration::from_millis(80), Some(vec![0xff, 0xd8]));
        assert_eq!(
            cache
                .ready(t0 + Duration::from_millis(80))
                .unwrap()
                .as_slice(),
            [0xff, 0xd8]
        );
        assert!(matches!(
            cache.plan(t0 + Duration::from_millis(900)),
            Plan::Serve(_)
        ));
        assert_eq!(cache.encodes(), 1);
        assert_eq!(
            cache.plan(t0 + Duration::from_millis(1100)),
            Plan::Encode,
            "stale: encode again"
        );
        assert_eq!(cache.encodes(), 2);
    }

    #[test]
    fn a_failed_encode_frees_the_encoder_and_serves_nothing_stale() {
        let t0 = Instant::now();
        let mut cache = SnapshotCache::default();
        assert_eq!(cache.plan(t0), Plan::Encode);
        cache.finished(t0, Some(vec![1]));
        assert_eq!(cache.plan(t0 + Duration::from_secs(3)), Plan::Encode);
        cache.finished(t0 + Duration::from_secs(3), None);
        assert!(!cache.busy());
        assert_eq!(cache.ready(t0 + Duration::from_secs(3)), None);
        assert_eq!(cache.plan(t0 + Duration::from_secs(3)), Plan::Encode);
    }

    #[test]
    fn waiting_callers_share_one_answer_and_a_silent_owner_times_out() {
        let w = Arc::new(Waiters::default());
        assert!(!w.wanted());
        let callers: Vec<_> = (0..3)
            .map(|_| {
                let w = Arc::clone(&w);
                std::thread::spawn(move || w.wait(Instant::now() + Duration::from_secs(5)))
            })
            .collect();
        while w.waiting(Instant::now()).len() < 3 {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(w.wanted());
        w.answer(Ok(vec![0xff, 0xd8]));
        for c in callers {
            assert_eq!(c.join().unwrap(), Ok(vec![0xff, 0xd8]));
        }
        assert!(!w.wanted());
        assert!(w.wait(Instant::now() + Duration::from_millis(50)).is_err());
        assert!(!w.wanted(), "a caller that timed out is no longer waiting");
    }
}
