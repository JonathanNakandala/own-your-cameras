//! PullPoint subscriptions (ONVIF Events): at most [`MAX_SUBSCRIPTIONS`], each with its own bounded
//! message queue and termination time. Digital inputs (`tns1:Device/Trigger/DigitalInput`) are the
//! only topic. Pure: the time is passed in; the adapter adds locking and waiting.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

pub const MAX_SUBSCRIPTIONS: usize = 4;
/// Messages kept per subscription; the oldest is dropped when a client stops pulling.
const QUEUE: usize = 64;
/// Lifetime when the client asks for none, and the most it may ask for.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(60);
pub const MAX_LIFETIME: Duration = Duration::from_secs(3600);
pub const TOPIC: &str = "tns1:Device/Trigger/DigitalInput";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Initialized,
    Changed,
}

impl Operation {
    pub fn onvif(self) -> &'static str {
        match self {
            Self::Initialized => "Initialized",
            Self::Changed => "Changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub time: SystemTime,
    pub input: String,
    pub state: bool,
    pub operation: Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    TooManySubscriptions,
    NoSuchSubscription,
}

struct Subscription {
    id: u32,
    expires: SystemTime,
    queue: VecDeque<Message>,
}

#[derive(Default)]
pub struct PullPointBook {
    subs: Vec<Subscription>,
    next_id: u32,
    /// Current state of every input, in the camera's order.
    inputs: Vec<(String, bool)>,
}

/// When a subscription created or renewed at `now` ends for a requested termination.
pub fn termination(now: SystemTime, requested: Option<Termination>) -> SystemTime {
    let lifetime = match requested {
        None => DEFAULT_LIFETIME,
        Some(Termination::After(d)) => d,
        Some(Termination::At(t)) => t.duration_since(now).unwrap_or_default(),
    };
    now + lifetime.clamp(Duration::from_secs(1), MAX_LIFETIME)
}

/// `InitialTerminationTime` / `TerminationTime`: a duration or an absolute time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    After(Duration),
    At(SystemTime),
}

impl Termination {
    pub fn parse(text: &str) -> Option<Self> {
        crate::time::parse_duration(text)
            .map(Self::After)
            .or_else(|| crate::time::parse(text).map(Self::At))
    }
}

impl PullPointBook {
    /// The inputs this camera reports, all initially inactive.
    pub fn new(inputs: &[&str]) -> Self {
        Self {
            inputs: inputs.iter().map(|i| (i.to_string(), false)).collect(),
            ..Self::default()
        }
    }

    pub fn expire(&mut self, now: SystemTime) {
        self.subs.retain(|s| s.expires > now);
    }

    fn sub(&mut self, id: u32, now: SystemTime) -> Result<&mut Subscription, EventError> {
        self.expire(now);
        self.subs
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(EventError::NoSuchSubscription)
    }

    fn initialized(&self, now: SystemTime) -> impl Iterator<Item = Message> + '_ {
        self.inputs.iter().map(move |(input, state)| Message {
            time: now,
            input: input.clone(),
            state: *state,
            operation: Operation::Initialized,
        })
    }

    /// A new subscription, primed with the current state of every input. Returns its id and end.
    pub fn create(
        &mut self,
        now: SystemTime,
        requested: Option<Termination>,
    ) -> Result<(u32, SystemTime), EventError> {
        self.expire(now);
        if self.subs.len() >= MAX_SUBSCRIPTIONS {
            return Err(EventError::TooManySubscriptions);
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        let expires = termination(now, requested);
        let queue = self.initialized(now).collect();
        self.subs.push(Subscription { id, expires, queue });
        Ok((id, expires))
    }

    pub fn renew(
        &mut self,
        id: u32,
        now: SystemTime,
        requested: Option<Termination>,
    ) -> Result<SystemTime, EventError> {
        let sub = self.sub(id, now)?;
        sub.expires = termination(now, requested);
        Ok(sub.expires)
    }

    pub fn unsubscribe(&mut self, id: u32, now: SystemTime) -> Result<(), EventError> {
        self.sub(id, now)?;
        self.subs.retain(|s| s.id != id);
        Ok(())
    }

    /// `SetSynchronizationPoint`: queue the current state of every input again.
    pub fn synchronize(&mut self, id: u32, now: SystemTime) -> Result<(), EventError> {
        let messages: Vec<Message> = self.initialized(now).collect();
        let sub = self.sub(id, now)?;
        for m in messages {
            push(&mut sub.queue, m);
        }
        Ok(())
    }

    /// Up to `limit` queued messages, oldest first, and the subscription's end.
    pub fn pull(
        &mut self,
        id: u32,
        now: SystemTime,
        limit: usize,
    ) -> Result<(Vec<Message>, SystemTime), EventError> {
        let sub = self.sub(id, now)?;
        let n = limit.min(sub.queue.len());
        Ok((sub.queue.drain(..n).collect(), sub.expires))
    }

    pub fn exists(&self, id: u32) -> bool {
        self.subs.iter().any(|s| s.id == id)
    }

    pub fn has_messages(&self, id: u32) -> bool {
        self.subs.iter().any(|s| s.id == id && !s.queue.is_empty())
    }

    /// An input changed (or was re-reported): every live subscription gets a `Changed` message when the
    /// state differs from the last one. Unknown inputs are ignored.
    pub fn input(&mut self, input: &str, state: bool, now: SystemTime) {
        self.expire(now);
        let Some(entry) = self.inputs.iter_mut().find(|(i, _)| i == input) else {
            return;
        };
        if entry.1 == state {
            return;
        }
        entry.1 = state;
        for sub in &mut self.subs {
            push(
                &mut sub.queue,
                Message {
                    time: now,
                    input: input.to_string(),
                    state,
                    operation: Operation::Changed,
                },
            );
        }
    }
}

fn push(queue: &mut VecDeque<Message>, m: Message) {
    if queue.len() >= QUEUE {
        queue.pop_front();
    }
    queue.push_back(m);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + s)
    }

    #[test]
    fn a_subscription_starts_with_the_current_state_then_sees_only_changes() {
        let mut book = PullPointBook::new(&["call_button", "door"]);
        book.input("door", true, t(0));
        let (id, expires) = book
            .create(t(1), Some(Termination::After(Duration::from_secs(600))))
            .unwrap();
        assert_eq!(expires, t(601));
        let (first, _) = book.pull(id, t(2), 100).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|m| (m.input.as_str(), m.state, m.operation))
                .collect::<Vec<_>>(),
            [
                ("call_button", false, Operation::Initialized),
                ("door", true, Operation::Initialized)
            ]
        );
        book.input("door", true, t(3));
        book.input("call_button", true, t(4));
        book.input("unknown", true, t(4));
        assert!(book.has_messages(id));
        let (changes, _) = book.pull(id, t(5), 100).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].input.as_str(), changes[0].operation),
            ("call_button", Operation::Changed)
        );
        book.synchronize(id, t(6)).unwrap();
        assert_eq!(
            book.pull(id, t(6), 1).unwrap().0.len(),
            1,
            "the limit applies"
        );
    }

    #[test]
    fn subscriptions_are_capped_expire_renew_and_unsubscribe() {
        let mut book = PullPointBook::new(&["door"]);
        let ids: Vec<u32> = (0..MAX_SUBSCRIPTIONS)
            .map(|_| book.create(t(0), None).unwrap().0)
            .collect();
        assert_eq!(
            book.create(t(0), None),
            Err(EventError::TooManySubscriptions)
        );
        book.unsubscribe(ids[0], t(1)).unwrap();
        assert_eq!(
            book.pull(ids[0], t(1), 1),
            Err(EventError::NoSuchSubscription)
        );
        let renewed = book
            .renew(ids[1], t(50), Some(Termination::At(t(500))))
            .unwrap();
        assert_eq!(renewed, t(500));
        // The others end after the 60 s default.
        assert_eq!(
            book.pull(ids[2], t(61), 1),
            Err(EventError::NoSuchSubscription)
        );
        assert!(book.pull(ids[1], t(61), 1).is_ok());
        assert_eq!(
            termination(t(0), Some(Termination::After(Duration::from_secs(99_999)))),
            t(0) + MAX_LIFETIME
        );
    }

    #[test]
    fn a_client_that_stops_pulling_keeps_only_the_newest_messages() {
        let mut book = PullPointBook::new(&["door"]);
        let (id, _) = book
            .create(t(0), Some(Termination::After(MAX_LIFETIME)))
            .unwrap();
        for i in 0..200 {
            book.input("door", i % 2 == 0, t(1));
        }
        let (messages, _) = book.pull(id, t(2), 1000).unwrap();
        assert_eq!(messages.len(), 64);
        assert!(!messages.last().unwrap().state, "the newest state survives");
    }

    #[test]
    fn terminations_parse_as_durations_or_times() {
        assert_eq!(
            Termination::parse("PT10M"),
            Some(Termination::After(Duration::from_secs(600)))
        );
        assert!(matches!(
            Termination::parse("2026-11-07T12:00:00Z"),
            Some(Termination::At(_))
        ));
        assert_eq!(Termination::parse("soon"), None);
    }
}
