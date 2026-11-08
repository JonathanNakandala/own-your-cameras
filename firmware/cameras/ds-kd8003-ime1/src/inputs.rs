//! The one source of input edges (call button, reed, tamper, strike, access decisions). The input loop
//! emits each edge once; every consumer (MQTT, later ONVIF PullPoint) subscribes and receives all of
//! them in order. A consumer that falls behind loses edges instead of stalling the input loop.
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::SystemTime;

/// The running strike pulse at the moment its edge was emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockPulse {
    pub pulse_ms: u64,
    pub remaining_ms: u64,
    /// The absolute relock time, only while the wall clock is known to be in sync.
    pub relock_at: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Call button pressed (true) or released.
    CallButton(bool),
    /// Reed contact on the leaf: open (true) or closed.
    Reed(bool),
    /// Any tamper input active.
    Tamper(bool),
    /// The gate as a cover: open while the strike is released or the leaf is away from the reed.
    Cover(bool),
    /// Strike released with its running pulse (`Some`), or locked. Emitted on every pulse change,
    /// a restart during a pulse included.
    Lock(Option<LockPulse>),
    /// A PIN opened the gate for this user.
    Granted(String),
    /// A PIN was refused.
    Denied,
    /// A remote command opened the gate.
    Remote,
}

impl Edge {
    /// Binary inputs as `(name, state)`: `call_button`, `door` (the reed) and `tamper`. Strike and
    /// access edges have no such form.
    pub fn binary(&self) -> Option<(&'static str, bool)> {
        match *self {
            Self::CallButton(on) => Some(("call_button", on)),
            Self::Reed(open) => Some(("door", open)),
            Self::Tamper(on) => Some(("tamper", on)),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Edges {
    subscribers: Mutex<Vec<SyncSender<Edge>>>,
}

impl Edges {
    pub fn new() -> Self {
        Self::default()
    }
    /// Every edge emitted from now on, up to `capacity` waiting.
    pub fn subscribe(&self, capacity: usize) -> Receiver<Edge> {
        let (tx, rx) = sync_channel(capacity);
        self.lock().push(tx);
        rx
    }
    /// Never blocks. Returns how many subscribers missed the edge because they were full; a
    /// subscriber that went away is forgotten.
    pub fn emit(&self, edge: Edge) -> usize {
        let mut missed = 0;
        self.lock().retain(|tx| match tx.try_send(edge.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                missed += 1;
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
        missed
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<SyncSender<Edge>>> {
        self.subscribers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_gets_every_edge_in_order() {
        let edges = Edges::new();
        let (a, b) = (edges.subscribe(4), edges.subscribe(4));
        edges.emit(Edge::Reed(true));
        edges.emit(Edge::Granted("resident".into()));
        for rx in [&a, &b] {
            assert_eq!(rx.try_recv(), Ok(Edge::Reed(true)));
            assert_eq!(rx.try_recv(), Ok(Edge::Granted("resident".into())));
        }
    }

    #[test]
    fn a_slow_or_gone_subscriber_never_blocks_the_input_loop() {
        let edges = Edges::new();
        let slow = edges.subscribe(1);
        drop(edges.subscribe(1));
        assert_eq!(edges.emit(Edge::CallButton(true)), 0);
        assert_eq!(
            edges.emit(Edge::CallButton(false)),
            1,
            "full subscriber misses it"
        );
        assert_eq!(edges.lock().len(), 1, "gone subscriber forgotten");
        assert_eq!(slow.try_recv(), Ok(Edge::CallButton(true)));
    }

    #[test]
    fn binary_inputs_have_names() {
        assert_eq!(Edge::CallButton(true).binary(), Some(("call_button", true)));
        assert_eq!(Edge::Reed(false).binary(), Some(("door", false)));
        assert_eq!(Edge::Tamper(true).binary(), Some(("tamper", true)));
        assert_eq!(Edge::Lock(None).binary(), None);
        assert_eq!(Edge::Denied.binary(), None);
    }
}
