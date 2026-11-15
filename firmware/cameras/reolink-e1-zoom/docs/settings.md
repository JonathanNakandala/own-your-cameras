# Runtime settings

Every setting is a Home Assistant entity of the `cam_<id>` device (`entity_category: config`,
[home-assistant.md](home-assistant.md)), command
`camera/<id>/<key>/set` (non-retained), retained state `camera/<id>/<key>/state` reporting the
**applied** value. Out-of-range or malformed payloads are rejected with a log line and change nothing.
The schema is one table, `cam::settings::specs()` (plus the picture items in `cam::isp_iq::ITEMS`); file
parsing, payload validation, HA discovery and the table below all derive from it (a host test keeps
this table identical to the code).

| Key | Name | Section | HA type | Range | First-run default | Applies |
|---|---|---|---|---|---|---|
| `mic_volume` | Mic volume | Audio | number | 0…100 % | 100 | live, media thread |
| `speaker_volume` | Speaker volume | Audio | number | 0…100 % | 100 | live, media thread |
| `aec` | Echo cancellation | Audio | switch | ON/OFF | OFF | live, media thread |
| `aec_noise_level` | AEC noise level | Audio | number | -60…-3 dB | -60 | live, media thread |
| `aec_echo_level` | AEC echo level | Audio | number | -60…-30 dB | -60 | live, media thread |
| `aec_notch_radius` | AEC notch radius | Audio | number | 0…1000 | 992 | live, media thread |
| `aec_leak_estimate` | AEC leak estimate | Audio | switch | ON/OFF | OFF | live, media thread |
| `aec_leak_value` | AEC leak value | Audio | number | 25…99 | 99 | live, media thread |
| `anr` | Noise reduction | Audio | switch | ON/OFF | OFF | live, media thread |
| `anr_suppress_level` | ANR suppress level | Audio | number | 3…35 | 30 | live, media thread |
| `anr_hpf_cutoff` | ANR high-pass cutoff | Audio | number | 0…1000 Hz | 160 | live, media thread |
| `anr_bias_sensitive` | ANR bias sensitivity | Audio | number | 1…9 | 9 | live, media thread |
| `mirror` | Mirror | Picture | switch | ON/OFF | stock `0_isp.cfg` `mirror` (else OFF) | live, ISP item 44 with readback |
| `flip` | Flip | Picture | switch | ON/OFF | stock `0_isp.cfg` `flip` (else OFF) | live, ISP item 44 with readback |
| `ir_to_night` | IR to-night threshold | Night vision | number | 0…4095 | 2300 | next auto tick / live |
| `ir_to_day` | IR to-day threshold | Night vision | number | 0…4095 | 1900 | next auto tick / live |
| `ir_hold` | IR switch hold | Night vision | number | 1…60 s | 4 | next auto tick / live |
| `ir_leds_at_night` | IR LEDs at night | Night vision | switch | ON/OFF | ON | next auto tick / live |
| `ir` | IR mode | Night vision | select | auto, day, night | auto | next auto tick / live |
| `led` | Status LED | camera | switch | ON/OFF | ON | live |
| `ptz_speed` | Speed | Pan & tilt | number | 100…720 pps | 720 | next move |
| `ptz_start_speed` | Start speed | Pan & tilt | number | 100…720 pps | 300 | next move |
| `ptz_ramp` | Ramp | Pan & tilt | number | 0…3000 ms | 1500 | next move |
| `ptz_hold_timeout` | Hold timeout | Pan & tilt | number | 1…60 s | 20 | next move |
| `tap_delay` | Tap delay | Pan & tilt | number | 0…3000 ms | 500 | next move |
| `zoom_speed` | Zoom speed | Lens | select | 20, 40, 60, 80, 100 | 80 | next zoom move |
| `autofocus` | Autofocus | Lens | switch | ON/OFF | ON | live |
| `af_watch_hold` | Autofocus watchdog hold | Lens | number | 10…300 s | 30 | live |
| `af_watch_drop` | Autofocus watchdog drop | Lens | number | 20…90 % | 60 | live |
| `af_move_drop` | Autofocus move drop | Lens | number | 50…99 % | 80 | live |
| `af_climb_drop` | Autofocus climb drop | Lens | number | 1…20 % | 2 | live |
| `main_bitrate` | Main bitrate | Main stream | number | 256…8192 kbps | 1024 | main path restart (≤ 2 s) |
| `main_resolution` | Main resolution | Main stream | select | 2592x1944, 1920x1440, 1280x960 | 1920x1440 | main path restart (≤ 2 s) |
| `main_fps` | Main frame rate | Main stream | number | 5…30 fps | 15 | main path restart (≤ 2 s) |
| `main_gdr` | Main gradual refresh | Main stream | switch | ON/OFF | OFF | main path restart (≤ 2 s) |
| `isp_3dnr` | 3D noise reduction | Picture | switch | ON/OFF | ON | after reboot |
| `isp_brightness` | Brightness | Picture | number | 0…200 | stock `0_isp.cfg` `brightness` (else 100) | live, with readback |
| `isp_contrast` | Contrast | Picture | number | 0…200 | stock `0_isp.cfg` `contrast` (else 100) | live, with readback |
| `isp_saturation` | Saturation | Picture | number | 0…200 | stock `0_isp.cfg` `saturation` (else 100) | live, with readback |
| `isp_sharpness` | Sharpness | Picture | number | 0…200 | stock `0_isp.cfg` `sharpen` (else 100) | live, with readback |
| `isp_hue` | Hue | Picture | number | 0…360 | stock `0_isp.cfg` `hue` (else 0) | live, with readback |
| `isp_noise_reduction` | Noise reduction (2D) | Picture | number | 0…200 | 100 | live, with readback |
| `isp_exposure` | Exposure compensation | Picture | number | 0…32 | 16 | live, with readback |
| `isp_anti_flicker` | Anti-flicker | Picture | select | 50 Hz, 60 Hz | stock `0_isp.cfg` `antiflick` (else 50 Hz) | live, with readback |

Cross-field rules: `ir_to_night` > `ir_to_day`; `ptz_start_speed` ≤ `ptz_speed`. A change that breaks
them is rejected; a file that breaks them loads both fields' defaults.

## 3D noise reduction (`isp_3dnr`)

On by default and read **once**, when the media graph is built: changing it stores the value and the camera
uses it after the next reboot. It needs the **2592x1944** main stream: the video processor's temporal noise reduction
compares each frame with the previous one at the processor's input size, and the main encoder reads that frame
directly. With another main size, including the 1920x1440 first-run default, the setting is stored and the daemon runs
the plain graph (and says so in its log): the setting only has an effect once the main stream is 2592x1944.
While it runs, a change of `main_resolution` away from 2592x1944 is refused (the stream would stall) and the
setting snaps back; `main_fps` and `main_bitrate` change live as usual. Night frames carry about 65 % less noise
at the same bitrate ([hardware/isp-iq.md](hardware/isp-iq.md#3d-noise-reduction-in-the-video-processor-isp_3dnr)).

Safety: a start with the setting on writes the marker `/mnt/para/cam-3dnr.trial` before building the graph and
deletes it once both streams delivered for 180 s. A start that finds the marker runs the plain graph and logs why,
because the last attempt never proved healthy. To try again: turn `isp_3dnr` off, reboot (the marker is removed),
turn it on, reboot. A marker that cannot be written also means no 3D noise reduction.

## Gradual refresh (`main_gdr`)

Off by default; live (the main encoder restarts, 4 to 15 ms). With it on, the main stream no longer sends a full key
frame every second. The encoder's gradual decoder refresh instead re-codes a band of rows of every frame without
reference to earlier frames, so the whole picture is refreshed once a second (9 rows a frame at 2592x1944 and 15 fps)
and a viewer that lost a packet is clean again within a second, as before. A full key frame still comes every
8 s (the encoder takes no interval of 0) and whenever a viewer joins: the encoder ignores its own "key frame now"
request, so the daemon briefly restarts it (the first frame after a start is a key frame), at most once a second.
The ONVIF relay output `gradual_refresh` switches the same setting.

Measured on a camera at 2592x1944, 15 fps, 4096 kbit/s with 3D noise reduction [V]:

- key frames: one per second off; one per 8 s plus one per joining viewer on;
- each remaining key frame is larger (about 340 KB against 205 KB: the constant bitrate controller spends what the
  missing ones saved), so the burst per key frame is not smaller, there are just 8 times fewer of them;
- viewers joining with a 2 s timeout got video at once; a viewer already watching lost at most one frame (0.13 s
  gap) to each join; no decoder errors over TCP or UDP, no lost UDP packets in 35 s either way.

Worth trying when a client shows a broken band at every key frame over a weak link. Recorders that cut files at key
frames (Frigate segments, go2rtc HLS) get them every 8 s instead of every second.

## Persistence

`/mnt/para/cam-settings.toml` (a RAM path, `/mnt/tmp/cam-settings.toml`, for `cam serve --diagnostic`),
written only by the daemon, mode 0600: temp file, `fsync`, rename, directory `fsync`.

- **Save window:** a change is written within 10 s of the **first** unsaved change and is never
  postponed by later changes, so a slider burst costs one write per 10 s and a power cut loses at most
  the last 10 s.
- Flushed on orderly shutdown and before an MQTT reboot. A failed write stays pending, is retried one
  window later and shows in the `settings` sensor.
- Absent file: first-run defaults. Unreadable/invalid file or field: that field's default, one log line,
  the diagnostic `settings` sensor reads `invalid: <keys>`; never a startup failure (three failed starts
  would hand the camera back to the vendor stack). Unknown keys are ignored, so a downgrade keeps working.
- First-run defaults from the stock `/mnt/para/0_isp.cfg` (read, never written): `mirror`, `flip` and
  the picture levels (`v·200/255`, hue rotated so 128 = 0°, `antiflick` 1 = 60 Hz).
- `cam preflight` does not require the file.

## Not exposed, and why

- AEC `filter_length`, `frame_size`, `lb_channel`: they size AEC buffers inside the `aec_max` envelope
  and have no audible value without a loopback route ([hardware/audio.md](hardware/audio.md)).
- Watchdog and health budgets: a wrong value resets the camera or masks a hung boot.
- Sub-stream parameters: the sub stream feeds every HA tile and stays 640×480 full-frame.
- Any non-4:3 or cropped main size: the main stream is always the full sensor frame scaled.
- Presets (named in `cam.toml`; `preset_save/set` saves a position, [home-assistant.md](home-assistant.md)),
  MQTT/Wi-Fi/RTSP credentials (they stay in the hand-written private files).

## Main stream: measured envelope and the stall guard

Measured on the tested unit: every size is the full 4:3 sensor frame (no crop; main and sub show the same
field of view). Changes restart the main path alone; the main stream's longest gap was 0.2 s (limit 2 s), and a
go2rtc stream of the main path followed every change by itself, including 2592×1944. Ring: the main path config is sized for 2592×1944 at
8.192 Mbit/s at startup (12.1 MiB ring), so a raise never re-opens it.

| Main | Main delivered | Sub stream (10 fps) |
|---|---|---|
| 1280×960 at 10–30 fps | as set | 10 fps |
| 1920×1440 at 5–15 fps | as set | 10 fps |
| 1920×1440 at 20 / 25 / 30 fps | as set | **0 / 5 / 0 fps** |
| 2592×1944 at 5–15 fps | as set, up to 8.2 Mbit/s | 10 fps |
| 2592×1944 at 20 fps | 10 fps | 10 fps |
| 2592×1944 at 25–30 fps | as set | 0 fps (camera reset before the guard) |

A sub stream that stops delivering trips the watchdog's 5 s encoder budget and resets the camera, and a
persisted value would repeat that every boot. So a main-stream change, and a non-stock persisted main
stream at boot, are on probation for 10 s: if either stream is silent for 2 s the media thread restores
the previous (at boot: stock) main stream, the `settings` sensor reports the refusal, and the store
saves what runs. Resolution and fps stay independent settings; combinations the encoders cannot
sustain are refused by measurement, not by a table. Highest sustained bitrate: 8.2 Mbit/s
(2592×1944 at 30 fps, before the sub stream's starvation was known).
