//! What the camera publishes: retained states, events, acknowledgments, removals and option changes.
//! The pure [`Outbox`] owns the rules; [`Publisher`] is the thread-safe handle onto it.
use super::{Config, Domain, Publication};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// An event older than this is not worth delivering after an outage.
pub const EVENT_TTL: Duration = Duration::from_secs(10);
/// A state, event or acknowledgment payload: room for an attributes object (Reolink's autofocus report
/// passes 512 bytes) inside the transport's packet limit.
const MAX_VALUE: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Message {
    Publish {
        publication: Publication,
        /// Events and acknowledgments expire.
        expires: Option<Instant>,
    },
    /// An empty retained payload clearing a retired entity's topic; never expires, and the next
    /// announcement sends every pending one ahead of the discovery configs.
    Remove(Publication),
    Options {
        id: String,
        options: Vec<String>,
    },
}

/// Pure publication rules, time injected.
pub(crate) struct Outbox {
    topics: BTreeMap<String, String>,
    prefix: String,
    retained: BTreeMap<String, String>,
    dirty: BTreeSet<String>,
    /// Ids a command was addressed to: their next state goes out even when unchanged, so a refused
    /// or failed change shows the value in force again.
    answer: BTreeSet<String>,
    queue: VecDeque<Message>,
    capacity: usize,
}

impl Outbox {
    pub(crate) fn new(config: &Config) -> Self {
        let mut topics = BTreeMap::new();
        for e in &config.entities {
            if e.domain.has_state() {
                topics.insert(e.id.clone(), config.state_topic_of(&e.id));
            }
            if let Some(id) = &e.attributes {
                topics.insert(id.clone(), config.state_topic_of(id));
            }
        }
        Self {
            topics,
            prefix: config.prefix.clone(),
            retained: BTreeMap::new(),
            dirty: BTreeSet::new(),
            answer: BTreeSet::new(),
            queue: VecDeque::new(),
            capacity: config.limits.outbox,
        }
    }
    fn topic(&self, id: &str) -> Option<&String> {
        self.topics.get(id)
    }
    /// Remembered for replay; published once per change, coalesced. The value already remembered
    /// is not published again (only a reconnect or HA birth replays it), except as the answer to a
    /// command addressed to `id` ([`Outbox::commanded`]).
    pub(crate) fn state(&mut self, id: &str, value: String) -> io::Result<()> {
        if self.topic(id).is_none() || value.len() > MAX_VALUE {
            return Err(io::Error::other(format!(
                "unknown or oversized MQTT state {id}"
            )));
        }
        let answer = self.answer.remove(id);
        if answer || self.retained.get(id) != Some(&value) {
            self.retained.insert(id.to_owned(), value);
            self.dirty.insert(id.to_owned());
        }
        Ok(())
    }
    /// A command for `id` reached the camera: the next state it publishes for `id` is sent even if
    /// unchanged.
    pub(crate) fn commanded(&mut self, id: &str) {
        if self.topic(id).is_some() {
            self.answer.insert(id.to_owned());
        }
    }
    /// Publishes the remembered value of `id` again, unchanged: the answer to a refused or failed
    /// change, so Home Assistant drops the value it asked for. Nothing remembered: nothing to send.
    pub(crate) fn resend(&mut self, id: &str) {
        if self.retained.contains_key(id) {
            self.dirty.insert(id.to_owned());
        }
    }
    pub(crate) fn push(&mut self, message: Message, now: Instant) -> io::Result<()> {
        if self.queue.len() >= self.capacity {
            // Expired events must not crowd out fresh ones after an outage.
            self.queue
                .retain(|m| !matches!(m, Message::Publish { expires: Some(at), .. } if now > *at));
        }
        if self.queue.len() >= self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "MQTT outbox full",
            ));
        }
        self.queue.push_back(message);
        Ok(())
    }
    pub(crate) fn event(&mut self, id: &str, payload: String, now: Instant) -> io::Result<()> {
        let topic = self
            .topic(id)
            .ok_or_else(|| io::Error::other(format!("unknown MQTT event {id}")))?
            .clone();
        if payload.len() > MAX_VALUE {
            return Err(io::Error::other("oversized MQTT event"));
        }
        self.push(
            Message::Publish {
                publication: Publication {
                    topic,
                    payload: payload.into_bytes(),
                    retain: false,
                },
                expires: Some(now + EVENT_TTL),
            },
            now,
        )
    }
    /// A non-retained reply on the state topic of `id` (any identifier, also one without an entity).
    pub(crate) fn ack(&mut self, id: &str, value: String, now: Instant) -> io::Result<()> {
        if !super::identifier(id) || value.len() > MAX_VALUE {
            return Err(io::Error::other("invalid MQTT acknowledgment"));
        }
        let topic = self
            .topic(id)
            .cloned()
            .unwrap_or_else(|| format!("{}/{id}/state", self.prefix));
        self.push(
            Message::Publish {
                publication: Publication {
                    topic,
                    payload: value.into_bytes(),
                    retain: false,
                },
                expires: Some(now + EVENT_TTL),
            },
            now,
        )
    }
    /// Everything remembered, for a fresh connection or HA birth; nothing stays dirty.
    pub(crate) fn replay(&mut self) -> Vec<Publication> {
        self.dirty.clear();
        self.retained
            .iter()
            .filter_map(|(id, value)| {
                Some(Publication::retained(
                    self.topics.get(id)?.clone(),
                    value.as_bytes().to_vec(),
                ))
            })
            .collect()
    }
    /// Pending removals, taken out of the queue in order (the announcement sends them first).
    pub(crate) fn take_removals(&mut self) -> Vec<Publication> {
        let mut removals = Vec::new();
        self.queue.retain(|m| match m {
            Message::Remove(p) => {
                removals.push(p.clone());
                false
            }
            _ => true,
        });
        removals
    }
    /// Changed states, then queued messages; expired events are dropped here, never delivered.
    pub(crate) fn take(&mut self, now: Instant) -> (Vec<Publication>, Vec<Message>) {
        let states = std::mem::take(&mut self.dirty)
            .into_iter()
            .filter_map(|id| {
                let value = self.retained.get(&id)?;
                Some(Publication::retained(
                    self.topics.get(&id)?.clone(),
                    value.as_bytes().to_vec(),
                ))
            })
            .collect();
        let messages = self
            .queue
            .drain(..)
            .filter(|m| !matches!(m, Message::Publish { expires: Some(at), .. } if now > *at))
            .collect();
        (states, messages)
    }
    /// Puts back what the client could not take, ahead of anything newer.
    pub(crate) fn unsent(&mut self, states: Vec<Publication>, messages: Vec<Message>) {
        for p in states {
            if let Some((id, _)) = self.topics.iter().find(|(_, t)| **t == p.topic) {
                self.dirty.insert(id.clone());
            }
        }
        for m in messages.into_iter().rev() {
            self.queue.push_front(m);
        }
    }
}

pub(crate) struct Shared {
    pub(crate) outbox: Mutex<Outbox>,
    pub(crate) online: AtomicBool,
    pub(crate) refresh: AtomicBool,
    pub(crate) drops: AtomicU32,
    config_topics: BTreeMap<String, String>,
    discovery_prefix: String,
    node_id: String,
}

/// The camera's handle; cheap to clone, never blocks on the network.
#[derive(Clone)]
pub struct Publisher {
    pub(crate) shared: Arc<Shared>,
}

impl Publisher {
    pub(crate) fn new(config: &Config) -> Self {
        let config_topics = config
            .entities
            .iter()
            .map(|e| (e.id.clone(), config.config_topic(&e.domain, &e.id)))
            .collect();
        Self {
            shared: Arc::new(Shared {
                outbox: Mutex::new(Outbox::new(config)),
                online: AtomicBool::new(false),
                refresh: AtomicBool::new(true),
                drops: AtomicU32::new(0),
                config_topics,
                discovery_prefix: config.discovery_prefix.clone(),
                node_id: config.node_id.clone(),
            }),
        }
    }
    fn outbox(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.shared.outbox.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn is_connected(&self) -> bool {
        self.shared.online.load(Ordering::Acquire)
    }
    /// Broker connections lost since start.
    pub fn drops(&self) -> u32 {
        self.shared.drops.load(Ordering::Relaxed)
    }
    /// Retained state of an entity (or attributes id), replayed after every reconnect.
    pub fn state(&self, id: &str, value: impl Into<String>) -> io::Result<()> {
        self.outbox().state(id, value.into())
    }
    /// Publishes the value in force of `id` again although it did not change. The transport does
    /// this for a command it or the parser rejects; for a command that reached the camera, the
    /// camera's next [`Publisher::state`] of `id` goes out even when unchanged, so an owner that
    /// refuses or fails a change answers by publishing the value in force as usual.
    pub fn resend(&self, id: &str) {
        self.outbox().resend(id);
    }
    pub(crate) fn commanded(&self, id: &str) {
        self.outbox().commanded(id);
    }
    /// A command for `id` never reached the camera: the value in force again, no answer pending.
    pub(crate) fn refused(&self, id: &str) {
        let mut outbox = self.outbox();
        outbox.answer.remove(id);
        outbox.resend(id);
    }
    /// Non-retained event on the entity's state topic: never coalesced, never replayed, dropped when
    /// it cannot be delivered within [`EVENT_TTL`].
    pub fn event(&self, id: &str, payload: impl Into<String>) -> io::Result<()> {
        self.outbox().event(id, payload.into(), Instant::now())
    }
    /// Non-retained reply on `<state topic of id>`, for the request that caused it only.
    pub fn ack(&self, id: &str, value: impl Into<String>) -> io::Result<()> {
        self.outbox().ack(id, value.into(), Instant::now())
    }
    /// Clears an entity that no longer exists: empty retained discovery config and state. A removal
    /// still pending at the next announcement goes out ahead of its discovery configs; ids retired for
    /// good belong in [`Config::retired`] instead, which every announcement clears first.
    ///
    /// [`Config::retired`]: super::Config::retired
    pub fn remove(&self, domain: &Domain, id: &str) -> io::Result<()> {
        if !super::identifier(id) {
            return Err(io::Error::other("invalid MQTT entity id"));
        }
        let config = self
            .shared
            .config_topics
            .get(id)
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{}/{}/{}/{id}/config",
                    self.shared.discovery_prefix,
                    domain.component(),
                    self.shared.node_id
                )
            });
        let mut outbox = self.outbox();
        let state = outbox
            .topic(id)
            .cloned()
            .unwrap_or_else(|| format!("{}/{id}/state", outbox.prefix));
        for topic in [config, state] {
            outbox.push(
                Message::Remove(Publication::retained(topic, Vec::new())),
                Instant::now(),
            )?;
        }
        Ok(())
    }
    /// New options of a select; its discovery config is republished.
    pub fn set_options(&self, id: &str, options: Vec<String>) -> io::Result<()> {
        self.outbox().push(
            Message::Options {
                id: id.to_owned(),
                options,
            },
            Instant::now(),
        )
    }
    /// Asks the state owner to publish a full refresh (a reconnect does this too).
    pub fn request_refresh(&self) {
        self.shared.refresh.store(true, Ordering::Release);
    }
    /// True once after a refresh was requested.
    pub fn take_refresh(&self) -> bool {
        self.shared.refresh.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config;
    use super::super::*;
    use super::*;

    fn outbox() -> Outbox {
        Outbox::new(&config(vec![
            Entity::new("reed", "Door", Domain::BinarySensor).state_topic("reed"),
            Entity::new(
                "doorbell",
                "Doorbell",
                Domain::Event {
                    types: vec!["press"],
                },
            )
            .state_topic("call_button"),
            Entity::new("gate", "Gate", Domain::Cover).attributes("lock"),
        ]))
    }
    fn topics(p: &[Publication]) -> Vec<(&str, &[u8], bool)> {
        p.iter()
            .map(|p| (p.topic.as_str(), p.payload.as_slice(), p.retain))
            .collect()
    }
    fn published(messages: &[Message]) -> Vec<(&str, &[u8], bool)> {
        messages
            .iter()
            .filter_map(|m| match m {
                Message::Publish { publication: p, .. } | Message::Remove(p) => {
                    Some((p.topic.as_str(), p.payload.as_slice(), p.retain))
                }
                Message::Options { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_commanded_state_is_answered_even_when_unchanged() {
        let mut o = outbox();
        let now = Instant::now();
        o.state("reed", "closed".into()).unwrap();
        assert_eq!(o.take(now).0.len(), 1);
        o.state("reed", "closed".into()).unwrap();
        assert!(o.take(now).0.is_empty(), "unchanged: not published");
        o.commanded("reed");
        o.state("reed", "closed".into()).unwrap();
        assert_eq!(
            topics(&o.take(now).0),
            [("camera/test/reed", &b"closed"[..], true)],
            "the answer to a command"
        );
        o.state("reed", "closed".into()).unwrap();
        assert!(o.take(now).0.is_empty(), "answered once");
        o.resend("reed");
        assert_eq!(
            o.take(now).0.len(),
            1,
            "a rejected command resends the value in force"
        );
        o.resend("doorbell");
        o.commanded("unknown");
        assert!(o.take(now).0.is_empty(), "nothing remembered: nothing sent");
    }

    #[test]
    fn states_are_retained_coalesced_and_replayed() {
        let mut o = outbox();
        o.state("reed", "open".into()).unwrap();
        o.state("reed", "closed".into()).unwrap();
        o.state("lock", r#"{"a":1}"#.into()).unwrap();
        assert!(o.state("nope", "1".into()).is_err());
        assert!(o.state("doorbell", "x".repeat(MAX_VALUE + 1)).is_err());
        o.state("lock", "x".repeat(600)).unwrap();
        o.state("lock", r#"{"a":1}"#.into()).unwrap();
        let (states, _) = o.take(Instant::now());
        assert_eq!(
            topics(&states),
            [
                ("camera/test/lock/state", br#"{"a":1}"#.as_slice(), true),
                ("camera/test/reed", b"closed", true)
            ]
        );
        assert!(o.take(Instant::now()).0.is_empty(), "published once");
        o.state("reed", "closed".into()).unwrap();
        assert!(
            o.take(Instant::now()).0.is_empty(),
            "an unchanged value is not republished"
        );
        assert_eq!(o.replay().len(), 2, "replayed after reconnect");
        o.state("reed", "open".into()).unwrap();
        assert_eq!(
            topics(&o.take(Instant::now()).0),
            [("camera/test/reed", b"open".as_slice(), true)],
            "a change is"
        );
    }

    #[test]
    fn events_are_not_replayed_not_coalesced_and_expire_after_ten_seconds() {
        let mut o = outbox();
        let t0 = Instant::now();
        o.event("doorbell", r#"{"event_type":"press"}"#.into(), t0)
            .unwrap();
        o.event("doorbell", r#"{"event_type":"press"}"#.into(), t0)
            .unwrap();
        assert!(o.replay().is_empty(), "events are never replayed");
        let (_, messages) = o.take(t0 + Duration::from_secs(10));
        assert_eq!(
            published(&messages),
            [(
                "camera/test/call_button",
                br#"{"event_type":"press"}"#.as_slice(),
                false
            ); 2],
            "two presses stay two"
        );
        o.event("doorbell", "{}".into(), t0).unwrap();
        assert!(
            o.take(t0 + Duration::from_millis(10_001)).1.is_empty(),
            "expired"
        );
        assert!(o.event("reed_typo", "{}".into(), t0).is_err());
        for _ in 0..64 {
            o.event("doorbell", "{}".into(), t0).unwrap();
        }
        let later = t0 + Duration::from_secs(11);
        assert!(
            o.event("doorbell", "{}".into(), later).is_ok(),
            "expired events make room"
        );
        assert_eq!(o.take(later).1.len(), 1);
    }

    #[test]
    fn acks_are_nonretained_and_the_outbox_is_bounded() {
        let mut o = outbox();
        let now = Instant::now();
        o.ack("stop", "stopped".into(), now).unwrap();
        let (_, messages) = o.take(now);
        assert_eq!(
            published(&messages),
            [("camera/test/stop/state", b"stopped".as_slice(), false)]
        );
        for _ in 0..64 {
            o.ack("stop", "x".into(), now).unwrap();
        }
        assert_eq!(
            o.ack("stop", "x".into(), now).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let (_, mut messages) = o.take(now);
        let rest = messages.split_off(1);
        o.unsent(Vec::new(), rest);
        assert_eq!(o.take(now).1.len(), 63, "unsent messages come back");
    }

    #[test]
    fn removal_clears_config_and_state_without_expiry() {
        let publisher = Publisher::new(&config(vec![Entity::new("cpu", "CPU", Domain::Sensor)]));
        publisher.remove(&Domain::Sensor, "old_cpu").unwrap();
        let (_, messages) = publisher
            .outbox()
            .take(Instant::now() + Duration::from_secs(3600));
        assert_eq!(
            published(&messages),
            [
                (
                    "homeassistant/sensor/cam_test/old_cpu/config",
                    b"".as_slice(),
                    true
                ),
                ("camera/test/old_cpu/state", b"", true)
            ]
        );
    }
}
