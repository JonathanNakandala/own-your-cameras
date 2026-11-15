# ISP picture controls

Image-quality and exposure items set through `/dev/nvt_isp`, exposed as settings
(`cam::isp_iq::ITEMS`, [../settings.md](../settings.md)). Tags as in [README.md](README.md#evidence-tags). Applied on the hardware thread through the one
`/dev/nvt_isp` owner (the lens's `NvtIsp`), after orientation at startup and on every change.

## Transport and safety

- `vendor_isp_{get,set}_iq` = ioctl type `'q'` (get `0xc00471nn`, set `0x400471nn`), `vendor_isp_*_ae` =
  type `'e'` (`0xc00465nn` / `0x400465nn`; stock `vendor_isp_set_ae` @ 0x154a10, get @ 0x153e80). The ioctl
  size field encodes a pointer, not the payload.
- Payload `{u32 isp id = 0, u32 value}`, **8 bytes both directions**, from the kernel's own size tables
  [D]: `nvt_iq.ko` (SHA256 `c435ceda…d25e`) `.data + 0x5a24 + item*8` = `{item, 8}`
  for items 10–15 and 20; `nvt_ae.ko` (`281a0f42…`) `.data + 0x2de8 + item*8` = `{item, 8}` for 12 and 14.
  Every item and direction is in the `NvtIsp` allowlist with that size ([../hdal-abi.md](../hdal-abi.md#isp-payloads)).
- **The kernel set path ignores a rejected value and still returns 0** (`iqt_api_set_cmd` @ .text
  0x1bcdc: the `blx` is followed by an unconditional return of 0; `aet_api_set_cmd` @ 0x14df4). Measured: every out-of-range set below
  left the readback at the previous value. So every set is followed by a get, and a mismatch is an error
  (the previous value stays, the `settings` sensor reports it).
- Accepted ranges are the kernel UI tables (`iq_ui_set_info` @ .text 0x21334, `.data + 0x10a88`, `max`
  exclusive; `ae_ui_set_info` @ 0x16970).

## Items

Stock evidence: `device` 4417 message handlers (VMA) that read the `0_isp.cfg` 0..255 values, scale and
issue the ioctl; the "apply all" path 0xc9bf0 re-applies them at start and day/night switches.
Live [V]: on `cam` with frames flowing (sub-stream JPEGs, the RAM probe `cam/examples/probe90.rs`).
Mean luma, luma spread (std), mean HSV saturation, edge energy (mean |∇Y|), per-channel means.

| Key | Item | Range | Stock (handler) | First-run default | Live: readback and frame effect |
|---|---|---|---|---|---|
| `isp_brightness` | IQ 14 | 0–200 | `brightness`, curve base 116 (0xc5cfc) | 90 | read 90 at start; 200 → luma 217, 0 → luma 8.7 (from 95); 201 refused |
| `isp_contrast` | IQ 13 | 0–200 | `contrast`, base 129 (0xc5dc0) | 101 | 200 → spread 75.9, 0 → spread 0.2 (from 53.3) |
| `isp_saturation` | IQ 12 | 0–200 | `saturation`, base 128 (0xc5f64; stock forces 0 at night) | 100 | 200 → saturation 0.53, 0 → 0.00 (from 0.33), luma unchanged |
| `isp_sharpness` | IQ 11 | 0–200 | `sharpen`, base 77 (0xc6034) | 60 | 200 → edge 12.5, 0 → 5.6 (from 8.6) |
| `isp_hue` | IQ 20 | 0–360° | `hue`, 128 = 0° (0xc5e84) | 0 | 180 → R/G/B 78.6/83.7/81.3 vs 80.9/82.1/82.2; 361 refused |
| `isp_noise_reduction` | IQ 10 (2D NR level) | 0–200 | kernel UI only (stock never sets it) | 100 | 0 → edge 10.9, 200 → edge 5.8 (from 8.7) |
| `isp_exposure` | AE 14 (EV) | 0–32, 16 = 0 EV | kernel UI only | 16 | 32 → luma 143, 0 → luma 4.0 (from 84) |
| `isp_anti_flicker` | AE 12 | 50 Hz / 60 Hz | `antiflick`, 1 = 60 Hz (0xc6680) | 50 Hz | readback 1/0; exposure re-converges (luma 72 → 80); 3 refused |

`0_isp.cfg` on the tested unit: brightness/contrast/saturation/sharpen/hue 128, antiflick 3 → levels
90/101/100/60, hue 0°, 50 Hz, through the per-sensor curve (table @ .data 0x356f30) with this unit's
bases fitted from the capture below.

## Not exposed, with reasons

| Item | Why |
|---|---|
| IQ 22 3D NR level (0–200, 8 B) | accepted and reads back, but no measurable effect on still frames (edge/luma within noise at 0 and 200); temporal NR needs a motion test — not shown to take effect |
| IQ 40 3D NR param (8040 B), IQ 45 WDR/DRC (744 B), AE 32 target Y (180 B), AE 47/48 exposure/gain bounds (12 B) | only a few fields of large structs are evidenced from stock writes (read-modify-write), field meanings unverified; IQ 40 exceeds the 4 KiB buffer. Not written without a pinned layout |
| AWB 11 R/B ratio (12 B) | needs AWB manual mode; stock never uses it |
| IQ 15 night/B&W | owned by the IR day/night switch ([ir.md](ir.md)) |

## Stock capture [V]

On the stock stack, a RAM-only ptrace observer printed every `/dev/nvt_isp` ioctl (item, direction, first
payload words; it never writes to the tracee) while the stock HTTPS API (`SetImage`) set each control to 200, 30
and back to 128. On every
change stock re-applied all five levels, each as one 8-byte set `{0, level}`:

| UI value | brightness IQ 14 | contrast IQ 13 | saturation IQ 12 | sharpness IQ 11 | hue IQ 20 |
|---|---|---|---|---|---|
| 128 (default) | 90 | 101 | 100 | 60 | 0 |
| 200 | 152 | 156 | 156 | — | — |
| 30 | 21 | 23 | — | — | — |

This confirms the items, the payload and the per-sensor curve on the message path; its bases for this
unit fit exactly: brightness 116, contrast 129, saturation 128, sharpness 77
(`v = ui ≤ 127 ? ui·base/128 : base + (256−base)·(ui−128)/128`, then `v·200/255`). The first-run
defaults are what stock applies for the unit's `0_isp.cfg`: brightness 90, contrast 101, saturation
100, sharpness 60, hue 0° (our daemon also reads brightness 90 at start). The stock API's anti-flicker
change did not reach AE 12 in this capture (the anti-flicker evidence stays the handler @ 0xc6680 plus
the live readback). Failed logins count toward the stock web API's own lockout.

## 3D noise reduction in the video processor (`isp_3dnr`)

The stock stack runs the processor with `ctrl func 03830000` and `3dnr_ref 4` (`/proc/hdal/vprc/info` [V]); `cam`
without the setting runs `02000000` (autofocus statistics only), so no temporal noise reduction smooths the sensor
noise before the encoder and night frames are grainy. What `libhdal` does with the bits of
`HD_VIDEOPROC_CTRL.func` (`hd_videoproc_set`, PARAM_CTRL, 0x396e0 [D]):

| HDAL bit | Kernel flow flag | Notes |
|---|---|---|
| `0x00010000` | `0x08` | 3D noise reduction; the only bit that reads `ref_path_3dnr` |
| `0x00020000` | `0x01` | set by stock |
| `0x00040000` | `0x02` | not set by stock |
| `0x00080000` | `0x04` | not set by stock |
| `0x00400000` | `0xc0` | not set by stock |
| `0x00800000` | `0x80` | set by stock |
| `0x02000000` | `0x100` | autofocus statistics (`HD_VIDEOPROC_FUNC_AF`) |
| `0x01000000` | `0x200` | set by stock |
| `0x04000000` | `0x800` | not set by stock |

The strings of the kernel flow module name the functions but not which bit is which; `cam` sets the bits stock sets.
Other decoded values [D]:

- `ref_path_3dnr` is a processor output path id; the driver uses its low byte minus one, so `HD_VIDEOPROC_0_OUT_n`
  (low byte n+1) is output n.
- Reference path: the whole processor input frame (the driver refuses another size), pixel format `NVX2` =
  `0x610c2420` (compressed YUV420; `NVX1h5` is `0x610c1420`, `RAW12` `0x410c0000`), output function `0x800` set
  through `HD_VIDEOPROC_PARAM_FUNC_CONFIG` (id 24; the videoenc one is id 26, not the `func` field of
  `HD_VIDEOPROC_OUT`), depth 1.
- The OUT parameter takes outputs 0 to 4; outputs 5 and up use OUT_EX (id 13).

### The graph that works [V]

Stock feeds its main encoder from the reference path: encoder input `NVX2` with `in_func 2` (`func 00000002` in
`venc/info`) and its own input frame-rate control (stock's 20 of 25 fps). `cam` does the same when the setting is on
and the main stream is the full 2592x1944:

- proc output 0 is the `NVX2` reference at the capture rate (30 fps), `0x800`, the single-buffer mode
  (`vprc: enable one-buf func at path 0, (3ndr_ref=1)` in the kernel log), and is also the main encoder's input;
- the main encoder takes `NVX2` with function 2 and a frame-rate control of `fps/30` (15 of 30 by default);
- the sub stream (output 1) and the snapshot (output 2) stay plain scaled YUV420 from the same denoised frame;
- `PARAM_CTRL` with `03830000` and `ref_path_3dnr` = output 0 is the last configuration step.

Everything else in `cam/src/pipeline.rs` is unchanged. What did **not** work, so nobody tries it again: a reference
path beside the streams with nothing bound to it is never enabled (`3dnr ref path(n) enable = 0, buffer = 0, drop
frame` for every frame, and **every output stalls**), and pulling and releasing that path's frames from the media loop
does not count as a consumer.

### Memory

One pool of identical 7.2 MiB blocks cannot hold this graph. `/proc/hdal/comm/info` showed all four taken: two raw
sensor frames (0x605480 bytes each) and two `NVX2` main frames (0x567f80, the single-buffer mode takes two), leaving
nothing for the sub stream and the snapshot, whose outputs then warned on every frame and delivered nothing. The
graph therefore asks for pools sized per output, smallest first (the driver takes a block from a pool that fits):
3 x 480 000 B (the sub stream, as stock's three 0x70a00 blocks), 2 x 1 900 000 B (the snapshot), 2 x 5 800 000 B
(the compressed frames) and 2 x 6 400 000 B (the raw frames). That is about 29.6 MB against 28.8 MB for the single
pool; the media area keeps about 1.5 MB free (`Free` in the `VB PUB INFO` line). The plain graph keeps its single
pool.

### Measured effect [V]

Same night scene, same settings (2592x1944, 15 fps, 4096 kbit/s), the same RAM-only diagnostic run with the setting
off and on, in a flat wall region at full resolution (mean absolute difference, median over 280 P frames):

| | off | on |
|---|---|---|
| frame to frame (temporal noise) | 0.32 | 0.11 |
| within a frame (high-frequency noise) | 0.41 | 0.15 |

About 65 % less noise at the same bitrate; a late frame of the long run, after autofocus settled, is as sharp as the
baseline. A first frame of a run looked soft because autofocus had not settled, not because of the noise reduction.

Under load: four minutes with three viewers (main over TCP, main over UDP, sub over TCP): 3602, 3602 and 2401 frames,
no decoder errors, no kernel warnings, the trial proven at 180 s. Live changes with it on: a frame-rate change
restarts only the main encoder (4 ms, 0.2 s gap in the stream), a bitrate change likewise, a resolution change away
from 2592x1944 is refused (it would break the size match and stall every output), the sub stream never moved.

### Safe start

See `settings.md` (`isp_3dnr`) and `recovery.md` (the trial marker): a graph the processor does not accept stalls
every output and a stalled stream resets the camera, so a start with the setting on writes `/mnt/para/cam-3dnr.trial`
first and deletes it after 180 s of both streams delivering; a start that finds it runs the plain graph.
