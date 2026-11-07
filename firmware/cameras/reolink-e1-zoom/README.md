# Reolink E1 Zoom firmware

Local video and two-way audio over RTSP, pan/tilt/zoom, autofocus, day/night controls
and Home Assistant discovery over MQTT. Uses the camera's stock kernel and drivers.

**Supported:** E1 Zoom · IPC_566SD664M5MP · stock firmware v3.1.0.4417_2412122178.

## Get started

1. [Get root SSH access](../../../docs/devices/reolink-e1-zoom/root-access.md) and keep your NAND backup.
2. [Build the firmware](docs/build.md).
3. [Configure and install](docs/install.md).

## Use

ONVIF (discovery, profiles, snapshot, PTZ and presets) is on by default: [ONVIF](../../../docs/shared/onvif.md).

```yaml
streams:
  e1zoom:
    - rtsp://USER:PASSWORD@CAMERA_IP:554/main
  e1zoom_sub:
    - rtsp://USER:PASSWORD@CAMERA_IP:554/sub
```

Main video defaults to 1920×1440; sub video is 640×480. Both include Opus audio (48 kHz mono) and an
Opus talkback channel that go2rtc uses; up to four clients can talk at once. `main_pcmu` and `sub_pcmu`
carry the same video with G.711 µ-law 8 kHz audio for ONVIF recorders such as Surveillance Station. Connect Home Assistant to your MQTT broker to discover
controls and settings; see [configuration and entities](docs/home-assistant.md).

## Recover

```sh
ssh root@CAMERA_IP 'rm -f /mnt/para/cam.enabled; sync; reboot'
```

This returns to the vendor firmware and keeps settings. Three failed starts also select vendor
firmware. For reset-button recovery and its data loss, see [recovery](docs/recovery.md).

[Settings](docs/settings.md) · [Hardware](docs/hardware/README.md) · [Development](docs/development.md) ·
[Architecture](docs/architecture.md) · [MIT license](../../LICENSE)
