# Install and enable

## With the installer

On an untouched panel (stock V2.2.62 build 230204), from a [release bundle](../../../RELEASES.md#ds-kd8003-ime1)
(or one built locally with `python3 firmware/scripts/release.py ds-kd8003-ime1 --version
ds-kd8003-ime1-v0.0.0-local --output DIR`):

```sh
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python install.py
```

It asks for the panel address, the administrator password, the MQTT broker, the RTSP login, the PIN
table and your SSH public key (any of them can be a flag or a `--config` TOML entry instead; `--help`
lists them), then:

1. checks the model and firmware over ISAPI and enables the stock SSH service;
2. reads the firmware partition (`/dev/mtd1`, 14 MiB, about half a minute) over the stock SSH service,
   rebuilds the stock package from it and checks it by SHA-256, takes the keypad key from the stock
   `hicore` in it, adds the [startup hook](../../../../docs/devices/ds-kd8003-ime1/firmware.md) and,
   after you confirm, flashes the result over ISAPI;
3. uploads the files below, and after you confirm switches to the native application;
4. waits until `cam.log` shows RTSP, MQTT and the keypad running.

A panel that already has the hook skips the flash. The firmware read and the build evidence stay in
`--work-dir` (a new temporary directory by default).

## What goes on the config partition

The partition is small and holds only compressed files; [`start.sh`](../firmware/start.sh) unpacks
`native/*.lzma` into RAM (`/home/app/custom/native`) at every boot with BusyBox `unlzma`. Each file is
compressed on its own in the `lzma` (LZMA-alone) format.

| `/home/config/custom/…` | Content |
|---|---|
| `start.sh` | the boot supervisor, from `firmware/start.sh` |
| `native/ds-kd8003-ime1-cam.lzma`, `libloader_compat.so.lzma`, `libnative_compat.so.lzma`, `libnative_media_group.so.lzma` | [this build](build.md) |
| `native/libmpi.so.lzma`, `libVoiceEngine.so.lzma`, `libdnvqe.so.lzma`, `libupvqe.so.lzma` | HiSilicon Hi3516CV300 MPP V1 user-space libraries matching the panel's drivers (the stock application links them statically); release bundles carry them in `third-party/`, from OpenIPC at a pinned commit |
| `native/clips.tar.lzma` | the [clips](configuration.md#clips); the installer adds a chime as `door_opened.pcm` unless you supply your own |
| `native/dropbearmulti.lzma` | the [key-only SSH server](build.md#maintenance-ssh) |
| `cam.conf`, `pins.conf` | [configuration](configuration.md) |
| `keypad.key` | 16 bytes: the bootstrap key the stock application uses with the keypad module, read from the stock `hicore` at a fixed offset of the pinned build and checked by hash. Without it the keypad session cannot start; video and MQTT still run |
| `ssh/authorized_keys` | who may log in over the key-only SSH server |
| `enabled` | the hook's own flag: without it the hook runs no script |
| `native.enabled` | the switch |

`libe3_isp_access.so` is not copied: the supervisor links the stock image's own file.

## Check by hand

```sh
ssh -T root@CAMERA_HOST <<'SH'
cat /home/app/custom/native/supervisor.log
grep -E 'start,|rtsp: serving|mqtt: connected|keypad: ready|running|health' /home/app/custom/native/cam.log
ls /home/config/custom/native.ok
SH
```

Expected in `supervisor.log`: `supervisor start`, `dropbear started`, `network <address> via <gateway>`,
`start ds-kd8003-ime1-cam (boot 1)`; in `cam.log`: `start, N PINs, open 5 s`, `rtsp: serving on 554`,
`running`, `mqtt: connected`, `keypad: ready` and, after two minutes of streaming,
`health: boot marked good`. `native.ok` exists from then on.

In native mode the vendor web interface, ISAPI and the vendor SSH daemon are stopped: log in with your key.

## Switch

| Goal | Command, then `reboot` |
|---|---|
| native | `touch /home/config/custom/native.enabled` |
| stock | `rm /home/config/custom/native.enabled` |
| re-arm after the fallback tripped | `rm /home/config/custom/native.boots /home/config/custom/native.fallback` |
| run no custom script at all | `touch /home/config/custom/disabled` |
