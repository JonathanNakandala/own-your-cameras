# MQTT and Home Assistant

The application uses the shared MQTT layer of all cameras: one retained discovery config per entity,
`homeassistant/<component>/<mqtt_node>/<id>/config`, and every topic under `<mqtt_prefix>`. States and
availability are plain text (`open`, `online`); events and `lock/state` are JSON.

| Entity | Component | Topic |
|---|---|---|
| door | `cover`, device class `gate`; open while the strike is released or the reed is open | `door/state` (`open`/`closed`), commands on `door/command` |
| Open with announcement | `button` | `door/command` ← `open` |
| Open quietly | `button` | `door/command` ← `open_quiet` |
| Close now | `button`: releases the strike before the pulse runs out | `door/command` ← `close` |
| Door | `binary_sensor`, the reed contact | `reed` (retained `open`/`closed`) |
| Lock actuator | `binary_sensor`, the strike; the pulse keys below are also attributes of the `door` cover | `lock/state` (retained JSON, [relock countdown](#relock-countdown)) |
| Doorbell | `event`: `press`, `release` | `call_button` |
| Access | `event`: `granted` (with `name`), `denied`, `remote` | `access` |

Availability is `availability` (retained `online`/`offline`, the will on disconnect). A
command the broker replays from its retained store (on subscribe) is ignored; a retained publish
delivered live arrives as a normal command and acts. A door command that waited more than 2 s is
dropped, so a stale message cannot open the door. Events are never retained or replayed; one
that cannot be delivered within 10 s is dropped.

## Relock countdown

`lock/state` is republished on every change of the strike pulse: an open, a restart by a second open
during the pulse (it runs again from now with the current `open_seconds`), "Close now", and the
confirmed release. It is not republished every second.

```json
{"lock_actuator":"unlocked","lock_actuator_unlocked":true,"pulse_s":120,"relock_at":"2026-11-08T14:03:27.412Z","relock_in_s":119.95}
{"lock_actuator":"locked","lock_actuator_unlocked":false,"pulse_s":null,"relock_at":null,"relock_in_s":0}
```

| Key | Meaning |
|---|---|
| `pulse_s` | length of the running pulse (the full scale of a countdown ring) |
| `relock_at` | UTC deadline, ISO 8601 with milliseconds; `null` when locked or while the clock is not in sync (`ntp`) |
| `relock_in_s` | seconds left at publication, from the monotonic clock |

Consumer rule: draw remaining = `relock_at` − now; when `relock_at` is `null`, `relock_in_s` counted
from when the message arrived. No countdown when locked, when the gate is `offline`, or once the
deadline has passed (a release still being retried, an open the relay rejected, or a gate that reset mid-pulse).

The call-button light follows day and night from the sensor's exposure, together with the IR light.

## Settings

Every setting below is a Home Assistant `number`, `switch` or `select` with `entity_category: config`
on the gate's device. State `<mqtt_prefix>/<id>/state` (retained), command `<mqtt_prefix>/<id>/set`.
A change applies within a second and the entity then shows the value the hardware reports back; a
rejected or failed change snaps back to the value in force. Values are saved on the panel within 10 s
and restored at start. Numbers take a whole number, switches `ON`/`OFF`, selects the option text:

```sh
mosquitto_pub -h MQTT_HOST -u MQTT_USER -P MQTT_PASSWORD -t 'ds-kd8003-ime1/main_fps/set' -m 15
mosquitto_pub -h MQTT_HOST -u MQTT_USER -P MQTT_PASSWORD -t 'ds-kd8003-ime1/main_resolution/set' -m 1280x720
```

(`ds-kd8003-ime1` is the default `mqtt_prefix`.)

- `clip_db`/`talk_db` restart the speaker thread: a clip that is playing finishes at the old gain.
- A stream rate, frame-rate, GOP or rate-control change is live (the viewer gets a new key frame); a
  profile change recreates the encoder; a main resolution change can resize the scaler, which stops
  the streams for a moment.
- Saturation is the day value: by night the image is monochrome.
- Encoder ranges: main 30 fps at 4096 kbit/s together with sub 30 fps at 2048 kbit/s ran without a
  frame shortfall on the panel (CPU about 90 % busy with four viewers).
- Only image (ISP) keys seen to change the picture are exposed: see [Image settings](#image-settings).
- `isp_manual_shutter` acts only while `isp_auto_exposure` is `OFF`.
- `isp_awb_mode` offers the modes the vendor firmware labels (vendor numbers 0, 1, 3-8, 14-16).
  `auto` is the default; `manual` turns the picture magenta; `outdoor`, `fluorescent` and
  `incandescent` were seen to change the colour cast, the others are untested.

<!-- settings:begin -->
| Entity id | Name | Values | Default |
|---|---|---|---|
| `open_seconds` | Door open time | 1..120 s | 5 |
| `codec_out_db` | Speaker level | -30..6 dB | 0 |
| `clip_db` | Announcement gain | -40..12 dB | 0 |
| `talk_db` | Talk gain | 0..18 dB | 6 |
| `mic_gain_db` | Microphone gain | -5..40 dB | 0 |
| `main_resolution` | Main resolution | `1920x1080`, `1280x720`, `640x360` | 1920x1080 |
| `main_fps` | Main frame rate | 1..30 fps | 30 |
| `main_kbps` | Main bitrate | 512..4096 kbit/s | 4096 |
| `main_gop` | Main keyframe interval | 1..400 frames | 30 |
| `main_rate_control` | Main rate control | `CBR`, `VBR` | VBR |
| `main_profile` | Main profile | `baseline`, `main`, `high` | baseline |
| `sub_fps` | Sub frame rate | 1..30 fps | 15 |
| `sub_kbps` | Sub bitrate | 128..2048 kbit/s | 1024 |
| `sub_gop` | Sub keyframe interval | 1..400 frames | 30 |
| `sub_rate_control` | Sub rate control | `CBR`, `VBR` | VBR |
| `sub_profile` | Sub profile | `baseline`, `main`, `high` | main |
| `isp_auto_exposure` | Auto exposure (ISP key 32) | `ON`/`OFF` | ON |
| `isp_brightness` | Brightness (ISP key 33) | 0..100 | 50 |
| `isp_shutter_slowest` | Slowest shutter (ISP key 36) | 10..40000 µs | 40000 |
| `isp_gain_limit` | Gain limit (ISP key 39) | 0..100 | 100 |
| `isp_manual_shutter` | Manual shutter (ISP key 40) | 10..40000 µs | 40000 |
| `isp_awb_mode` | White balance mode (ISP key 82) | `manual`, `auto`, `locked`, `outdoor`, `indoor`, `fluorescent`, `sodium`, `auto-track`, `incandescent`, `warm`, `natural` | auto |
| `isp_saturation` | Saturation (ISP key 84) | 0..100 | 50 |
| `isp_contrast` | Contrast (ISP key 144) | 0..100 | 50 |
| `isp_sharpness` | Sharpness (ISP key 145) | 0..100 | 50 |
| `isp_nr_2d` | 2D noise reduction (ISP key 148) | 0..100 | 50 |
| `isp_nr_3d` | 3D noise reduction (ISP key 149) | 0..100 | 50 |
| `isp_hlc` | Highlight compensation (ISP key 153) | `ON`/`OFF` | OFF |
<!-- settings:end -->

## Image settings

Observed on a DS-KD8003-IME1 (AR0237 sensor, vendor firmware V2.2.62) with this firmware build, by day,
outdoors: one RTSP frame before and after each change, then the old value back. "Tried" starts from
the panel's value at the time. Every call below returned success and read back the value set, unless
noted. Keys without a visible effect are not exposed.

| Key | Setting | Tried | Seen | Exposed |
|---:|---|---|---|---|
| 32 | `isp_auto_exposure` | OFF | exposure freezes; manual shutter takes effect only while OFF | yes |
| 33 | `isp_brightness` | 50 → 90 | brighter picture (mean luma 107 → 132) | yes |
| 35 | anti-flicker | 50 → 60 Hz | no change (daylight, no mains lighting) | no |
| 36 | `isp_shutter_slowest` | 40000 → 50 µs | darker, then gain makes up part of it (luma 103 → 90) | yes |
| 37 | fastest shutter | 10 → 10000 µs | no change | no |
| 39 | `isp_gain_limit` | 100 → 0, slowest shutter 50 µs | gain can no longer compensate (luma 92 → 15) | yes |
| 40 | `isp_manual_shutter` | AE off, 40000 → 5000 µs | fixed exposure follows (luma 240 → 174) | yes |
| 41 | manual gain | AE off, 0 → 80 | no change; reads back 0 | no |
| 46 | day/night sensitivity | 4 → 0, 7 | no change; no read-back | no |
| 51 | white-light sensitivity | 4 → 7 | no change; no read-back | no |
| 80 | auto white balance | OFF → ON | no change | no |
| 81 | white balance speed | 0 → 100 | no change | no |
| 82 | `isp_awb_mode` | auto → outdoor, fluorescent, incandescent, manual | colour cast changes; `manual` turns the picture magenta | yes |
| 84 | `isp_saturation` | 50 → 0 | black and white | yes |
| 144 | `isp_contrast` | 50 → 100 | stronger contrast | yes |
| 145 | `isp_sharpness` | 0 → 100 | harder edges, JPEG 37 % larger | yes |
| 146 | noise reduction mode | 0 → 1, 2 | no change | no |
| 148 | `isp_nr_2d` | 0 → 100 at high gain | smoother noise, JPEG 9 % smaller (three repeats) | yes |
| 149 | `isp_nr_3d` | 0 → 100 at high gain | visibly smoother texture, JPEG 11 % smaller | yes |
| 150 / 151 | WDR mode / level | 0 → 50 / 0 → 100 | no visible change | no |
| 153 | `isp_hlc` | OFF → ON | darker picture, highlights held (luma 101 → 83) | yes |
| 154 | HLC level | 0 → 100 with HLC on | no change | no |
| 155 / 156 | defog / level | 0 → 2 / 0 → 100 | no change | no |
| 157 / 158 | contrast enhancement / level | OFF → ON / 0 → 100 | no change | no |

Not tested at night: keys 35, 41, 46 and 51 may matter only in the dark or under mains lighting.

## Diagnostics

Sensors with `entity_category: diagnostic`, sampled every 30 s. A value is sent only when it moved
beyond its sensor's deadband or 5 minutes passed; a missing reading is `unknown`, never zero.

| Sensor | Enabled by default |
|---|---|
| `cpu_busy`, `memory_available`, `uptime`, `main_fps_actual`, `main_bitrate_actual` | yes |
| `load`, `daemon_memory`, `sub_fps_actual`, `sub_bitrate_actual` | no |
| `isp_luma`, `isp_night` (ISP read-outs, keys 34 and 45) | no |

Stream fps and bitrate are counted from what the encoder delivers. The ISP's total gain (key 42) is
not published: on the vendor library tested its getter returns no value.

## Video and talk

Video does not go through MQTT: point go2rtc (or a recorder) at the [RTSP streams](../README.md#use).
A client that requires the ONVIF backchannel on the RTSP session can send Opus to the device's
speaker; go2rtc does this for the WebRTC card's microphone. The device accepts Opus with payload
type 111, as offered in its SDP, or 96, which ffmpeg uses.
