//! Motion request policy and trajectory transitions.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MotionKind {
    Fixed,
    Hold(camera_transport::motion_control::Velocity),
}
pub(super) fn legacy_velocity(
    direction: mqtt::Direction,
) -> camera_transport::motion_control::Velocity {
    let (pan, tilt) = direction.axes();
    camera_transport::motion_control::Velocity {
        pan: (pan * 100) as i16,
        tilt: (tilt * 100) as i16,
        zoom: 0,
    }
}
impl MotionKind {
    pub(super) fn hold_axes(self) -> Option<[i32; 2]> {
        match self {
            Self::Fixed => None,
            Self::Hold(v) => Some([i32::from(v.pan), i32::from(v.tilt)]),
        }
    }
    pub(super) fn ceiling(self, configured: u32) -> u32 {
        match self {
            Self::Hold(v) => {
                (configured * u32::from(v.pan.unsigned_abs().max(v.tilt.unsigned_abs())) / 100)
                    .max(100)
            }
            _ => configured,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Plan {
    pub(super) target: board::Position,
    pub(super) preset: Option<camera_transport::motion_control::PresetName>,
    pub(super) approach_target: Option<board::Position>,
    pub(super) kind: MotionKind,
    pub(super) received: Instant,
    pub(super) until: Instant,
    pub(super) epoch: u32,
}
impl Plan {
    pub(super) fn next_stage(mut self, now: Instant) -> Option<Self> {
        if now >= self.until || now < self.received {
            return None;
        }
        self.target = self.approach_target.take()?;
        self.received = now;
        Some(self)
    }
}
pub(super) struct Running {
    pub(super) plan: Plan,
    pub(super) cancel: board::Cancellation,
    pub(super) braking: bool,
    pub(super) max_speed: u32,
}
impl Running {
    pub(super) fn brake(&mut self, now: Instant) {
        self.braking = true;
        // An explicit release/retarget permits the configured (up to 3s)
        // deceleration. This bounded deadline is never renewed while braking.
        // Silent loss of an active hold still uses its original hard 2s lease.
        self.plan.until = now + Duration::from_millis(3500);
    }
}
#[derive(Default)]
pub(super) struct Motion {
    pub(super) active: Option<Running>,
    pub(super) pending: Option<Plan>,
}
impl Motion {
    pub(super) fn compatible_hold(&self, plan: Plan) -> Option<[i32; 2]> {
        let active = self.active.as_ref()?;
        let [ox, oy] = active.plan.kind.hold_axes()?;
        let [nx, ny] = plan.kind.hold_axes()?;
        (!active.cancel.is_cancelled()
            && (active.plan.epoch == plan.epoch || active.braking)
            && plan.received >= active.plan.received
            && ox * nx >= 0
            && oy * ny >= 0
            && (active.plan.kind != plan.kind || active.braking))
            .then_some([nx, ny])
    }
    /// Returns true only when a newly superseded active target needs braking.
    pub(super) fn queue(&mut self, plan: Plan) -> bool {
        if let Some(active) = &mut self.active {
            if plan.kind.hold_axes().is_some()
                && active.plan.kind == plan.kind
                && active.plan.epoch == plan.epoch
                && !active.braking
                && !active.cancel.is_cancelled()
                && plan.received >= active.plan.received
            {
                // One rail target remains in the kernel. Refresh only its lease.
                active.plan.until = plan.until;
                active.plan.received = plan.received;
                self.pending = None;
                return false;
            }
            self.pending = Some(plan); // Latest target wins while braking.
            if !active.braking {
                active.brake(plan.received);
                return true;
            }
        } else {
            self.pending = Some(plan);
        }
        false
    }
    pub(super) fn release(&mut self, epoch: u32, now: Instant) -> bool {
        // Fresh work accepted after this release must survive its observation.
        if self.pending.is_some_and(|p| p.epoch != epoch) {
            self.pending = None;
        }
        if let Some(active) = &mut self.active
            && active.plan.epoch != epoch
            && !active.braking
        {
            active.brake(now);
            return true;
        }
        false
    }
    pub(super) fn expire(&self, now: Instant) {
        if let Some(active) = &self.active
            && now >= active.plan.until
        {
            active.cancel.cancel();
        }
    }
    pub(super) fn fixed_target(&self, current: board::Position) -> board::Position {
        self.pending
            .filter(|p| p.kind == MotionKind::Fixed)
            .map(|p| p.target)
            .or_else(|| {
                self.active
                    .as_ref()
                    .filter(|a| a.plan.kind == MotionKind::Fixed)
                    .map(|a| a.plan.target)
            })
            .unwrap_or(current)
    }
}
pub(super) fn refresh_orientation(controls: &Controls, actual_raw: io::Result<i32>) {
    let logical = match actual_raw {
        Ok(raw @ 0..=3) => (raw ^ 3) as u32,
        _ => 4,
    };
    if controls.orientation.swap(logical, Ordering::AcqRel) != logical {
        // Even failed readback revokes queued work and an active trajectory.
        controls.cancel(true);
    }
}
pub(super) fn screen_axes(
    calibration: Option<pointing::Calibration>,
    mask: u32,
) -> io::Result<pointing::Axes> {
    let mask = u8::try_from(mask)
        .ok()
        .filter(|m| *m <= 3)
        .ok_or_else(|| io::Error::other("actual image orientation unavailable"))?;
    match calibration {
        Some(calibration) => calibration.screen_axes(mask),
        // Signs measured on this Mi360 with logical Mirror/Flip off. These are
        // board direction facts, not invented optical coefficients.
        None => Ok(pointing::Axes {
            right: if mask & 1 == 0 { 1 } else { -1 },
            up: if mask & 2 == 0 { -1 } else { 1 },
        }),
    }
}
pub(super) fn is_motion(command: Command) -> bool {
    matches!(
        command,
        Command::App(App::Ptz(Ptz::Pan(_)))
            | Command::App(App::Ptz(Ptz::Tilt(_)))
            | Command::App(App::Ptz(Ptz::Move(_)))
            | Command::App(App::Ptz(Ptz::Velocity(_)))
            | Command::App(App::Ptz(Ptz::Home))
            | Command::App(App::Ptz(Ptz::Preset(_)))
            | Command::App(App::Ptz(Ptz::Position3d(_)))
            | Command::Release
            | Command::Stop
    )
}
pub(super) fn fresh(work: &Work, epoch: u32, now: Instant) -> bool {
    (!is_motion(work.command) || work.epoch == epoch)
        && now
            .checked_duration_since(work.received)
            .is_some_and(|age| age <= pointing::MAX_COMMAND_AGE)
}
pub(super) fn motion_plan(
    work: &Work,
    current: board::MotorStatus,
    motion: &Motion,
    calibration: Option<pointing::Calibration>,
    pointing: Option<&pointing::Pointing>,
    mask: u32,
    clock: (Instant, Instant),
) -> io::Result<Plan> {
    let (started, now) = clock;
    let axes = screen_axes(calibration, mask)?;
    let mut target = motion.fixed_target(current.position);
    let kind = match work.command {
        Command::App(App::Ptz(Ptz::Pan(position)))
        | Command::App(App::Ptz(Ptz::Tilt(position))) => {
            let pan = matches!(work.command, Command::App(App::Ptz(Ptz::Pan(_))));
            let axis = if pan { &mut target.x } else { &mut target.y };
            *axis = match position {
                mqtt::Position::Absolute(value) => value, // Physical slider.
                mqtt::Position::Relative(delta) => {
                    let sign = if pan { axes.right } else { axes.up };
                    let at = if pan {
                        current.position.x
                    } else {
                        current.position.y
                    };
                    delta
                        .checked_mul(sign)
                        .and_then(|delta| at.checked_add(delta))
                        .ok_or_else(|| io::Error::other("motor target overflow"))?
                }
            };
            MotionKind::Fixed
        }
        Command::App(App::Ptz(Ptz::Move(_))) | Command::App(App::Ptz(Ptz::Velocity(_))) => {
            target = current.position;
            let (pan, tilt, kind) = match work.command {
                Command::App(App::Ptz(Ptz::Move(d))) => {
                    let (x, y) = d.axes();
                    (x, y, MotionKind::Hold(legacy_velocity(d)))
                }
                Command::App(App::Ptz(Ptz::Velocity(v))) => {
                    camera_transport::motion_control::Capabilities {
                        zoom: false,
                        coupled_pan_tilt: true,
                    }
                    .validate(v)
                    .map_err(io::Error::other)?;
                    (
                        i32::from(v.pan.signum()),
                        i32::from(v.tilt.signum()),
                        MotionKind::Hold(v),
                    )
                }
                _ => unreachable!(),
            };
            for (axis, sign, max) in [
                (&mut target.x, pan * axes.right, current.max_x),
                (&mut target.y, tilt * axes.up, current.max_y),
            ] {
                if sign != 0 {
                    *axis = if sign > 0 { max } else { 0 };
                }
            }
            kind
        }
        Command::App(App::Ptz(Ptz::Position3d(request))) => {
            target = pointing
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "point calibration unavailable")
                })?
                .resolve(
                    request,
                    work.received
                        .checked_duration_since(started)
                        .ok_or_else(|| io::Error::other("point predates runtime"))?,
                    now.duration_since(started),
                    mask as u8,
                    board::Position {
                        x: current.max_x,
                        y: current.max_y,
                    },
                )?;
            MotionKind::Fixed
        }
        _ => return Err(io::Error::other("not a motion target")),
    };
    if !(0..=current.max_x).contains(&target.x) || !(0..=current.max_y).contains(&target.y) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target exceeds qualified motor travel",
        ));
    }
    Ok(Plan {
        target,
        preset: None,
        approach_target: None,
        kind,
        received: work.received,
        epoch: work.epoch,
        until: work.received
            + if kind.hold_axes().is_some() {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(30)
            },
    })
}
pub(super) fn start_target(plan: Plan, current: board::Position) -> board::Position {
    let mut target = plan.target;
    // A changed direction may wait for the old axis to brake. The new hold must
    // leave that axis at its measured stopped position, never drive it backward
    // to the position sampled before braking began.
    match plan.kind {
        kind if kind.hold_axes().is_some() => {
            let [pan, tilt] = kind.hold_axes().unwrap();
            if pan == 0 {
                target.x = current.x;
            }
            if tilt == 0 {
                target.y = current.y;
            }
        }
        _ => {}
    }
    target
}
pub(super) fn urgent_motion(
    board: &mut board::Board,
    controls: &Controls,
    motion: &mut Motion,
    seen_release: &mut u32,
) -> io::Result<()> {
    if controls.stop.swap(false, Ordering::AcqRel) {
        // Stop is immediate. Board cleanup verifies stop and restores the prior
        // driver speed before the retained cancellation token can be removed.
        board.stop()?;
        motion.active = None;
        motion.pending = motion
            .pending
            .filter(|p| p.epoch == controls.epoch.load(Ordering::Acquire));
        controls.finished();
    }
    let epoch = controls.release_epoch.load(Ordering::Acquire);
    if epoch != *seen_release {
        *seen_release = epoch;
        if motion.release(epoch, Instant::now()) {
            board.request_brake()?;
        }
    }
    Ok(())
}
