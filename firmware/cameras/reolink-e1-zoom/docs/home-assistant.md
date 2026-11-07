# MQTT and Home Assistant

The daemon is an MQTT 3.1.1 client of your broker and announces itself through Home Assistant's MQTT discovery.
Video does not go through MQTT: point go2rtc at the RTSP streams ([../README.md](../README.md#use)).

## `cam.toml`

`/mnt/para/cam.toml`, mode 0600, at most 16 KiB. The installed daemon refuses to start without an `[mqtt]` section;
`cam preflight` says why. Passwords are never logged or echoed in errors.

```toml
[camera]               # optional; these are the defaults
id = "e1zoom"          # a-z, 0-9, _ (1-32): topics, client id, Home Assistant ids, DHCP hostname
name = "E1 Zoom"       # the Home Assistant device name (1-64 characters, no control characters)

[mqtt]
host = "broker.example"
port = 1883            # optional
username = "USER"
password = "PASSWORD"

[presets.Door]         # optional: up to 32 named positions, names up to 64 bytes
pan = 6195             # 0-7360 steps
tilt = 650             # 0-1240 steps
zoom = 0               # 0-1024 steps

[onvif]                # optional: ONVIF (../../../../docs/shared/onvif.md)
port = 80              # 0 turns it off

[time]                 # optional: NTP servers the clock follows; none leaves the clock alone
ntp = ["NTP_HOST"]
```

Unknown keys are refused. The presets here are the starting table; the daemon never writes this file.
`preset_save/set` saves where the camera stands under a name: an existing name moves that preset, a new name creates
one, and `{"v":1,"op":"remove_preset","name":"…"}` on `motion/set` deletes one (ONVIF creates and deletes presets the same way). The daemon keeps these
changes in `/mnt/para/cam-presets.toml`, each with the `cam.toml` position it replaced. Editing a preset here and
restarting takes precedence over an earlier save of it.
Runtime settings (volumes, IR thresholds, stream parameters, picture) are not in this file: they are entities
and the daemon keeps them itself ([settings.md](settings.md)).

## Session and topics

- Client id `cam_<id>`, clean session, five-second keep-alive, plain TCP. Never run two cameras with the same id.
- `camera/<id>/availability`: retained `online` once the hardware is up, retained `offline` through the last
  will on a crash or lost connection, and drained before an orderly disconnect.
- `camera/<id>/<entity>/set`: commands, never retained. A retained command is ignored, so an old retained reboot
  cannot loop the camera.
- `camera/<id>/<entity>/state`: the **confirmed** state, retained at QoS 1, published every five seconds, after
  each command and as soon as the media thread applies a change. A hardware failure is logged; it never echoes the
  requested value as success.
- Discovery: `homeassistant/<domain>/cam_<id>/<entity>/config`, retained at QoS 1, one device `cam_<id>`, unique ids
  `cam_<id>_<entity>`. A setting's name leads with its section (`Audio Mic volume`, `Pan & tilt Ramp`, `Lens
  Calibrate focus (15 min)`), so the device page's configuration list reads section by section; Home Assistant
  groups entities on a device only by category (controls, configuration, diagnostic), and separate devices per
  section were rejected: the camera is one device. Discovery and states are sent again on reconnect and when Home Assistant announces
  `homeassistant/status = online`. With no presets configured, the preset select gets an empty config (removed).

At startup the daemon homes pan/tilt and the lens once, goes to the `Default` preset if one exists (the Home
button does the same; without one the head stays at the end stops), switches the status LED on, then announces
`online`. Until homing succeeds the pan/tilt position is unknown and its retained states are cleared.

## Entities

Controls:

| Entity | Type | Command payload |
|---|---|---|
| Pan | number, 0–7360 steps | an integer, or `relative:<±steps>` |
| Tilt | number, 0–1240 steps | an integer, or `relative:<±steps>` |
| Zoom | number, 0–1024 steps | an integer, or `relative:<±steps>`; focus goes to the factory calibration, then autofocus refines it |
| Preset | select | an exact preset name; a manual move clears the selection |
| Refocus | button | `PRESS`: one autofocus search now, also with the `autofocus` setting off |
| Calibrate focus (15 min) | button (`entity_category: config`) | `PRESS`: walks the calibration grid (8 pans × 3 tilts × 5 zooms, about 15 minutes) and learns every point into the focus map, then returns to where it started; any other command but a setting or a reboot cancels it ([hardware/zoom-focus.md](hardware/zoom-focus.md#autofocus-in-the-daemon)) |
| Reboot | button (`entity_category: config`) | `PRESS`; settings are saved first |

Every runtime setting is an entity too (`entity_category: config`): the table is in [settings.md](settings.md).

Diagnostics (`entity_category: diagnostic`):

| Entity | Meaning |
|---|---|
| Pipeline health | binary sensor: both encoders delivered a frame in the last three seconds |
| Light sensor | the raw ADC; higher is darker. Use it to pick IR thresholds |
| Settings | `ok`, or `invalid: <keys>`, plus the last refused change |
| Autofocus | `idle`, `searching`, `focused <pos>`, `kept <pos>` (a pan/tilt found the scene still sharp), `calibrated <pos>` (flat scene), `failed: <reason>`, or `calibrating <done>/<total>, <n> min left` while the Calibrate focus walk runs; attributes `trigger`, `duration_ms`, `reference_va`, `va`, `searches`, the last search's `samples`, the watchdog's `history`, and the focus map's `map_points`, `park` (`learned`/`factory`), `park_focus`, `rows_pending`, and `calibration` (`state` running/done/cancelled, `done`, `total`, `learned`, `skipped` of the last walk, `remaining_s` while it runs) ([hardware/zoom-focus.md](hardware/zoom-focus.md#autofocus-in-the-daemon)) |
| CPU busy | % over the 10 s window |
| Memory available, Daemon memory | MiB (`MemAvailable`, the daemon's RSS) |
| Load average | one minute |
| Temperature | °C, `thermal_zone0` (`nvt_thermal`) |
| Wi-Fi signal | link quality %, from `/proc/net/wireless` (its level is not dBm) |
| Wi-Fi RSSI | dBm, from `iw dev wlan0 link` |
| Main/Sub bitrate (actual), frame rate (actual) | from the frames the encoders delivered |
| Main/Sub frames short | frames below the configured rate in the window (no HDAL drop counter exists) |
| Uptime | seconds |

Telemetry is published every 10 s by its own thread; a missing reading publishes empty (unknown), never zero.

## Commands without an entity

| Topic under `camera/<id>/` | Payload | Effect |
|---|---|---|
| `move/set` | `left`, `right`, `up`, `down`, `zoom_in`, `zoom_out`, `stop` | press and hold: moves until `stop`, the travel end, or `ptz_hold_timeout` without a release |
| `position3d/set` | JSON, see [hardware/position3d.md](hardware/position3d.md) | tap to centre, box to zoom; relative to the picture shown at the tap (the pose `tap_delay` before the tap arrived, from the recorded motion), and a tap still waiting behind a newer tap is dropped |
| `preset_save/set` | an exact preset name | saves the confirmed pan/tilt/zoom as that preset's position, written at once; nothing moves and a running search goes on; `preset/state` becomes that name. Refused for an unknown name, while a hold or the focus calibration runs, and while the position is unknown |
| `focus/set` | 0–1705 | diagnostic: parks the focus motor there without a search (the watchdog may correct it) |
| `stop/set` | `PRESS` | stops any motion now and discards queued moves; position becomes unknown until the next home. `stop/state` answers `stopped` or `error` (not retained) |

A hold runs on the hardware thread in ~100 ms chunks along the `ptz_start_speed` → `ptz_speed` ramp, every chunk
counted, so the position stays exact; `stop` ends it after the running chunk. Any absolute, relative or preset
command ends a hold first. A zoom hold is one press move of the lens driver; after the release, calibrated focus
settles for the reached zoom and autofocus searches. Relative moves start from the confirmed position, clamp to physical travel, and are
refused while the position is unknown.

Limits: packets and the config file 16 KiB, 256 queued outbound packets, 16 in flight, eight queued commands (a
ninth is refused), 64 event log lines per session. MQTT keeps polling during motor waits; lens startup and
compound zoom/focus share a 30 s motor budget.

## A dashboard card

Any card that can call `mqtt.publish` can drive the camera. With the stock Home Assistant `button` card a tap steps
the head:

```yaml
type: button
name: Left
icon: mdi:arrow-left
tap_action:
  action: perform-action
  perform_action: mqtt.publish
  data:
    topic: camera/e1zoom/pan/set
    payload: "relative:-150"
```

Steps that suit the lens: pan ±150, tilt ±80, zoom ±64. Press-and-hold needs a card that reports the release as
well as the press. The author uses the custom `master-camera-card` (not part of this project); its `ptz` feature,
with placeholders, looks like this:

```yaml
control_buttons:
  - label: "←"
    icon: mdi:arrow-left
    service: mqtt.publish
    data: {topic: camera/e1zoom/pan/set, payload: "relative:-150", qos: 0, retain: false}
    hold: {service: mqtt.publish, data: {topic: camera/e1zoom/move/set, payload: left, qos: 0, retain: false}}
    stop: {service: mqtt.publish, data: {topic: camera/e1zoom/move/set, payload: stop, qos: 0, retain: false}}
  # ↑ tilt relative:80 / up, ↓ tilt relative:-80 / down, → pan relative:150 / right,
  # + zoom relative:64 / zoom_in, - zoom relative:-64 / zoom_out
preset_buttons:
  - {label: "1", preset: Door}
preset_service: script.camera_preset
preset_save_service: script.camera_preset_save   # long press (800 ms) or Shift+click saves
selection_enabled: false
position3d_service: script.camera_position3d
```

A tap still steps; a press longer than 200 ms moves until the release. A preset button goes to its preset; a long
press saves where the camera stands into it. Presets and tap-to-centre go through scripts, which turn the card's
fields into what the daemon expects:

```yaml
script:
  camera_preset:
    mode: queued
    fields: {preset: {}}
    sequence:
      - action: select.select_option
        target: {entity_id: select.e1_zoom_preset}
        data: {option: "{{ preset }}"}
  camera_preset_save:
    mode: queued
    fields: {preset: {}}
    sequence:
      - action: mqtt.publish
        data: {topic: camera/e1zoom/preset_save/set, payload: "{{ preset }}", qos: 0, retain: false}
  camera_position3d:
    mode: parallel
    fields: {start_x: {}, start_y: {}, end_x: {}, end_y: {}, operation: {}}
    sequence:
      - action: mqtt.publish
        data:
          topic: camera/e1zoom/position3d/set
          payload: >-
            {"start_x": {{ start_x | int }}, "start_y": {{ start_y | int }},
             "end_x": {{ end_x | int }}, "end_y": {{ end_y | int }}, "operation": "{{ operation }}"}
```

Entity ids follow the device name and the entity name: with the default name `E1 Zoom` the preset select is
`select.e1_zoom_preset`, and a setting gets its section in its id on a fresh install (`number.e1_zoom_lens_zoom_speed`).
Home Assistant keeps the entity ids of an existing install, because the unique ids did not change when the sections
arrived; only the friendly names gained the section.
