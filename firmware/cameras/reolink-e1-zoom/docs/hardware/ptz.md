# Pan/tilt: the MS32006 stepper controller

Register map, units and homing of the pan/tilt stepper controller as driven by `cam::ptz` (`cam/src/ptz/`).
Tags as in [README.md](README.md#evidence-tags).

## Bus

- `/dev/i2c-2`, 7-bit address **0x10** [D `ms32006_init` @ 0xfa020]: `ioctl(I2C_TENBIT, 0)`, then
  `ioctl(I2C_SLAVE_FORCE, 0x10)`. Pinmux for the bus is set at boot by `vg_boot.sh`, nothing for us to do.
- Every access is an SMBus **I2C block transfer** (`ioctl(I2C_SMBUS=0x720)`, size 8, `read_write` 1/0, `command` =
  register, data block = length byte + payload, payload ≤ 31 bytes) [D `_ms32006_i2c_read` @ 0xf94ec,
  `_ms32006_i2c_write` @ 0xf9608]. `busybox i2cget/i2cset/i2cdump -f 2 0x10` hit the same registers [V].
- The register file is 0x00–0x0f; 0x80–0xff mirror it (`i2cdump` rows 0x00 and 0x80 are identical) [V]. The vendor
  reads the status block at the mirror 0x8b/0x8d; we do the same.
- The vendor `device` owns the chip while it runs (its `_ms32006_proc` thread polls it). Stop it (or kill the
  stack) before using `cam ptz`, and keep the hardware watchdog fed: with `device` gone the camera reboots after
  roughly 20–60 s [V, it happened twice].

## Registers

Two identical banks: **pan = bank 0x00**, **tilt = bank 0x04**. `b` is the bank base.

| Reg | Bits | Meaning | Evidence |
|---|---|---|---|
| 0x00 | 0 | chip enable: reset is `0x00` then `0x01`, then `usleep(50 ms)` | D `_ms32006_reset` @ 0xf97f0 |
| 0x00 | 7 | motor type, set to 1 at init (`_ms32006_set_motor_type(1)` RMW) | D @ 0xf9888 |
| 0x00 | 2 / 1 | **stop** pan / tilt: halts that axis' pulses. Cleared before every move, set by stop | D `_ms32006_move` @ 0xfa510, `_ms32006_stop` @ 0xfb070; V (`cam ptz stop` → 0x87) |
| `b`+1 | 7:0 | step-rate divisor, low byte | D `_ms32006_set_pps` @ 0xf9d9c |
| `b`+2 | 5:0 | divisor bits 13:8 | D, same |
| `b`+2 | 7 | drive mode (`_ms32006_set_drv_mode(axis, 0)` sets it; mode ≠ 0 clears bits 7:6) | D @ 0xf9f2c |
| `b`+3 | 7:0 | steps to move, low byte | D `_ms32006_move` |
| `b`+4 | 3:0 | steps bits 11:8 (move length is 12 bits: **max 4095 per command**) | D, V |
| `b`+4 | 6 | direction bit | D, V |
| `b`+4 | 7 | set on every move command (`0x80 \| dir<<6 \| steps>>8`) | D, V |
| 0x09 | 7 / 6 | **kick** pan / tilt: starts the programmed move; self-clears (reads 0 afterwards) | D (RMW `\| 0x80`/`0x40` after programming) @ 0xfa6dc, 0xfafb0; V |
| 0x09 | 3, 2 | IR-cut drive: the vendor writes 8 / 4 / 0 here in 300 ms pulses (`@ 0xf96f0`, `@ 0xf9750`); driven by `cam::ir`, see [ir.md](ir.md) | D |
| 0x0b / 0x0d | 4 | **busy**: 1 while pan / tilt pulses are being output | D `_ms32006_get_cur_status` @ 0xfa250, V |
| 0x0b / 0x0d | 3:0 + next reg | 12-bit step counter: counts steps issued since power-on, **saturates at 0xfff**, not a position | V (0x64, 0xc8, 0x1f4 after 100, 100, 300 steps; 0xfff after more) |
| 0x0b / 0x0d | 7 | reads 1, no function found | V |

Step rate: `divisor = (15 000 000 / pps) >> 7`. The vendor programs **600 pps** (divisor 195 = 0xc3, the value
found in reg 0x01/0x05 on a running camera) and has 1200 pps as its fast rate [D, runtime memory of `device`].
A 4095-step chunk takes about 6.8 s at 600 pps; measured full pan travel (7460 steps with margin) 12 s [V].

Registers 0x0a, 0x0c–0x0f and everything above 0x10 read 0 (apart from the 0x80 mirror) [V].

### Power-on state left by the vendor [V]

`i2cdump`: `81 c3 80 xx xx c3 80 xx xx 00 00 xx xx xx` for registers 0x00–0x0d. `cam::ptz` `init` writes the
same values (test `init_programs_the_registers_the_vendor_leaves_behind`).

## Move sequence (per axis chunk)

1. write `b`+3 ← `steps & 0xff`
2. write `b`+4 ← `0x80 | dir<<6 | steps>>8`
3. reg 0x00 ← reg 0x00 with this axis' stop bit cleared (RMW)
4. reg 0x09 ← reg 0x09 | this axis' kick bit (RMW)
5. poll reg 0x8b / 0x8d bit 4 until clear (the driver polls every 10 ms)

The two axes run independently: the driver starts both before it waits for either.

## Coordinates, travel and homing

- Travel from the vendor's per-axis table (runtime memory of `device`, struct stride 0xac, field +0xec): **pan
  7360 steps, tilt 1240 steps** [D/V]. 7360 steps is nearly a full turn: the head sees the same objects at both ends.
- **No optocoupler is readable from Linux on this unit** [V/I]: every MS32006 register the vendor touches is in
  the table above (all `i2c_read/write` call sites in `device` were checked), none holds a sensor level; the
  vendor's GPIO edge-poll table for the optocouplers is present but disabled (`enable` = 0 for both axes in the
  live process), and the only candidate sysfs GPIOs (100, 104, plus 9, 224, 225, 232, 233) never changed during
  full sweeps. The `OptocouplerPos` strings in `device` serve other motor types. So the vendor homes
  **open-loop against the mechanical end stop** (it overdrives by 100 steps, `+100`/`-100` in the state
  machine @ 0xfb64c), and so do we.
- Directions [V, from frames]: pan x grows with direction bit **0**, so the pan stop is at x = 0 with direction
  bit **1**; tilt y grows with direction bit **1**, the tilt stop is at y = 0 with bit **0**.
- **Homing by ear** (`cam::ptz::homing`): only the daemon (`cam serve`) does it, because only it captures
  audio; `cam ptz` always homes by time.
  - Tilt, then pan (a sound then belongs to one motor). Each axis listens to the quiet head (8 capture frames,
    at most 500 ms), then is driven toward its stop at 600 pps.
  - The axis halts when the median level of the last 32 capture frames (≈0.7 s) is ≥ quiet + 11 dB (rule A,
    off when "quiet" reads above −40 dBFS) or ≥ running motor + 4 dB (rule B; the motor level is frames 15–47
    after the kick).
  - A stop never heard: that axis runs its full travel. No audio: the axes not yet homed are homed by time.
  - Worst case about 16 s (tilt 1340 + pan 7460 steps at 600 pps, plus listening), against 12 s timed.
  - Daemon stderr says which: `ptz: Tilt heard the stop after N ms of driving (quiet … dBFS, motor …)`,
    `ptz: Pan ran its full travel without hearing the stop (…)`, `ptz: homing by ear unavailable (…)`, then
    `ptz: homed: ByEar { heard: [true, true] }` (pan, then tilt; `false`: that axis ran its full travel) or
    `ptz: homed: Timed(…)`.
  - Measured on one camera [V; AEC off, ANR off, `mic_volume` 100, the daemon's defaults, other audio
    settings untested; levels in `cam/src/ptz/homing_levels.rs`]:
    quiet −46 dBFS; pan runs at −41 and stalls at −27; tilt runs at −39…−38 and stalls at −33, 2 dB from
    rule A on either side. Once, at a cold boot, "quiet" read −29 dBFS; rule B still heard the stop.
  - Not handled: a sound of its own that lasts ≥0.7 s and is 11 dB above the quiet room (speech, a TV,
    talkback on the speaker) halts the axis early, and the head then thinks it is at the stop.
- Timed `home`: pan 7360+100 steps with bit 1, tilt 1240+100 steps with bit 0, both at once. Afterwards the driver's
  position is (0, 0). `goto(x, y)` then moves the difference. The driver keeps the position only inside one
  process: every `cam ptz` invocation that moves homes first, by time (12 s from anywhere).
- Check against the vendor: homing, then pan x = 6195 and tilt y = 650, gives the pose the vendor stack ends in at
  boot (`default_x=6195`, `default_y=650` in `/mnt/para/0_ptz.cfg`), frame for frame.
  The same manual register sequence was used for that frame as `cam ptz move 6195 650` runs.

## Presets

`/mnt/para/0_ptz.cfg` is one XML element with attributes `bval<N>` (slot used), `pos<N>_x` (pan steps),
`pos<N>_y` (tilt steps). Unused slots have `bval="0"` and are refused. `cam ptz preset <n>` reads slot `n` and moves there after a home.

## `cam ptz`

```
cam ptz [--dev /dev/i2c-2] [--cfg /mnt/para/0_ptz.cfg] home | move <pan> <tilt> | stop | preset <n>
```

`stop` from a second process is owner-mediated: the serialized hardware owner abandons the
entire active move or home, discards queued motion, and invalidates position until a successful
home. It acknowledges only after the hardware stop call for both axes returns successfully.
The request bypasses the ownership locks; a two-second caller timeout reports failure, never
"stopped". The local mode-0600 `cam-controls.stop` Unix datagram endpoint lives next to the runtime
lock. With no active owner, the command takes the locks and stops both axes directly.
MQTT `camera/<id>/stop/set = PRESS` uses the same owner and reports `stop/state` as `stopped`
or `error`. Retained stop commands are ignored. Unknown pan/tilt states clear retained coordinates.

## Vendor function offsets (`/mnt/app/device`, VMA)

| Function | VMA |
|---|---|
| `_ms32006_i2c_read` / `_i2c_write` | 0xf94ec / 0xf9608 |
| IR-cut pulse byte / pulse state machine | 0xf96f0 / 0xf9750 |
| `_ms32006_reset` | 0xf97f0 |
| `_ms32006_set_motor_type` | 0xf9888 |
| `ms32006_set_mtrinfo` (per-axis tables) | 0xf9940 |
| `_ms32006_set_pps` | 0xf9d9c |
| `_ms32006_set_drv_mode` | 0xf9f2c |
| `ms32006_init` | 0xfa020 |
| `_ms32006_get_cur_status` | 0xfa250 |
| `ms32006_get_state` | 0xfa434 |
| `_ms32006_move` (+ wrapper) | 0xfa510 (0xfa50c) |
| `_ms32006_clear_step_reg` | 0xfa774 |
| chunked move with a 20-step lead-in at a second rate [I] | 0xfa7f0, 0xfab78 |
| kick (`_ms32006_apply`) | 0xfafb0 |
| `_ms32006_stop` | 0xfb070 |
| `_ms32006_proc_pt`, `_ms32006_proc` (state machine, thread) | 0xfb1c0, 0xfb938 |

Not reproduced (not needed here): backlash compensation, cruise tracks, the sleep position.

## Speed profile and repeatability [V]

`cam::ptz::Profile { start_pps, max_pps, ramp }`: each leg starts at `start_pps`, rises linearly to
`max_pps` over `ramp` and brakes symmetrically before its target (`v² = start² + 2·a·d`). Chunks are
~100 ms while ramping or holding and one long chunk at cruise; the rate is written between chunks
(writing the divisor during a busy chunk has **no effect**: a 1200-step chunk at 600 pps took 2009 ms
with or without a 1200 pps write at 0.5 s). The gap from "idle seen" to the next kick is 3–7 ms.
Homing always uses the vendor 600 pps with no ramp (`Profile::homing()`).

Repeatability: a frame of `main` (1920×1440) at (6195, 650), 10 round trips
(3262, 739) ↔ (6195, 650) at the rate, a frame at (6195, 650) again, phase-correlation shift (the RAM probe
`cam/examples/probe90.rs`, stock stack stopped, ≈1 step per pixel horizontally):

| Max pps | Ramp (start, time) | Drift px (x, y) | 10 trips |
|---|---|---|---|
| 600 | 200, 0.8 s | −1.4, 1.0 / −0.1, −0.1 | 114 s |
| 800 | 200, 0.8 s | −3.6, 0.1 | 91 s |
| 900 | 200, 0.8 s | −3.6, −0.2 / −3.4, −0.1 | 84 s |
| 1000 | 200, 0.8 s | −6.0, −0.2 | 77 s |
| 1100 | 200, 0.8 s | −21.5, −0.1 | 72 s |
| 1200 / 1800 / 2400 | 200, 0.8 s | −88 / −147 / −165 | 68 / 52 / 44 s |
| 800 | 300, 1.5 s | −1.9, −0.1 | 104 s |
| **900** | **300, 1.5 s** | **−0.4, −0.0** | 97 s |
| 1000 | 300, 1.5 s | −13.8, −0.1 | 91 s |

The vendor's 1200 pps "fast" rate loses steps on this unit. The highest rate without measurable drift
(≤ 3 px) is 900 pps with the gentle ramp, so `PTZ_MAX_PPS` (`ptz::MAX_PPS`) = 900 / 1.25 = **720**, and
the first-run profile is 300 → 720 pps over 1.5 s (`ptz_start_speed` 300, `ptz_ramp` 1500 ms: the
measured passing shape; the 200 → 800 pps ramp drifted at 800–900).
Full pan travel (home to 7360) after homing: 13.1 s at 600, 10.1 s at 800, 8.3 s at 1000 pps (ramped).
