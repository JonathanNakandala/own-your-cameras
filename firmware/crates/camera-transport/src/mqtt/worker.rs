//! The network thread: one blocking MQTT connection with a clean session, bounded queues, discovery
//! and state replay on every connection and Home Assistant birth.
use super::client::{self, Client, Polled};
use super::command::interrupt;
use super::publisher::{Message, Publisher};
use super::{Command, Commands, Config, Domain, Parser, Publication, Queued, Urgent};
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

/// Poll interval while connected: how long a published state or event may wait for the network.
const POLL: Duration = Duration::from_millis(100);
/// How long one connection attempt (TCP connect and CONNACK) may take. With the 1 s retry delay a
/// failed attempt still reports progress within 3 s, the cameras' freshness bound for this thread.
const CONNECTING: Duration = Duration::from_millis(1500);
/// Longest time one pass spends writing publications; the rest wait for the next pass.
const PASS: Duration = Duration::from_millis(500);
/// How long the broker gets to deliver a retained device-discovery document before the migration
/// concludes there is none.
const MIGRATION_WAIT: Duration = Duration::from_secs(1);
/// After clearing an earlier firmware's separate configs, before a new config may land on one of
/// their topics: Home Assistant removes a discovered entity asynchronously.
const SUPERSEDED_WAIT: Duration = Duration::from_secs(2);

/// Decides what a Stop or Release does.
pub type UrgentHook<'a, C> = &'a dyn Fn(&Command<C>) -> Urgent;

/// Camera hooks into the network loop.
pub struct Hooks<'a, C> {
    /// After every loop iteration (watchdog progress).
    pub on_progress: Option<&'a dyn Fn()>,
    /// Decides what a Stop or Release does; the default interrupts the queue.
    pub on_urgent: Option<UrgentHook<'a, C>>,
    /// Log lines (rate limited); the default writes `mqtt: <line>` to stderr.
    pub log: Option<&'a dyn Fn(&str)>,
}

impl<C> Default for Hooks<'_, C> {
    fn default() -> Self {
        Self {
            on_progress: None,
            on_urgent: None,
            log: None,
        }
    }
}

struct LogBudget<'a> {
    left: AtomicUsize,
    sink: Option<&'a dyn Fn(&str)>,
}
impl<'a> LogBudget<'a> {
    fn new(sink: Option<&'a dyn Fn(&str)>) -> Self {
        Self {
            left: AtomicUsize::new(64),
            sink,
        }
    }

    fn line(&self, message: std::fmt::Arguments<'_>) {
        let left = self.left.load(Ordering::Relaxed);
        if left == 0 {
            return;
        }
        self.left.store(left - 1, Ordering::Relaxed);
        let text = if left == 1 {
            "further events suppressed".to_owned()
        } else {
            message.to_string()
        };
        match self.sink {
            Some(sink) => sink(&text),
            None => eprintln!("mqtt: {text}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Migration {
    /// Not yet checked on this broker connection.
    Pending,
    /// Waiting for the subscription acknowledgments; the instant is a fallback deadline.
    Subscribing(usize, Instant),
    /// Subscribed; a retained device document arrives before the deadline or there is none.
    Waiting(Instant),
    /// The marker and the earlier firmware's separate configs are in the backlog; once handed over
    /// in full (the instant is set then), and [`SUPERSEDED_WAIT`] later, the rest follows.
    Clearing(Option<Instant>),
    /// The migration sequence is in the backlog; done once it is handed to the client in full.
    Sending,
    Done,
}

pub struct Worker<P: Parser> {
    config: Config,
    parser: P,
    publisher: Publisher,
    client: Client,
    commands: SyncSender<Queued<P::App>>,
    generation: Arc<AtomicU32>,
    urgent: Arc<AtomicU32>,
    backlog: VecDeque<Publication>,
    /// Runtime removals the current backlog took from the outbox; any not yet written go back to
    /// the outbox when the backlog is abandoned, so a lost connection cannot lose them.
    removals: Vec<Publication>,
    /// Publications written in the current pass, bounded by `Limits::outbound`.
    sent: usize,
    /// Start of the current pass; a slow broker cannot hold one pass past [`PASS`].
    pass: Instant,
}

/// Validates the catalog and builds the camera's three handles: publisher, command queue, worker.
pub fn channel<P: Parser>(
    config: Config,
    parser: P,
) -> io::Result<(Publisher, Commands<P::App>, Worker<P>)> {
    channel_with_epoch(config, parser, Arc::default())
}

/// As [`channel`], with a command epoch the camera shares (a stop owner outside the network thread
/// that invalidates queued commands by advancing it).
pub fn channel_with_epoch<P: Parser>(
    config: Config,
    parser: P,
    epoch: Arc<AtomicU32>,
) -> io::Result<(Publisher, Commands<P::App>, Worker<P>)> {
    config.validate()?;
    let client = Client::new(client::Options {
        host: config.broker.host.clone(),
        port: config.broker.port,
        client_id: config.node_id.clone(),
        username: config.broker.username.clone(),
        password: config.broker.password.clone(),
        keep_alive: Duration::from_secs(5),
        will_topic: config.availability(),
        will_payload: b"offline".to_vec(),
        max_packet: config.limits.max_packet,
    });
    let (tx, rx) = sync_channel(config.limits.commands);
    let urgent = Arc::new(AtomicU32::new(0));
    let publisher = Publisher::new(&config);
    let commands = Commands {
        rx,
        generation: epoch.clone(),
        urgent: urgent.clone(),
        max_age: config.limits.command_age,
        started: Instant::now(),
    };
    Ok((
        publisher.clone(),
        commands,
        Worker {
            config,
            parser,
            publisher,
            client,
            commands: tx,
            sent: 0,
            pass: Instant::now(),
            generation: epoch,
            urgent,
            backlog: VecDeque::new(),
            removals: Vec::new(),
        },
    ))
}

/// Another producer of the camera's command queue (a second protocol beside MQTT, such as ONVIF): its
/// commands are stamped on the same clock and epoch as MQTT's, so a stop discards them alike.
pub struct Submitter<C> {
    commands: SyncSender<Queued<C>>,
    generation: Arc<AtomicU32>,
}

impl<C> Clone for Submitter<C> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            generation: self.generation.clone(),
        }
    }
}

impl<C> Submitter<C> {
    /// Queues `command` in order with MQTT's commands; Stop and Release get no urgent path here (the
    /// caller owns that). Fails when the queue is full or the camera's end is gone.
    pub fn submit(&self, command: Command<C>) -> Result<(), TrySendError<Command<C>>> {
        self.commands
            .try_send(Queued {
                command,
                received: Instant::now(),
                generation: self.generation.load(Ordering::Acquire),
            })
            .map_err(|e| match e {
                TrySendError::Full(q) => TrySendError::Full(q.command),
                TrySendError::Disconnected(q) => TrySendError::Disconnected(q.command),
            })
    }
}

impl<P: Parser> Worker<P> {
    /// A second producer of the command queue this worker feeds.
    pub fn submitter(&self) -> Submitter<P::App> {
        Submitter {
            commands: self.commands.clone(),
            generation: self.generation.clone(),
        }
    }

    fn cancel(&self) {
        if self.config.stop_on_disconnect {
            interrupt(&self.generation, &self.urgent, &Command::<P::App>::Stop);
        }
    }

    /// Drops the backlog; runtime removals still in it return to the outbox for the next
    /// announcement.
    fn abandon_backlog(&mut self) {
        let unsent: Vec<Message> = std::mem::take(&mut self.removals)
            .into_iter()
            .filter(|r| self.backlog.contains(r))
            .map(Message::Remove)
            .collect();
        self.backlog.clear();
        if !unsent.is_empty() {
            self.give_back(Vec::new(), unsent);
        }
    }

    /// Removals first (retired ids, then any still queued), so Home Assistant has released their
    /// entity ids before a new entity with the same name arrives; then discovery configs, retained
    /// states and availability. The caller replaces the backlog with it.
    fn announcement(&mut self) -> io::Result<Vec<Publication>> {
        self.abandon_backlog();
        let configs = self.config.discovery()?;
        let mut out = self.config.retirement();
        let mut outbox = self
            .publisher
            .shared
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let removals = outbox.take_removals();
        out.extend(removals.iter().cloned());
        out.extend(configs);
        out.extend(outbox.replay());
        drop(outbox);
        self.removals = removals;
        out.push(Publication::retained(
            self.config.availability(),
            b"online".to_vec(),
        ));
        Ok(out)
    }

    fn try_send(&mut self, p: &Publication) -> bool {
        if self.sent >= self.config.limits.outbound || self.pass.elapsed() > PASS {
            return false;
        }
        self.sent += 1;
        self.client.publish(&p.topic, &p.payload, p.retain)
    }

    /// Backlog first, then changed states and queued messages; whatever the client cannot take now
    /// (at most `Limits::outbound` per pass, nothing while disconnected) waits for the next pass.
    fn flush(&mut self, now: Instant) {
        self.sent = 0;
        self.pass = Instant::now();
        while let Some(p) = self.backlog.pop_front() {
            if !self.try_send(&p) {
                self.backlog.push_front(p);
                return;
            }
        }
        self.removals.clear();
        let (states, messages) = self
            .publisher
            .shared
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take(now);
        let mut states = states.into_iter();
        let mut messages = messages.into_iter();
        let mut blocked = false;
        for p in states.by_ref() {
            if !self.try_send(&p) {
                blocked = true;
                self.give_back(
                    std::iter::once(p).chain(states.by_ref()).collect(),
                    messages.by_ref().collect(),
                );
                break;
            }
        }
        if blocked {
            return;
        }
        while let Some(m) = messages.next() {
            match m {
                Message::Publish {
                    publication,
                    expires,
                } => {
                    if !self.try_send(&publication) {
                        let rest = std::iter::once(Message::Publish {
                            publication,
                            expires,
                        })
                        .chain(messages.by_ref())
                        .collect();
                        self.give_back(Vec::new(), rest);
                        return;
                    }
                }
                Message::Remove(publication) => {
                    if !self.try_send(&publication) {
                        let rest = std::iter::once(Message::Remove(publication))
                            .chain(messages.by_ref())
                            .collect();
                        self.give_back(Vec::new(), rest);
                        return;
                    }
                }
                Message::Options { id, options } => self.set_options(&id, options),
            }
        }
    }

    fn give_back(&self, states: Vec<Publication>, messages: Vec<Message>) {
        self.publisher
            .shared
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unsent(states, messages);
    }

    fn set_options(&mut self, id: &str, options: Vec<String>) {
        let Some(entity) = self.config.entities.iter_mut().find(|e| e.id == id) else {
            return;
        };
        let Domain::Select { options: current } = &mut entity.domain else {
            return;
        };
        let previous = std::mem::replace(current, options);
        match self.config.discovery() {
            Ok(all) => {
                if let Some(p) = all
                    .into_iter()
                    .find(|p| p.topic.ends_with(&format!("/{id}/config")))
                {
                    self.backlog.push_back(p);
                }
            }
            Err(_) => {
                // Invalid options: keep advertising the previous ones.
                if let Some(Domain::Select { options }) = self
                    .config
                    .entities
                    .iter_mut()
                    .find(|e| e.id == id)
                    .map(|e| &mut e.domain)
                {
                    *options = previous;
                }
            }
        }
    }

    /// False when the command was refused (full queue).
    fn dispatch(
        &self,
        command: Command<P::App>,
        hooks: &Hooks<'_, P::App>,
        log: &LogBudget<'_>,
    ) -> bool {
        let decision = match command {
            Command::Stop | Command::Release => {
                hooks.on_urgent.map_or(Urgent::Interrupt, |f| f(&command))
            }
            _ => Urgent::Queue,
        };
        match decision {
            Urgent::Handled => {}
            Urgent::Interrupt => {
                interrupt(&self.generation, &self.urgent, &command);
            }
            Urgent::Queue => {
                let queued = Queued {
                    command,
                    received: Instant::now(),
                    generation: self.generation.load(Ordering::Acquire),
                };
                if self.commands.try_send(queued).is_err() {
                    log.line(format_args!("command queue full; command rejected"));
                    return false;
                }
            }
        }
        true
    }

    /// Runs until `stop`, then publishes `offline`, disconnects and closes once the broker has read
    /// the earlier publications. Returns `Err`, after the same close, when the broker resumes a
    /// session or no announcement can be built; the caller decides what that means (Reolink and
    /// Mi 360 stop the camera). [`Worker::run_restarting`] reconnects instead.
    pub fn run(mut self, stop: &AtomicBool, hooks: Hooks<'_, P::App>) -> io::Result<()> {
        let log = LogBudget::new(hooks.log);
        self.session(stop, &hooks, &log)
    }

    /// As [`Worker::run`], but an error ends only the session: the connection is closed, and after
    /// `backoff` (cut short by `stop`) the worker connects again. The publisher and command queue
    /// handed out by [`channel`] stay valid throughout. Returns once `stop` is set.
    pub fn run_restarting(
        mut self,
        stop: &AtomicBool,
        hooks: Hooks<'_, P::App>,
        backoff: Duration,
    ) {
        let log = LogBudget::new(hooks.log);
        while let Err(e) = self.session(stop, &hooks, &log) {
            log.line(format_args!(
                "session ended ({e}); reconnecting in {} s",
                backoff.as_secs()
            ));
            let until = Instant::now() + backoff;
            while !stop.load(Ordering::Relaxed) {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                if let Some(progress) = hooks.on_progress {
                    progress();
                }
                std::thread::sleep(left.min(Duration::from_millis(100)));
            }
        }
    }

    /// One run of the network loop, from the first connection attempt to `stop` or an error; the
    /// client is closed either way.
    fn session(
        &mut self,
        stop: &AtomicBool,
        hooks: &Hooks<'_, P::App>,
        log: &LogBudget<'_>,
    ) -> io::Result<()> {
        let device_topic = self.config.device_discovery_topic();
        let status_topic = format!("{}/status", self.config.discovery_prefix);
        let mut migration = if self.config.migrate_from_device_discovery {
            Migration::Pending
        } else {
            Migration::Done
        };
        let mut connected = false;
        let mut announce = false;
        let result = (|| -> io::Result<()> {
            while !stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                match migration {
                    Migration::Subscribing(_, deadline) if now >= deadline => {
                        migration = Migration::Waiting(now);
                    }
                    Migration::Waiting(deadline) if now >= deadline => {
                        migration = Migration::Done;
                        announce = true;
                        let _ = self.client.unsubscribe(&device_topic);
                    }
                    Migration::Clearing(Some(deadline)) if connected && now >= deadline => {
                        // The marker is retained now: the sequence goes on without a second one.
                        let announcement = self.announcement()?;
                        self.backlog = super::migrate_from_device_discovery(
                            &device_topic,
                            super::MIGRATE_MARKER,
                            Vec::new(),
                            announcement,
                        )
                        .into();
                        self.publisher.request_refresh();
                        migration = Migration::Sending;
                    }
                    _ => {}
                }
                let checking = matches!(
                    migration,
                    Migration::Subscribing(..) | Migration::Waiting(_)
                );
                if connected && !checking {
                    // An announcement never replaces a migration sequence still being sent.
                    let migrating =
                        matches!(migration, Migration::Sending | Migration::Clearing(_));
                    if announce && !migrating {
                        announce = false;
                        self.backlog = self.announcement()?.into();
                        self.publisher.request_refresh();
                    }
                    self.flush(now);
                    if migration == Migration::Sending && self.backlog.is_empty() {
                        migration = Migration::Done;
                    }
                    if migration == Migration::Clearing(None) && self.backlog.is_empty() {
                        migration = Migration::Clearing(Some(Instant::now() + SUPERSEDED_WAIT));
                    }
                }
                let timeout = if connected { POLL } else { CONNECTING };
                match self.client.poll(timeout) {
                    Polled::ConnAck { session_present } => {
                        // CONNECT always requests a clean session. Accepting a resumed one could
                        // replay pending requests across repeated broken connections, so the
                        // session ends here: `run` returns the error, `run_restarting` closes the
                        // connection and connects again after its back-off.
                        if session_present {
                            return Err(io::Error::other("broker resumed a clean MQTT session"));
                        }
                        self.cancel();
                        connected = true;
                        self.publisher.shared.online.store(true, Ordering::Release);
                        let mut filters = vec![self.config.topic("+/set"), status_topic.clone()];
                        for e in &self.config.entities {
                            if let Some(path) = e.topics.as_ref().and_then(|t| t.command.as_ref()) {
                                let topic = self.config.topic(path);
                                if !filters.contains(&topic) {
                                    filters.push(topic);
                                }
                            }
                        }
                        let subscriptions = filters.len() + 1;
                        for filter in filters {
                            let _ = self.client.subscribe(&filter);
                        }
                        if migration == Migration::Done {
                            announce = true;
                        } else {
                            // The device subscription goes last, so its retained document can only
                            // arrive after the last acknowledgment; the wait starts there.
                            let _ = self.client.subscribe(&device_topic);
                            migration = Migration::Subscribing(
                                subscriptions,
                                Instant::now() + 10 * MIGRATION_WAIT,
                            );
                        }
                        log.line(format_args!("connected"));
                    }
                    Polled::SubAck { refused } => {
                        if refused {
                            log.line(format_args!("broker refused a subscription (0x80)"));
                        }
                        if let Migration::Subscribing(left, deadline) = migration {
                            migration = if left <= 1 {
                                Migration::Waiting(Instant::now() + MIGRATION_WAIT)
                            } else {
                                Migration::Subscribing(left - 1, deadline)
                            };
                        }
                    }
                    Polled::Oversized(len) => log.line(format_args!(
                        "discarded an incoming packet of {len} bytes (limit {})",
                        self.config.limits.max_packet
                    )),
                    Polled::Publish {
                        topic,
                        payload,
                        retain,
                    } => {
                        if topic == device_topic {
                            if let Migration::Subscribing(..) | Migration::Waiting(_) = migration {
                                let superseded = self.config.supersession();
                                if !payload.is_empty() {
                                    log.line(format_args!(
                                        "moving device discovery to per-entity discovery"
                                    ));
                                }
                                if payload.is_empty() || superseded.is_empty() {
                                    let announcement = self.announcement()?;
                                    self.backlog = super::migrate_from_device_discovery(
                                        &device_topic,
                                        &payload,
                                        Vec::new(),
                                        announcement,
                                    )
                                    .into();
                                    self.publisher.request_refresh();
                                    migration = Migration::Sending;
                                } else {
                                    // Marker and clears first; the announcement waits for Home
                                    // Assistant to remove the earlier entities.
                                    self.abandon_backlog();
                                    self.backlog = super::migrate_from_device_discovery(
                                        &device_topic,
                                        &payload,
                                        superseded,
                                        Vec::new(),
                                    )
                                    .into();
                                    // Without an announcement the sequence ends with the device
                                    // topic's clear: that goes out with the rest, later.
                                    self.backlog.pop_back();
                                    migration = Migration::Clearing(None);
                                }
                                announce = false;
                                let _ = self.client.unsubscribe(&device_topic);
                            }
                        } else if topic == status_topic {
                            if payload == b"online" {
                                announce = true;
                            }
                        } else {
                            match self.config.route(&self.parser, &topic, &payload, retain) {
                                Ok(Some(command)) => {
                                    let entity = self.config.commanded_entity(&topic);
                                    if let Some(id) = entity {
                                        self.publisher.commanded(id);
                                    }
                                    if !self.dispatch(command, hooks, log)
                                        && let Some(id) = entity
                                    {
                                        self.publisher.refused(id);
                                    }
                                }
                                Ok(None) => {}
                                Err(rejected) => {
                                    log.line(format_args!("command rejected: {}", rejected.reason));
                                    if let Some(id) = rejected.entity {
                                        self.publisher.refused(&id);
                                    }
                                }
                            }
                        }
                    }
                    Polled::Lost(reason) => {
                        if connected {
                            self.publisher.shared.drops.fetch_add(1, Ordering::Relaxed);
                        }
                        connected = false;
                        self.publisher.shared.online.store(false, Ordering::Release);
                        // No resumable session: discovery and states are regenerated on CONNECT;
                        // unsent runtime removals wait in the outbox for that announcement.
                        self.abandon_backlog();
                        self.client.reset();
                        self.cancel();
                        // An unfinished migration is checked again on the next connection.
                        if migration != Migration::Done {
                            migration = Migration::Pending;
                        }
                        log.line(format_args!("broker unavailable ({reason}); reconnecting"));
                        // Interruptible retry delay; no tight loop when the broker refuses credentials.
                        for _ in 0..10 {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                    Polled::Idle => {}
                }
                if let Some(progress) = hooks.on_progress {
                    progress();
                }
            }
            Ok(())
        })();
        self.publisher.shared.online.store(false, Ordering::Release);
        self.cancel();
        self.abandon_backlog();
        // Publishes `offline`, disconnects and closes once the broker has read everything.
        self.client.close(&self.config.availability(), b"offline");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config;
    use super::super::{Entity, Parser};
    use super::*;

    struct Ignore;
    impl Parser for Ignore {
        type App = ();
        fn parse(&self, _: &str, _: &str) -> Result<Option<Command<()>>, String> {
            Ok(None)
        }
    }

    fn cleared(announcement: &[Publication]) -> Vec<String> {
        announcement
            .iter()
            .take_while(|p| p.payload.is_empty())
            .map(|p| p.topic.clone())
            .collect()
    }

    #[test]
    fn runtime_removals_not_yet_written_survive_a_lost_connection() {
        let entities = vec![Entity::new("reed", "Door", Domain::BinarySensor).state_topic("reed")];
        let (publisher, _commands, mut worker) = channel(config(entities), Ignore).unwrap();
        publisher.remove(&Domain::Sensor, "old_a").unwrap();
        publisher.remove(&Domain::Sensor, "old_b").unwrap();
        let first = worker.announcement().unwrap();
        assert_eq!(cleared(&first).len(), 4);
        worker.backlog = first.into();
        // One removal reached the socket, then the connection was lost.
        let written = worker.backlog.pop_front().unwrap();
        worker.abandon_backlog();
        let again = worker.announcement().unwrap();
        let removals = cleared(&again);
        assert_eq!(removals.len(), 3, "{removals:?}");
        assert!(!removals.contains(&written.topic));
        // Written in full: nothing comes back.
        worker.backlog = again.into();
        worker.backlog.clear();
        worker.removals.clear();
        worker.abandon_backlog();
        assert!(cleared(&worker.announcement().unwrap()).is_empty());
    }
}
