//! Pan/tilt driver for the MS32006 stepper controller (register map: `docs/hardware/ptz.md`).
//!
//! Every register access goes through [`Bus`], so the sequencing (init, chunked moves, homing, absolute moves)
//! is unit-tested on the host against [`fake::FakeBus`]; the real bus is [`i2c::I2cBus`].

use std::time::Duration;

pub mod fake;
pub mod homing;
pub mod i2c;
mod motion;
pub(crate) mod vendor_presets;
pub use homing::Listener;
pub use motion::Profile;

/// How a home was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Homing {
    /// Homed with the microphone listening. `heard[axis.index()]`: that axis was halted when it was heard
    /// hitting its stop; otherwise it ran its full travel, as a timed home does.
    ByEar { heard: [bool; 2] },
    /// Axes not homed by ear were driven a full travel by time; the reason the microphone failed, if a
    /// listener was present. Axes already homed by ear before it failed are not driven again.
    Timed(Option<String>),
}

/// What homing one axis by ear came to.
enum ByEar {
    Heard,
    FullTravel,
    /// No usable microphone; the axis may be anywhere on its way.
    Unusable(String),
}

/// 7-bit I2C address of the MS32006 on `/dev/i2c-2`.
pub const ADDR: u16 = 0x10;
/// Largest step count one move command can carry (12 bits).
pub const MAX_CHUNK: u32 = 0xfff;
/// Default stepping rate in pulses per second (what the vendor `device` programs); homing uses it.
pub const DEFAULT_PPS: u32 = 600;
/// Highest rate a user profile may ask for: 900 pps was the highest rate with no measurable drift
/// (10 round trips pos1↔pos2, ramp 300→900 over 1.5 s), less a 25 % margin (`docs/hardware/ptz.md`).
pub const MAX_PPS: u32 = 720;
/// Longest a hold chunk may stay busy past its expected travel before the hold is aborted.
const CHUNK_SLACK: Duration = Duration::from_secs(1);
/// Poll interval bounds while a hold chunk runs.
const HOLD_POLL_MIN: Duration = Duration::from_millis(2);
const HOLD_POLL_MAX: Duration = Duration::from_millis(20);
/// Pan travel in steps (`device` per-axis table, read from its memory at runtime).
pub const PAN_MAX: u32 = 7360;
/// Tilt travel in steps.
pub const TILT_MAX: u32 = 1240;
/// Extra steps driven into the end stop when homing so a start at the far end still arrives.
pub const HOME_MARGIN: u32 = 100;

const REG_CTRL: u8 = 0x00;
const REG_KICK: u8 = 0x09;
/// Clock the step-rate divisor is derived from: `divisor = (15_000_000 / pps) >> 7`.
const PPS_CLOCK: u32 = 15_000_000;
/// How much of the recorded motion a tap may look back into.
const HISTORY: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);
/// Listening to the quiet head before homing an axis: up to `QUIET_LISTENS` × `QUIET_LISTEN`, until
/// `QUIET_FRAMES` microphone frames (about 21 ms each) are in.
const QUIET_LISTEN: Duration = Duration::from_millis(50);
const QUIET_LISTENS: u32 = 10;
const QUIET_FRAMES: usize = 8;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("PTZ operation cancelled by stop")]
    Cancelled,
    #[error("i2c: {0}")]
    Bus(String),
    #[error("{0:?} still busy after the expected travel time")]
    Timeout(Axis),
    #[error("{steps} steps do not fit one move command (max {MAX_CHUNK})")]
    TooManySteps { steps: u32 },
    #[error("{axis:?} position {pos} is outside 0..={max}")]
    OutOfRange { axis: Axis, pos: u32, max: u32 },
    #[error("position unknown: home first")]
    NotHomed,
    #[error("{primary}; stop also failed: {stop}")]
    StopFailed {
        primary: Box<Error>,
        stop: Box<Error>,
    },
}

/// Register access to the MS32006 plus the delay the init sequence needs.
pub trait Bus {
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Error>;
    fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), Error>;
    fn sleep(&mut self, d: Duration);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Pan,
    Tilt,
}

impl Axis {
    const ALL: [Axis; 2] = [Axis::Pan, Axis::Tilt];

    fn index(self) -> usize {
        self as usize
    }

    /// First register of the axis bank (pan 0x00, tilt 0x04); the bank holds rate, step count and control.
    fn bank(self) -> u8 {
        4 * self as u8
    }

    /// Bit in `REG_CTRL` that halts the axis' pulse output.
    fn stop_bit(self) -> u8 {
        [0x04, 0x02][self.index()]
    }

    /// Bit in `REG_KICK` that starts the programmed move.
    fn kick_bit(self) -> u8 {
        [0x80, 0x40][self.index()]
    }

    /// Status block: busy flag + 12-bit step counter (the vendor reads the mirror at 0x8b/0x8d).
    fn status_reg(self) -> u8 {
        0x8b + 2 * self as u8
    }

    pub fn max(self) -> u32 {
        [PAN_MAX, TILT_MAX][self.index()]
    }

    /// Direction that increases the vendor coordinate (pan x grows with `Zero`, tilt y with `One`).
    fn positive(self) -> Dir {
        [Dir::Zero, Dir::One][self.index()]
    }
}

/// Value of the direction bit in the move command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Zero,
    One,
}

impl Dir {
    fn flip(self) -> Dir {
        match self {
            Dir::Zero => Dir::One,
            Dir::One => Dir::Zero,
        }
    }
}

/// Vendor coordinates in steps: pan x from the stop that `home` drives into, tilt y likewise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub pan: u32,
    pub tilt: u32,
}

/// Which way a hold moves along its axis, in vendor coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toward {
    /// Toward the travel limit (`Axis::max`).
    Max,
    /// Toward the home stop (0).
    Zero,
}

/// What [`Ptz::poll_hold`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldState {
    /// No hold and no chunk in flight.
    Idle,
    /// A chunk is running; poll again within this long.
    Moving(Duration),
    /// The hold just ended (reported once); the position is exact.
    Ended(HoldEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEnd {
    Released,
    Limit,
    Timeout,
}

#[derive(Debug, Clone, Copy)]
struct Hold {
    toward: Toward,
    profile: Profile,
    timeout: Duration,
    /// Last `begin_hold` for this direction: the timeout runs from here.
    renewed: Duration,
    /// Start of the ramp (the first chunk of this direction).
    ramp_from: Duration,
    released: bool,
}

/// A hold chunk issued but not yet counted into the position.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    axis: Axis,
    toward: Toward,
    steps: u32,
    ends: Duration,
}

pub struct Ptz<B: Bus> {
    bus: B,
    /// Rate last programmed per axis (0: not yet), so unchanged rates are not rewritten.
    pps: [u32; 2],
    pos: Option<Position>,
    profile: Profile,
    hold: [Option<Hold>; 2],
    chunk: [Option<InFlight>; 2],
    /// Where the head was during the last moves (gotos and hold chunks), for taps made on an older picture.
    history: std::collections::VecDeque<motion::Trajectory>,
    stop_owner: Option<crate::control_stop::Owner>,
    /// Microphone levels for homing by ear; without them homing is timed.
    ears: Option<Box<dyn Listener + Send>>,
}

impl<B: Bus> Ptz<B> {
    pub fn new(bus: B) -> Self {
        Self {
            bus,
            pps: [0; 2],
            pos: None,
            profile: Profile::homing(),
            history: std::collections::VecDeque::new(),
            hold: [None; 2],
            chunk: [None; 2],
            stop_owner: None,
            ears: None,
        }
    }

    /// Microphone levels for homing by ear from now on; `None` goes back to timed homing.
    pub fn set_ears(&mut self, ears: Option<Box<dyn Listener + Send>>) {
        self.ears = ears;
    }

    /// Profile for absolute moves and holds from now on; it takes effect at the next move.
    pub fn set_profile(&mut self, profile: Profile) {
        self.profile = profile;
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn with_stop_owner(mut self, owner: crate::control_stop::Owner) -> Self {
        self.stop_owner = Some(owner);
        self
    }

    pub fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
        self.stop_owner.as_ref().map(|o| o.path().to_owned())
    }

    pub fn motion_epoch(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicU64>> {
        self.stop_owner
            .as_ref()
            .map(crate::control_stop::Owner::epoch)
    }

    /// Service stop without relinquishing ownership. A cancelled operation must return immediately.
    pub fn checkpoint(&mut self) -> Result<(), Error> {
        let requested = match self.stop_owner.as_mut() {
            Some(o) => o.requested().map_err(|e| Error::Bus(e.to_string()))?,
            None => false,
        };
        if !requested {
            return Ok(());
        }
        let result = self.stop();
        if let Some(o) = self.stop_owner.as_mut() {
            o.complete(result.is_ok());
        }
        result?;
        Err(Error::Cancelled)
    }

    pub fn bus(&self) -> &B {
        &self.bus
    }

    #[cfg(test)]
    fn bus_mut(&mut self) -> &mut B {
        &mut self.bus
    }

    pub fn position(&self) -> Option<Position> {
        self.pos
    }

    /// The vendor's `ms32006_init` sequence: reset, motor type, rate and drive mode of both axes.
    pub fn init(&mut self) -> Result<(), Error> {
        self.checkpoint()?;
        self.pos = None;
        self.bus.write(REG_CTRL, &[0x00])?;
        self.bus.write(REG_CTRL, &[0x01])?;
        self.bus.sleep(Duration::from_millis(50));
        self.checkpoint()?;
        self.update(REG_CTRL, |v| v | 0x80)?;
        for axis in Axis::ALL {
            self.set_pps(axis, Profile::homing().max_pps)?;
            self.update(axis.bank() + 2, |v| v | 0x80)?;
        }
        Ok(())
    }

    /// Programs the step rate (at least [`motion::MIN_PPS`]). Only valid between moves.
    pub fn set_pps(&mut self, axis: Axis, pps: u32) -> Result<(), Error> {
        let pps = pps.max(motion::MIN_PPS);
        let div = (PPS_CLOCK / pps) >> 7;
        self.pps[axis.index()] = 0;
        self.bus.write(axis.bank() + 1, &[div as u8])?;
        self.update(axis.bank() + 2, |v| (v & 0xc0) | ((div >> 8) as u8 & 0x3f))?;
        self.pps[axis.index()] = pps;
        Ok(())
    }

    fn rate(&mut self, axis: Axis, pps: u32) -> Result<(), Error> {
        if self.pps[axis.index()] == pps {
            return Ok(());
        }
        self.set_pps(axis, pps)
    }

    /// Starts one move of at most [`MAX_CHUNK`] steps and returns without waiting.
    pub fn start_move(&mut self, axis: Axis, dir: Dir, steps: u32) -> Result<(), Error> {
        self.pos = None;
        self.kick(axis, dir, steps)
    }

    fn kick(&mut self, axis: Axis, dir: Dir, steps: u32) -> Result<(), Error> {
        if steps > MAX_CHUNK {
            return Err(Error::TooManySteps { steps });
        }
        let bank = axis.bank();
        self.bus.write(bank + 3, &[steps as u8])?;
        let dir_bit = if dir == Dir::One { 0x40 } else { 0 };
        self.bus
            .write(bank + 4, &[0x80 | dir_bit | (steps >> 8) as u8])?;
        self.update(REG_CTRL, |v| v & !axis.stop_bit())?;
        self.update(REG_KICK, |v| v | axis.kick_bit())
    }

    pub fn is_busy(&mut self, axis: Axis) -> Result<bool, Error> {
        let mut s = [0u8; 2];
        self.bus.read(axis.status_reg(), &mut s)?;
        Ok(s[0] & 0x10 != 0)
    }

    /// Halts both axes and invalidates position. Cross-process callers use the owner transport.
    pub fn stop(&mut self) -> Result<(), Error> {
        self.history.clear();
        let bits = Axis::ALL.iter().fold(0, |m, a| m | a.stop_bit());
        self.pos = None;
        self.hold = [None; 2];
        self.chunk = [None; 2];
        self.update(REG_CTRL, |v| v | bits)?;
        Ok(())
    }

    /// Drives both axes into their end stops; afterwards the position is `(0, 0)`.
    pub fn home(&mut self) -> Result<(), Error> {
        let how = self.home_reporting()?;
        eprintln!("ptz: homed: {how:?}");
        Ok(())
    }

    /// [`Ptz::home`] that says how it homed. With a listener each axis (tilt, then pan) is driven toward its
    /// stop and halted when the microphone hears it hit; an axis never heard runs its full travel. Without
    /// a listener, or from the axis where the microphone fails, the rest is driven a full travel by time.
    pub fn home_reporting(&mut self) -> Result<Homing, Error> {
        self.settle()?;
        self.history.clear();
        self.pos = None;
        let mut left = Axis::ALL.to_vec();
        let mut heard = [false; 2];
        let mut why = None;
        if let Some(mut ears) = self.ears.take() {
            let r = self.home_by_ear(&mut *ears, &mut left, &mut heard);
            self.ears = Some(ears);
            match r {
                Ok(None) => {
                    self.pos = Some(Position { pan: 0, tilt: 0 });
                    return Ok(Homing::ByEar { heard });
                }
                Ok(Some(reason)) => {
                    eprintln!("ptz: homing by ear unavailable ({reason}); homing {left:?} by time");
                    why = Some(reason);
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(primary) => {
                    return match self.stop() {
                        Ok(()) => Err(primary),
                        Err(stop) => Err(Error::StopFailed {
                            primary: Box::new(primary),
                            stop: Box::new(stop),
                        }),
                    };
                }
            }
        }
        let plan = Axis::ALL.map(|a| {
            left.contains(&a)
                .then(|| (a.positive().flip(), a.max() + HOME_MARGIN))
        });
        self.run(plan, Profile::homing())?;
        self.pos = Some(Position { pan: 0, tilt: 0 });
        Ok(Homing::Timed(why))
    }

    /// Tilt, then pan, so a sound belongs to one motor; each homed axis leaves `left`. `Ok(None)`: both
    /// axes are at their stops; `Ok(Some(reason))`: no usable microphone, the caller homes what is left by time.
    fn home_by_ear(
        &mut self,
        ears: &mut dyn Listener,
        left: &mut Vec<Axis>,
        heard: &mut [bool; 2],
    ) -> Result<Option<String>, Error> {
        for axis in [Axis::Tilt, Axis::Pan] {
            match self.home_axis_by_ear(axis, ears)? {
                ByEar::Heard => heard[axis.index()] = true,
                ByEar::FullTravel => {}
                ByEar::Unusable(reason) => return Ok(Some(format!("{axis:?}: {reason}"))),
            }
            left.retain(|a| *a != axis);
        }
        Ok(None)
    }

    /// Listens to the quiet head, then drives `axis` toward its stop and halts it once the stall is heard.
    /// When the stop is never heard (a loud room) the axis runs its full travel plus margin: it is at the
    /// stop either way.
    fn home_axis_by_ear(&mut self, axis: Axis, ears: &mut dyn Listener) -> Result<ByEar, Error> {
        let toward_home = axis.positive().flip();
        self.rate(axis, DEFAULT_PPS)?;
        // Levels buffered before this axis started belong to whatever happened then.
        if let Err(e) = ears.heard() {
            return Ok(ByEar::Unusable(format!("no audio: {e}")));
        }
        let mut quiet = Vec::new();
        for _ in 0..QUIET_LISTENS {
            self.checkpoint()?;
            self.bus.sleep(QUIET_LISTEN);
            match ears.heard() {
                Ok(l) => quiet.extend(l),
                Err(e) => return Ok(ByEar::Unusable(format!("no audio: {e}"))),
            }
            if quiet.len() >= QUIET_FRAMES {
                break;
            }
        }
        let Some(mut detector) = homing::Detector::listening_to(&quiet) else {
            return Ok(ByEar::Unusable(format!(
                "microphone delivered {} frames in {} ms",
                quiet.len(),
                (QUIET_LISTEN * QUIET_LISTENS).as_millis()
            )));
        };
        let started = std::time::Instant::now();
        let mut left = axis.max() + HOME_MARGIN;
        while left > 0 {
            let n = left.min(MAX_CHUNK);
            self.checkpoint()?;
            self.kick(axis, toward_home, n)?;
            left -= n;
            let mut polls = (travel(n, DEFAULT_PPS) * 2 + Duration::from_secs(1)).as_millis()
                as u64
                / POLL.as_millis() as u64;
            while self.is_busy(axis)? {
                self.checkpoint()?;
                let heard = match ears.heard() {
                    Ok(l) => l,
                    Err(e) => {
                        self.halt(axis)?;
                        return Ok(ByEar::Unusable(format!("audio lost while homing: {e}")));
                    }
                };
                if detector.stalled(&heard) {
                    self.halt(axis)?;
                    eprintln!(
                        "ptz: {axis:?} heard the stop after {} ms of driving (quiet {:.0} dBFS, motor {:.0})",
                        started.elapsed().as_millis(),
                        detector.quiet_dbfs,
                        detector.motor_dbfs().unwrap_or(f32::NAN)
                    );
                    return Ok(ByEar::Heard);
                }
                if polls == 0 {
                    return Err(Error::Timeout(axis));
                }
                polls -= 1;
                self.bus.sleep(POLL);
            }
        }
        eprintln!(
            "ptz: {axis:?} ran its full travel without hearing the stop (quiet {:.0} dBFS, motor {:.0}; quiet frames {:?})",
            detector.quiet_dbfs,
            detector.motor_dbfs().unwrap_or(f32::NAN),
            quiet.iter().map(|l| l.round()).collect::<Vec<_>>()
        );
        Ok(ByEar::FullTravel)
    }

    /// Halts one axis' pulses and waits for it to report idle.
    fn halt(&mut self, axis: Axis) -> Result<(), Error> {
        self.update(REG_CTRL, |v| v | axis.stop_bit())?;
        let mut polls = 50;
        while self.is_busy(axis)? {
            if polls == 0 {
                return Err(Error::Timeout(axis));
            }
            polls -= 1;
            self.bus.sleep(POLL);
        }
        Ok(())
    }

    /// Moves to an absolute position; needs a prior [`Ptz::home`] in this process.
    pub fn goto(&mut self, target: Position) -> Result<(), Error> {
        self.goto_at(target, None)
    }

    /// [`Ptz::goto`] that also records the move's trajectory from `now` for [`Ptz::pose_at`].
    pub fn goto_at(&mut self, target: Position, now: Option<Duration>) -> Result<(), Error> {
        self.settle()?;
        let cur = self.pos.ok_or(Error::NotHomed)?;
        if let Some(now) = now {
            self.remember(motion::Trajectory::plan(
                now,
                (cur.pan, cur.tilt),
                (target.pan, target.tilt),
                &self.profile,
            ));
        }
        for (axis, pos) in [(Axis::Pan, target.pan), (Axis::Tilt, target.tilt)] {
            if pos > axis.max() {
                return Err(Error::OutOfRange {
                    axis,
                    pos,
                    max: axis.max(),
                });
            }
        }
        let leg = |axis: Axis, from: u32, to: u32| match to.cmp(&from) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some((axis.positive(), to - from)),
            std::cmp::Ordering::Less => Some((axis.positive().flip(), from - to)),
        };
        self.run(
            [
                leg(Axis::Pan, cur.pan, target.pan),
                leg(Axis::Tilt, cur.tilt, target.tilt),
            ],
            self.profile,
        )?;
        self.pos = Some(target);
        Ok(())
    }

    /// Runs one leg per axis concurrently, splitting each into chunks, until both are idle.
    fn run(&mut self, plan: [Option<(Dir, u32)>; 2], profile: Profile) -> Result<(), Error> {
        self.pos = None;
        match self.run_plan(plan, profile) {
            Ok(()) => Ok(()),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(primary) => match self.stop() {
                Ok(()) => Err(primary),
                Err(stop) => Err(Error::StopFailed {
                    primary: Box::new(primary),
                    stop: Box::new(stop),
                }),
            },
        }
    }

    /// Each axis follows `profile` on its own: its leg's elapsed time is the sum of the travel of the
    /// chunks it issued, so the ramp does not depend on poll timing.
    fn run_plan(&mut self, plan: [Option<(Dir, u32)>; 2], profile: Profile) -> Result<(), Error> {
        let mut left = plan.map(|p| p.map_or(0, |(_, n)| n));
        let mut elapsed = [Duration::ZERO; 2];
        let longest = left.iter().copied().max().unwrap_or(0);
        let budget = profile.worst_case(longest) * 2 + Duration::from_secs(2);
        let mut polls = budget.as_millis() as u64 / POLL.as_millis() as u64 + 1;
        loop {
            let mut active = false;
            for axis in Axis::ALL {
                self.checkpoint()?;
                let i = axis.index();
                if self.is_busy(axis)? {
                    active = true;
                } else if left[i] > 0 {
                    let (pps, chunk) = profile.next_chunk(elapsed[i], left[i], false);
                    let dir = plan[i].map_or(Dir::Zero, |(d, _)| d);
                    self.rate(axis, pps)?;
                    self.kick(axis, dir, chunk)?;
                    left[i] -= chunk;
                    elapsed[i] += travel(chunk, pps);
                    active = true;
                }
            }
            if !active {
                return Ok(());
            }
            if polls == 0 {
                let axis = if self.is_busy(Axis::Pan)? {
                    Axis::Pan
                } else {
                    Axis::Tilt
                };
                return Err(Error::Timeout(axis));
            }
            polls -= 1;
            self.bus.sleep(POLL);
        }
    }

    /// Starts (or redirects) a continuous move of `axis` toward one end. Each chunk is short, so a
    /// release stops within one chunk; the position is counted chunk by chunk and stays exact. A new
    /// direction or axis waits for the running chunk and starts its ramp again from `start_pps`.
    pub fn begin_hold(
        &mut self,
        axis: Axis,
        toward: Toward,
        profile: Profile,
        timeout: Duration,
        now: Duration,
    ) -> Result<(), Error> {
        if self.pos.is_none() {
            return Err(Error::NotHomed);
        }
        let mut targets = [None; 2];
        targets[axis.index()] = Some((toward, profile));
        self.begin_holds(targets, timeout, now)
    }

    /// Replace the complete axis intent. Each axis retains its running chunk
    /// and ramp when unchanged; adding/removing the other never restarts it.
    pub fn begin_holds(
        &mut self,
        targets: [Option<(Toward, Profile)>; 2],
        timeout: Duration,
        now: Duration,
    ) -> Result<(), Error> {
        if self.pos.is_none() {
            return Err(Error::NotHomed);
        }
        for axis in Axis::ALL {
            let i = axis.index();
            match targets[i] {
                Some((toward, profile)) => match self.hold[i].as_mut() {
                    Some(h) if h.toward == toward => {
                        h.released = false;
                        h.renewed = now;
                        h.timeout = timeout;
                        if profile.max_pps != h.profile.max_pps {
                            h.profile = Profile::new(
                                self.pps[i].max(motion::MIN_PPS),
                                profile.max_pps,
                                profile.ramp,
                            );
                            h.ramp_from = now;
                        }
                    }
                    _ => {
                        self.hold[i] = Some(Hold {
                            toward,
                            profile,
                            timeout,
                            renewed: now,
                            ramp_from: now,
                            released: false,
                        })
                    }
                },
                None => {
                    if let Some(h) = self.hold[i].as_mut() {
                        h.released = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Ends each axis after its running chunk, retaining the known position.
    pub fn release_hold(&mut self) {
        for h in self.hold.iter_mut().flatten() {
            h.released = true;
        }
    }
    pub fn holding(&self) -> bool {
        self.hold.iter().any(Option::is_some) || self.chunk.iter().any(Option::is_some)
    }
    /// Advance both axes independently; no axis waits for the other's chunk.
    pub fn poll_hold(&mut self, now: Duration) -> Result<HoldState, Error> {
        self.checkpoint()?;
        let mut wait: Option<Duration> = None;
        let mut ended = None;
        for axis in Axis::ALL {
            let i = axis.index();
            if let Some(c) = self.chunk[i] {
                if self.is_busy(c.axis)? {
                    if now > c.ends + CHUNK_SLACK {
                        let primary = Error::Timeout(c.axis);
                        return Err(match self.stop() {
                            Ok(()) => primary,
                            Err(stop) => Error::StopFailed {
                                primary: Box::new(primary),
                                stop: Box::new(stop),
                            },
                        });
                    }
                    let d = c
                        .ends
                        .saturating_sub(now)
                        .clamp(HOLD_POLL_MIN, HOLD_POLL_MAX);
                    wait = Some(wait.map_or(d, |old| old.min(d)));
                    continue;
                }
                self.chunk[i] = None;
                if let Some(p) = self.pos.as_mut() {
                    let v = if i == 0 { &mut p.pan } else { &mut p.tilt };
                    *v = match c.toward {
                        Toward::Max => (*v + c.steps).min(axis.max()),
                        Toward::Zero => v.saturating_sub(c.steps),
                    };
                }
            }
            let Some(h) = self.hold[i] else {
                continue;
            };
            let pos = self.pos.ok_or(Error::NotHomed)?;
            let at = if i == 0 { pos.pan } else { pos.tilt };
            let remaining = match h.toward {
                Toward::Max => axis.max().saturating_sub(at),
                Toward::Zero => at,
            };
            let end = if h.released {
                Some(HoldEnd::Released)
            } else if now.saturating_sub(h.renewed) >= h.timeout {
                Some(HoldEnd::Timeout)
            } else if remaining == 0 {
                Some(HoldEnd::Limit)
            } else {
                None
            };
            if let Some(end) = end {
                self.hold[i] = None;
                ended = Some(end);
                continue;
            }
            let (pps, steps) =
                h.profile
                    .next_chunk(now.saturating_sub(h.ramp_from), remaining, true);
            let dir = match h.toward {
                Toward::Max => axis.positive(),
                Toward::Zero => axis.positive().flip(),
            };
            self.rate(axis, pps)?;
            self.kick(axis, dir, steps)?;
            let duration = travel(steps, pps);
            let ends = now + duration;
            self.remember(motion::Trajectory::chunk(
                now,
                ends,
                i,
                (pos.pan, pos.tilt),
                steps,
                h.toward == Toward::Max,
            ));
            self.chunk[i] = Some(InFlight {
                axis,
                toward: h.toward,
                steps,
                ends,
            });
            let d = duration.clamp(HOLD_POLL_MIN, HOLD_POLL_MAX);
            wait = Some(wait.map_or(d, |old| old.min(d)));
        }
        if let Some(d) = wait {
            Ok(HoldState::Moving(d))
        } else if let Some(end) = ended {
            Ok(HoldState::Ended(end))
        } else {
            Ok(HoldState::Idle)
        }
    }

    /// Keeps a trajectory for [`Ptz::pose_at`]; older ones than [`HISTORY`] before its end go.
    fn remember(&mut self, t: motion::Trajectory) {
        let keep_from = t.end.saturating_sub(HISTORY);
        while self.history.front().is_some_and(|h| h.end < keep_from) {
            self.history.pop_front();
        }
        self.history.push_back(t);
    }

    /// Where the head was at `t` according to the recorded moves: inside a move its trajectory, between two
    /// moves the earlier one's end, before the first kept move that move's start (the head stood there since
    /// whatever the history has forgotten); `None` after a stop or with nothing recorded (the caller then uses the
    /// confirmed position).
    pub fn pose_at(&self, t: Duration) -> Option<Position> {
        let first = self.history.front()?;
        let mut pose = [first.from.0, first.from.1];
        for (axis, value) in pose.iter_mut().enumerate() {
            let mut chosen = None;
            for h in &self.history {
                if h.start > t {
                    break;
                }
                if !h.legs[axis].is_empty() {
                    chosen = Some(h);
                }
            }
            if let Some(h) = chosen {
                let p = h.at(t);
                *value = if axis == 0 { p.0 } else { p.1 };
            }
        }
        Some(Position {
            pan: pose[0],
            tilt: pose[1],
        })
    }

    /// Ends holds, then waits only for the outstanding chunks.
    fn settle(&mut self) -> Result<(), Error> {
        self.hold = [None; 2];
        let Some(max_steps) = self.chunk.iter().flatten().map(|c| c.steps).max() else {
            return Ok(());
        };
        let mut polls = (travel(max_steps, motion::MIN_PPS) + CHUNK_SLACK).as_millis() as u64
            / POLL.as_millis() as u64;
        while self.is_busy(Axis::Pan)? || self.is_busy(Axis::Tilt)? {
            self.checkpoint()?;
            if polls == 0 {
                self.stop()?;
                return Err(Error::Timeout(Axis::Pan));
            }
            polls -= 1;
            self.bus.sleep(POLL);
        }
        self.poll_hold(Duration::ZERO).map(|_| ())
    }

    fn update(&mut self, reg: u8, f: impl FnOnce(u8) -> u8) -> Result<(), Error> {
        let mut v = [0u8];
        self.bus.read(reg, &mut v)?;
        self.bus.write(reg, &[f(v[0])])
    }
}

use motion::travel;

#[cfg(test)]
mod tests {
    use super::fake::FakeBus;
    use super::*;

    fn ptz() -> Ptz<FakeBus> {
        Ptz::new(FakeBus::new(3000, 500))
    }

    #[test]
    fn init_programs_the_registers_the_vendor_leaves_behind() {
        let mut p = ptz();
        p.init().unwrap();
        let r = p.bus().regs();
        assert_eq!(
            [r[0x00], r[0x01], r[0x02], r[0x05], r[0x06]],
            [0x81, 0xc3, 0x80, 0xc3, 0x80]
        );
        // reset is 0 then 1, then a 50 ms settle
        assert_eq!(p.bus().writes_to(REG_CTRL)[..2], [0x00, 0x01]);
        assert_eq!(p.bus().sleeps()[0], Duration::from_millis(50));
    }

    #[test]
    fn pps_divisor_and_floor() {
        let mut p = ptz();
        p.set_pps(Axis::Pan, 1200).unwrap();
        p.set_pps(Axis::Tilt, 600).unwrap();
        let r = p.bus().regs();
        assert_eq!([r[0x01], r[0x02] & 0x3f], [0x61, 0]);
        assert_eq!([r[0x05], r[0x06] & 0x3f], [0xc3, 0]);
        p.set_pps(Axis::Tilt, 0).unwrap();
        assert_eq!(p.bus().regs()[0x05], 0x93, "0 is raised to MIN_PPS");
        // 100 pps needs the high bits: (15e6/100)>>7 = 1171 = 0x493
        p.set_pps(Axis::Pan, 100).unwrap();
        assert_eq!(
            [p.bus().regs()[0x01], p.bus().regs()[0x02] & 0x3f],
            [0x93, 0x04]
        );
    }

    #[test]
    fn pps_keeps_the_drive_mode_bits() {
        let mut p = ptz();
        p.init().unwrap();
        p.set_pps(Axis::Pan, 100).unwrap();
        assert_eq!(p.bus().regs()[0x02] & 0xc0, 0x80);
    }

    #[test]
    fn move_encodes_steps_direction_and_kicks_the_axis() {
        let mut p = ptz();
        p.start_move(Axis::Pan, Dir::One, 1500).unwrap();
        let r = p.bus().regs();
        assert_eq!([r[3], r[4]], [0xdc, 0xc5]);
        assert_eq!(r[REG_CTRL as usize] & 0x04, 0, "stop bit cleared");
        assert_eq!(p.bus().kicks(), [Axis::Pan]);

        p.start_move(Axis::Tilt, Dir::Zero, 20).unwrap();
        assert_eq!([p.bus().regs()[7], p.bus().regs()[8]], [20, 0x80]);
        assert_eq!(p.bus().kicks(), [Axis::Pan, Axis::Tilt]);
    }

    #[test]
    fn move_rejects_more_than_one_chunk() {
        assert_eq!(
            ptz().start_move(Axis::Pan, Dir::Zero, 4096),
            Err(Error::TooManySteps { steps: 4096 })
        );
    }

    #[test]
    fn home_reaches_both_stops_from_anywhere() {
        for start in [(0, 0), (3000, 500), (PAN_MAX, TILT_MAX)] {
            let mut p = Ptz::new(FakeBus::new(start.0, start.1));
            p.init().unwrap();
            p.home().unwrap();
            assert_eq!(p.bus().mech(), (0, 0), "from {start:?}");
            assert_eq!(p.position(), Some(Position { pan: 0, tilt: 0 }));
        }
    }

    #[test]
    fn home_splits_travel_into_chunks_and_drives_both_axes_together() {
        let mut p = Ptz::new(FakeBus::new(PAN_MAX, TILT_MAX));
        p.init().unwrap();
        p.home().unwrap();
        let moves = p.bus().moves();
        assert!(moves.iter().all(|m| m.steps <= MAX_CHUNK));
        let total = |a: Axis| {
            moves
                .iter()
                .filter(|m| m.axis == a)
                .map(|m| m.steps)
                .sum::<u32>()
        };
        assert_eq!(total(Axis::Pan), PAN_MAX + HOME_MARGIN);
        assert_eq!(total(Axis::Tilt), TILT_MAX + HOME_MARGIN);
        // tilt's single chunk starts right after pan's first, before anything was waited for
        assert_eq!((moves[0].axis, moves[1].axis), (Axis::Pan, Axis::Tilt));
    }

    /// Steps driven toward the home stop on `axis` by every move the fake saw.
    fn moved(p: &Ptz<FakeBus>, axis: Axis) -> u32 {
        p.bus()
            .moves()
            .iter()
            .filter(|m| m.axis == axis)
            .map(|m| m.steps)
            .sum()
    }

    fn eared(pan: u32, tilt: u32, ears: impl FnOnce(fake::Ears) -> fake::Ears) -> Ptz<FakeBus> {
        let bus = FakeBus::new(pan, tilt);
        let mic = ears(bus.ears());
        let mut p = Ptz::new(bus);
        p.init().unwrap();
        p.set_ears(Some(Box::new(mic)));
        p
    }

    /// Control-register writes that newly set an axis' stop bit: the halts homing by ear issues.
    fn halts(p: &Ptz<FakeBus>, axis: Axis) -> usize {
        let bit = axis.stop_bit();
        p.bus()
            .writes_to(0)
            .windows(2)
            .filter(|w| w[0] & bit == 0 && w[1] & bit != 0)
            .count()
    }

    #[test]
    fn homing_by_ear_halts_each_axis_when_the_stop_is_heard() {
        let mut p = eared(3000, 500, |e| e);
        assert_eq!(
            p.home_reporting().unwrap(),
            Homing::ByEar { heard: [true; 2] }
        );
        assert_eq!(p.bus().mech(), (0, 0));
        assert_eq!(p.position(), Some(Position { pan: 0, tilt: 0 }));
        assert_eq!(halts(&p, Axis::Tilt), 1);
        assert_eq!(halts(&p, Axis::Pan), 1);
        // tilt first, then pan: a sound belongs to one motor
        let first_pan = p
            .bus()
            .kicks()
            .iter()
            .position(|a| *a == Axis::Pan)
            .unwrap();
        assert!(
            p.bus().kicks()[..first_pan]
                .iter()
                .all(|a| *a == Axis::Tilt)
        );
    }

    #[test]
    fn homing_by_ear_from_the_stop_and_from_the_far_end_works() {
        for (pan, tilt) in [(0, 0), (PAN_MAX, TILT_MAX), (PAN_MAX, 0), (0, TILT_MAX)] {
            let mut p = eared(pan, tilt, |e| e);
            assert_eq!(
                p.home_reporting().unwrap(),
                Homing::ByEar { heard: [true; 2] },
                "from ({pan}, {tilt})"
            );
            assert_eq!(p.bus().mech(), (0, 0), "from ({pan}, {tilt})");
        }
    }

    #[test]
    fn a_room_too_loud_to_hear_the_stop_runs_the_full_travel_and_still_homes() {
        let mut p = eared(3000, 500, |e| e.loud_room());
        assert_eq!(
            p.home_reporting().unwrap(),
            Homing::ByEar { heard: [false; 2] }
        );
        assert_eq!(p.bus().mech(), (0, 0));
        assert_eq!(halts(&p, Axis::Pan), 0);
        assert!(moved(&p, Axis::Pan) >= PAN_MAX + HOME_MARGIN);
    }

    #[test]
    fn no_audio_falls_back_to_homing_by_time() {
        let mut p = eared(3000, 500, |e| e.deaf());
        let how = p.home_reporting().unwrap();
        assert!(matches!(how, Homing::Timed(Some(_))), "{how:?}");
        assert_eq!(p.bus().mech(), (0, 0));
        assert!(moved(&p, Axis::Pan) >= PAN_MAX + HOME_MARGIN);
    }

    #[test]
    fn audio_lost_on_pan_homes_only_pan_by_time() {
        let mut p = eared(3000, 500, |e| e.deaf_after(1));
        let how = p.home_reporting().unwrap();
        assert!(matches!(how, Homing::Timed(Some(_))), "{how:?}");
        assert_eq!(p.bus().mech(), (0, 0));
        // tilt was heard and halted, never kicked again by the timed fallback
        assert_eq!(halts(&p, Axis::Tilt), 1);
        assert_eq!(
            p.bus().kicks().iter().filter(|a| **a == Axis::Tilt).count(),
            1
        );
        assert!(moved(&p, Axis::Pan) >= PAN_MAX + HOME_MARGIN);
    }

    #[test]
    fn without_ears_homing_is_timed_and_says_so() {
        let mut p = ptz();
        p.init().unwrap();
        assert_eq!(p.home_reporting().unwrap(), Homing::Timed(None));
    }

    #[test]
    fn goto_needs_home_first() {
        let mut p = ptz();
        assert_eq!(p.goto(Position { pan: 10, tilt: 10 }), Err(Error::NotHomed));
    }

    #[test]
    fn goto_lands_on_vendor_coordinates() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        for target in [
            Position {
                pan: 6195,
                tilt: 650,
            },
            Position {
                pan: 3262,
                tilt: 739,
            },
            Position { pan: 3577, tilt: 0 },
            Position { pan: 3577, tilt: 0 },
        ] {
            p.goto(target).unwrap();
            assert_eq!(p.bus().mech(), (target.pan, target.tilt));
            assert_eq!(p.position(), Some(target));
        }
    }

    #[test]
    fn goto_to_the_current_position_moves_nothing() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        let before = p.bus().moves().len();
        p.goto(Position { pan: 0, tilt: 0 }).unwrap();
        assert_eq!(p.bus().moves().len(), before);
    }

    #[test]
    fn goto_rejects_positions_beyond_the_travel() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        let before = p.bus().moves().len();
        assert_eq!(
            p.goto(Position {
                pan: PAN_MAX + 1,
                tilt: 0
            }),
            Err(Error::OutOfRange {
                axis: Axis::Pan,
                pos: PAN_MAX + 1,
                max: PAN_MAX
            })
        );
        assert_eq!(p.bus().moves().len(), before, "nothing moved");
    }

    #[test]
    fn stop_sets_both_stop_bits_aborts_the_move_and_forgets_the_position() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        p.start_move(Axis::Pan, Dir::Zero, 1000).unwrap();
        p.stop().unwrap();
        assert_eq!(p.bus().regs()[REG_CTRL as usize] & 0x06, 0x06);
        assert!(!p.is_busy(Axis::Pan).unwrap());
        assert_eq!(p.position(), None);
    }

    #[test]
    fn a_stuck_busy_flag_times_out() {
        let mut p = Ptz::new(FakeBus::new(0, 0).stuck_busy());
        p.init().unwrap();
        assert!(matches!(p.home(), Err(Error::Timeout(_))));
    }

    fn homed(profile: Profile) -> Ptz<FakeBus> {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        p.set_profile(profile);
        p
    }

    fn fast() -> Profile {
        Profile::new(200, 1200, Duration::from_millis(800))
    }

    /// Polls a hold every `step` of fake time until it ends; returns the end and the time it took.
    fn drive(p: &mut Ptz<FakeBus>, from: Duration, step: Duration) -> (HoldEnd, Duration) {
        let mut now = from;
        for _ in 0..100_000 {
            match p.poll_hold(now).unwrap() {
                HoldState::Ended(e) => return (e, now),
                HoldState::Idle => panic!("idle before ended"),
                HoldState::Moving(_) => now += step,
            }
        }
        panic!("hold never ended")
    }

    #[test]
    fn homing_always_runs_at_the_vendor_rate() {
        let mut p = ptz();
        p.init().unwrap();
        p.set_profile(fast());
        p.home().unwrap();
        assert!(p.bus().moves().iter().all(|m| m.pps() == 600));
    }

    #[test]
    fn goto_ramps_the_divisor_up_and_down_and_lands_exactly() {
        let mut p = homed(fast());
        let before = p.bus().moves().len();
        p.goto(Position { pan: 6000, tilt: 0 }).unwrap();
        let rates: Vec<u32> = p.bus().moves()[before..]
            .iter()
            .map(fake::Move::pps)
            .collect();
        assert!(rates[0] < 300, "starts slow: {rates:?}");
        assert!(rates.iter().any(|r| *r > 1100), "reaches cruise: {rates:?}");
        assert!(*rates.last().unwrap() < 500, "brakes: {rates:?}");
        assert_eq!(p.bus().mech(), (6000, 0));
        assert_eq!(p.position(), Some(Position { pan: 6000, tilt: 0 }));
    }

    #[test]
    fn hold_ramps_and_release_stops_after_the_current_chunk_with_exact_position() {
        let mut p = homed(fast());
        let ms = Duration::from_millis;
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            ms(0),
        )
        .unwrap();
        let mut now = ms(0);
        // the fake finishes a chunk after two status reads: 60 ms polls keep fake travel near real time
        while now < ms(3000) {
            assert!(matches!(p.poll_hold(now).unwrap(), HoldState::Moving(_)));
            now += ms(60);
        }
        let issued = p.bus().moves().len();
        p.release_hold();
        let (end, at) = drive(&mut p, now, ms(60));
        assert_eq!(end, HoldEnd::Released);
        assert!(at - now <= ms(150), "stopped {:?} after release", at - now);
        assert!(
            p.bus().moves().len() <= issued + 1,
            "no new chunk after release"
        );
        let pos = p.position().unwrap();
        assert_eq!((pos.pan, pos.tilt), p.bus().mech(), "position stays exact");
        let rates: Vec<u32> = p
            .bus()
            .moves()
            .iter()
            .skip(2)
            .map(fake::Move::pps)
            .collect();
        assert!(
            rates[1] < 300 && rates.iter().any(|r| *r > 1100),
            "{rates:?}"
        );
        assert_eq!(p.poll_hold(now).unwrap(), HoldState::Idle);
        // a later absolute move needs no re-home
        p.goto(Position { pan: 10, tilt: 10 }).unwrap();
        assert_eq!(p.bus().mech(), (10, 10));
    }

    #[test]
    fn hold_ends_at_the_travel_limit() {
        let mut p = homed(fast());
        p.goto(Position {
            pan: 0,
            tilt: TILT_MAX - 30,
        })
        .unwrap();
        p.begin_hold(
            Axis::Tilt,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            Duration::ZERO,
        )
        .unwrap();
        let (end, _) = drive(&mut p, Duration::ZERO, Duration::from_millis(10));
        assert_eq!(end, HoldEnd::Limit);
        assert_eq!(p.position().unwrap().tilt, TILT_MAX);
        assert_eq!(p.bus().mech().1, TILT_MAX);
    }

    #[test]
    fn hold_without_release_ends_at_the_timeout() {
        let mut p = homed(Profile::new(100, 100, Duration::ZERO));
        let slow = p.profile();
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            slow,
            Duration::from_secs(2),
            Duration::ZERO,
        )
        .unwrap();
        let (end, at) = drive(&mut p, Duration::ZERO, Duration::from_millis(10));
        assert_eq!(end, HoldEnd::Timeout);
        assert!(at >= Duration::from_secs(2) && at < Duration::from_millis(2200));
        assert_eq!(p.position().unwrap().pan, p.bus().mech().0);
    }

    #[test]
    fn a_new_direction_redirects_after_the_running_chunk() {
        let mut p = homed(fast());
        p.goto(Position { pan: 3000, tilt: 0 }).unwrap();
        let ms = Duration::from_millis;
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            ms(0),
        )
        .unwrap();
        for t in (0..500).step_by(10) {
            p.poll_hold(ms(t)).unwrap();
        }
        p.begin_hold(
            Axis::Pan,
            Toward::Zero,
            fast(),
            Duration::from_secs(20),
            ms(500),
        )
        .unwrap();
        for t in (500..1500).step_by(10) {
            p.poll_hold(ms(t)).unwrap();
        }
        p.release_hold();
        drive(&mut p, ms(1500), ms(10));
        assert_eq!(p.position().unwrap().pan, p.bus().mech().0);
        assert!(p.position().unwrap().pan < 3000, "moved back toward zero");
    }

    #[test]
    fn an_absolute_move_ends_the_hold_first() {
        let mut p = homed(fast());
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            Duration::ZERO,
        )
        .unwrap();
        p.poll_hold(Duration::ZERO).unwrap();
        p.goto(Position { pan: 50, tilt: 50 }).unwrap();
        assert!(!p.holding());
        assert_eq!(p.bus().mech(), (50, 50));
    }

    #[test]
    fn emergency_stop_during_a_hold_still_forgets_the_position() {
        let mut p = homed(fast());
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            Duration::ZERO,
        )
        .unwrap();
        p.poll_hold(Duration::ZERO).unwrap();
        p.stop().unwrap();
        assert_eq!(p.position(), None);
        assert!(!p.holding());
        assert_eq!(
            p.poll_hold(Duration::from_millis(10)).unwrap(),
            HoldState::Idle
        );
    }

    #[test]
    fn hold_needs_a_known_position() {
        assert_eq!(
            ptz().begin_hold(
                Axis::Pan,
                Toward::Max,
                fast(),
                Duration::from_secs(1),
                Duration::ZERO
            ),
            Err(Error::NotHomed)
        );
    }

    #[test]
    fn a_hold_chunk_stuck_busy_aborts_with_stop() {
        let mut p = Ptz::new(FakeBus::new(0, 0));
        p.init().unwrap();
        p.home().unwrap();
        *p.bus_mut() = FakeBus::new(0, 0).stuck_busy();
        p.begin_hold(
            Axis::Pan,
            Toward::Max,
            fast(),
            Duration::from_secs(20),
            Duration::ZERO,
        )
        .unwrap();
        p.poll_hold(Duration::ZERO).unwrap();
        assert!(matches!(
            p.poll_hold(Duration::from_secs(3)),
            Err(Error::Timeout(Axis::Pan))
        ));
        assert_eq!(p.position(), None);
    }

    #[test]
    fn bus_errors_surface() {
        let mut p = Ptz::new(FakeBus::new(0, 0).failing());
        assert!(matches!(p.init(), Err(Error::Bus(_))));
    }

    #[test]
    fn pose_at_follows_a_recorded_goto_and_forgets_on_stop() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        assert_eq!(
            p.pose_at(Duration::from_secs(1)),
            None,
            "nothing recorded yet"
        );
        p.set_profile(Profile::new(300, 1200, Duration::from_millis(500)));
        let t0 = Duration::from_secs(10);
        p.goto_at(
            Position {
                pan: 2000,
                tilt: 400,
            },
            Some(t0),
        )
        .unwrap();
        assert_eq!(
            p.pose_at(Duration::from_secs(9)),
            Some(Position { pan: 0, tilt: 0 }),
            "before the first kept move: where it started"
        );
        assert_eq!(p.pose_at(t0), Some(Position { pan: 0, tilt: 0 }));
        let mid = p.pose_at(t0 + Duration::from_millis(600)).unwrap();
        assert!(mid.pan > 0 && mid.pan < 2000, "{mid:?}");
        assert_eq!(
            p.pose_at(Duration::from_secs(60)),
            Some(Position {
                pan: 2000,
                tilt: 400
            })
        );
        // a second move: between the two the pose is the first one's end
        let t1 = Duration::from_secs(20);
        p.goto_at(
            Position {
                pan: 1000,
                tilt: 400,
            },
            Some(t1),
        )
        .unwrap();
        assert_eq!(
            p.pose_at(Duration::from_secs(15)),
            Some(Position {
                pan: 2000,
                tilt: 400
            })
        );
        assert_eq!(
            p.pose_at(Duration::from_secs(30)),
            Some(Position {
                pan: 1000,
                tilt: 400
            })
        );
        p.stop().unwrap();
        assert_eq!(
            p.pose_at(Duration::from_secs(30)),
            None,
            "a stop forgets the history"
        );
    }

    #[test]
    fn a_tap_older_than_the_kept_history_resolves_from_where_the_last_move_started() {
        let mut p = ptz();
        p.init().unwrap();
        p.home().unwrap();
        p.set_profile(Profile::new(300, 1200, Duration::from_millis(500)));
        p.goto_at(
            Position {
                pan: 2000,
                tilt: 400,
            },
            Some(Duration::from_secs(10)),
        )
        .unwrap();
        // the next move comes a minute later: the first is forgotten, yet a picture from just before the second
        // move showed the head where the second move started
        p.goto_at(
            Position {
                pan: 1000,
                tilt: 400,
            },
            Some(Duration::from_secs(70)),
        )
        .unwrap();
        assert_eq!(
            p.pose_at(Duration::from_millis(69_800)),
            Some(Position {
                pan: 2000,
                tilt: 400
            })
        );
    }
}
