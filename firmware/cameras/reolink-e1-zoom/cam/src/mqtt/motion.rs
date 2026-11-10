//! The hardware thread's command queue, shared: MQTT and ONVIF submit the same [`Command`]s to the one
//! owner of pan, tilt, zoom and the preset table, on the same clock and motion epoch (the shared MQTT
//! layer's queue, through its [`transport::Submitter`]).

use super::Command;
use camera_transport::mqtt as transport;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::TrySendError;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A submitted move counts as under way for this long even before the hardware thread has picked it up.
/// The thread takes a command within one loop pass, so this only has to cover that latency; a command a
/// stop discarded from the queue then cannot leave `MoveStatus` stuck on `MOVING`.
const QUEUE_GRACE: Duration = Duration::from_millis(250);

fn since_start() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

#[derive(Default)]
pub struct Motion {
    /// Set when the MQTT layer starts; `None` until then (and when MQTT is not configured).
    owner: Mutex<Option<Owner>>,
    /// The preset names in force, as last read from the hardware thread.
    pub(super) presets: Mutex<Vec<String>>,
    /// The hardware thread is applying a move, or a hold is running.
    busy: AtomicBool,
    /// Until when (`since_start` ms) a submitted move counts as under way; 0 for none.
    queued_until: AtomicU64,
    /// The pose after the latest move, for ONVIF `GetStatus`.
    pose: Mutex<Option<super::Pose>>,
    /// The latest published states (id to text), for ONVIF to read the settings in force.
    states: Mutex<std::collections::BTreeMap<String, String>>,
}

#[derive(Clone)]
struct Owner {
    queue: transport::Submitter<Command>,
    stop: Option<std::path::PathBuf>,
}

impl Motion {
    /// The hardware thread is fed through `queue` and stops through `stop`.
    pub(super) fn attach(
        &self,
        queue: transport::Submitter<Command>,
        stop: Option<std::path::PathBuf>,
    ) {
        *self.owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(Owner { queue, stop });
    }

    pub(super) fn set_presets(&self, names: Vec<String>) -> bool {
        let mut held = self.presets.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *held != names;
        *held = names;
        changed
    }

    /// Whether the head or lens is moving or about to: a move is queued, being applied, or a hold runs.
    pub fn moving(&self) -> bool {
        self.busy() || since_start() < self.queued_until.load(Ordering::Acquire)
    }

    /// A move was queued: it counts as under way for `grace` even before the hardware thread applies it.
    fn expect_move(&self, grace: Duration) {
        let until = since_start() + grace.as_millis() as u64;
        self.queued_until.fetch_max(until, Ordering::AcqRel);
    }

    pub(super) fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub(super) fn set_busy(&self, busy: bool) {
        self.busy.store(busy, Ordering::Release);
    }

    /// The hardware thread finished a pass: `busy` is true while a hold goes on. `pose` replaces the stored
    /// pose when read (`Some(None)` clears it: the position is unknown).
    pub(super) fn settle(&self, busy: bool, pose: Option<Option<super::Pose>>) {
        if let Some(pose) = pose {
            *self.pose.lock().unwrap_or_else(|e| e.into_inner()) = pose;
        }
        self.set_busy(busy);
    }

    /// Replaces the snapshot of published states.
    pub(crate) fn set_states(&self, states: &std::collections::BTreeMap<String, String>) {
        let mut held = self.states.lock().unwrap_or_else(|e| e.into_inner());
        if *held != *states {
            held.clone_from(states);
        }
    }

    /// The last published state of `id` (a setting key such as `isp_brightness`), as MQTT carries it.
    pub fn state(&self, id: &str) -> Option<String> {
        self.states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// The confirmed pan, tilt and zoom after the latest move; `None` while pan/tilt is unknown.
    pub fn pose(&self) -> Option<super::Pose> {
        *self.pose.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn owner(&self) -> Result<Owner, String> {
        self.owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "the camera's motion owner is not running".into())
    }

    /// Queues `command` for the hardware thread exactly as an MQTT command. [`Command::Stop`] goes the urgent
    /// way, past the queue.
    pub fn submit(&self, command: Command) -> Result<(), String> {
        let owner = self.owner()?;
        if command == Command::Stop {
            let path = owner.stop.ok_or("stop owner unavailable")?;
            // The urgent stop discards what is queued: nothing is "about to move" any more.
            self.queued_until.store(0, Ordering::Release);
            return crate::control_stop::request(&path).map_err(|e| e.to_string());
        }
        let moves = command.moves();
        owner
            .queue
            .submit(transport::Command::App(command))
            .map_err(|e| match e {
                TrySendError::Full(_) => "the camera is busy: command dropped".to_string(),
                TrySendError::Disconnected(_) => "the camera's motion owner stopped".to_string(),
            })?;
        if moves {
            self.expect_move(QUEUE_GRACE);
        }
        Ok(())
    }

    /// The preset names in force.
    pub fn presets(&self) -> Vec<String> {
        self.presets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mqtt::Pose;

    #[test]
    fn moving_covers_the_queue_the_apply_and_a_running_hold_but_not_a_discarded_command() {
        let m = Motion::default();
        assert!(!m.moving());
        // Queued, not yet picked up: moving for the grace period only, so a command a stop threw away
        // cannot hold the status on MOVING for good.
        m.expect_move(Duration::from_millis(30));
        assert!(m.moving());
        std::thread::sleep(Duration::from_millis(60));
        assert!(!m.moving());
        // Being applied (a blocking goto): moving however long it takes.
        m.set_busy(true);
        std::thread::sleep(Duration::from_millis(30));
        assert!(m.moving());
        // Finished with no hold left: idle. A hold keeps it moving until the tick that ends it.
        m.settle(true, None);
        assert!(m.moving());
        m.settle(false, None);
        assert!(!m.moving());
        // A later, shorter grace never shortens an earlier one.
        m.expect_move(Duration::from_secs(5));
        m.expect_move(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(20));
        assert!(m.moving());
    }

    #[test]
    fn the_pose_is_replaced_when_read_kept_otherwise_and_cleared_when_unknown() {
        let m = Motion::default();
        assert_eq!(m.pose(), None);
        let here = Pose {
            pan: 10,
            tilt: 20,
            zoom: 30,
        };
        m.settle(false, Some(Some(here)));
        assert_eq!(m.pose(), Some(here));
        m.settle(true, None);
        assert_eq!(m.pose(), Some(here), "not read: kept");
        m.settle(false, Some(None));
        assert_eq!(m.pose(), None, "a stop made the position unknown");
    }
}
