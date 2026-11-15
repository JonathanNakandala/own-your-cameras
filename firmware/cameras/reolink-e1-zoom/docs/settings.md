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
| `main_bitrate` | Main bitrate | Main stream | number | 256…8192 kbps | 6144 | main path restart (≤ 2 s) |
| `main_resolution` | Main resolution | Main stream | select | 2592x1944, 1920x1440, 1280x960 | 2592x1944 | main path restart (≤ 2 s) |
| `main_fps` | Main frame rate | Main stream | number | 5…30 fps | 30 | main path restart (≤ 2 s) |
| `main_codec` | Main codec | Main stream | select | H.264, H.265 | H.265 | main path restart (≤ 2 s) |
| `main_gdr` | Main gradual refresh | Main stream | switch | ON/OFF | ON | main path restart (≤ 2 s) |
| `main_slice_split` | Main slice split | Main stream | switch | ON/OFF | ON | main path restart (≤ 2 s) |
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

## Main codec and first-run size (`main_codec`)

`main_codec` is `H.265` by default and live (the main encoder restarts, a few milliseconds); `H.264` is the other
choice. The first-run main stream is the full sensor frame, 2592x1944 at 30 fps and 6144 kbit/s, which is also what
lets 3D noise reduction run. Only the main stream has the choice: the sub stream stays H.264.

- **H.265** is the same picture at about half the bitrate: on frames from this camera it scored at 0.5 Mbit/s what
  H.264 scored at 1 Mbit/s, and at 1 Mbit/s nearly what H.264 scored at 2 [V]. It runs with 3D noise reduction,
  gradual refresh and slice split at 15 and 30 fps, and the CPU load is the same [V]. The encoder takes Main
  profile at level 5.1 and writes the same full-range colour flags as H.264.
- **The defaults go together:** at 30 fps each frame gets half the bits it gets at 15, which is why the first-run
  bitrate is the 6144 kbit/s it is. 30 fps at full size works with 3D noise reduction on, the default. With it off
  the plain graph gives all outputs one pool of four sensor-size buffers, and at 30 fps the sub stream is starved
  (1 frame in 10 s [V]); keep `main_fps` at 15 there, or leave 3D noise reduction on.
- **Settings saved by an older firmware keep their values.** A camera whose file already says `main_fps = 15` or
  `main_bitrate = 4096` keeps them; only `main_codec`, which was not in the file, takes the new default. Change those
  two over MQTT (or delete the lines) to move to the new first-run values.
- **Slice and refresh rows:** H.265 counts rows in coding tree units of 64 lines against H.264's 16-line macroblock
  rows. The daemon converts, so slices are the same height either way (8 H.264 rows are 2 H.265 rows) and gradual
  refresh covers the picture in the same time. The row unit of gradual refresh is assumed to follow slice split's;
  only slice split was measured [V].
- **Changing the codec disconnects the main stream's viewers.** Their session description (the codec and its
  parameter sets) no longer fits, so the RTSP server ends their video and they reconnect to get the new one [V].
  The sub stream, audio and the other paths are not touched.

What a client sees: RTSP carries H.265 as RFC 7798 with the parameter sets in the session description. Media2 reports
the encoder as `H265` for Profile T clients. Media1 has no H.265 in its schema; cameras that stream it still put the
value `H265` there, and so does this one, so a Media1 client that only accepts `H264` (Home Assistant's ONVIF
integration filters on exactly that) does not list the main profile: use H.264 for the main stream there. See
[shared/onvif.md](../../../../docs/shared/onvif.md#video-codec).

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

On by default ([why](#what-a-phone-decodes)); live (the main encoder restarts, 4 to 15 ms). With it on, the main stream no longer sends a full key
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

Turn it off for a recorder that cuts files at key frames (Frigate segments, go2rtc HLS): with it on they get one
every 8 s instead of every second. Turn it off too when many viewers join and leave often, since each join costs the
viewers already watching up to one frame.

## Slice split (`main_slice_split`)

On by default ([why](#what-a-phone-decodes)); live (the main encoder restarts, 6 to 13 ms). With it on, every main-stream frame is cut into slices of
8 macroblock rows (16 slices at 2592x1944, 12 at 1920x1440, 8 at 1280x960), each one an independently decodable piece
that travels in its own RTP packets. A lost packet then breaks only its slice; without it the decoder loses everything
from the hole to the bottom of the frame, until the next key frame (or the next gradual refresh cycle). The ONVIF relay
output `slice_split` switches the same setting; it combines with `main_gdr` and `isp_3dnr`.

Measured [V]: the camera's encoder reported the split and the stream carried exactly 16 slices a frame at 2592x1944
(1 with it off); ffmpeg over TCP and UDP decoded it without errors. The damage was measured offline on raw captures
from the camera, cut into RTP packets as this server does, with one packet lost and the receiver keeping what arrived
before the hole (as ffmpeg-based clients do), while the camera panned:

| One lost packet in | One slice a frame | 8-row slices |
|---|---|---|
| a key frame | 27 % of the picture wrong (up to 32 %), for a second | 2.9 % (up to 7.7 %) |
| a P frame | 12 % (up to 46 %), for about 7 frames | 1.6 % (up to 3.6 %) |

In a still scene both are small (about 2 % against nothing visible), because the decoder hides a lost band by copying
it from the previous frame. The cost is a little bitrate efficiency (prediction does not cross slice borders; not
measured here) and more, smaller packets: a P frame at 4096 kbit/s left as about 22 packets instead of 19.

## What a phone decodes

The main stream's defaults use only what Android's own decoders must handle, so a phone viewer such as tinyCam
decodes them in hardware:

- **Slice split:** several slices a frame, sent in order, are part of every H.264 profile. Android requires
  Baseline and Main profile decoders; only arbitrary slice order (ASO), flexible macroblock order (FMO) and redundant
  slices are optional, and the encoder uses none of them.
- **Gradual refresh:** the refresh bands are ordinary intra macroblocks inside P frames, and a joining viewer
  still starts at a real key frame, so no decoder support beyond plain P frames is needed.
- **Colour range:** the video processor writes luma over the whole 0 to 255 range, and both H.264 streams say
  so in their VUI (full range, BT.709 primaries and transfer, BT.601 matrix as in JPEG). Without the flag, which
  the vendor stack never sends, a player assumes 16 to 235 and clips what lies outside: 8 to 20 % of the picture
  in frames from this camera, mostly shadows [V]. Not a setting; a player that ignores the flag shows what it
  showed before.
- **H.265:** the S24 Ultra's decoder handles it in hardware, and Android lists H.265 Main among the supported
  decoders ([formats](https://developer.android.com/media/platform/supported-formats)). Other phones and players vary.
  `main_codec` goes back to H.264 for anything that cannot play it.
- **Profile and level stay at the vendor's High profile, level 5.1.** Android guarantees only Main profile at level
  3.1, but current phones decode High profile at 5.1 and above in hardware (a Galaxy S24 Ultra, for one), and High
  profile's CABAC and 8x8 transform give a better picture at the same bitrate. The level is kept at 5.1 even for
  smaller sizes, as the vendor stack does. (H.264 only: H.265 runs Main profile at 5.1.)

Sources: [Android supported media formats](https://developer.android.com/media/platform/supported-formats),
[Android 16 Compatibility Definition, 5.3.4 H.264](https://source.android.com/docs/compatibility/16/android-16-cdd).

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
