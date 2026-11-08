//! Commands: topic routing, the generic setting contract and the bounded queue to the camera.
use super::{Config, Control, Domain, Entity};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A validated command. `C` is the camera's own command set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<C> {
    /// Opaque setting identity and a value validated against the entity's domain (switch 0/1,
    /// select index, button 0); hardware policy belongs to the camera.
    Setting {
        slot: u16,
        value: i32,
    },
    /// Hard stop.
    Stop,
    /// Graceful end of held motion.
    Release,
    App(C),
}

impl<C> Command<C> {
    pub fn map<D>(self, f: impl FnOnce(C) -> D) -> Command<D> {
        match self {
            Self::Setting { slot, value } => Command::Setting { slot, value },
            Self::Stop => Command::Stop,
            Self::Release => Command::Release,
            Self::App(c) => Command::App(f(c)),
        }
    }
}

/// What the network thread does with a Stop or Release, decided by [`super::Hooks::on_urgent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgent {
    /// Start a new epoch: discard queued commands and deliver this one ahead of a full queue.
    Interrupt,
    /// Queue it in order like any other command.
    Queue,
    /// The camera acted on it already (a direct stop path); nothing is queued.
    Handled,
}

/// The camera's command vocabulary.
pub trait Parser: Send {
    type App: Send;
    /// `id` is the `<id>` of `<prefix>/<id>/set`, or the route of the entity whose command topic
    /// received the message. `Ok(None)` ignores the message, `Err` rejects it (and is logged).
    fn parse(&self, id: &str, text: &str) -> Result<Option<Command<Self::App>>, String>;
}

/// A command the transport or the camera's parser refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    /// The entity whose command topic received it, if any; its value in force is published again so
    /// Home Assistant shows what applies instead of the refused value.
    pub entity: Option<String>,
    pub reason: String,
}

/// Payload bound before any parsing allocates.
const MAX_PAYLOAD: usize = 256;

impl Config {
    /// Retained commands are never executed: a retained command would act again on every reconnect
    /// and boot. The broker sets the retain flag only when it replays a stored message to a new
    /// subscription; a message delivered live arrives with the flag cleared, so this catches the
    /// replay, not the publisher's flag. (The camera subscribes at QoS 0, where the broker never
    /// marks a delivery as a duplicate, so there is no duplicate flag to check.)
    pub(crate) fn route<P: Parser>(
        &self,
        parser: &P,
        topic: &str,
        payload: &[u8],
        retain: bool,
    ) -> Result<Option<Command<P::App>>, Rejected> {
        if retain {
            return Ok(None);
        }
        let Some((id, entity)) = self.command_target(topic) else {
            return Ok(None);
        };
        let rejected = |reason: String| Rejected {
            entity: entity.map(|e| e.id.clone()),
            reason,
        };
        if payload.len() > MAX_PAYLOAD {
            return Err(rejected(format!("{id}: command too long")));
        }
        let text = std::str::from_utf8(payload)
            .map_err(|_| rejected(format!("{id}: command is not UTF-8")))?;
        match entity.map(|e| (e, e.control)) {
            Some((e, Control::Slot(slot))) => slot_value(e, text)
                .map(|value| Some(Command::Setting { slot, value }))
                .ok_or_else(|| rejected(format!("{id}: invalid value"))),
            Some((_, Control::Route(route))) => parser.parse(route, text).map_err(rejected),
            _ => parser.parse(id, text).map_err(rejected),
        }
    }

    /// The `<id>` (or entity) a command topic addresses, and the entity whose command it is, if any.
    fn command_target<'a>(&'a self, topic: &'a str) -> Option<(&'a str, Option<&'a Entity>)> {
        // An explicit command topic routes whatever the domain: the worker subscribes to every one,
        // and a state-only entity (a sensor) may carry a command topic its discovery never advertises.
        let routed = self.entities.iter().find(|e| {
            e.topics
                .as_ref()
                .and_then(|t| t.command.as_deref())
                .is_some_and(|p| self.topic(p) == topic)
        });
        match routed {
            Some(e) => Some((e.id.as_str(), Some(e))),
            None => {
                let id = topic
                    .strip_prefix(&self.prefix)
                    .and_then(|t| t.strip_prefix('/'))
                    .and_then(|t| t.strip_suffix("/set"))
                    .filter(|id| super::identifier(id))?;
                let entity = self
                    .entities
                    .iter()
                    .find(|e| e.id == id && e.topics.as_ref().is_none_or(|t| t.command.is_none()));
                // A route is reachable only on its entities' own command topic, never also as
                // `<route>/set` (a second topic that could open a gate past broker ACLs).
                if entity.is_none()
                    && self
                        .entities
                        .iter()
                        .any(|e| matches!(e.control, Control::Route(r) if r == id))
                {
                    return None;
                }
                Some((id, entity))
            }
        }
    }

    /// The entity whose command topic `topic` is; its state answers the command.
    pub(crate) fn commanded_entity<'a>(&'a self, topic: &'a str) -> Option<&'a str> {
        self.command_target(topic)
            .and_then(|(_, e)| e)
            .map(|e| e.id.as_str())
    }
}

/// The value contract advertised in discovery: numbers in range (an integral `N.0` from a slider is
/// accepted), select options by index, switch on/off, the button's press payload.
pub(crate) fn slot_value(entity: &Entity, text: &str) -> Option<i32> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    match &entity.domain {
        Domain::Number { min, max, .. } => text
            .strip_suffix(".0")
            .unwrap_or(text)
            .parse::<i32>()
            .ok()
            .filter(|v| (min..=max).contains(&v)),
        Domain::Select { options } => i32::try_from(options.iter().position(|o| o == text)?).ok(),
        Domain::Switch => {
            if text == entity.payload(|p| p.on, "ON") {
                Some(1)
            } else if text == entity.payload(|p| p.off, "OFF") {
                Some(0)
            } else {
                None
            }
        }
        Domain::Button => (text == entity.payload(|p| p.press, "PRESS")).then_some(0),
        _ => None,
    }
}

/// A command on its way to the camera, with the epoch it was accepted under and when it arrived.
#[derive(Debug)]
pub struct Queued<C> {
    pub command: Command<C>,
    pub received: Instant,
    pub generation: u32,
}

const RELEASE: u32 = 1;
const STOP: u32 = 2;

/// The network thread publishes the new epoch before the urgent kind, and before it can queue later
/// commands. Priority is monotonic while pending: a Release never downgrades a pending Stop.
pub(crate) fn interrupt<C>(
    generation: &AtomicU32,
    urgent: &AtomicU32,
    command: &Command<C>,
) -> bool {
    let kind = match command {
        Command::Release => RELEASE,
        Command::Stop => STOP,
        _ => return false,
    };
    generation.fetch_add(1, Ordering::AcqRel);
    urgent.fetch_max(kind, Ordering::AcqRel);
    true
}

/// The camera's end of the command queue.
pub struct Commands<C> {
    pub(crate) rx: Receiver<Queued<C>>,
    pub(crate) generation: Arc<AtomicU32>,
    pub(crate) urgent: Arc<AtomicU32>,
    pub(crate) max_age: Option<Duration>,
    pub(crate) started: Instant,
}

impl<C> Commands<C> {
    /// The clock commands are stamped on.
    pub fn started(&self) -> Instant {
        self.started
    }
    /// The current command epoch.
    pub fn epoch(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }
    fn take_urgent(&self) -> Option<Command<C>> {
        match self.urgent.swap(0, Ordering::AcqRel) {
            0 => None,
            RELEASE => Some(Command::Release),
            _ => Some(Command::Stop),
        }
    }
    fn fresh(&self, item: &Queued<C>, now: Instant) -> bool {
        item.generation == self.epoch()
            && self
                .max_age
                .is_none_or(|age| now.saturating_duration_since(item.received) <= age)
    }
    /// A pending Stop/Release first; then the oldest queued command of the current epoch that is not
    /// older than the age limit.
    pub fn try_next(&self, now: Instant) -> Option<Command<C>> {
        self.take_urgent().or_else(|| {
            while let Ok(item) = self.rx.try_recv() {
                if self.fresh(&item, now) {
                    return Some(item.command);
                }
            }
            None
        })
    }
    /// As [`Self::try_next`], waiting up to `wait` and keeping the arrival stamp. `Disconnected`
    /// means the network side is gone.
    pub fn recv_timeout(&self, wait: Duration) -> Result<Queued<C>, RecvTimeoutError> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(command) = self.take_urgent() {
                return Ok(Queued {
                    command,
                    received: Instant::now(),
                    generation: self.epoch(),
                });
            }
            let item = self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            if self.fresh(&item, Instant::now()) {
                return Ok(item);
            }
        }
    }
    /// Commands that already arrived, without waiting (the hardware loop batches them).
    pub fn try_recv(&self) -> Option<Queued<C>> {
        while let Ok(item) = self.rx.try_recv() {
            if self.fresh(&item, Instant::now()) {
                return Some(item);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config;
    use super::super::*;
    use super::*;
    use std::sync::mpsc::sync_channel;

    struct Echo;
    impl Parser for Echo {
        type App = String;
        fn parse(&self, id: &str, text: &str) -> Result<Option<Command<String>>, String> {
            Ok(Some(Command::App(format!("{id}={text}"))))
        }
    }
    fn catalog() -> Config {
        config(vec![
            Entity::new("level", "Level", Domain::number(-3, 4, Mode::Slider)).slot(17),
            Entity::new("mode", "Mode", Domain::select(["first", "second"])).slot(42),
            Entity::new("toggle", "Toggle", Domain::Switch).slot(u16::MAX),
            Entity::new("apply", "Apply", Domain::Button).slot(110),
            Entity::new("gate", "Gate", Domain::Cover)
                .command_topic("door/command")
                .route("door"),
            Entity::new("open", "Open", Domain::Button)
                .command_topic("door/command")
                .route("door"),
            Entity::new("call", "Call", Domain::Sensor)
                .command_topic("call/command")
                .route("call"),
        ])
    }
    fn send(cfg: &Config, topic: &str, payload: &[u8]) -> Result<Option<Command<String>>, String> {
        cfg.route(&Echo, topic, payload, false)
            .map_err(|r| r.reason)
    }

    #[test]
    fn retained_commands_are_never_executed() {
        let cfg = catalog();
        for (topic, payload) in [
            ("camera/test/apply/set", "PRESS"),
            ("camera/test/door/command", "open"),
            ("camera/test/anything/set", "x"),
        ] {
            assert!(send(&cfg, topic, payload.as_bytes()).unwrap().is_some());
            assert_eq!(
                cfg.route(&Echo, topic, payload.as_bytes(), true),
                Ok(None),
                "retained {topic}"
            );
        }
    }

    #[test]
    fn slots_obey_the_advertised_number_select_switch_and_button_contracts() {
        let cfg = catalog();
        for (id, payload, slot, value) in [
            ("level", "-3", 17, -3),
            ("level", "4", 17, 4),
            ("level", "3.0", 17, 3),
            ("mode", "first", 42, 0),
            ("mode", "second", 42, 1),
            ("toggle", "ON", u16::MAX, 1),
            ("toggle", "OFF", u16::MAX, 0),
            ("apply", "PRESS", 110, 0),
        ] {
            assert_eq!(
                send(&cfg, &format!("camera/test/{id}/set"), payload.as_bytes()),
                Ok(Some(Command::Setting { slot, value })),
                "{id} {payload}"
            );
        }
        for (id, payload) in [
            ("level", "-4"),
            ("level", "5"),
            ("level", "1.5"),
            ("level", "NaN"),
            ("level", "2147483648"),
            ("level", "1\0"),
            ("mode", "third"),
            ("mode", "1"),
            ("toggle", "true"),
            ("apply", "press"),
            ("apply", ""),
        ] {
            assert!(
                send(&cfg, &format!("camera/test/{id}/set"), payload.as_bytes()).is_err(),
                "{id} {payload}"
            );
        }
    }

    #[test]
    fn routes_scope_and_size_reach_the_camera_parser() {
        let cfg = catalog();
        assert_eq!(
            send(&cfg, "camera/test/door/command", b"open"),
            Ok(Some(Command::App("door=open".into())))
        );
        assert_eq!(
            send(&cfg, "camera/test/move/set", b"left"),
            Ok(Some(Command::App("move=left".into())))
        );
        for topic in [
            "camera/other/move/set",
            "camera/test/move",
            "camera/test/a/b/set",
            "camera/testx/move/set",
        ] {
            assert_eq!(send(&cfg, topic, b"1"), Ok(None), "{topic}");
        }
        assert!(send(&cfg, "camera/test/move/set", &[255]).is_err());
        assert!(send(&cfg, "camera/test/move/set", &[b' '; 257]).is_err());
        assert!(send(&cfg, "camera/test/move/set", &[b' '; 256]).is_ok());
        // A sensor's own command topic reaches its route, and only that topic does.
        assert_eq!(
            send(&cfg, "camera/test/call/command", b"{}"),
            Ok(Some(Command::App("call={}".into())))
        );
        assert_eq!(send(&cfg, "camera/test/call/set", b"{}"), Ok(None));
        // An entity with its own command topic is not also reachable under <id>/set as a slot.
        assert_eq!(
            send(&cfg, "camera/test/gate/set", b"x"),
            Ok(Some(Command::App("gate=x".into())))
        );
    }

    fn queue(age: Option<Duration>) -> (std::sync::mpsc::SyncSender<Queued<u8>>, Commands<u8>) {
        let (tx, rx) = sync_channel(8);
        let commands = Commands {
            rx,
            generation: Arc::new(AtomicU32::new(0)),
            urgent: Arc::new(AtomicU32::new(0)),
            max_age: age,
            started: Instant::now(),
        };
        (tx, commands)
    }
    fn item(command: Command<u8>, received: Instant, generation: u32) -> Queued<u8> {
        Queued {
            command,
            received,
            generation,
        }
    }

    #[test]
    fn stale_commands_are_dropped_only_when_an_age_limit_is_set() {
        let now = Instant::now();
        let (tx, c) = queue(Some(Duration::from_secs(2)));
        tx.send(item(Command::App(1), now, 0)).unwrap();
        assert_eq!(c.try_next(now + Duration::from_secs(3)), None);
        let (tx, c) = queue(None);
        tx.send(item(Command::App(1), now, 0)).unwrap();
        assert_eq!(
            c.try_next(now + Duration::from_secs(30)),
            Some(Command::App(1))
        );
    }

    #[test]
    fn release_bypasses_a_full_queue_and_discards_older_commands() {
        let now = Instant::now();
        let (tx, c) = queue(Some(Duration::from_secs(2)));
        for _ in 0..8 {
            tx.try_send(item(Command::App(1), now, 0)).unwrap();
        }
        assert!(interrupt(&c.generation, &c.urgent, &Command::<u8>::Release));
        assert_eq!(c.try_next(now), Some(Command::Release));
        assert_eq!(c.try_next(now), None, "older commands discarded");
        tx.try_send(item(Command::App(2), now, c.epoch())).unwrap();
        assert_eq!(c.try_next(now), Some(Command::App(2)));
        assert!(!interrupt(&c.generation, &c.urgent, &Command::App(3)));
    }

    #[test]
    fn stop_dominates_release_in_either_order() {
        for order in [
            [Command::<u8>::Release, Command::Stop],
            [Command::Stop, Command::Release],
        ] {
            let now = Instant::now();
            let (tx, c) = queue(None);
            for command in order {
                assert!(interrupt(&c.generation, &c.urgent, &command));
                tx.try_send(item(Command::App(1), now, c.epoch())).unwrap();
            }
            assert_eq!(c.try_next(now), Some(Command::Stop));
            assert_eq!(
                c.try_next(now),
                Some(Command::App(1)),
                "only the latest epoch survives"
            );
            assert_eq!(c.try_next(now), None);
            assert_eq!(c.epoch(), 2);
        }
    }

    #[test]
    fn blocking_receive_keeps_stamps_and_reports_disconnection() {
        let (tx, c) = queue(None);
        let at = Instant::now();
        tx.send(item(Command::App(5), at, 0)).unwrap();
        let got = c.recv_timeout(Duration::from_millis(10)).unwrap();
        assert_eq!(
            (got.command, got.received, got.generation),
            (Command::App(5), at, 0)
        );
        assert_eq!(
            c.recv_timeout(Duration::from_millis(10)).unwrap_err(),
            RecvTimeoutError::Timeout
        );
        drop(tx);
        assert_eq!(
            c.recv_timeout(Duration::from_millis(10)).unwrap_err(),
            RecvTimeoutError::Disconnected
        );
    }
}
