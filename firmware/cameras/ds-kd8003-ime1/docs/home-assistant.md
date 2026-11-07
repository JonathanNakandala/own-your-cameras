# MQTT and Home Assistant

The application publishes one retained device-discovery document,
`homeassistant/device/<mqtt_node>/config`, and keeps every topic under `<mqtt_prefix>`. Every payload
is JSON, as the discovery templates read `value_json`.

| Entity | Component | Topic |
|---|---|---|
| door | `cover`, device class `gate`; open while the strike is released or the reed is open | `door/state`, commands on `door/command` |
| Open with announcement | `button` | `door/command` ← `{"command":"open"}` |
| Open quietly | `button` | `door/command` ← `{"command":"open_quiet"}` |
| Close now | `button`: releases the strike before the pulse runs out | `door/command` ← `{"command":"close"}` |
| Door | `binary_sensor`, the reed contact | `reed` (retained `"open"`/`"closed"`) |
| Lock actuator | `binary_sensor`, the strike | `lock/state` (`{"lock_actuator":"unlocked",…}`) |
| Doorbell | `event`: `press`, `release` | `call_button` |
| Access | `event`: `granted` (with `name`), `denied`, `remote` | `access` |

Availability is `availability` (retained `"online"`/`"offline"`, the will on disconnect).
`door/command` also accepts the bare words `open`, `open_quiet` and `close`, quoted or not. A
retained command is ignored, so a stale message cannot open the door on every reconnect.

The call-button light follows day and night from the sensor's exposure, together with the IR light.

## Video and talk

Video does not go through MQTT: point go2rtc (or a recorder) at the [RTSP streams](../README.md#use).
A client that requires the ONVIF backchannel on the RTSP session can send Opus to the device's
speaker; go2rtc does this for the WebRTC card's microphone. The device accepts Opus with payload
type 111, as offered in its SDP, or 96, which ffmpeg uses.
