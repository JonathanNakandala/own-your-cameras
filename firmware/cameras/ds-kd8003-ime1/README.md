# DS-KD8003-IME1 firmware

Local video and two-way audio over RTSP, keypad PIN access with a timed strike pulse and a spoken
clip per code, and Home Assistant controls and events over MQTT. Replaces the vendor application at
run time; the stock image, kernel and drivers stay, and stock comes back after two unhealthy boots.

**Supported:** original non-B DS-KD8003-IME1 · board DS-17040, Hi3516CV300 / AR0237 · stock
firmware V2.2.62 build 230204.

## Get started

Download the [release bundle](../../RELEASES.md#ds-kd8003-ime1) and run `install.py` on an untouched
panel: it asks for the panel address, the administrator password, MQTT, the RTSP login, the PIN table
and your SSH key, then flashes the startup hook and installs and enables the application.
[What it does, and the manual path](docs/install.md) · [Build from source](docs/build.md) ·
[Configuration](docs/configuration.md)

## Use

| RTSP stream | Video | Audio |
|---|---|---|
| `Streaming/Channels/101` (also `main`) | 1920×1080 H.264 | PCMU, 8 kHz mono |
| `Streaming/Channels/102` (also `sub`) | 704×480 H.264, 15 fps | PCMU, 8 kHz mono |

The microphone is PCMU, not Opus as on the other cameras, because Synology Surveillance Station records
G.711 but drops Opus audio. The stock paths and digest authentication keep existing recorders working. Both paths offer an Opus
backchannel (ONVIF `Require: www.onvif.org/ver20/backchannel`) that plays on the device's speaker.
Home Assistant discovers the door, its open and close buttons, the reed and lock states, the
doorbell and the access events through your broker: [MQTT and Home Assistant](docs/home-assistant.md).

Not yet: card reader, image settings over MQTT, snapshots, calls to the DS-KH9510-WTE1.

## Recover

Remove `native.enabled` next to `start.sh` and reboot to get the vendor application back. Two
unhealthy boots in a row do the same by themselves. See [recovery](docs/recovery.md).

[Architecture](docs/architecture.md) · [Device notes](../../../docs/devices/ds-kd8003-ime1/README.md) ·
[Shared firmware development](../../README.md) · [MIT license](../../LICENSE)
