//! Single media owner, independent board owner and portable network workers.
//! Real hardware progress qualifies health; queued intent never becomes state.
use crate::graph_settings::{self, GraphSettings};
use crate::media::{RustVideoSpec, VideoMetadata};
use crate::{board, config, media::Media, pointing, settings};
use camera_transport::{AudioHub, Credentials, VideoHubs, mqtt};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod motion;
use motion::*;

mod health;
use health::*;

mod telemetry;
use telemetry::*;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

struct Controls {
    epoch: AtomicU32,
    stop: AtomicBool,
    release_epoch: AtomicU32,
    orientation: AtomicU32,
    active: Mutex<Option<(board::Cancellation, bool)>>,
    speaker_requested: AtomicBool,
    speaker_enabled: AtomicBool,
    ptz_speed: AtomicU32,
    ptz_start_speed: AtomicU32,
    ptz_ramp_up: AtomicU32,
    ptz_ramp_down: AtomicU32,
    mqtt_ready: AtomicBool,
}
impl Controls {
    fn new() -> Self {
        Self {
            epoch: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            release_epoch: AtomicU32::new(0),
            orientation: AtomicU32::new(4), // Unknown: never infer image orientation.
            active: Mutex::new(None),
            speaker_requested: AtomicBool::new(false),
            speaker_enabled: AtomicBool::new(false),
            ptz_speed: AtomicU32::new(board::DEFAULT_MOTOR_SPEED as u32),
            ptz_start_speed: AtomicU32::new(crate::motion::Timings::default().start_speed),
            ptz_ramp_up: AtomicU32::new(crate::motion::Timings::default().ramp_up_ms),
            ptz_ramp_down: AtomicU32::new(crate::motion::Timings::default().ramp_down_ms),
            mqtt_ready: AtomicBool::new(false),
        }
    }
    fn software(&self, slot: u16) -> io::Result<&AtomicU32> {
        match slot {
            settings::SPEED => Ok(&self.ptz_speed),
            settings::START_SPEED => Ok(&self.ptz_start_speed),
            settings::RAMP_UP => Ok(&self.ptz_ramp_up),
            settings::RAMP_DOWN => Ok(&self.ptz_ramp_down),
            _ => Err(io::Error::other("unknown software setting")),
        }
    }
    fn timings(&self) -> crate::motion::Timings {
        crate::motion::Timings {
            start_speed: self.ptz_start_speed.load(Ordering::Acquire),
            ramp_up_ms: self.ptz_ramp_up.load(Ordering::Acquire),
            ramp_down_ms: self.ptz_ramp_down.load(Ordering::Acquire),
        }
    }
    fn cancel(&self, motion_only: bool) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.stop.store(true, Ordering::Release);
        if let Ok(active) = self.active.try_lock()
            && let Some((cancel, motor)) = active.as_ref()
            && (*motor || !motion_only)
        {
            cancel.cancel();
        }
    }
    fn release(&self) {
        // Publish before any fresh work in this epoch. Unlike Stop, this never
        // cancels the active token: the board must execute its braking profile.
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        self.release_epoch.store(epoch, Ordering::Release);
    }
    fn operation(&self, motor: bool) -> board::Cancellation {
        let cancel = board::Cancellation::default();
        *self.active.lock().unwrap_or_else(|p| p.into_inner()) = Some((cancel.clone(), motor));
        if motor && self.stop.load(Ordering::Acquire) {
            cancel.cancel();
        }
        cancel
    }
    fn finished(&self) {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

struct SettingHardware<'a> {
    media: &'a mut Media,
    controls: &'a Controls,
}

struct GraphSelector {
    slot: u16,
    key: &'static str,
    name: &'static str,
    field: &'static str,
    options: &'static [&'static str],
}
const GRAPH_SELECTORS: &[GraphSelector] = &[
    GraphSelector {
        slot: 100,
        key: "main_resolution_pending",
        name: "Staged main resolution",
        field: "main_resolution",
        options: graph_settings::MAIN_RESOLUTIONS,
    },
    GraphSelector {
        slot: 101,
        key: "sub_resolution_pending",
        name: "Staged sub resolution",
        field: "sub_resolution",
        options: graph_settings::SUB_RESOLUTIONS,
    },
    GraphSelector {
        slot: 102,
        key: "main_profile_pending",
        name: "Staged main AVC profile",
        field: "main_profile",
        options: graph_settings::PROFILES,
    },
    GraphSelector {
        slot: 103,
        key: "sub_profile_pending",
        name: "Staged sub AVC profile",
        field: "sub_profile",
        options: graph_settings::PROFILES,
    },
];
const APPLY_VIDEO: u16 = 110;
const DISCARD_VIDEO: u16 = 111;

#[derive(Debug, PartialEq, Eq)]
enum GraphCommand {
    Stage(&'static str, &'static str),
    Apply,
    Discard,
}
fn graph_command(slot: u16, value: i32) -> io::Result<Option<GraphCommand>> {
    if let Some(field) = GRAPH_SELECTORS.iter().find(|field| field.slot == slot) {
        let option = usize::try_from(value)
            .ok()
            .and_then(|index| field.options.get(index))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cold video selection out of range",
                )
            })?;
        return Ok(Some(GraphCommand::Stage(field.field, option)));
    }
    match slot {
        APPLY_VIDEO | DISCARD_VIDEO if value != 0 => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cold video button requires PRESS",
        )),
        APPLY_VIDEO => Ok(Some(GraphCommand::Apply)),
        DISCARD_VIDEO => Ok(Some(GraphCommand::Discard)),
        _ => Ok(None),
    }
}
fn request_graph(
    graph: Option<&mut GraphSettings>,
    slot: u16,
    value: i32,
) -> io::Result<Option<bool>> {
    let Some(command) = graph_command(slot, value)? else {
        return Ok(None);
    };
    let graph = graph
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "cold_controls disabled"))?;
    let reboot = matches!(&command, GraphCommand::Apply);
    match command {
        GraphCommand::Stage(field, value) => graph.stage(field, value)?,
        GraphCommand::Discard => graph.discard()?,
        GraphCommand::Apply => graph.arm()?,
    }
    // In particular an arm/dirsync failure never returns a reboot instruction.
    Ok(Some(reboot))
}
fn entity(id: &str, name: &str, domain: mqtt::Domain, unit: Option<&str>) -> mqtt::Entity {
    mqtt::Entity {
        id: id.into(),
        name: name.into(),
        domain,
        minimum: 0,
        maximum: 0,
        options: vec![],
        unit: unit.map(str::to_owned),
        diagnostic: matches!(domain, mqtt::Domain::Sensor),
        control_slot: None,
    }
}
fn graph_entities() -> Vec<mqtt::Entity> {
    let mut entities = GRAPH_SELECTORS
        .iter()
        .map(|field| mqtt::Entity {
            id: field.key.into(),
            name: field.name.into(),
            domain: mqtt::Domain::Select,
            minimum: 0,
            maximum: field.options.len() as i32 - 1,
            options: field
                .options
                .iter()
                .map(|option| (*option).into())
                .collect(),
            unit: None,
            diagnostic: false,
            control_slot: Some(field.slot),
        })
        .collect::<Vec<_>>();
    for (slot, key, name) in [
        (
            APPLY_VIDEO,
            "apply_video_settings",
            "Apply video settings and reboot",
        ),
        (
            DISCARD_VIDEO,
            "discard_video_settings",
            "Discard staged video settings",
        ),
    ] {
        let mut button = entity(key, name, mqtt::Domain::Button, None);
        button.control_slot = Some(slot);
        entities.push(button);
    }
    for (key, name) in [
        ("main_resolution", "Main encoded resolution"),
        ("sub_resolution", "Sub encoded resolution"),
        ("main_profile", "Main encoded AVC profile"),
        ("sub_profile", "Sub encoded AVC profile"),
        ("video_settings_status", "Video settings status"),
    ] {
        entities.push(entity(key, name, mqtt::Domain::Sensor, None));
    }
    entities
}
fn profile_text(profile: u32) -> Option<&'static str> {
    match profile {
        66 => Some("baseline"),
        77 => Some("main"),
        100 => Some("high"),
        _ => None,
    }
}
fn pending_graph_updates(spec: RustVideoSpec) -> Vec<(&'static str, String)> {
    vec![
        (
            "main_resolution_pending",
            format!("{}x{}", spec.main_width, spec.main_height),
        ),
        (
            "sub_resolution_pending",
            format!("{}x{}", spec.sub_width, spec.sub_height),
        ),
        (
            "main_profile_pending",
            profile_text(spec.main_profile).unwrap_or("unknown").into(),
        ),
        (
            "sub_profile_pending",
            profile_text(spec.sub_profile).unwrap_or("unknown").into(),
        ),
    ]
}
fn encoded_graph_updates(actual: [Option<VideoMetadata>; 2]) -> Vec<(&'static str, String)> {
    let mut updates = vec![];
    for (channel, metadata) in actual.into_iter().enumerate() {
        if let Some(metadata) = metadata {
            updates.push((
                if channel == 0 {
                    "main_resolution"
                } else {
                    "sub_resolution"
                },
                format!("{}x{}", metadata.width, metadata.height),
            ));
            updates.push((
                if channel == 0 {
                    "main_profile"
                } else {
                    "sub_profile"
                },
                profile_text(metadata.profile_idc)
                    .unwrap_or("unknown")
                    .into(),
            ));
        }
    }
    updates
}
fn graph_status(status: graph_settings::Status) -> &'static str {
    match status {
        graph_settings::Status::Confirmed => "confirmed",
        graph_settings::Status::Staged => "staged",
        graph_settings::Status::Armed => "armed_reboot_required",
        graph_settings::Status::Attempted => "trial_in_progress",
        graph_settings::Status::RolledBack => "rolled_back",
        graph_settings::Status::StorageError => "storage_error",
    }
}
fn graph_states(
    publisher: &mqtt::Publisher,
    graph: &GraphSettings,
    actual: [Option<VideoMetadata>; 2],
) {
    for (key, value) in pending_graph_updates(graph.values())
        .into_iter()
        .chain(encoded_graph_updates(actual))
    {
        let _ = publisher.state(key, value);
    }
    let _ = publisher.state("video_settings_status", graph_status(graph.status()));
}

const ISP_STATISTICS: &[(u32, &str, &str, Option<&str>)] = &[
    (50, "ae_luma", "AE luma", None),
    (51, "exposure_us", "Exposure time", Some("µs")),
];
#[derive(Default)]
struct IspStatistics {
    supported: [bool; 2],
    updates: Vec<(&'static str, String)>,
}
impl IspStatistics {
    fn discover(mut read: impl FnMut(u32) -> io::Result<i32>) -> Self {
        let mut result = Self {
            supported: [false; 2],
            updates: vec![],
        };
        for (index, &(control, key, _, _)) in ISP_STATISTICS.iter().enumerate() {
            if let Ok(value) = read(control) {
                result.supported[index] = true;
                result.updates.push((key, value.to_string()));
            }
        }
        result
    }
    fn entities(&self) -> Vec<mqtt::Entity> {
        ISP_STATISTICS
            .iter()
            .zip(self.supported)
            .filter(|(_, supported)| *supported)
            .map(|(&(_, key, name, unit), _)| entity(key, name, mqtt::Domain::Sensor, unit))
            .collect()
    }
    fn sample(&mut self, mut read: impl FnMut(u32) -> io::Result<i32>) {
        for (index, &(control, key, _, _)) in ISP_STATISTICS.iter().enumerate() {
            if self.supported[index] {
                // Unknown records missing data; it is never a fabricated measurement.
                let value =
                    read(control).map_or_else(|_| "unknown".into(), |value| value.to_string());
                self.updates.push((key, value));
            }
        }
    }
    fn take_updates(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.updates)
    }
}
impl crate::setting_service::Hardware for SettingHardware<'_> {
    fn get(&mut self, control: u32) -> io::Result<i32> {
        self.media.control_get(control)
    }
    fn set(&mut self, control: u32, value: i32) -> io::Result<crate::media::ControlResult> {
        self.media.control_set(control, value)
    }
    fn software_get(&self, slot: u16) -> io::Result<i32> {
        Ok(self.controls.software(slot)?.load(Ordering::Acquire) as i32)
    }
    fn software_set(&mut self, slot: u16, value: i32) -> io::Result<()> {
        if !settings::descriptor(slot).is_some_and(|d| d.hardware.is_none() && d.valid(value)) {
            return Err(io::Error::other("invalid software setting"));
        }
        self.controls
            .software(slot)?
            .store(value as u32, Ordering::Release);
        Ok(())
    }
}
struct Work {
    command: mqtt::Command,
    received: Instant,
    epoch: u32,
}
struct IspRequest {
    night: bool,
    reply: SyncSender<io::Result<()>>,
}
fn status_states(publisher: &mqtt::Publisher, status: board::MotorStatus) {
    let _ = publisher.state("pan", status.position.x.to_string());
    let _ = publisher.state("tilt", status.position.y.to_string());
}
fn pin_states(publisher: &mqtt::Publisher) {
    if let Ok(snapshot) = board::snapshot() {
        if let board::Observation::Known(on) = snapshot.blue_led_high {
            let _ = publisher.state("status_led", if on { "ON" } else { "OFF" });
        }
        if let board::Observation::Known(on) = snapshot.ir_illumination_high {
            let _ = publisher.state("ir_lamp", if on { "ON" } else { "OFF" });
        }
    }
}
struct HardwareWiring {
    rx: Receiver<Work>,
    isp: SyncSender<IspRequest>,
    publisher: mqtt::Publisher,
    ready: SyncSender<io::Result<board::MotorStatus>>,
    calibration: Option<pointing::Calibration>,
}
fn hardware_loop(
    wiring: HardwareWiring,
    controls: Arc<Controls>,
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
) -> io::Result<()> {
    let HardwareWiring {
        rx,
        isp,
        publisher,
        ready,
        calibration,
    } = wiring;
    let mut board = match board::Board::open() {
        Ok(board) => board,
        Err(error) => {
            let _ = ready.send(Err(io::Error::other(error.to_string())));
            return Err(error);
        }
    };
    let initial = board.motor_status()?;
    if initial.owner_open {
        let error = io::Error::other("stock motor owner remains active");
        let _ = ready.send(Err(io::Error::other(error.to_string())));
        return Err(error);
    }
    let mut pointing = calibration.map(pointing::Pointing::new).transpose()?;
    if let Some(pointing) = &mut pointing {
        pointing.observe(progress.started.elapsed(), initial.position)?;
    }
    let _ = ready.send(Ok(initial));
    let mut motion = Motion::default();
    let mut presets = crate::presets::Presets::load("/mnt/data/mi360/presets.json")?;
    let mut homed = false;
    let mut approach = crate::presets::Approach::homed(initial.position);
    let mut request_home = true;
    let mut homing: Option<board::Cancellation> = None;
    let _ = publisher.state("positioning", "unhomed");
    let _ = publisher.state("preset", "");

    let mut seen_release = 0;
    let mut next_state = Instant::now();
    let outcome = (|| -> io::Result<()> {
        board.speaker_gate(false)?;
        board.set_led(board::Led::Yellow, false)?;
        board.set_led(board::Led::Blue, true)?;
        while !stop.load(Ordering::Acquire) {
            urgent_motion(&mut board, &controls, &mut motion, &mut seen_release)?;
            let requested = controls.speaker_requested.load(Ordering::Acquire);
            if requested != controls.speaker_enabled.load(Ordering::Acquire) {
                board.speaker_gate(requested)?;
                eprintln!(
                    "speaker: amplifier {}",
                    if requested { "on" } else { "off" }
                );
                controls.speaker_enabled.store(requested, Ordering::Release);
            }
            if let Some(cancel) = homing.as_ref() {
                match board.poll_home(cancel) {
                    Ok(board::HomeProgress::Running) => {}
                    Ok(board::HomeProgress::Ready(status)) => {
                        homing = None;
                        homed = true;
                        approach = crate::presets::Approach::homed(status.position);
                        controls.finished();
                        pointing = calibration.map(pointing::Pointing::new).transpose()?;
                        if let Some(p) = pointing.as_mut() {
                            p.observe(progress.started.elapsed(), status.position)?;
                        }
                        status_states(&publisher, status);
                        let _ = publisher.state("positioning", "ready");
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        homing = None;
                        homed = false;
                        controls.finished();
                        let _ = publisher.state("positioning", "unhomed: home cancelled");
                    }
                    Err(e) => return Err(e),
                }
            }
            if !homed {
                for _ in 0..8 {
                    let Ok(work) = rx.try_recv() else {
                        break;
                    };
                    if !fresh(
                        &work,
                        controls.epoch.load(Ordering::Acquire),
                        Instant::now(),
                    ) {
                        continue;
                    }
                    match work.command {
                        mqtt::Command::Home => {
                            board.stop()?;
                            homing = None;
                            controls.finished();
                            request_home = true;
                        }
                        mqtt::Command::Led(on) => {
                            board.set_led(board::Led::Blue, on)?;
                            pin_states(&publisher);
                        }
                        _ => {
                            let _ = publisher.state("motion_status", "unhomed: motion unavailable");
                        }
                    }
                }
                if request_home
                    && homing.is_none()
                    && controls.orientation.load(Ordering::Acquire) <= 3
                    && controls.mqtt_ready.load(Ordering::Acquire)
                    && !controls.stop.load(Ordering::Acquire)
                {
                    let cancel = controls.operation(true);
                    board.begin_home()?;
                    homing = Some(cancel);
                    request_home = false;
                    let _ = publisher.state("positioning", "homing");
                    let _ = publisher.state("pan", "unknown");
                    let _ = publisher.state("tilt", "unknown");
                }
                progress.mark(&progress.hardware);
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let current = board.motor_status()?;
            approach.observe(current.position);
            if let Some(pointing) = &mut pointing {
                pointing.observe(progress.started.elapsed(), current.position)?;
            }
            // Hard lease expiry is independent of graceful deceleration and of
            // future command traffic. Cancellation is inspected before ioctl.
            motion.expire(Instant::now());
            for _ in 0..8 {
                let Ok(work) = rx.try_recv() else {
                    break;
                };
                urgent_motion(&mut board, &controls, &mut motion, &mut seen_release)?;
                let now = Instant::now();
                if !fresh(&work, controls.epoch.load(Ordering::Acquire), now) {
                    continue;
                }
                match work.command {
                    mqtt::Command::Pan(_)
                    | mqtt::Command::Tilt(_)
                    | mqtt::Command::Move(_)
                    | mqtt::Command::Velocity(_)
                    | mqtt::Command::Position3d(_) => {
                        let current = board.motor_status()?;
                        match motion_plan(
                            &work,
                            current,
                            &motion,
                            calibration,
                            pointing.as_ref(),
                            controls.orientation.load(Ordering::Acquire),
                            (progress.started, now),
                        ) {
                            Ok(plan) => {
                                let _ = publisher.state("preset", "");
                                if let Some(axes) = motion.compatible_hold(plan) {
                                    if board.retarget_profiled_move(plan.target, axes)? {
                                        motion.active.as_mut().unwrap().plan = plan;
                                        motion.active.as_mut().unwrap().braking = false;
                                        motion.pending = None;
                                    } else {
                                        motion.active = None;
                                        motion.pending = None;
                                        controls.finished();
                                    }
                                } else if motion.queue(plan) {
                                    board.request_brake()?;
                                }
                            }
                            Err(error) => eprintln!("PTZ command rejected: {error}"),
                        }
                    }
                    mqtt::Command::Home => {
                        board.stop()?;
                        motion.active = None;
                        motion.pending = None;
                        controls.finished();
                        homed = false;
                        request_home = true;
                        let _ = publisher.state("preset", "");
                        break;
                    }
                    mqtt::Command::SavePreset(name) => {
                        let result = presets.save(
                            name,
                            board.motor_status()?,
                            homed && motion.active.is_none() && motion.pending.is_none(),
                            approach.directions(),
                        );
                        if result.is_ok() {
                            let _ =
                                publisher.state("preset", name.as_str().map_err(io::Error::other)?);
                        }
                        let message = match result {
                            Ok(()) => {
                                format!("saved preset {}", name.as_str().map_err(io::Error::other)?)
                            }
                            Err(e) => format!("preset save failed: {e}"),
                        };
                        let _ = publisher.state("motion_status", message);
                    }
                    mqtt::Command::Preset(name) => match presets.get(name, homed) {
                        Ok(route) => {
                            let plan = Plan {
                                target: route.waypoint,
                                preset: Some(name),
                                approach_target: Some(route.target),
                                kind: MotionKind::Fixed,
                                received: work.received,
                                until: work.received + Duration::from_secs(30),
                                epoch: work.epoch,
                            };
                            if motion.queue(plan) {
                                board.request_brake()?;
                            }
                        }
                        Err(e) => {
                            let _ = publisher
                                .state("motion_status", format!("preset recall failed: {e}"));
                        }
                    },
                    mqtt::Command::Stop => {
                        board.stop()?;
                        motion.active = None;
                        motion.pending = None;
                        controls.finished();
                    }
                    mqtt::Command::Release => {
                        if motion.release(work.epoch, Instant::now()) {
                            board.request_brake()?;
                        }
                    }
                    mqtt::Command::Led(on) => {
                        board.set_led(board::Led::Blue, on)?;
                        pin_states(&publisher);
                    }
                    mqtt::Command::IrLight(on) => {
                        board.set_ir_illumination(on)?;
                        pin_states(&publisher);
                    }
                    mqtt::Command::IrMode(mode) => {
                        let night = match mode {
                            mqtt::IrMode::Day => false,
                            mqtt::IrMode::Night => true,
                            mqtt::IrMode::Auto => continue,
                        };
                        // The filter pulse is blocking. Stop/verify/release the
                        // motor first, invalidate queued trajectories, and only
                        // then replace Controls.active with the filter token.
                        controls.epoch.fetch_add(1, Ordering::AcqRel);
                        board.stop()?;
                        motion.active = None;
                        motion.pending = None;
                        controls.finished();
                        let (reply, answer) = sync_channel(1);
                        isp.try_send(IspRequest { night, reply })
                            .map_err(|_| io::Error::other("ISP request queue unavailable"))?;
                        let deadline = Instant::now() + Duration::from_secs(2);
                        loop {
                            match answer.recv_timeout(Duration::from_millis(25)) {
                                Ok(result) => {
                                    result?;
                                    break;
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                                    if !stop.load(Ordering::Acquire)
                                        && Instant::now() < deadline => {}
                                _ => {
                                    return Err(io::Error::other(
                                        "ISP coordination did not finish",
                                    ));
                                }
                            }
                        }
                        let cancel = controls.operation(false);
                        let pulse = board.pulse_ircut(
                            if night {
                                board::IrCut::Night
                            } else {
                                board::IrCut::Day
                            },
                            &cancel,
                        );
                        controls.finished();
                        if let Err(error) = pulse {
                            if error.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            return Err(error);
                        }
                        board.set_ir_illumination(night)?;
                        let _ = publisher.state("ir_mode", if night { "night" } else { "day" });
                        pin_states(&publisher);
                    }
                    mqtt::Command::Reboot
                    | mqtt::Command::MicVolume(_)
                    | mqtt::Command::SpeakerVolume(_)
                    | mqtt::Command::Setting { .. } => {}
                }
            }
            urgent_motion(&mut board, &controls, &mut motion, &mut seen_release)?;
            motion.expire(Instant::now());
            if let Some(active) = &mut motion.active {
                let maximum = active
                    .plan
                    .kind
                    .ceiling(controls.ptz_speed.load(Ordering::Acquire));
                if maximum != active.max_speed {
                    board.set_profiled_max_speed(maximum)?;
                    active.max_speed = maximum;
                }
                let result = board.poll_profiled_move(&active.cancel);
                match result {
                    Ok(Some(status)) => {
                        status_states(&publisher, status);
                        approach.observe(status.position);
                        let next = if !active.braking && !active.cancel.is_cancelled() {
                            active.plan.next_stage(Instant::now())
                        } else {
                            None
                        };
                        if let Some(next) = next {
                            if motion.pending.is_none() {
                                motion.pending = Some(next);
                            }
                        } else if !active.braking
                            && active.plan.approach_target.is_none()
                            && let Some(name) = active.plan.preset
                        {
                            let _ =
                                publisher.state("preset", name.as_str().map_err(io::Error::other)?);
                            let _ = publisher.state(
                                "motion_status",
                                format!(
                                    "reached preset {}",
                                    name.as_str().map_err(io::Error::other)?
                                ),
                            );
                        }
                        motion.active = None;
                        controls.finished();
                    }
                    Ok(None) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        motion.active = None;
                        controls.finished();
                    }
                    Err(error) => return Err(error),
                }
                if let Some(pointing) = &mut pointing {
                    pointing.observe(progress.started.elapsed(), board.motor_status()?.position)?;
                }
            }
            if motion.active.is_none()
                && let Some(plan) = motion.pending.take()
            {
                let now = Instant::now();
                if plan.epoch == controls.epoch.load(Ordering::Acquire)
                    && now
                        .checked_duration_since(plan.received)
                        .is_some_and(|age| age <= pointing::MAX_COMMAND_AGE)
                    && now < plan.until
                    && controls.orientation.load(Ordering::Acquire) <= 3
                {
                    let current = board.motor_status()?;
                    let target = start_target(plan, current.position);
                    let delta = board::Position {
                        x: target.x - current.position.x,
                        y: target.y - current.position.y,
                    };
                    if delta != (board::Position { x: 0, y: 0 }) {
                        let cancel = controls.operation(true);
                        if cancel.is_cancelled() {
                            controls.finished();
                        } else {
                            let maximum = plan
                                .kind
                                .ceiling(controls.ptz_speed.load(Ordering::Acquire));
                            let before = board
                                .begin_profiled_move(delta, controls.timings().profile(maximum)?)?;
                            if let Some(pointing) = &mut pointing {
                                pointing.observe(progress.started.elapsed(), before.position)?;
                            }
                            motion.active = Some(Running {
                                plan,
                                cancel,
                                braking: false,
                                max_speed: maximum,
                            });
                        }
                    } else {
                        status_states(&publisher, current);
                        if let Some(next) = plan.next_stage(now) {
                            motion.pending = Some(next);
                        } else if let Some(name) = plan.preset {
                            let _ =
                                publisher.state("preset", name.as_str().map_err(io::Error::other)?);
                        }
                    }
                }
            }
            if Instant::now() >= next_state {
                status_states(&publisher, board.motor_status()?);
                pin_states(&publisher);
                next_state = Instant::now() + Duration::from_secs(1);
            }
            progress.mark(&progress.hardware);
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })();
    let amplifier = board.speaker_gate(false);
    if amplifier.is_ok() {
        controls.speaker_enabled.store(false, Ordering::Release);
    }
    let shutdown = board.shutdown();
    if shutdown.is_ok() {
        controls.finished();
    }
    outcome.and(amplifier).and(shutdown)
}

fn write_health(
    started: Instant,
    boot_id: &str,
    sequence: u32,
    healthy: bool,
    video: [u64; 2],
    audio: u64,
) -> io::Result<()> {
    let path = "/tmp/mi360-runtime.health";
    let temporary = format!("{path}.{}.new", std::process::id());
    let bytes=serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"boot_id":boot_id,"sequence":sequence,"uptime_ms":started.elapsed().as_millis() as u64,"healthy":healthy,"main_frames":video[0],"sub_frames":video[1],"audio_frames":audio}))
        .map_err(|_|io::Error::other("cannot encode runtime health"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let result = file
        .write_all(&bytes)
        .and_then(|()| file.flush())
        .and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn join_worker(handle: JoinHandle<io::Result<()>>) -> io::Result<()> {
    handle
        .join()
        .map_err(|_| io::Error::other("runtime worker panicked"))?
}

struct Workers {
    stop: Arc<AtomicBool>,
    controls: Arc<Controls>,
    hardware: Option<JoinHandle<io::Result<()>>>,
    rtsp: Option<JoinHandle<io::Result<()>>>,
    mqtt: Option<JoinHandle<io::Result<()>>>,
}
impl Workers {
    fn finish(&mut self) -> [io::Result<()>; 3] {
        self.stop.store(true, Ordering::Release);
        self.controls.cancel(false);
        self.controls
            .speaker_requested
            .store(false, Ordering::Release);
        [self.hardware.take(), self.rtsp.take(), self.mqtt.take()]
            .map(|worker| worker.map_or(Ok(()), join_worker))
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        for result in self.finish() {
            if let Err(error) = result {
                eprintln!("worker cleanup: {error}");
            }
        }
    }
}

pub fn run(path: &str) -> io::Result<()> {
    let cfg = config::load(path)?;
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if boot_id.len() != 36
        || !boot_id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(io::Error::other("invalid kernel boot identity"));
    }
    INTERRUPTED.store(false, Ordering::Relaxed);
    // SAFETY: handlers only set a lock-free atomic flag, and remain valid for process lifetime.
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(signal, interrupt as *const () as libc::sighandler_t);
        }
    }
    let progress = Arc::new(Progress::new());
    let started = progress.started;
    let stop = Arc::new(AtomicBool::new(false));
    let controls = Arc::new(Controls::new());
    let calibration = pointing::load(std::path::Path::new(pointing::CALIBRATION_PATH))?;
    let mut watcher = if cfg.runtime.watchdog {
        let watchdog = crate::watchdog::Watchdog::open(40)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Some(Watcher::start(
            watchdog,
            stop.clone(),
            progress.clone(),
            cfg.runtime.audio,
        )?)
    } else {
        None
    };
    // Cold trials are consumed durably before the SDK acquires a graph. The
    // watchdog already covers store/media failures; default-off RAM development
    // never creates or consumes a persistent video trial.
    let mut graph = if cfg.runtime.cold_controls {
        Some(GraphSettings::load(
            "/mnt/data/mi360/video-settings.json",
            0,
        )?)
    } else {
        None
    };
    let video_spec = match graph.as_mut() {
        Some(graph) => graph.select_for_boot()?,
        None => RustVideoSpec::default(),
    };
    let mut media = Media::open_spec(cfg.runtime.audio, video_spec)?;
    let mut settings = crate::setting_service::Service::initialize(
        &mut SettingHardware {
            media: &mut media,
            controls: &controls,
        },
        crate::settings::Store::new("/mnt/data/mi360/settings.json"),
        cfg.runtime.audio,
        started.elapsed(),
    )?;
    refresh_orientation(&controls, media.control_get(0));
    let board_snapshot = board::snapshot()?;
    let motor = match board_snapshot.motor {
        board::Observation::Known(status) => status,
        board::Observation::Unavailable(_) => {
            return Err(io::Error::other("motor diagnostics unavailable"));
        }
    };
    let mut entities = mqtt::Entity::mi360_catalog(motor.max_x, motor.max_y);
    entities.push(entity("home", "Home PTZ", mqtt::Domain::Button, None));
    entities.push(entity(
        "positioning",
        "PTZ positioning",
        mqtt::Domain::Sensor,
        None,
    ));
    entities.push(entity("preset", "PTZ preset", mqtt::Domain::Sensor, None));
    entities.push(entity(
        "motion_status",
        "PTZ command status",
        mqtt::Domain::Sensor,
        None,
    ));
    let mut isp_statistics = if cfg.runtime.extended_controls {
        IspStatistics::discover(|control| media.control_get(control))
    } else {
        IspStatistics::default()
    };
    let memory_available = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| proc_kib(&text, "MemAvailable:"))
        .is_some();
    let signal_available = std::fs::read_to_string("/proc/net/wireless")
        .ok()
        .and_then(|text| wifi_signal(&text))
        .is_some();
    entities.retain(|entity| match entity.id.as_str() {
        "mic_volume" | "speaker_volume" => false,
        "memory_available" => memory_available,
        "wifi_signal" => signal_available,
        "ir_lamp" => matches!(
            board_snapshot.ir_illumination_high,
            board::Observation::Known(_)
        ),
        "status_led" => matches!(board_snapshot.blue_led_high, board::Observation::Known(_)),
        _ => true,
    });
    if cfg.runtime.extended_controls {
        entities.extend(settings.entities());
        entities.extend(isp_statistics.entities());
    }
    if cfg.runtime.cold_controls {
        entities.extend(graph_entities());
    }
    let mqtt_cfg = mqtt::Config {
        position3d: calibration.is_some(),
        broker: mqtt::Broker {
            host: cfg.mqtt.host,
            port: cfg.mqtt.port,
            username: cfg.mqtt.username,
            password: cfg.mqtt.password,
        },
        node_id: format!("cam_{}", cfg.camera.id),
        device_name: cfg.camera.name,
        manufacturer: "Xiaomi".into(),
        model: "MJSXJ10CM".into(),
        software_version: env!("CARGO_PKG_VERSION").into(),
        prefix: format!("camera/{}", cfg.camera.id),
        discovery_prefix: "homeassistant".into(),
        entities,
    };
    let (publisher, commands, mqtt_worker) = mqtt::channel(mqtt_cfg)?;
    for (key, value) in isp_statistics.take_updates() {
        let _ = publisher.state(key, value);
    }
    if let Some(graph) = graph.as_ref() {
        graph_states(&publisher, graph, [None, None]);
    }
    let hubs = Arc::new(VideoHubs::with_limits(camera_transport::VideoLimits::MI360));
    let microphone = AudioHub::new();
    let mut encoder = camera_opus::microphone()?;
    // The Mi 360 speaker plays 16 kHz blocks; its microphone captures 48 kHz on a separate device.
    let talk = camera_opus::talkback(16_000);
    let rtsp_cfg = Arc::new(camera_transport::Config {
        streams: hubs.streams(),
        creds: Some(Credentials {
            user: cfg.rtsp.username,
            password: cfg.rtsp.password,
        }),
        audio: cfg.runtime.audio.then(|| microphone.clone()),
        talk: cfg.runtime.audio.then(|| talk.clone()),
    });
    let listener =
        camera_transport::bind_with_limits(cfg.rtsp.port, camera_transport::ServerLimits::MI360)?;
    let (work_tx, work_rx) = sync_channel(8);
    let (isp_tx, isp_rx) = sync_channel(1);
    let (ready_tx, ready_rx) = sync_channel(1);
    let mut workers = Workers {
        stop: stop.clone(),
        controls: controls.clone(),
        hardware: None,
        rtsp: None,
        mqtt: None,
    };
    workers.hardware = Some({
        let (controls, stop, progress, publisher) = (
            controls.clone(),
            stop.clone(),
            progress.clone(),
            publisher.clone(),
        );
        std::thread::Builder::new()
            .name("hardware".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let result = hardware_loop(
                    HardwareWiring {
                        rx: work_rx,
                        isp: isp_tx,
                        publisher,
                        ready: ready_tx,
                        calibration,
                    },
                    controls,
                    stop.clone(),
                    progress,
                );
                if result.is_err() {
                    stop.store(true, Ordering::Release);
                }
                result
            })?
    });
    workers.rtsp = Some({
        let (stop, progress) = (stop.clone(), progress.clone());
        std::thread::Builder::new()
            .name("rtsp".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let result = camera_transport::serve_with_limits(
                    &listener,
                    rtsp_cfg,
                    &stop,
                    Some(&|| progress.mark(&progress.rtsp)),
                    camera_transport::ServerLimits::MI360,
                );
                if result.is_err() {
                    stop.store(true, Ordering::Release);
                }
                result
            })?
    });
    workers.mqtt = Some({
        let (stop, progress) = (stop.clone(), progress.clone());
        std::thread::Builder::new()
            .name("mqtt".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = mqtt_worker.run(&stop, Some(&|| progress.mark(&progress.mqtt)));
                if result.is_err() {
                    stop.store(true, Ordering::Release);
                }
                result
            })?
    });
    let mut ready = false;
    let mut reboot = false;
    let mut last_video = [None, None];
    let mut last_audio = None;
    let mut counts = [0_u64; 2];
    let mut bytes = [0_u64; 2];
    let mut total_frames = [0_u64; 2];
    let mut audio_frames = 0_u64;
    let mut next_report = Instant::now();
    let mut report_started = Instant::now();
    let mut sequence = 0_u32;
    let mut cpu = Cpu::default();
    let mut qualified = false;
    let mut pcm = VecDeque::<i16>::with_capacity(1920);
    // The AO device restarts (and drops audio) whenever its stream stops, so it is fed silence between bursts.
    let mut next_silence = Instant::now();
    let mut amplifier_on: Option<Instant> = None;
    // Speaker-loop losses, logged once a talk burst has been quiet for a second.
    let (mut spk_played, mut spk_expired, mut spk_overflow, mut spk_full) =
        (0u64, 0u64, 0u64, 0u64);
    let mut last_talk = None;
    let mut next_orientation = Instant::now();
    let outcome = (|| -> io::Result<()> {
        loop {
            if INTERRUPTED.load(Ordering::Relaxed) || stop.load(Ordering::Acquire) {
                break;
            }
            let now = Instant::now();
            if now >= next_orientation {
                refresh_orientation(&controls, media.control_get(0));
                next_orientation = now + Duration::from_millis(100);
            }
            if !ready {
                match ready_rx.try_recv() {
                    Ok(result) => {
                        status_states(&publisher, result?);
                        ready = true;
                        let _ = work_tx.try_send(Work {
                            command: mqtt::Command::IrMode(mqtt::IrMode::Day),
                            received: now,
                            epoch: controls.epoch.load(Ordering::Acquire),
                        });
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return Err(io::Error::other("hardware startup exited"));
                    }
                    Err(_) => {
                        if started.elapsed() > Duration::from_secs(5) {
                            return Err(io::Error::other("hardware startup deadline exceeded"));
                        }
                    }
                }
            }
            for _ in 0..8 {
                let Some(command) = commands.try_next(now) else {
                    break;
                };
                match command {
                    mqtt::Command::Setting { slot, value } => {
                        // Absent catalog entries are also rejected at the owner
                        // when the opt-in is false. This seam performs no SDK call.
                        match request_graph(graph.as_mut(), slot, value) {
                            Ok(Some(applying)) => {
                                if applying {
                                    // arm's file+directory sync precedes
                                    // this existing orderly reboot path.
                                    reboot = true;
                                    stop.store(true, Ordering::Release);
                                }
                                if let Some(graph) = graph.as_ref() {
                                    graph_states(
                                        &publisher,
                                        graph,
                                        [media.video_metadata(0), media.video_metadata(1)],
                                    );
                                }
                                if applying {
                                    break;
                                }
                                continue;
                            }
                            Err(error) => {
                                eprintln!("cold video command {slot} rejected: {error}");
                                if let Some(graph) = graph.as_ref() {
                                    graph_states(
                                        &publisher,
                                        graph,
                                        [media.video_metadata(0), media.video_metadata(1)],
                                    );
                                }
                                continue;
                            }
                            Ok(None) => {}
                        }
                        if cfg.runtime.extended_controls
                            && let Err(error) = settings.request(
                                slot,
                                value,
                                &mut SettingHardware {
                                    media: &mut media,
                                    controls: &controls,
                                },
                                started.elapsed(),
                            )
                        {
                            eprintln!("setting command {slot} rejected: {error}");
                        }
                        refresh_orientation(&controls, media.control_get(0));
                    }
                    mqtt::Command::Release => controls.release(),
                    mqtt::Command::Stop => controls.cancel(true),
                    mqtt::Command::Reboot => {
                        reboot = true;
                        stop.store(true, Ordering::Release);
                    }
                    _ => {
                        let _ = work_tx.try_send(Work {
                            command,
                            received: now,
                            epoch: controls.epoch.load(Ordering::Acquire),
                        });
                    }
                }
            }
            controls
                .mqtt_ready
                .store(publisher.is_connected(), Ordering::Release);
            if stop.load(Ordering::Acquire) || INTERRUPTED.load(Ordering::Relaxed) {
                break;
            }
            while let Ok(request) = isp_rx.try_recv() {
                let result = media.set_night(request.night);
                let failed = result.is_err();
                let _ = request.reply.send(result);
                if failed {
                    return Err(io::Error::other("ISP mode update failed"));
                }
            }
            for channel in 0..2 {
                if let Some(frame) = media.next_video(channel)? {
                    last_video[channel] = Some(Instant::now());
                    progress.mark(if channel == 0 {
                        &progress.main_video
                    } else {
                        &progress.sub_video
                    });
                    counts[channel] += 1;
                    total_frames[channel] += 1;
                    bytes[channel] += frame.data.len() as u64;
                    hubs.publish_annexb(channel, frame.timestamp_us, frame.data);
                }
            }
            if cfg.runtime.audio {
                for _ in 0..4 {
                    let Some(frame) = media.next_audio()? else {
                        break;
                    };
                    last_audio = Some(Instant::now());
                    progress.mark(&progress.audio);
                    audio_frames += 1;
                    for (pts_us, packet) in encoder.push(frame.timestamp_us, &frame.samples) {
                        microphone.publish(pts_us, packet);
                    }
                }
                // Take every mixer block that is due; keep at most 200 ms, dropping the oldest.
                for _ in 0..8 {
                    let Some(samples) = talk.next_pcm(Instant::now()) else {
                        break;
                    };
                    pcm.extend(samples);
                    last_talk = Some(Instant::now());
                    let excess = pcm.len().saturating_sub(3200);
                    pcm.drain(..excess);
                    spk_overflow += excess as u64;
                }
                let quiet = |ms| {
                    last_talk.is_none_or(|at: Instant| at.elapsed() > Duration::from_millis(ms))
                };
                // A burst's last partial block would otherwise wait for samples that never come.
                if !pcm.len().is_multiple_of(640) && quiet(40) {
                    pcm.resize(pcm.len().next_multiple_of(640), 0);
                }
                // Audio the amplifier never got to play is stale, not merely late.
                if !pcm.is_empty() && quiet(300) {
                    spk_expired += pcm.len() as u64;
                    pcm.clear();
                }
                if spk_played + spk_expired + spk_overflow > 0
                    && last_talk.is_none_or(|at: Instant| at.elapsed() > Duration::from_secs(1))
                {
                    eprintln!(
                        "speaker: burst ended played_ms={} expired_ms={} overflow_ms={} full={}",
                        spk_played / 16,
                        spk_expired / 16,
                        spk_overflow / 16,
                        spk_full
                    );
                    (spk_played, spk_expired, spk_overflow, spk_full) = (0, 0, 0, 0);
                }
                // The amplifier stays on a second past the last talk, so pauses between words never toggle it.
                controls.speaker_requested.store(
                    last_talk.is_some_and(|at: Instant| at.elapsed() < Duration::from_secs(1)),
                    Ordering::Release,
                );
                let enabled = controls.speaker_enabled.load(Ordering::Acquire);
                if enabled != amplifier_on.is_some() {
                    amplifier_on = enabled.then(Instant::now);
                }
                // Audio starts 60 ms after the amplifier, so its switch-on transient never lands on speech.
                let settled =
                    amplifier_on.is_some_and(|at| at.elapsed() >= Duration::from_millis(60));
                let now = Instant::now();
                // Hand the hardware every full block: its queue, not this loop's cadence, paces playback.
                while settled && pcm.len() >= 640 {
                    let block: Vec<i16> = pcm.iter().take(640).copied().collect();
                    if !media.play(&block)? {
                        spk_full += 1;
                        break;
                    }
                    spk_played += 640;
                    pcm.drain(..640);
                    next_silence = next_silence.max(now) + Duration::from_millis(40);
                }
                // Between bursts (and while the amplifier settles) keep the AO stream running with silence,
                // paced by the clock: at most one block ahead.
                if pcm.len() < 640 || !settled {
                    if now.saturating_duration_since(next_silence) > Duration::from_millis(100) {
                        next_silence = now;
                    }
                    if now >= next_silence {
                        let _ = media.play(&[0; 640])?;
                        next_silence += Duration::from_millis(40);
                    }
                }
            }
            let now = Instant::now();
            let pipeline = last_video.iter().all(|stamp| {
                stamp.is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(3))
            }) && (!cfg.runtime.audio
                || last_audio
                    .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(3)));
            qualified |= pipeline;
            if qualified
                && (last_video.iter().any(|stamp| {
                    stamp.is_some_and(|at| {
                        now.saturating_duration_since(at) > Duration::from_secs(5)
                    })
                }) || (cfg.runtime.audio
                    && last_audio.is_some_and(|at| {
                        now.saturating_duration_since(at) > Duration::from_secs(5)
                    })))
            {
                return Err(io::Error::other("real media progress stopped"));
            }
            if !qualified && started.elapsed() > Duration::from_secs(30) {
                return Err(io::Error::other("real media never became ready"));
            }
            let healthy = pipeline && ready && progress.healthy();
            progress.mark(&progress.main);
            if healthy {
                progress.qualified.store(true, Ordering::Release);
            }
            if let Some(graph) = graph.as_mut()
                && graph.needs_confirmation()
                && graph.observe_health(
                    video_spec,
                    [media.video_metadata(0), media.video_metadata(1)],
                    healthy,
                    started.elapsed(),
                )?
            {
                graph.confirm_healthy(video_spec)?;
            }
            if now >= next_report {
                if let Some(graph) = graph.as_ref() {
                    let actual = [media.video_metadata(0), media.video_metadata(1)];
                    graph_states(&publisher, graph, actual);
                }
                isp_statistics.sample(|control| media.control_get(control));
                for (key, value) in isp_statistics.take_updates() {
                    let _ = publisher.state(key, value);
                }
                if let Err(error) = settings.tick(
                    &mut SettingHardware {
                        media: &mut media,
                        controls: &controls,
                    },
                    started.elapsed(),
                    healthy,
                ) {
                    eprintln!("settings update: {error}");
                }
                for (key, value) in settings.take_updates() {
                    if cfg.runtime.extended_controls {
                        let _ = publisher.state(key, value);
                    }
                }
                let _ = publisher.state("pipeline_health", if healthy { "ON" } else { "OFF" });
                telemetry(&publisher, &mut cpu);
                let elapsed = report_started.elapsed().as_secs_f64();
                if elapsed >= 5.0 {
                    for channel in 0..2 {
                        let name = if channel == 0 { "main" } else { "sub" };
                        let _ = publisher.state(
                            &format!("{name}_fps"),
                            format!("{:.2}", counts[channel] as f64 / elapsed),
                        );
                        let _ = publisher.state(
                            &format!("{name}_bitrate"),
                            format!("{:.2}", bytes[channel] as f64 * 8.0 / elapsed / 1000.0),
                        );
                    }
                    counts = [0, 0];
                    bytes = [0, 0];
                    report_started = now;
                }
                sequence = sequence.wrapping_add(1);
                write_health(
                    started,
                    &boot_id,
                    sequence,
                    healthy,
                    total_frames,
                    audio_frames,
                )?;
                next_report = now + Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    })();
    sequence = sequence.wrapping_add(1);
    let settings_result =
        if outcome.is_ok() && progress.healthy() && progress.media_fresh(cfg.runtime.audio) {
            settings.flush()
        } else {
            Ok(())
        };
    let final_health = write_health(
        started,
        &boot_id,
        sequence,
        false,
        total_frames,
        audio_frames,
    );
    let [hardware_result, rtsp_result, mqtt_result] = workers.finish();
    let _ = publisher.state("pipeline_health", "OFF");
    if let Err(error) = hardware_result {
        let _retained = std::mem::ManuallyDrop::new(media);
        return Err(io::Error::other(format!(
            "hardware shutdown failed: {error}; SDK retained; clean boot required"
        )));
    }
    let media_result = media.close();
    let watchdog_result = watcher.as_mut().map_or(Ok(()), Watcher::finish);
    let errors = [
        ("runtime", outcome),
        ("settings persistence", settings_result),
        ("RTSP", rtsp_result),
        ("MQTT", mqtt_result),
        ("media cleanup", media_result),
        ("watchdog", watchdog_result),
        ("final health", final_health),
    ]
    .into_iter()
    .filter_map(|(name, result)| result.err().map(|error| format!("{name}: {error}")))
    .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(io::Error::other(errors.join("; ")));
    }
    if reboot {
        #[cfg(all(target_os = "linux", target_arch = "mips"))]
        {
            let status = std::process::Command::new("/sbin/reboot")
                .arg("-f")
                .status()?;
            if !status.success() {
                return Err(io::Error::other("reboot request failed"));
            }
        }
        #[cfg(not(all(target_os = "linux", target_arch = "mips")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "reboot requires the camera target",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;

    struct PrivateGraph(PathBuf);
    impl PrivateGraph {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let parent = std::env::temp_dir().canonicalize().unwrap();
            for _ in 0..32 {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!("mi360-graph-wiring-{}-{id}", std::process::id()));
                match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("private graph fixture: {error}"),
                }
            }
            panic!("no exclusive private fixture directory");
        }
        fn load(&self) -> GraphSettings {
            // SAFETY: geteuid takes no arguments and only returns process identity.
            let uid = unsafe { libc::geteuid() };
            GraphSettings::load(self.0.join("video-settings.json"), uid).unwrap()
        }
    }
    impl Drop for PrivateGraph {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn cold_command_slots_are_finite_and_disabled_owner_never_requests_reboot() {
        assert_eq!(
            graph_command(100, 0).unwrap(),
            Some(GraphCommand::Stage("main_resolution", "1920x1080"))
        );
        assert_eq!(
            graph_command(101, 1).unwrap(),
            Some(GraphCommand::Stage("sub_resolution", "320x180"))
        );
        assert_eq!(
            graph_command(102, 1).unwrap(),
            Some(GraphCommand::Stage("main_profile", "main"))
        );
        assert_eq!(
            graph_command(103, 2).unwrap(),
            Some(GraphCommand::Stage("sub_profile", "high"))
        );
        for (slot, value) in [
            (100, -1),
            (100, 3),
            (101, 2),
            (102, 3),
            (103, i32::MAX),
            (110, 1),
            (111, -1),
        ] {
            assert!(graph_command(slot, value).is_err());
        }
        assert_eq!(request_graph(None, 7, 200).unwrap(), None);
        for slot in [100, 101, 102, 103, 110, 111] {
            assert_eq!(
                request_graph(None, slot, 0).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        }
    }
    #[test]
    fn apply_requires_a_durable_trial_and_never_replaces_the_running_spec() {
        let fixture = PrivateGraph::new();
        let mut graph = fixture.load();
        let live = graph.select_for_boot().unwrap();
        assert!(request_graph(Some(&mut graph), APPLY_VIDEO, 0).is_err());
        assert_eq!(
            request_graph(Some(&mut graph), 100, 1).unwrap(),
            Some(false)
        );
        assert_eq!(graph.values().main_width, 1280);
        assert_eq!(graph.confirmed(), live);
        assert_eq!(
            request_graph(Some(&mut graph), APPLY_VIDEO, 0).unwrap(),
            Some(true)
        );
        assert_eq!(graph.status(), graph_settings::Status::Armed);
        assert_eq!(graph.select_for_boot().unwrap(), live);
        assert_eq!(
            request_graph(Some(&mut graph), DISCARD_VIDEO, 0).unwrap(),
            Some(false)
        );
        assert_eq!(graph.staged(), None);
        request_graph(Some(&mut graph), 102, 2).unwrap();
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(request_graph(Some(&mut graph), APPLY_VIDEO, 0).is_err());
        assert_eq!(graph.status(), graph_settings::Status::StorageError);
    }
    #[test]
    fn graph_catalog_separates_staged_intent_from_readonly_actual_sps() {
        let entities = graph_entities();
        let slots = entities
            .iter()
            .filter_map(|entity| entity.control_slot)
            .collect::<Vec<_>>();
        assert_eq!(slots, vec![100, 101, 102, 103, 110, 111]);
        for item in entities
            .iter()
            .filter(|item| item.domain == mqtt::Domain::Sensor)
        {
            assert!(item.control_slot.is_none() && item.unit.is_none());
        }
        assert!(encoded_graph_updates([None, None]).is_empty());
        let samples = encoded_graph_updates([
            Some(VideoMetadata {
                width: 1280,
                height: 720,
                profile_idc: 77,
            }),
            None,
        ]);
        assert_eq!(
            samples,
            vec![
                ("main_resolution", "1280x720".into()),
                ("main_profile", "main".into())
            ]
        );
        assert!(!samples.iter().any(|(key, _)| key.starts_with("sub_")));
        assert!(
            pending_graph_updates(RustVideoSpec::default())
                .iter()
                .all(|(key, _)| key.ends_with("_pending"))
        );
    }
    #[test]
    fn isp_statistics_discover_only_real_success_and_report_read_failures_unknown() {
        let mut seen = vec![];
        let mut statistics = IspStatistics::discover(|control| {
            seen.push(control);
            if control == 50 {
                Ok(118)
            } else {
                Err(io::Error::other("isolated getter fault"))
            }
        });
        assert_eq!(seen, vec![50, 51]);
        let entities = statistics.entities();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, "ae_luma");
        assert!(entities[0].control_slot.is_none());
        assert_eq!(statistics.take_updates(), vec![("ae_luma", "118".into())]);
        statistics.sample(|control| {
            assert_eq!(control, 50);
            Err(io::Error::other("isolated failure"))
        });
        assert_eq!(
            statistics.take_updates(),
            vec![("ae_luma", "unknown".into())]
        );
        statistics.sample(|control| {
            assert_eq!(control, 50);
            Ok(123)
        });
        assert_eq!(statistics.take_updates(), vec![("ae_luma", "123".into())]);
        let mut exposure = IspStatistics::discover(|control| {
            if control == 51 {
                Ok(40000)
            } else {
                Err(io::Error::other("missing luma"))
            }
        });
        assert_eq!(exposure.entities()[0].unit.as_deref(), Some("µs"));
        assert_eq!(
            exposure.take_updates(),
            vec![("exposure_us", "40000".into())]
        );
    }
    #[test]
    fn motor_stop_cancels_motion_but_shutdown_also_cancels_filter_work() {
        let controls = Controls::new();
        let motor = controls.operation(true);
        controls.cancel(true);
        assert!(motor.is_cancelled());
        controls.finished();
        let filter = controls.operation(false);
        controls.cancel(true);
        assert!(!filter.is_cancelled());
        controls.cancel(false);
        assert!(filter.is_cancelled());
    }
    #[test]
    fn early_owner_drop_stops_and_joins_an_already_started_worker() {
        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_finished = finished.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            thread_finished.store(true, Ordering::Release);
            Ok(())
        });
        let workers = Workers {
            stop,
            controls: Arc::new(Controls::new()),
            hardware: Some(thread),
            rtsp: None,
            mqtt: None,
        };
        drop(workers);
        assert!(finished.load(Ordering::Acquire));
    }
    #[test]
    fn startup_grace_is_bounded_and_cannot_mask_a_qualified_pipeline_stall() {
        assert!(feed_allowed(false, Duration::from_secs(89), false, false));
        assert!(!feed_allowed(false, Duration::from_secs(90), true, true));
        assert!(!feed_allowed(true, Duration::from_secs(1), true, false));
        assert!(!feed_allowed(true, Duration::from_secs(1), false, true));
        assert!(feed_allowed(true, Duration::from_secs(100), true, true));
    }
    fn test_status() -> board::MotorStatus {
        board::MotorStatus {
            position: board::Position { x: 600, y: 300 },
            moving: false,
            speed: 200,
            owner_open: false,
            max_x: board::PAN_TRAVEL,
            max_y: board::TILT_TRAVEL,
        }
    }
    fn work(command: mqtt::Command, received: Instant, epoch: u32) -> Work {
        Work {
            command,
            received,
            epoch,
        }
    }
    fn plan(kind: MotionKind, received: Instant, epoch: u32, target: board::Position) -> Plan {
        Plan {
            preset: None,
            approach_target: None,
            kind,
            received,
            epoch,
            target,
            until: received + Duration::from_secs(2),
        }
    }
    fn running(plan: Plan) -> Running {
        Running {
            plan,
            cancel: board::Cancellation::default(),
            braking: false,
            max_speed: 900,
        }
    }
    #[test]
    fn diagonal_holds_transform_both_axes_and_keep_a_single_hard_lease() {
        let now = Instant::now();
        for mask in 0..4 {
            for direction in [
                mqtt::Direction::UpLeft,
                mqtt::Direction::UpRight,
                mqtt::Direction::DownLeft,
                mqtt::Direction::DownRight,
            ] {
                let (pan, tilt) = direction.axes();
                let axes = screen_axes(None, mask).unwrap();
                let plan = motion_plan(
                    &work(mqtt::Command::Move(direction), now, 7),
                    test_status(),
                    &Motion::default(),
                    None,
                    None,
                    mask,
                    (now, now),
                )
                .unwrap();
                assert_eq!(
                    plan.target.x,
                    if pan * axes.right > 0 {
                        board::PAN_TRAVEL
                    } else {
                        0
                    }
                );
                assert_eq!(
                    plan.target.y,
                    if tilt * axes.up > 0 {
                        board::TILT_TRAVEL
                    } else {
                        0
                    }
                );
                assert_eq!(plan.until, now + Duration::from_secs(2));
                assert_eq!(
                    start_target(plan, board::Position { x: 900, y: 400 }),
                    plan.target
                );
            }
        }
    }
    #[test]
    fn preset_second_stage_keeps_original_deadline_epoch_and_release_cancels_it() {
        let now = Instant::now();
        let mut plan = plan(
            MotionKind::Fixed,
            now,
            7,
            board::Position { x: 1100, y: 300 },
        );
        plan.until = now + Duration::from_secs(30);
        plan.approach_target = Some(board::Position { x: 1202, y: 356 });
        let until = plan.until;
        let later = now + Duration::from_secs(5);
        let next = plan.next_stage(later).unwrap();
        assert_eq!(next.target, board::Position { x: 1202, y: 356 });
        assert_eq!(next.received, later);
        assert_eq!(next.until, until);
        assert_eq!(next.epoch, 7);
        assert!(next.next_stage(later).is_none());
        let mut motion = Motion {
            active: None,
            pending: Some(next),
        };
        assert!(!motion.release(8, later));
        assert!(motion.pending.is_none());
    }

    #[test]
    fn compatible_vectors_preserve_motion_and_release_can_be_resumed_but_reversal_cannot() {
        let now = Instant::now();
        let left = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Left)),
            now,
            1,
            board::Position { x: 0, y: 300 },
        );
        let mut m = Motion {
            active: Some(running(left)),
            pending: None,
        };
        let diagonal = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::UpLeft)),
            now,
            1,
            board::Position { x: 0, y: 0 },
        );
        assert_eq!(m.compatible_hold(diagonal), Some([-100, 100]));
        let opposite = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Right)),
            now,
            1,
            board::Position {
                x: board::PAN_TRAVEL,
                y: 300,
            },
        );
        assert_eq!(m.compatible_hold(opposite), None);
        m.release(2, now);
        let mut resumed = left;
        resumed.epoch = 2;
        assert_eq!(m.compatible_hold(resumed), Some([-100, 0]));
        m.active.as_ref().unwrap().cancel.cancel();
        assert_eq!(m.compatible_hold(resumed), None);
        let v = camera_transport::motion_control::Velocity {
            pan: 50,
            tilt: 50,
            zoom: 0,
        };
        let kind = MotionKind::Hold(v);
        assert_eq!(kind.ceiling(386), 193);
        let p = motion_plan(
            &work(mqtt::Command::Velocity(v), now, 0),
            test_status(),
            &Motion::default(),
            None,
            None,
            0,
            (now, now),
        )
        .unwrap();
        assert_eq!(p.until, now + Duration::from_secs(2));
    }

    #[test]
    fn measured_fallback_arrows_and_relative_steps_follow_each_logical_flag() {
        let start = Instant::now();
        let current = test_status();
        let motion = Motion::default();
        for mask in 0..4 {
            let right = if mask & 1 == 0 { board::PAN_TRAVEL } else { 0 };
            let up = if mask & 2 == 0 { 0 } else { board::TILT_TRAVEL };
            for (command, want) in [
                (
                    mqtt::Command::Move(mqtt::Direction::Right),
                    board::Position { x: right, y: 300 },
                ),
                (
                    mqtt::Command::Move(mqtt::Direction::Up),
                    board::Position { x: 600, y: up },
                ),
                (
                    mqtt::Command::Pan(mqtt::Position::Relative(20)),
                    board::Position {
                        x: 600 + if mask & 1 == 0 { 20 } else { -20 },
                        y: 300,
                    },
                ),
                (
                    mqtt::Command::Tilt(mqtt::Position::Relative(20)),
                    board::Position {
                        x: 600,
                        y: 300 + if mask & 2 == 0 { -20 } else { 20 },
                    },
                ),
            ] {
                let p = motion_plan(
                    &work(command, start, 0),
                    current,
                    &motion,
                    None,
                    None,
                    mask,
                    (start, start),
                )
                .unwrap();
                assert_eq!(p.target, want);
            }
            let p = motion_plan(
                &work(mqtt::Command::Pan(mqtt::Position::Absolute(1000)), start, 0),
                current,
                &motion,
                None,
                None,
                mask,
                (start, start),
            )
            .unwrap();
            assert_eq!(p.target, board::Position { x: 1000, y: 300 }); // Physical slider never mirrors.
        }
    }
    #[test]
    fn unavailable_orientation_disables_even_absolute_motion_and_invalid_targets_do_not_clamp() {
        let start = Instant::now();
        let current = test_status();
        let motion = Motion::default();
        assert!(
            motion_plan(
                &work(mqtt::Command::Pan(mqtt::Position::Absolute(1000)), start, 0),
                current,
                &motion,
                None,
                None,
                4,
                (start, start)
            )
            .is_err()
        );
        for value in [-1, 2405, i32::MAX] {
            assert!(
                motion_plan(
                    &work(
                        mqtt::Command::Pan(mqtt::Position::Absolute(value)),
                        start,
                        0
                    ),
                    current,
                    &motion,
                    None,
                    None,
                    0,
                    (start, start)
                )
                .is_err()
            );
        }
        assert!(
            motion_plan(
                &work(
                    mqtt::Command::Tilt(mqtt::Position::Relative(i32::MIN)),
                    start,
                    0
                ),
                current,
                &motion,
                None,
                None,
                0,
                (start, start)
            )
            .is_err()
        );
    }
    #[test]
    fn hold_refresh_keeps_one_rail_target_and_retained_cancellation() {
        let now = Instant::now();
        let first = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Right)),
            now,
            7,
            board::Position {
                x: board::PAN_TRAVEL,
                y: 300,
            },
        );
        let mut motion = Motion {
            active: Some(running(first)),
            pending: None,
        };
        let token = motion.active.as_ref().unwrap().cancel.clone();
        let refresh = plan(
            first.kind,
            now + Duration::from_millis(700),
            7,
            board::Position {
                x: board::PAN_TRAVEL,
                y: 301,
            },
        );
        assert!(!motion.queue(refresh));
        let active = motion.active.as_ref().unwrap();
        assert_eq!(active.plan.target, first.target);
        assert_eq!(active.plan.until, refresh.until);
        assert!(!active.braking);
        assert!(motion.pending.is_none());
        active.cancel.cancel();
        assert!(token.is_cancelled());
        // Expired/cancelled motion cannot silently extend its old token.
        assert!(motion.queue(plan(
            first.kind,
            now + Duration::from_millis(800),
            7,
            first.target
        )));
        assert!(motion.pending.is_some());
    }
    #[test]
    fn hard_lease_cancels_even_during_braking_and_new_axis_holds_use_stopped_other_axis() {
        let now = Instant::now();
        let old = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Right)),
            now,
            0,
            board::Position {
                x: board::PAN_TRAVEL,
                y: 300,
            },
        );
        let mut motion = Motion {
            active: Some(running(old)),
            pending: None,
        };
        motion.active.as_mut().unwrap().braking = true;
        motion.expire(now + Duration::from_millis(1999));
        assert!(!motion.active.as_ref().unwrap().cancel.is_cancelled());
        motion.expire(now + Duration::from_secs(2));
        assert!(motion.active.as_ref().unwrap().cancel.is_cancelled());
        let vertical = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Up)),
            now,
            0,
            board::Position { x: 600, y: 0 },
        );
        assert_eq!(
            start_target(vertical, board::Position { x: 750, y: 280 }),
            board::Position { x: 750, y: 0 }
        );
        assert_eq!(
            start_target(old, board::Position { x: 750, y: 280 }),
            board::Position {
                x: board::PAN_TRAVEL,
                y: 280
            }
        );
        let fixed = plan(
            MotionKind::Fixed,
            now,
            0,
            board::Position { x: 600, y: 300 },
        );
        assert_eq!(
            start_target(fixed, board::Position { x: 750, y: 280 }),
            fixed.target
        );
    }
    #[test]
    fn retarget_brakes_once_and_only_the_latest_fixed_target_survives() {
        let now = Instant::now();
        let old = plan(
            MotionKind::Fixed,
            now,
            1,
            board::Position { x: 1000, y: 400 },
        );
        let mut motion = Motion {
            active: Some(running(old)),
            pending: None,
        };
        let next = plan(
            MotionKind::Fixed,
            now,
            1,
            board::Position { x: 1200, y: 500 },
        );
        assert!(motion.queue(next));
        let latest = plan(
            MotionKind::Fixed,
            now,
            1,
            board::Position { x: 1400, y: 600 },
        );
        assert!(!motion.queue(latest));
        assert_eq!(motion.pending.unwrap().target, latest.target);
        assert_eq!(motion.active.as_ref().unwrap().plan.target, old.target);
        assert!(!motion.active.as_ref().unwrap().cancel.is_cancelled());
    }
    #[test]
    fn release_advances_epoch_without_cancelling_and_preserves_only_post_release_work() {
        let controls = Controls::new();
        let token = controls.operation(true);
        controls.release();
        assert!(!token.is_cancelled());
        assert!(!controls.stop.load(Ordering::Acquire));
        let epoch = controls.epoch.load(Ordering::Acquire);
        assert_eq!(controls.release_epoch.load(Ordering::Acquire), epoch);
        let now = Instant::now();
        let old = plan(
            MotionKind::Hold(legacy_velocity(mqtt::Direction::Right)),
            now,
            epoch - 1,
            board::Position {
                x: board::PAN_TRAVEL,
                y: 300,
            },
        );
        let mut motion = Motion {
            active: Some(running(old)),
            pending: Some(old),
        };
        assert!(motion.release(epoch, now));
        assert!(motion.pending.is_none());
        assert!(!motion.active.as_ref().unwrap().cancel.is_cancelled());
        assert!(!motion.release(epoch, now + Duration::from_secs(1))); // Never extends braking.
        motion.expire(now + Duration::from_secs(3));
        assert!(!motion.active.as_ref().unwrap().cancel.is_cancelled());
        motion.expire(now + Duration::from_millis(3500));
        assert!(motion.active.as_ref().unwrap().cancel.is_cancelled());
        let fresh = plan(
            MotionKind::Fixed,
            now,
            epoch,
            board::Position { x: 1000, y: 300 },
        );
        motion.pending = Some(fresh);
        assert!(!motion.release(epoch, now));
        assert_eq!(motion.pending.unwrap().epoch, epoch);
        controls.cancel(true);
        controls.release();
        assert!(token.is_cancelled());
        assert!(controls.stop.load(Ordering::Acquire)); // Release cannot downgrade Stop.
    }
    #[test]
    fn epochs_expiry_and_orientation_readback_revoke_only_motion_work() {
        let now = Instant::now();
        let controls = Controls::new();
        refresh_orientation(&controls, Ok(3));
        assert_eq!(controls.orientation.load(Ordering::Acquire), 0);
        controls.stop.store(false, Ordering::Release);
        let epoch = controls.epoch.load(Ordering::Acquire);
        let cancel = controls.operation(true);
        let old = work(mqtt::Command::Move(mqtt::Direction::Up), now, epoch);
        refresh_orientation(&controls, Ok(3));
        assert!(!cancel.is_cancelled());
        refresh_orientation(&controls, Ok(2));
        assert!(cancel.is_cancelled());
        assert_eq!(controls.orientation.load(Ordering::Acquire), 1);
        let new = controls.epoch.load(Ordering::Acquire);
        assert!(!fresh(&old, new, now));
        assert!(fresh(&work(mqtt::Command::Led(true), now, epoch), new, now));
        assert!(fresh(
            &work(old.command, now, new),
            new,
            now + Duration::from_secs(2)
        ));
        assert!(!fresh(
            &work(old.command, now, new),
            new,
            now + Duration::from_millis(2001)
        ));
        assert!(!fresh(
            &work(old.command, now + Duration::from_millis(1), new),
            new,
            now
        ));
        refresh_orientation(&controls, Err(io::Error::other("isolated getter failure")));
        assert_eq!(controls.orientation.load(Ordering::Acquire), 4);
    }
    #[test]
    fn point_route_needs_calibration_and_uses_arrival_history_through_runtime_adapter() {
        use camera_transport::position3d::{Operation, Request};
        let start = Instant::now();
        let mut current = test_status();
        current.max_y = 2000;
        let request = Request {
            start: (255, 255),
            end: (255, 255),
            operation: Operation::ZoomTo,
        };
        let w = work(
            mqtt::Command::Position3d(request),
            start + Duration::from_millis(1250),
            0,
        );
        assert!(
            motion_plan(
                &w,
                current,
                &Motion::default(),
                None,
                None,
                0,
                (start, start + Duration::from_millis(1500))
            )
            .is_err()
        );
        let c = pointing::Calibration {
            pan_a: 1000.0,
            pan_b: 0.0,
            tilt_a: 1000.0,
            tilt_b: 0.0,
            pan_right_grows: true,
            tilt_up_grows: false,
            display_delay_ms: 500,
        };
        let mut pointing = pointing::Pointing::new(c).unwrap();
        pointing
            .observe(Duration::ZERO, board::Position { x: 500, y: 500 })
            .unwrap();
        pointing
            .observe(Duration::from_secs(1), board::Position { x: 700, y: 600 })
            .unwrap();
        let p = motion_plan(
            &w,
            current,
            &Motion::default(),
            Some(c),
            Some(&pointing),
            0,
            (start, start + Duration::from_millis(1500)),
        )
        .unwrap();
        assert_eq!(p.target, board::Position { x: 1150, y: 294 });
    }
    #[test]
    fn slider_pair_retains_latest_other_axis_target_and_profile_preserves_user_ceiling() {
        let now = Instant::now();
        let current = test_status();
        let first = plan(
            MotionKind::Fixed,
            now,
            0,
            board::Position { x: 1000, y: 500 },
        );
        let motion = Motion {
            active: None,
            pending: Some(first),
        };
        let p = motion_plan(
            &work(mqtt::Command::Pan(mqtt::Position::Absolute(1500)), now, 0),
            current,
            &motion,
            None,
            None,
            3,
            (now, now),
        )
        .unwrap();
        assert_eq!(p.target, board::Position { x: 1500, y: 500 });
        let timings = Controls::new().timings();
        let p = timings.profile(900).unwrap();
        assert_eq!(
            (
                p.start_speed,
                p.max_speed,
                p.end_speed,
                p.acceleration,
                p.deceleration
            ),
            (100, 900, 100, 3200, 5334)
        );
        assert!(timings.profile(99).is_err());
        assert!(timings.profile(901).is_err());
    }
    #[test]
    fn telemetry_parsers_keep_missing_and_invalid_signals_unknown() {
        let mut cpu = Cpu::default();
        assert!(cpu.sample("cpu 10 0 10 80 0 0 0 0\n").is_none());
        assert_eq!(cpu.sample("cpu 20 0 30 150 0 0 0 0\n"), Some(30.0));
        assert_eq!(proc_kib("MemFree: 123 kB\n", "MemAvailable:"), None);
        assert_eq!(
            proc_kib("MemAvailable: 1024 kB\n", "MemAvailable:"),
            Some(1024)
        );
        assert_eq!(wifi_signal("wlan0: 0000 60. -55. -256 0\n"), Some(-55.0));
        assert_eq!(wifi_signal("wlan0: 0000 60. 55. 0 0\n"), None);
    }
}
