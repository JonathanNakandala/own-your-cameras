//! The hardware thread's command queue, shared: MQTT and ONVIF submit the same [`Command`]s to the one
//! owner of pan, tilt, zoom and the preset table, on the same clock and motion epoch (the shared MQTT
//! layer's queue, through its [`transport::Submitter`]).

use super::Command;
use camera_transport::mqtt as transport;
use std::sync::Mutex;
use std::sync::mpsc::TrySendError;

#[derive(Default)]
pub struct Motion {
    /// Set when the MQTT layer starts; `None` until then (and when MQTT is not configured).
    owner: Mutex<Option<Owner>>,
    /// The preset names in force, as last read from the hardware thread.
    pub(super) presets: Mutex<Vec<String>>,
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
        owner
            .queue
            .submit(transport::Command::App(command))
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
