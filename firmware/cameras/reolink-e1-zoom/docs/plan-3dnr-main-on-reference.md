# Plan: 3D noise reduction with the main encoder on the reference path

Status: **built and verified on a camera in RAM-only sessions; not flashed.** Results are in
`hardware/isp-iq.md` (the graph that works, memory, measured effect). What differed from this plan: the reference path
needs one single-buffer `NVX2` output and the memory pools had to be split per output (section 4 did not foresee
that), and a diagnostic-only `--retune` option exists to test live changes without a broker. Evidence tags follow
`hardware/README.md`: [V] verified on the camera, [D] read from the stock binaries, [I] inferred.

## 1. The problem

At night the picture is much grainier than on the stock firmware. The video processor runs with only the
autofocus statistics on (`func 02000000`), so no temporal noise reduction smooths the sensor noise before the
encoder, and the encoder spends its bitrate on grain. Stock runs `func 03830000`, which includes 3D noise
reduction (3DNR) [V]. The encoder settings that were cheap to match (High profile, level 5.1, quantiser cap 45,
static time 4) are already done; 3DNR is the remaining gap.

## 2. What the experiments proved

Four RAM-only sessions on the unit, with a reference path added **beside** the existing streams [V]:

- The driver accepts the whole configuration: control `03830000`, reference output 3, a full-size `NVX2`
  output with output function `0x800`.
- It then drops every frame of **every** output, because the reference path "enable = 0, buffer = 0". A
  reference path nobody consumes is never enabled, and pulling its frames from our loop does not count.
- So 3DNR cannot be an add-on. The path that carries the reference has to be a path something real consumes.
  On stock that something is the main encoder.

## 3. How stock does it, and how that differs from today

| | Today (`cam`) | Stock firmware | Proposed |
|---|---|---|---|
| Processor control | `02000000` (autofocus) | `03830000`, reference = output 4 | `03830000`, reference = output 0 |
| Main output (proc out 0) | YUV420, main size, `fps/30` rate | `NVX2` (compressed) 2560x1920, rate 1/1 | `NVX2` 2592x1944, rate 1/1, output function `0x800` |
| Main encoder input | YUV420 | `NVX2`, input function 2, frame-rate control 20/25 | `NVX2`, input function 2, frame-rate control `fps/30` |
| Main size choices | 2592x1944, 1920x1440, 1280x960 | fixed 2560x1920 | **2592x1944 only while 3DNR is on** |
| Sub stream, snapshot | scaled YUV420 outputs | scaled YUV420 outputs | unchanged |
| Capture format | RAW10 | RAW12 | RAW10 first, RAW12 only if needed |
| Noise reduction | none before the encoder | temporal, in the processor | temporal, in the processor |

Why the main stream must be full size: the driver requires the reference frame to be the same size as the
processor input (`3dnr size not match`, `drop all frames` in `kflow_videoprocess.ko` [D]), and the main
encoder cannot scale. We run the processor on the whole 2592x1944 frame, so the main stream is that size.
The sub stream and the snapshot are scaled from the same denoised frame and are unaffected otherwise.

What does not change: the sub stream, snapshots, audio, RTSP, ONVIF, MQTT, the watchdog and the PTZ code. Main
bitrate and frame rate stay live settings.

## 4. Design

### 4.1 Graph (`cam/src/pipeline.rs`, `hdal`)

When 3DNR is on and the main size is 2592x1944:

1. `videoproc_set_dev_config` allows the 3DNR bits in the control maximum (done in the experiment).
2. Proc out 0 is configured as `NVX2` at the capture rate (1/1, 30 fps) with output function `0x800` set through
   `PARAM_FUNC_CONFIG` (id 24). It is also the reference path.
3. The main encoder gets an `NVX2` input, input function 2 through the encoder's `PARAM_FUNC_CONFIG` (id 26),
   and an input frame-rate control of `fps/30` (the encoder drops the extra frames, as stock's 20/25 does).
4. `videoproc_enable_3dnr(ctrl, out 0)` is the last configuration step, before anything starts.

When 3DNR is off, or the main size is not 2592x1944, the graph is exactly today's.

Code removed from the experiment: the separate reference output slot 3, `videoproc_drain_ref`, the
`hd_videoproc_pull_out_buf` / `release_out_buf` declarations and stubs, and the `CAM_3DNR` environment switch.
Kept: the control-bit and pixel-format constants, `videoproc_enable_3dnr`, the `FUNC_CONFIG` writes.

New hdal surface: an encoder input kind (YUV420 or NVX2) and input frame-rate on `videoenc_configure` /
`videoenc_retune`; a processor output kind on `videoproc_set_out`; `Fake` records both so host tests can assert
the order.

### 4.2 Live changes and the trap they create

`retune_main` restarts only the main path and is how resolution, frame rate and bitrate change live. With 3DNR
on:

- **Frame rate and bitrate** change as today (encoder only; the processor output stays at 30 fps).
- **A resolution change away from 2592x1944** would break the size match and stall every output [V]. It is
  refused with the existing "refused" report (the settings layer already reports applied versus requested), and
  the setting snaps back. Moving back to 2592x1944 is fine.

### 4.3 The setting

One new switch, `isp_3dnr`, default **off**, section Picture. It is read **at start only**: the graph topology
cannot change live (the processor control and the main output format are fixed before start). Its row in the
settings table says "applies after reboot", a new category beside "live" and "next move". Turning it on while the
main size is not 2592x1944 is accepted and stored, and the daemon logs and runs without it, so a stored value can
never stop the camera from coming up (the same rule persisted main-stream values already follow).

### 4.4 Safety: a trial boot, because a stall resets the camera

A stalled stream is reset by the watchdog after about 5 s, and three counted starts select the vendor stack
(`recovery.md`). The experiment showed that a wrong 3DNR graph stalls everything, so a bad build must not be able
to loop the camera into stock:

1. Before enabling 3DNR the daemon writes `/mnt/para/cam-3dnr.trial`.
2. If the file exists at the **next** start, the previous attempt never proved healthy: run without 3DNR, log
   why, and leave the file so the setting stays "tried and failed" until a human clears it.
3. The file is deleted once the stream has delivered frames from both encoders for the existing 180 s
   qualification window.

This costs one extra reset in the worst case and never reaches the vendor fallback. It reuses the 180 s health
window the crash counter already has.

## 5. Unknowns, and the hardware ladder that settles them

Each rung is a RAM-only diagnostic session (`device-session`, nothing flashed, the camera ends on stock and is
restored with `cam.enabled` plus a reboot, about 8 minutes per run). The diagnostic run reads its settings from
`/mnt/tmp/cam-settings.toml`, so the 2592x1944 main size can be set per session. A rung only moves on when its
pass line holds.

| # | Question | Pass line |
|---|---|---|
| 1 | Does the encoder accept `NVX2` with input function 2 and a 15/30 frame-rate control? | Encoder status `NEW > 0`, no `drop`/`err`; kernel log free of `3dnr ref path ... drop frame` |
| 2 | Is the reference actually enabled and do all outputs flow? | Processor status shows output 0 `PROC/PUSH` counting at 30/s; sub and snapshot deliver |
| 3 | Is RAW10 capture enough, or does 3DNR need RAW12 as on stock? | Decoded stream clean; if not, test RAW12 (sensor mode change, bigger scope) |
| 4 | Does it hold under load: three viewers, 4 minutes? | Zero decoder errors, no sub starvation (the sub stream starves at main 20-30 fps today), no watchdog reset |
| 5 | Memory and CPU headroom | Free memory stays above the level the production daemon needs (about 21 MiB free measured with the extra path); load no worse than today |
| 6 | Does it look better? | Same static night scene, 3DNR off then on: lower mean frame-to-frame change in static regions, smaller key and P frames at the same bitrate, and side-by-side 1:1 crops |
| 7 | Do live changes behave? | Bitrate and fps changes apply; a resolution change is refused and snaps back; no stall |
| 8 | Does the trial-boot protocol work? | Kill the run mid-trial: next start runs without 3DNR and logs the reason; a healthy run deletes the marker |

Expected outcome for rung 6 is the part I cannot promise: the processor's 3DNR strength is whatever the stock
tuning files set, and `cam` does not write those. If the gain is small, the work still leaves a cleaner graph
that matches stock, but the plan stops before flashing.

## 6. Work breakdown

1. **hdal layer** (small): input kind, frame rate, `FUNC_CONFIG` id 26, processor output kind; `Fake` support;
   remove the add-on reference path and pull code.
2. **Pipeline** (medium): build the stock-shaped graph, `retune_main` refusal rule, `PipelineConfig` field.
3. **Settings and trial protocol** (medium): the `isp_3dnr` row, "applies after reboot", Home Assistant
   discovery through the single settings table, the marker file, docs table kept identical by the existing
   host test.
4. **Host tests**: graph order with 3DNR on and off; main size other than 2592x1944 falls back; refusal of a
   resolution change; refusal path report; marker logic with a fake filesystem; settings parsing and the
   documented table.
5. **Hardware ladder** (section 5), roughly 4 to 8 sessions including re-runs.
6. **Docs**: `hardware/isp-iq.md` (replace the "not enabled" section with the verified shape),
   `settings.md`, `architecture.md`.
7. **Flash** only after rung 8, with your web administrator password at the flash tool's prompt.

## 7. Risks

- **Stall on a wrong graph.** Mitigated by the trial-boot marker and by defaulting the setting off.
- **Encoder throughput.** The main encoder still encodes 15 fps, but the processor now produces 30 fps full-size
  compressed frames. Stock does the equivalent at 20/25, so it is plausible, not proven on our graph.
- **Memory.** A full-size compressed output at 30 fps holds extra buffers beside the four 7.5 MiB capture blocks.
- **Coupling to one main size.** Users on 1920x1440 or 1280x960 get nothing from this. If that matters, a second
  design (a full-size reference encoder at a very low rate) is possible but costs a second full-size encoder's
  memory, which the unit likely cannot spare [I].
- **Unnamed bits.** The three companion control bits (`0x20000`, `0x800000`, `0x1000000`) are enabled because
  stock enables them; what each switches is not named in any string [D]. They are the first suspect if rung 4
  or 5 misbehaves.

## 8. Decisions I need from you

1. **Startup-only switch, default off**, as in 4.3? The alternative, a live toggle, means rebuilding the whole
   graph inside the running daemon, which I would not attempt without evidence that HDAL survives an
   uninitialise and initialise cycle in one process.
2. **Accept the 2592x1944 coupling** (3DNR only at that main size)?
3. **Go-ahead for the hardware ladder**: up to about 8 RAM sessions, each leaves the camera on stock for several
   minutes and ends with it restored to the custom daemon.
4. **Scope on exit**: if rung 6 shows only a small gain, is stopping there (nothing flashed) acceptable?
