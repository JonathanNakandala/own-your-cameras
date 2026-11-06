# Mi 360 firmware

Local video and two-way audio over RTSP, pan/tilt, saved positions, image settings and
Home Assistant controls over MQTT. Uses the camera's stock kernel and drivers.

**Supported:** MJSXJ10CM · T31 / SC2336 · stock firmware 4.3.4_0376.

## Get started

1. [Get root SSH access](../../../docs/devices/mi-360/root-access.md) and keep your original backup.
2. [Build the daemon and image](docs/build.md).
3. [Configure Wi-Fi, RTSP and MQTT](docs/configuration.md).
4. [Install and enable](docs/install.md) using the inactive slot to preserve settings and SSH keys.

## Use

| RTSP stream | Default video | Audio |
|---|---|---|
| `Preview_01_main` | 1920×1080, 20 fps | Opus, 48 kHz mono |
| `Preview_01_sub` | 640×360, 20 fps | Opus, 48 kHz mono |

Both paths offer an Opus backchannel (ONVIF `Require: www.onvif.org/ver20/backchannel`); up to four
clients can talk at once and are mixed on the camera.

Connect go2rtc to the camera's RTSP port with your configured username/password.
Home Assistant discovers the controls through your MQTT broker.
Image-point controls need [pointing calibration](docs/hardware.md#image-pointing).

## Recover

Remove `/mnt/data/mi360/enabled` and reboot to keep only networking and SSH.
Three failed starts also select this mode. See [recovery](docs/recovery.md) for AP access.

[Hardware](docs/hardware.md) · [Shared firmware development](../../README.md) · [MIT license](../../LICENSE)
