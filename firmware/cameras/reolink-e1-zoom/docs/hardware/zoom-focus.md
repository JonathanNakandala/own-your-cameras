# Zoom and focus: `vendor_isp` items on `/dev/nvt_isp`

The E1 Zoom's YT3005 lens has two steppers on an AN41908 driver (`nvt_mtr_an41908.ko`, loaded by `start_app` with
`mtr_an41908_OS05A10_YT3005.cfg`). User space never talks SPI: it sends `vendor_isp` items to `/dev/nvt_isp` and the
kernel driver moves the motors. The driver is `cam::zoom`; the CLI is `cam zoom` / `cam focus`.

Evidence: the stock `device` binary (see [README.md](README.md#evidence-tags) for its hash and the tags), read with
`arm-none-eabi-objdump -d`, then every item exercised on a camera with the vendor stack stopped (dmesg and frames
below).

## Lens curve rows

Before either motor homes, the daemon checks the lens variant (1 or 2) and the read-only ZFTable in mtd7 (below), then
installs and reads back all 50 `MOTOR_MISC` curve rows plus the focus backlash (`Lens::configure`). The rows keep the
ZFTable's zooms and sharp points; their windows are ours: 300 steps either side of each sharp point, widened further
around learned focus points (the map, below), clamped to mechanical travel. Unused rows are sentinels and row 49
repeats the last point. Invalid calibration or a readback mismatch fails startup. The vendor's own windows are
sharp −144/+100 at zoom 0 and ±64 elsewhere, which `cam zoom init` still installs; a room scene is sharp outside them
at mid and tele zoom (Measured limits). Rows can be rewritten after homing (`cam zoom rows <below> <above>` does
it from the command line; the daemon reinstalls them when learning changed them, while nothing moves, at most once a
minute). The ZFTable itself is never written.

## Transport [V]

`device` links its own `libvendor_isp` statically (no `.so` on the camera). The functions are thin ioctl wrappers on
one descriptor:

| Function | VMA | ioctl |
|---|---|---|
| `vendor_isp_init` | 0x153d44 | `open("/dev/nvt_isp", O_RDWR)`, no handshake |
| `vendor_isp_get_common(item, p)` | 0x1584dc | `_IOWR('c', item, 4)` = `0xc00463nn`, arg = pointer to the item's struct |
| `vendor_isp_set_common(item, p)` | 0x158df4 | `_IOW('c', item, 4)` = `0x400463nn` |
| `vendor_isp_get_af` / `_set_af` | 0x15559c / 0x155b5c | same with type `'f'` (`0xc00466nn` / `0x400466nn`) |

The size field is always 4 (the pointer); the kernel copies its own struct size, so `cam` passes a 4 KiB buffer.

## Items [V]

| Item | Name (vendor log strings) | Payload |
|---|---|---|
| 30 | `ISPT_ITEM_VA_DATA` | get: `[0]` ISP id 0 → `[1..=256]` VA cells (four 8×8 grids, 2052-byte struct). `Na_af_get_cur_va` sums all 256. |
| 41 | `ISPT_ITEM_MOTOR_FOCUS` | 20-byte struct, word 0 = motor command (below) |
| 42 | `ISPT_ITEM_MOTOR_ZOOM` | same layout |
| 43 | `ISPT_ITEM_MOTOR_MISC` | 132-byte struct: focus-curve rows, backlash (size from the kernel's own table, [../hdal-abi.md](../hdal-abi.md#isp-payloads)) |

Motor commands (word 0):

| Cmd | Item | Direction | Meaning |
|---|---|---|---|
| 1 | 41, 42 | set | init/home the motor (`Na_af_motor_init`). **The driver drops zoom moves until this ran** (`an41908_set_zoom_position() not initialized yet!`); `device` does it some time after boot, so killing it early leaves zoom uninitialised. Both together take ≈17 s. |
| 3 | 41 | get | `MTR_GET_FOCUS_RANGE`: `[3]..[4]` = focus window of the curve row for the *current* zoom (86..330 at zoom 0, 958..1086 at 1024), not the motor's travel |
| 3 | 43 | get | curve row `[1]` → `[18]` zoom, `[19]` focus min, `[20]` focus max |
| 1 | 43 | set | curve row `[1]` = (`[2]` zoom, `[3]` focus min, `[4]` focus max) (`Na_af_set_zftable_item`) [D] |
| 4 | 41, 42 | get | `[3]` low 16 bits = position, high 16 = direction flag |
| 5 | 42 | set | zoom to `[1]`; the driver drags focus along its curve and lands on a curve row (100 → 128) |
| 8 | 42 | set | zoom variant used when `Na_af_zoom_move`'s flag is 0; no caller does [D] |
| 6 | 41 | get | `[3]` (preset 0xff) non-zero while a lens motor moves (the vendor polls this for both motors) |
| 6 | 41 | set | focus to `[1]` |
| 7 | 41 | set | `MTR_SET_FOCUS_PRESS`: `[1]` direction, `[2]` target (backlash-compensated move) [D] |

Limits: dmesg at load says `an41908_init_zoom max_distance:1148`, `an41908_init_focus max_distance:1705`, but the
driver accepts zoom only up to the **last row of its curve** (1024 on the tested unit): 1025…1148 log
`set out of range pos!` and do not move. `cam zoom` checks against that row and refuses beyond it.

AF table (`'f'`, word 0 = ISP id): 31 `AFT_ITEM_ENABLE`, 34 search direction, 37 `AFT_ITEM_STEP_SIZE`
(`[1..=3]` coarse, fine, 1), 41 `AFT_ITEM_RETRIGGER`, 43 `AFT_ITEM_EXEC_STATUS` (1 done, 32 timeout). The kernel
`nvt_af` search is what `Na_af_start_af` (0xa15a4) triggers. On our pipeline it ran but ended in status 32 at a
visibly soft focus (301 where the sharp point was ≈200), so `cam` does not use it (below).

## ZFTable (the per-unit calibration) [V]

`<ZFTable>` in the `sp` partition (mtd7, `/dev/mtd7ro`, read-only; never erase it) is a comma list of hex words read
by `Sp_get_zoomfocustable` (0x65b50) and consumed by `Na_af_refresh_zf_info` (0xa5130) as pairs:

- pair *i*: zoom = low 16 bits of the even word, sharp focus = low 16 bits of the odd word;
- first pair's even word bits 24..25: lens variant (1 on the first tested unit, 2 on another), picks the spread table; first pair's odd word bits
  16..23: focus backlash (3 here, `Na_af_set_backlash`); top byte of other odd words: per-row spread class (0 here).

The tested unit: `1000000,300e6,20,f3,40,125,…,400,3fe` = 33 points (0, 230), (32, 243), (64, 293) … (1024, 1022).

A second unit has `2000000,500dd,20,de,40,11f,…,400,3f3`: lens variant 2, backlash 5, the same 33 zoom positions, (0, 221) … (1024, 1011).
The first build refused it ("unsupported factory lens calibration"), which made the daemon exit at start. In the stock
`device` the dispatch in `Na_af_adapt_lens_focus_range` is a 13-entry jump table on the variant: entries 1, 2 and 9
(and anything above 12) all go to the bare return at 0xa49fc, so variants 1 and 2 get the default windows above, and
the AF parameter loader (0xa4e0c) takes `variant - 1 < 2` as one group. `cam` therefore accepts 1 and 2 and still
refuses every other variant until its entry in that table has been read. [D]

`device` turns each point into a driver row (focus min, max) = (sharp − 144, sharp + 100) for row 0 and
(sharp − 64, sharp + 64) for the rest (`Na_af_adapt_lens_focus_range` @ 0xa4658 for lens variants 1 and 2), loads them with
`MOTOR_MISC` set cmd 1 and repeats the last row in slot 49. Verified: the 33 rows read back from the driver match
exactly (row 0 = 86..330, row 1 = 179..307 … row 32 = 958..1086). Those rows are the zoom limit and the focus window.

## What `cam` does

- `cam zoom init` — cmd 1 on focus, then zoom, waiting for each (≈17 s). Needed once per boot when `device` did not
  get to it.
- `cam zoom <pos>` — refuses beyond the driver's last row, then cmd 5 and waits for the motors (≤ 60 s, like
  `Na_af_wait_motor_free`).
- `cam focus <pos>` — cmd 6, refuses beyond 1705.
- `cam focus` — one contrast search (below) run to the end, printing every sample.
- `cam zoom info` — positions, focus window, zoom limit, VA, the driver rows and the parsed ZFTable.

Preconditions: vendor stack killed (it owns the same items) and **frames flowing** (`cam serve`), because VA and 3A
only run on a live pipeline. The pipeline must open capture with `HD_VIDEOCAP_PARAM_CTRL.func = AE | AWB | AF` and
videoproc with `ctrl_max.func` and `CTRL.func = HD_VIDEOPROC_FUNC_AF`: without the first the kernel logs
`ctl_sie_isp_set_iq_param() id 0 force disable AE/AWB` every frame (black night image, no exposure control); without
the second the VA cells stay 0. Both are set in `hdal::sys`.

## Autofocus in the daemon

The search (`cam/src/zoom/af.rs`) uses the ISP's VA and climbs from where the focus is:

- **Climb.** Sample the current focus (clamped into the driver window for this zoom), then step upward in the
  vendor's coarse step (12, or 8 in the tele third of the calibrated range, as `Na_af_start_af` picks) until a
  sample above the best one reads `af_climb_drop` % (default 2) below it, then the same downward. Samples already
  taken on a side count, and a new best re-centres the sides. A still indoor scene brackets its peak in 4–9 samples.
- **Fine and settle.** One coarse step either side of the best in the fine step (6 / 4), skipping positions the
  climb sampled (normally 2 moves), then the vertex of a least-squares parabola through the samples within 2.5
  coarse steps of the best one, approached from below so the gear backlash lands the same way every time. Near the
  peak VA is flat within about 1 % at zoom 0, so the single best sample wanders by up to 18 steps between runs on a
  still scene; the fitted vertex stays within 4.
- **Fallback.** A climb that would step out of the window before its side is bracketed (no contrast, or the peak
  beyond the window) runs the full coarse sweep of the window, then the fine stage. Only this path tests for a flat
  scene: if the best VA is under 10 % above the worst (dark or blank scene, or a near scene at a zoom whose window
  does not reach it) it settles on the ZFTable focus for this zoom (`calibrated`).
- Each sample waits for a stable VA (two reads 20 ms apart within 1/30, as `Na_af_get_cur_stable_va`). The kernel's
  own `nvt_af` search is not used: it ended soft on this lens.

Replayed on the recordings below (zoom 0, window 86..330, peak near 185): from the ZFTable park (230) the search
takes 11–12 focus moves, from the previous peak 7–8; the full sweep took 27–28. The flat zoom 512 / 1024 scenes
climb to the window edge and then sweep, 19–32 moves.

The daemon runs the search in steps on its hardware thread, one focus move or one VA read per step, so commands,
holds and stop keep working during the ≈1–1.5 s it takes. Every zoom first parks the ZFTable focus, as before.

- **Triggers:** the end of a zoom (absolute, relative, hold, preset), the end of a pan/tilt move (absolute, relative,
  hold, preset), a day/night switch (manual or auto, after 1 s for exposure to catch up), the Refocus button, and
  startup once the pipeline delivers VA. A search starts only once no hold runs.
- **Pan/tilt check:** a pan or tilt does not move the lens, so its trigger first reads one stable VA without
  moving. When the lens is still at the zoom and focus where the last successful search ended and VA is at least
  `af_move_drop` % (default 80) of that search's best, the focus is kept (`kept <pos>`, one sample in the
  attributes) and no search runs. Otherwise, with no reference, or after a cancel parked the ZFTable focus, it
  searches. An unsettled VA searches rather than fails.
- **Cancel:** any motion command abandons a running search and parks the ZFTable focus; the trigger that command
  fires then starts a fresh search. A trigger during a search restarts it from the current focus; at most one
  search is ever pending, and a pan/tilt never downgrades a pending zoom, button, day/night, startup or watchdog
  search to a check (a preset sends its move before its zoom).
- **Failure:** unsettled VA or a failed ioctl ends the search at the ZFTable focus and shows `failed: <reason>`.
- **Watchdog:** passive. After a search that found contrast, its best VA is the reference. While nothing moves, VA is
  read once a second; when it stays under `af_watch_drop` % of the reference for `af_watch_hold` seconds and is still
  (the samples spread no more than 10 % of their mean), it runs one search. At most one watchdog search per 10
  minutes. A people-moving scene is never still, so it never triggers; a flat-scene fallback or a day/night switch
  clears the reference. It never moves the focus except by a full search.
- **Focus map:** every `focused` search end is learned as (pan, tilt, zoom) → focus and kept in
  `cam-focus-map.toml` beside the settings (written at most once a minute after a change, and at shutdown; an
  absent or corrupt file is an empty map and one log line). The park point after a zoom, a cancel, a flat or failed
  search, and the start of the search after a pan/tilt is the focus the map predicts for the view: the offset from
  the ZFTable focus at the nearest learned points within 350 pan-equivalent steps (one pan step = 0.91 tilt steps),
  interpolated in zoom between a point at or below and one at or above the zoom, or carried from one point within
  256 zoom steps; otherwise the ZFTable focus. A new point replaces one within 100 steps and 32 zoom steps; the map
  holds 256 points, the oldest learned goes first. After a pan/tilt the lens first goes to a learned park focus that
  differs from its own by more than a fine step, then searches; otherwise the check above decides. The sensor
  shows `map_points`, `park` (`learned`/`factory`), `park_focus` and `rows_pending`.
- **Calibrate focus:** the button walks a grid of 8 pans (cell centres 920 steps apart) × 3 tilts (250, 620, 990)
  × 5 zooms (0, 256, 512, 768, 1024), serpentine over pan within each tilt row and alternating the zoom direction
  at each angle, so the lens never crosses its whole travel between points. Each point is one goto, a park at
  the map's prediction and one full search (`trigger: calibration`); a focused result is learned, a flat or failed
  one is counted as skipped. One step per tick on the hardware thread, so commands stay responsive, and any command
  but a setting or a reboot cancels the walk where it is (the points learned so far stay). It ends back at the
  starting pose with a normal zoom search. The sensor reads `calibrating <done>/<total>, <n> min left` meanwhile
  (the walk's own pace, 7 s a point before the first one) and keeps the outcome in its `calibration` attribute.
  Measured on the camera: the 120 points took 15 minutes and learned 82 (the rest were flat views of ceiling or
  wall). The camera is stationary, so a long walk is acceptable as long as the duration is announced.
- `autofocus` off: only the ZFTable focus, the Refocus button and the calibration walk; no automatic search, no
  check, no watchdog.

Measured limits (one indoor scene about 3 m away, recordings in `cam/tests/fixtures/2026-10-24-af/`):

- At zoom 0 the whole driver window spans only about 1.26× in VA: focus at the window edge, visibly soft, still
  reads about 78 % of the peak. The watchdog therefore fires on defocus at zoom 0 only with `af_watch_drop` near 90.
- At zoom 512 and 1024 such a near scene is sharp outside the driver's ±64 window, and the driver refuses focus
  outside it (`set out of range pos!` in dmesg): VA is flat across the window and the search falls back to the
  ZFTable focus. The vendor stack uses the same windows.
- Day and night (IR-cut filter out) sharp points at zoom 0 differ by about 4 steps, inside the plateau.
- With the rows widened to −400/+200 (recordings in `cam/tests/fixtures/2026-10-25-af-wide/`, diagnostic
  pipeline), a room about 3 m deep is sharp at 708 (zoom 256, ZFTable 596), 935 (512, ZFTable 837, factory window
  up to 901), 1085 (768, ZFTable 966) and 1124 (1024, ZFTable 1022, factory window up to 1086): outside the vendor's
  windows from zoom 512 on, which is why the map's rows are wide. A plain wall closer than 0.5 m gives almost no
  VA (under 1 M against 25 M for the room) and is not a usable scene.

## Measured run (night, room lights on, vendor stack stopped, `cam serve` running)

```
zoom 1148  → zoom failed: zoom 1148 is beyond the driver's 1024
zoom 1024  → zoom=1024 focus=1021   (driver-tracked)
focus      → af_focus=1014           2.9 s
zoom 0     → zoom=0 focus=142
focus      → af_focus=218            4.1 s, VA 3.5 M at the window edge → 25.9 M at 218
```

`make device-zoom CAMERA=<ssh host> AUTH=…` repeats it and saves the frames in `target/zoom/`.

## Image orientation

Mirror and flip are ISP **common item 44**, a 12-byte payload `[sensor_id, mirror, flip]`: `_IOW('c', 44, 4)`
sets it, `_IOWR('c', 44, 4)` reads it [D stock `0xc6258..0xc62e0`; the `nvt_isp.ko` item-size table at
`0x12128 + 44*8` holds 12]. The getter only writes set bits, so the read-back goes into a zeroed buffer.
The stock `dvr.xml` mounts this model mirrored (`ipc_mirror=1`, `ipc_flip=0`); the `mirror` and `flip`
settings ([../settings.md](../settings.md)) are relative to that, so both off sets sensor mirror 1, flip 0.
First-run values come from the stock `/mnt/para/0_isp.cfg`.

Mirroring anywhere else in the HDAL graph fails on this camera [V]: RAWALL videoproc output reports mirror-y
unsupported, the encoder input refuses direction (-34) and MIPI capture only flips CCIR input (-35). The sensor
route keeps the RAW10 graph and both streams unchanged.

## Speed, halt and hold-to-zoom

Loaded driver `hdal/af/nvt_mtr_an41908.ko` (the copy `start_app` insmods; SHA256 prefix `9345607f`),
item 42 (zoom) word 0 = command [D, then V with the RAM probe `cam/examples/probe90.rs`]:

| Cmd | Effect | Stock use | Measured |
|---|---|---|---|
| 2 | `an41908_set_zoom_speed(word1)`, quantized to 20/40/60/80/100 (else 20) | never (cfg `zoom_move_spd=80` at load) | full range 0→1024: 24.2 s at 20, 13.7 s at 40, 7.2 s at 60, 5.5 s at 80, 5.3 s at 100 |
| 6 | press move toward the long end (`word1` ≠ 0) or 0; stops by itself at the end | `Na_af_continue_move` @ 0xa5d28 | moves continuously |
| 7 | release: halts the running move | `Na_af_stop_move` @ 0xa5ea8 | a 600 / 300 ms press moved 0→64, 64→128 (and 128→64 back): the halt lands on the next 64-step section, 0.2–0.4 s after release |

`zoom_speed` is a setting (select 20/40/60/80/100, default the stock 80). There is no getter for the speed.
Hold-to-zoom (`move/set zoom_in|zoom_out`) is one press move; release sends the halt, the hold ends
when the motor stopped, then calibrated focus settles for the reached zoom (`zf.focus_at`, as an
absolute zoom does). A lost release ends at `ptz_hold_timeout`; the travel end ends it by itself.
Granularity of a short press is one 64-step section of the 1024 range.
