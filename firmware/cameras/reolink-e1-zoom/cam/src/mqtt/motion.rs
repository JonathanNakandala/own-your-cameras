//! The hardware thread's command queue, shared: MQTT and ONVIF submit the same [`Command`]s to the one
//! owner of pan, tilt, zoom and the preset table, on the same clock and motion epoch.

use super::{Command, Queued};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Commands waiting for the hardware thread at most.
const QUEUE: usize = 8;

pub struct Motion {
    tx: SyncSender<Queued>,
    rx: Mutex<Option<Receiver<Queued>>>,
    /// The one clock: commands are stamped on it when they arrive, the hardware thread ticks on it.
    pub(super) started: Instant,
    /// Set by the hardware thread when it starts; `None` until then (and when MQTT is not configured).
    owner: Mutex<Option<Owner>>,
    /// The preset names in force, as last read from the hardware thread.
    pub(super) presets: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct Owner {
    epoch: Arc<AtomicU64>,
    stop: Option<std::path::PathBuf>,
}

impl Default for Motion {
    fn default() -> Self {
        let (tx, rx) = sync_channel(QUEUE);
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            started: Instant::now(),
            owner: Mutex::default(),
            presets: Mutex::default(),
        }
    }
}

impl Motion {
    /// The receiving end, once: the hardware thread's.
    pub(super) fn take_receiver(&self) -> Option<Receiver<Queued>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub(super) fn sender(&self) -> SyncSender<Queued> {
        self.tx.clone()
    }

    /// The hardware thread runs under `epoch` and stops through `stop`.
    pub(super) fn attach(&self, epoch: Arc<AtomicU64>, stop: Option<std::path::PathBuf>) {
        *self.owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(Owner { epoch, stop });
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
            return crate::control_stop::request(&path).map_err(|e| e.to_string());
        }
        self.tx
            .try_send(Queued {
                generation: owner.epoch.load(Ordering::Acquire),
                received: self.started.elapsed(),
                command,
            })
            .map_err(|e| match e {
                TrySendError::Full(_) => "the camera is busy: command dropped".to_string(),
                TrySendError::Disconnected(_) => "the camera's motion owner stopped".to_string(),
            })
    }

    /// The preset names in force.
    pub fn presets(&self) -> Vec<String> {
        self.presets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
