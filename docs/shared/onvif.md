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
| PTZ presets: list, go to, create, delete | up to 32 | up to 16 | — |
| Events: digital inputs | — | — | `call_button`, `door` |
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

## Presets

Preset tokens are the preset names. Creating a preset through ONVIF saves the camera's current position under a
new name; saving under an existing name moves that preset; deleting removes it. MQTT and ONVIF share one preset
table, so Home Assistant's preset list follows changes made either way.
