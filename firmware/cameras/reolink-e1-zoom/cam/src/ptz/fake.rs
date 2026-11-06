//! Host fake of the MS32006: a register file plus a tiny model of the two motors and their end stops.
//!
//! A kick (`reg 9`) makes the axis report busy for [`BUSY_POLLS`] status reads plus one per
//! [`STEPS_PER_POLL`] steps, moving the head a share each read, clamped to `0..=max` like a head driven into
//! its stop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Axis, Bus, Error, PAN_MAX, TILT_MAX};

const BUSY_POLLS: u32 = 2;
/// Extra status polls a move stays busy per this many steps.
const STEPS_PER_POLL: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub axis: Axis,
    pub steps: u32,
    /// Step-rate divisor programmed when the move was kicked.
    pub divisor: u32,
}

impl Move {
    /// The rate the divisor stands for (`15 MHz / (divisor << 7)`).
    pub fn pps(&self) -> u32 {
        15_000_000 / (self.divisor.max(1) << 7)
    }
}

pub struct FakeBus {
    regs: [u8; 128],
    /// Mechanical position in vendor coordinates (pan x, tilt y).
    mech: [u32; 2],
    /// Status reads left before the running move completes, per axis.
    busy: [u32; 2],
    /// Move waiting to be applied when its busy polls run out: signed distance in vendor coordinates.
    pending: [i64; 2],
    moves: Vec<Move>,
    kicks: Vec<Axis>,
    ctrl_writes: Vec<u8>,
    sleeps: Vec<Duration>,
    stuck: bool,
    failing: bool,
    /// What the motors are doing, for [`Ears`] that listen to this head.
    motor: Arc<Motor>,
}

/// Per axis: pulses are being output; the head is pushing against its stop.
#[derive(Debug, Default)]
struct Motor {
    running: [AtomicBool; 2],
    grinding: [AtomicBool; 2],
}

impl FakeBus {
    pub fn new(pan: u32, tilt: u32) -> Self {
        Self {
            regs: [0; 128],
            mech: [pan, tilt],
            busy: [0; 2],
            pending: [0; 2],
            moves: Vec::new(),
            kicks: Vec::new(),
            ctrl_writes: Vec::new(),
            sleeps: Vec::new(),
            stuck: false,
            failing: false,
            motor: Arc::new(Motor::default()),
        }
    }

    /// A microphone in this head's body: one level per call, quiet, running or grinding.
    pub fn ears(&self) -> Ears {
        Ears {
            motor: Arc::clone(&self.motor),
            room_dbfs: -45.0,
            grinding_calls: 0,
            deaf_after: None,
        }
    }

    /// Every status read reports busy and no move ever completes.
    pub fn stuck_busy(mut self) -> Self {
        self.stuck = true;
        self
    }

    /// Every register access fails.
    pub fn failing(mut self) -> Self {
        self.failing = true;
        self
    }

    pub fn regs(&self) -> &[u8; 128] {
        &self.regs
    }

    pub fn mech(&self) -> (u32, u32) {
        (self.mech[0], self.mech[1])
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    pub fn kicks(&self) -> &[Axis] {
        &self.kicks
    }

    pub fn sleeps(&self) -> &[Duration] {
        &self.sleeps
    }

    /// Values written to the control register, in order.
    pub fn writes_to(&self, reg: u8) -> &[u8] {
        assert_eq!(reg, 0, "only the control register is logged");
        &self.ctrl_writes
    }

    fn kick(&mut self, axis: Axis) {
        let i = axis.index();
        let bank = axis.bank() as usize;
        let steps = u32::from(self.regs[bank + 3]) | u32::from(self.regs[bank + 4] & 0x0f) << 8;
        let dir_one = self.regs[bank + 4] & 0x40 != 0;
        let divisor = u32::from(self.regs[bank + 1]) | u32::from(self.regs[bank + 2] & 0x3f) << 8;
        self.moves.push(Move {
            axis,
            steps,
            divisor,
        });
        self.kicks.push(axis);
        if self.regs[0] & axis.stop_bit() != 0 {
            return;
        }
        let towards_positive = (axis.positive() == super::Dir::One) == dir_one;
        let signed = i64::from(steps);
        self.pending[i] = if towards_positive { signed } else { -signed };
        // Long moves take longer, so a head driven past its stop grinds for a while before the move ends.
        self.busy[i] = BUSY_POLLS + steps / STEPS_PER_POLL;
        self.motor.running[i].store(true, Ordering::Relaxed);
        self.motor.grinding[i].store(false, Ordering::Relaxed);
    }

    /// Applies the running move's share for one status poll; a head that hits its stop grinds there.
    fn advance(&mut self, i: usize) {
        let max = [PAN_MAX, TILT_MAX][i];
        let share = self.pending[i] / i64::from(self.busy[i] + 1);
        let to = i64::from(self.mech[i]) + share;
        let clamped = to.clamp(0, i64::from(max));
        self.mech[i] = clamped as u32;
        self.pending[i] -= share;
        if clamped != to {
            self.motor.grinding[i].store(true, Ordering::Relaxed);
        }
    }

    fn halt(&mut self, i: usize) {
        self.busy[i] = 0;
        self.pending[i] = 0;
        self.motor.running[i].store(false, Ordering::Relaxed);
        self.motor.grinding[i].store(false, Ordering::Relaxed);
    }

    fn status(&mut self, axis: Axis) -> [u8; 2] {
        let i = axis.index();
        if self.stuck {
            return [0x90, 0];
        }
        if self.busy[i] > 0 {
            self.busy[i] -= 1;
            self.advance(i);
            if self.busy[i] > 0 {
                return [0x90, 0];
            }
            self.pending[i] = 0;
            self.motor.running[i].store(false, Ordering::Relaxed);
            self.motor.grinding[i].store(false, Ordering::Relaxed);
        }
        [0x80, 0]
    }
}

impl Bus for FakeBus {
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Error> {
        if self.failing {
            return Err(Error::Bus("fake read failure".into()));
        }
        for (o, slot) in buf.iter_mut().enumerate() {
            let r = reg.wrapping_add(o as u8);
            if (r == Axis::Pan.status_reg() || r == Axis::Tilt.status_reg()) && o == 0 {
                let axis = if r == Axis::Pan.status_reg() {
                    Axis::Pan
                } else {
                    Axis::Tilt
                };
                let s = self.status(axis);
                *slot = s[0];
                if let Some(next) = buf.get_mut(1) {
                    *next = s[1];
                }
                return Ok(());
            }
            *slot = self.regs[usize::from(r & 0x7f)];
        }
        Ok(())
    }

    fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), Error> {
        if self.failing {
            return Err(Error::Bus("fake write failure".into()));
        }
        for (o, v) in data.iter().enumerate() {
            let r = usize::from(reg.wrapping_add(o as u8) & 0x7f);
            self.regs[r] = *v;
            match r {
                0 => {
                    self.ctrl_writes.push(*v);
                    for axis in Axis::ALL {
                        // a set stop bit aborts a running move on that axis
                        if *v & axis.stop_bit() != 0 {
                            self.halt(axis.index());
                        }
                    }
                }
                9 => {
                    for axis in Axis::ALL {
                        if *v & axis.kick_bit() != 0 {
                            self.kick(axis);
                        }
                    }
                    // the kick bits self-clear on the real chip (register 9 reads 0 after a move)
                    self.regs[9] = 0;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn sleep(&mut self, d: Duration) {
        self.sleeps.push(d);
    }
}

/// A microphone over a [`FakeBus`] head, with the levels measured on a tested camera: room −45 dBFS, a running motor
/// −40, a motor grinding at its stop −28. Host tests only.
pub struct Ears {
    motor: Arc<Motor>,
    room_dbfs: f32,
    /// `heard` calls that reported a stall, for [`Ears::deaf_after`].
    grinding_calls: u32,
    deaf_after: Option<u32>,
}

impl Ears {
    /// A room so loud the stall cannot be told from it.
    pub fn loud_room(mut self) -> Self {
        self.room_dbfs = -30.0;
        self
    }

    /// No audio at all.
    pub fn deaf(self) -> Self {
        self.deaf_after(0)
    }

    /// Audio stops after `calls` `heard` calls that reported a stall (one call holds a whole detector
    /// window, so 1 means: after the first axis was heard).
    pub fn deaf_after(mut self, calls: u32) -> Self {
        self.deaf_after = Some(calls);
        self
    }
}

impl super::homing::Listener for Ears {
    fn heard(&mut self) -> Result<Vec<f32>, String> {
        if self.deaf_after.is_some_and(|n| self.grinding_calls >= n) {
            return Err("capture down".into());
        }
        let any = |flags: &[AtomicBool; 2]| flags.iter().any(|f| f.load(Ordering::Relaxed));
        let motor = if any(&self.motor.grinding) {
            self.grinding_calls += 1;
            -28.0
        } else if any(&self.motor.running) {
            -40.0
        } else {
            -100.0
        };
        // A whole detector window per call: the fake's moves end in two polls, far sooner than real ones.
        // The window and both rules are tested on real levels in `homing`.
        Ok(vec![
            self.room_dbfs.max(motor);
            super::homing::RECENT_FRAMES
        ])
    }
}
