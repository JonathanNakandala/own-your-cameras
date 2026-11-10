# ONVIF

Every camera in this project speaks ONVIF Profile S, so a recorder or a home automation system can find it on
the network, read its streams and snapshot, and drive it without a camera-specific integration. The same code
(`firmware/crates/camera-onvif`) runs on all of them; only the hardware side differs.

| | Reolink E1 Zoom | Mi 360 | DS-KD8003-IME1 |
|---|---|---|---|
| Discovery (WS-Discovery) | yes | yes | yes |
| Main and sub profiles | yes | yes | yes |
| Snapshot (hardware JPEG) | 1280×960 | the sub picture, 640×360 | the main picture, 1920×1080 |
| PTZ: continuous move, home | pan, tilt, zoom | pan, tilt | — |
| PTZ: relative move (field of view), absolute move, position in `GetStatus` | yes | — | — |
| Imaging: brightness, saturation, contrast, sharpness, IR-cut filter, focus mode | yes | — | — |
| PTZ presets: list, go to, create, delete | up to 32 | up to 16 | — |
| Events: digital inputs | — | — | `call_button`, `door` |
| Relay outputs (LEDs) | 2 | — | — |
| Audio output (talkback) | yes | yes | yes |

## Streams

Every camera serves the same four RTSP paths and nothing else:

| Path | Video | Audio |
|---|---|---|
| `main`, `sub` | main and sub stream | Opus 48 kHz mono (DS-KD8003-IME1: G.711 µ-law 8 kHz) |
| `main_pcmu`, `sub_pcmu` | the same video | G.711 µ-law 8 kHz |

ONVIF `GetStreamUri` hands out `main_pcmu` and `sub_pcmu`, because recorders such as Synology Surveillance
Station record G.711 but drop Opus. A camera encodes each codec only while someone listens, so the two path pairs
cost nothing unused. Talkback (the ONVIF backchannel, `Require: www.onvif.org/ver20/backchannel`) takes Opus or
G.711 µ-law on any path; each path offers its own codec.

## Login

ONVIF uses the RTSP user name and password of the camera. Requests are signed with WS-Security
UsernameToken digests (HTTP Digest is accepted too); a signature more than five minutes off the camera's clock is
refused, as is a replayed one. Keep the camera's clock right with NTP (below).

## Settings

| Camera | File | ONVIF port | NTP servers |
|---|---|---|---|
| Reolink E1 Zoom | `cam.toml` | `[onvif] port = 80` | `[time] ntp = ["NTP_HOST"]` |
| Mi 360 | `cam.toml` | `[onvif] port = 80` | `[time] ntp = ["NTP_HOST"]` |
| DS-KD8003-IME1 | `cam.conf` | `onvif_port=80` | `ntp=NTP_HOST` (comma-separated) |

ONVIF is on by default on port 80; port 0 turns it off. Without NTP servers the camera's clock is left alone and
ONVIF reports the time as set manually, so a client may offer to set it. Up to four servers are tried in order,
hourly once one answers.

## Synology Surveillance Station

Add the camera with **Brand: ONVIF**, the camera's address, port 80 and the RTSP user name and password. Surveillance
Station finds both profiles, records `main_pcmu` with sound and moves PTZ cameras through their presets.

## Home Assistant

The ONVIF integration discovers the cameras by itself. It shows the snapshot and the stream, and for PTZ
cameras offers the `onvif.ptz` action with the camera's presets; for the door panel the call button and the door
become binary sensors. The cameras' MQTT entities keep working alongside it.

## Relative and absolute PTZ, Frigate

The E1 Zoom also takes `RelativeMove` and `AbsoluteMove`, and `GetStatus` reports `Position` and an honest
`MoveStatus`. This is what [Frigate's autotracking](https://docs.frigate.video/configuration/autotracking/)
requires: a `RelativePanTiltTranslationSpace` in the **field-of-view** space
(`.../PanTiltSpaces/TranslationSpaceFov`) and `MoveStatus`.

| Call | Space | Meaning |
|---|---|---|
| `RelativeMove` pan/tilt | `TranslationSpaceFov`, -1…1 | a fraction of the picture shown: 1.0 is one whole width (pan) or height (tilt), right and up positive. Converted to motor steps with the lens map measured for tap-to-centre ([position3d](../../firmware/cameras/reolink-e1-zoom/docs/hardware/position3d.md)) at the lens's zoom now, under the picture's mirror and flip. Both axes move together |
| `RelativeMove` zoom | `TranslationGenericSpace`, -1…1 | a fraction of the lens travel, positive in |
| `AbsoluteMove` pan/tilt | `PositionGenericSpace`, -1…1 | across the head's whole travel, raw head coordinates (mirror and flip do not apply) |
| `AbsoluteMove` zoom | `PositionGenericSpace`, 0…1 | across the lens's travel |
| `GetStatus` | | `Position` in the same units once the head has homed; `MoveStatus` is `MOVING` from the moment a move is accepted (queued moves included) until head and lens settle, or while a hold runs |

`Speed` is accepted and ignored: a correction is always made with the camera's motion settings
(`Speed`, `Start speed`, `Ramp`), which stay the place to tune it. A request naming another space, a non-finite
number or no axis at all is refused with `ter:SpaceNotSupported` / `ter:InvalidPosition` before anything moves.
Moves go through the same queue as MQTT, so a stop discards them alike.

```yaml
cameras:
  e1zoom:
    onvif:
      host: CAMERA_IP
      port: 80
      user: USER
      password: PASSWORD
      autotracking:
        enabled: true
        zooming: disabled   # the lens zooms only 1.3×; tracking by pan/tilt alone is the sensible start
```

## Relay outputs

ONVIF models any switched output as a relay, so the E1 Zoom's LEDs are two relay outputs
(`GetRelayOutputs`, `SetRelayOutputState`; Device IO adds `GetRelayOutputOptions` and `SetRelayOutputSettings`):

| Token | Setting | Active means |
|---|---|---|
| `status_led` | `led` | the status LED is on |
| `ir_leds` | `ir_leds_at_night` | night mode may light the IR LEDs; inactive leaves the night picture dark |

Both are bistable and idle open (inactive). `SetRelayOutputSettings` accepts exactly that and refuses a
monostable mode or a closed idle state with `ter:ModeError`; an unknown token is `ter:RelayToken`. The state
is not reported back (ONVIF has no read call for it, and no relay event is sent); the MQTT entities show it.

## More PTZ calls

| Call | Behaviour |
|---|---|
| `SetHomePosition` | saves where the camera stands as the `Default` preset, which is where it goes at startup and for `GotoHomePosition` (the node reports `FixedHomePosition="false"`). With no `Default` yet the preset table needs room, as for any new preset |
| `SendAuxiliaryCommand` | `tt:IRLamp|On`, `tt:IRLamp|Off`, `tt:IRLamp|Auto`: the `ir` mode `night`, `day`, `auto`, the same switch as Imaging's `IrCutFilter` (On is night, with the IR LEDs). Listed in the node's `AuxiliaryCommands`; anything else is `ter:InvalidArgs` |
| `GetCompatibleConfigurations` | the one PTZ configuration |
| `Stop` | with no flag, or both flags true, stops everything. The head cannot stop one axis alone, so a flag naming one axis stops both; only `PanTilt` and `Zoom` both false leaves the camera moving |

## Imaging

The E1 Zoom offers the Imaging service at `/onvif/imaging_service`, so a client's image sliders and day/night
switch work without an MQTT broker. Each field is one of the camera's own [settings](../../firmware/cameras/reolink-e1-zoom/docs/settings.md),
and the ranges `GetOptions` reports are those settings' ranges (0…200 for the four levels).

| ONVIF | Setting | Notes |
|---|---|---|
| `Brightness`, `ColorSaturation`, `Contrast`, `Sharpness` | `isp_brightness`, `isp_saturation`, `isp_contrast`, `isp_sharpness` | whole numbers; a fraction is rounded |
| `IrCutFilter` `ON` / `OFF` / `AUTO` | `ir` = `day` / `night` / `auto` | `ON` keeps the filter in for day colour, `OFF` takes it out for night |
| `Focus` `AutoFocusMode` `AUTO` / `MANUAL` | `autofocus` `ON` / `OFF` | |

`GetImagingSettings` reports what the camera says it runs (the state it last published), so a value read
straight after a set may take a moment to follow. A request with any value out of range, not a number or
not one of the modes is refused with `ter:SettingsInvalid` and nothing in it is applied. Sections the camera has
no use for (exposure, white balance, backlight, wide dynamic range) are ignored, so a client that sends back
the document it read still works. Focus *moves* (`Move`, `GetMoveOptions`, `GetStatus`, `Stop`) are not offered:
the focus position is not published, and the focus motor follows the lens zoom.

## Device and media calls

Beyond the read-only device calls, these are answered (the E1 Zoom; a camera without the hook says
`ActionNotSupported`):

| Call | Behaviour |
|---|---|
| `SystemReboot` | queues the same restart as the Reboot button (settings are saved first); the reply may arrive before the camera goes down |
| `GetUsers` | the one user, the RTSP user name, as `Administrator`; users cannot be created or changed |
| `GetSystemLog` (`System`) | the last 32 KiB of the daemon's log, from a line start; `Access` is `ter:SystemlogUnavailable`. The log never holds passwords, and the call needs the camera's login like every other |
| `GetSystemSupportInformation` | manufacturer, model, firmware, hardware, serial, MAC and hostname, one per line |
| `SetSynchronizationPoint` | accepted for any profile; the stream's key frames come every second, so a client waits at most that long and nothing is forced |
| `GetMetadataConfigurations` | an empty list: there is no metadata stream |

Not offered, because the daemon never writes `cam.toml` and does not set the clock from a client:
`SetSystemDateAndTime`, `SetNTP`, `SetHostname`, `CreateUsers` and the other writes. Set the NTP servers in
`cam.toml` instead.

## Presets

Preset tokens are the preset names. Creating a preset through ONVIF saves the camera's current position under a
new name; saving under an existing name moves that preset; deleting removes it. MQTT and ONVIF share one preset
table, so Home Assistant's preset list follows changes made either way.
