//! The optional hardware thread: the camera's device behind [`Hardware`], fed from the command queue
//! so a slow motor never blocks MQTT polling.
use super::{Command, Commands, Publisher, Queued};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// The device boundary the hardware thread owns.
pub trait Hardware<C>: Send {
    /// A terminal fault: the loop exits at once, without [`Hardware::shutdown`].
    fn failed(&self) -> bool {
        false
    }
    /// `received` is the arrival time on the loop's clock, the same clock `tick` gets.
    fn apply_at(&mut self, command: &Command<C>, received: Duration) -> Result<(), String>;
    /// Periodic work; returns how soon it wants to run again (`None`: the idle cadence).
    fn tick(&mut self, now: Duration) -> Result<Option<Duration>, String>;
    fn states(&mut self) -> Result<BTreeMap<String, String>, String>;
    /// True once after something changed asynchronously, so states are published now.
    fn changed(&mut self) -> bool {
        false
    }
    /// Orderly end (save what is unsaved).
    fn shutdown(&mut self) {}
    /// Commands that a newer one of the same kind makes pointless (an absolute tap on the picture).
    fn supersedable(&self, command: &Command<C>) -> bool {
        let _ = command;
        false
    }
}

/// Drops every supersedable command that a later one follows with nothing but supersedable commands
/// in between; other commands keep their order and are never dropped. Returns how many were dropped.
pub fn supersede<C>(
    queue: &mut VecDeque<Queued<C>>,
    supersedable: impl Fn(&Command<C>) -> bool,
) -> usize {
    let mut keep = Vec::with_capacity(queue.len());
    let mut pending: Option<Queued<C>> = None;
    let mut dropped = 0;
    for q in queue.drain(..) {
        if supersedable(&q.command) {
            if pending.replace(q).is_some() {
                dropped += 1;
            }
        } else {
            keep.extend(pending.take());
            keep.push(q);
        }
    }
    keep.extend(pending);
    queue.extend(keep);
    dropped
}

/// Cadences and hooks of the hardware thread.
pub struct HardwareLoop<'a> {
    /// Longest wait between ticks.
    pub idle: Duration,
    /// States are read back at least this often while connected; the publisher sends only the
    /// values that changed (a reconnect or HA birth replays the rest).
    pub refresh: Duration,
    /// Called once per pass (watchdog progress).
    pub beat: Option<&'a dyn Fn()>,
    pub log: Option<&'a dyn Fn(&str)>,
}

impl Default for HardwareLoop<'_> {
    fn default() -> Self {
        Self {
            idle: Duration::from_millis(500),
            refresh: Duration::from_secs(5),
            beat: None,
            log: None,
        }
    }
}

impl HardwareLoop<'_> {
    fn log(&self, message: std::fmt::Arguments<'_>) {
        match self.log {
            Some(log) => log(&message.to_string()),
            None => eprintln!("mqtt: {message}"),
        }
    }
    fn tick<C, H: Hardware<C>>(&self, hw: &mut H, started: Instant, wait: &mut Duration) {
        match hw.tick(started.elapsed()) {
            Ok(w) => *wait = w.unwrap_or(self.idle).min(self.idle),
            Err(e) => {
                *wait = self.idle;
                self.log(format_args!("hardware tick failed: {e}"));
            }
        }
    }

    /// Runs until `stop`, a closed queue or a terminal fault. Each command waits for a tick (which
    /// services stop requests first) and is skipped when a stop advanced the epoch meanwhile.
    pub fn run<C, H: Hardware<C>>(
        &self,
        hw: &mut H,
        commands: &Commands<C>,
        publisher: &Publisher,
        stop: &AtomicBool,
    ) {
        let started = commands.started();
        let beat = || {
            if let Some(beat) = self.beat {
                beat();
            }
        };
        beat();
        let mut next = Instant::now();
        let mut wait = self.idle;
        let mut queue: VecDeque<Queued<C>> = VecDeque::new();
        while !stop.load(Ordering::Relaxed) {
            let next_command = match queue.pop_front() {
                Some(q) => Ok(q),
                None => commands.recv_timeout(wait),
            };
            let command = match next_command {
                Ok(q) => {
                    queue.push_front(q);
                    while let Some(more) = commands.try_recv() {
                        queue.push_back(more);
                    }
                    let dropped = supersede(&mut queue, |c| hw.supersedable(c));
                    if dropped > 0 {
                        self.log(format_args!("superseded ({dropped})"));
                    }
                    let Some(q) = queue.pop_front() else {
                        continue;
                    };
                    if hw.failed() {
                        return;
                    }
                    self.tick(hw, started, &mut wait);
                    if hw.failed() {
                        return;
                    }
                    if q.generation != commands.epoch() {
                        continue;
                    }
                    if let Err(e) =
                        hw.apply_at(&q.command, q.received.saturating_duration_since(started))
                    {
                        self.log(format_args!("hardware command failed: {e}"));
                    }
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if hw.failed() {
                return;
            }
            self.tick(hw, started, &mut wait);
            if hw.failed() {
                return;
            }
            if publisher.is_connected()
                && (command || hw.changed() || publisher.take_refresh() || Instant::now() >= next)
            {
                match hw.states() {
                    Ok(states) => {
                        for (id, value) in states {
                            if let Err(e) = publisher.state(&id, value) {
                                self.log(format_args!("{e}"));
                            }
                        }
                    }
                    Err(e) => self.log(format_args!("state read failed: {e}")),
                }
                next = Instant::now() + self.refresh;
            }
            beat();
        }
        hw.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(command: Command<u8>, at: u64) -> Queued<u8> {
        Queued {
            command,
            received: Instant::now() + Duration::from_millis(at),
            generation: 0,
        }
    }
    fn tap(c: &Command<u8>) -> bool {
        *c == Command::App(0)
    }
    fn shape(queue: &VecDeque<Queued<u8>>) -> Vec<Command<u8>> {
        queue.iter().map(|q| q.command).collect()
    }

    #[test]
    fn consecutive_taps_keep_only_the_newest() {
        let mut queue: VecDeque<_> = [
            q(Command::App(0), 1),
            q(Command::App(0), 2),
            q(Command::App(0), 3),
        ]
        .into();
        let newest = queue[2].received;
        assert_eq!(supersede(&mut queue, tap), 2);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].received, newest);
    }

    #[test]
    fn other_commands_are_barriers_and_keep_their_order() {
        let mut queue: VecDeque<_> = [q(Command::App(0), 1), q(Command::Stop, 2)].into();
        assert_eq!(supersede(&mut queue, tap), 0);
        assert_eq!(shape(&queue), [Command::App(0), Command::Stop]);
        let mut queue: VecDeque<_> = [
            q(Command::App(0), 1),
            q(Command::App(5), 2),
            q(Command::App(0), 3),
            q(Command::App(0), 4),
        ]
        .into();
        assert_eq!(supersede(&mut queue, tap), 1);
        assert_eq!(
            shape(&queue),
            [Command::App(0), Command::App(5), Command::App(0)]
        );
    }
}
