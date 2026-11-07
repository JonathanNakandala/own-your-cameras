# Day/night: IR-cut, IR LEDs, ISP mode, light sensor, status LED

What `cam::ir` (`cam/src/ir/`) drives and why. Tags as in [README.md](README.md#evidence-tags); [V] here means
with the vendor stack stopped and `cam serve` running.

## Summary

| Part | How | Evidence |
|---|---|---|
| IR-cut filter | MS32006 (`/dev/i2c-2`, 0x10) register **0x09**: write **0x04** = filter **out** (night), **0x08** = filter **in** (day), hold **300 ms**, then write **0x00** | D @ 0xf96f0 (`ircut(1)` → 8, `ircut(0)` → 4, `ircut(3)` → 0), D @ 0xf9750, its caller @ 0xfb9bc passes 300 ms; V (frames below) |
| IR LEDs | GPIO **134**, 1 = on | V: with the filter out the scene lights up in IR; R: the vendor had it at 1 in the dark room |
| ISP colour/B&W | `vendor_isp_set_iq` item **15** on `/dev/nvt_isp`: ioctl `0x4004710f` (`_IOW('q', 15, 4)`), payload `{isp id 0, night}`; get = `0xc004710f` | D @ 0xc6aac (sets `{id, mode == 1}`), D jump table of `vendor_isp_set_iq` @ 0x1576c4; V: 1 renders B&W (saturation 0.000), 0 colour; R: the vendor had it at 1 in the dark room |
| Light sensor | ADC **channel 1**: `/sys/bus/iio/devices/iio:device0/in_voltage1_raw`, **higher = darker** | D: `daynight_cds` thread @ 0xed430 reads `f8864(1, &v)`; V: lit room 858–1418, dark room 2590–2626, and lighting the IR LEDs lowers it by 30–40 |
| Status LED | GPIO **226**, 1 = on | V (sysfs read-back; the LED itself is not visible to the camera) |

Channels 0 and 2 read within a few counts of channel 1; only channel 1 is used.

## Auto mode (vendor `daynight_cds` thread @ 0xed430) [D/R]

- Thresholds from `0_isp.cfg` / `dvr.xml`: `color2bw_threshold=2300`, `bw2color_threshold=1900` (live values at
  `0x365850 + 0xa0/0xa4` in `device`).
- In colour, a reading **≥ 2300** held for the hold time switches to B&W; in B&W a reading **≤ 1900** held switches
  back. A reading on the other side restarts the hold. The hold is **4 s** (`[0x35a214 + 0x10]` = 4 at runtime); the
  thread polls every 100 ms.
- The 400-count band matters: the IR LEDs' own light lowers the reading by 30–40 once night mode is on.
- The vendor also flips AF item 45 (`vendor_isp_set_af`, `{0, mode ∈ {1, 2}}`) with the mode; `cam` does not run the
  kernel AF, so it skips that.

`cam` reproduces this: `Auto` (thresholds, 4 s hold) fed every 500 ms by `run_auto`. The first reading decides at once
(night at or above `to_night`, day otherwise).

**Open:** one daytime reading was 2518 (afternoon). That is above 2300, so either the room was dim then or
daylight through the window reads differently. Watch the first sunny day before trusting 2300/1900 unattended;
`cam ir auto --to-night N --to-day N` takes other values.

## Order of operations

- night: pulse the filter out → ISP B&W → IR LEDs on
- day: IR LEDs off → pulse the filter in → ISP colour

The coil is always released after a pulse, even when the drive write failed.

## `cam ir` / `cam led`

```
cam ir day | night | status | auto [--to-night N] [--to-day N] [--seconds N]
cam led on | off
```

Needs the vendor `device` killed (it owns the MS32006 and the ISP). The ISP switch only shows with frames flowing
(`cam serve`). `make device-ir CAMERA=<ssh host> AUTH=file` runs the acceptance: day/night/day frames into `target/ir/`, the status LED,
and the auto mode with thresholds just under the room's reading so the IR LEDs' own light flips it night → day → night
on its own. Then it reboots the camera.

## Verification frames (`sub`)

- Lit room, IR-cut pulses alone: 0x04 gives R/G/B 119/101/72 against 101/96/73 for 0x08, reproducibly over two
  cycles (more red and brighter = IR passing). With GPIO 134 on: behind 0x04 the scene floods with IR
  (R/G/B 57/61/130 under colour AWB), behind 0x08 barely changes. So 0x04 = filter out.
- `cam ir night` in the lit room: saturation 0.000 (B&W), IR-lit; `cam ir day`: colour again.
- Dark room: day frame black, night frame B&W with the IR-lit scene, day frame black again.
- Auto with `--to-night 2621 --to-day 2611` in the dark room (reading 2626 with the LEDs off, 2590 with them on):
  night at 0 s, day at 5 s, night at 11 s, day at 16 s.

A physical "cover the sensor" test was not done; the room going dark between runs and the IR LEDs' own light
served as the real light changes.
