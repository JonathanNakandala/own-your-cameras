# Private configuration

Create these files through your already trusted root SSH session before enabling the native daemon.
Keep local copies outside the checkout. Do not pass passwords in command-line arguments or public logs.

## Camera configuration

`/mnt/data/mi360/cam.toml` is a root-owned 0600 regular file with one link, at most 16 KiB.
Its immediate directory must be root-owned 0700. Replace all placeholders:

```toml
[camera]
id = "mi360"
name = "Mi360"
[mqtt]
host = "MQTT_HOST"
port = 1883
username = "MQTT_USERNAME"
password = "MQTT_PASSWORD"
[rtsp]
username = "RTSP_USERNAME"
password = "RTSP_PASSWORD"
port = 554
[runtime]
audio = true
watchdog = true
extended_controls = true
cold_controls = true
[onvif]
port = 80
[time]
ntp = ["NTP_HOST"]
```

`[onvif]` and `[time]` are optional: ONVIF is on by default on port 80 (0 turns it off,
[ONVIF](../../../../docs/shared/onvif.md)), and without NTP servers the clock is left alone.

Choose a unique camera ID on your broker: 1–48 lowercase letters, digits or underscores.
The default `mi360` preserves existing installations. The ID selects `camera/<id>/…` topics,
MQTT client identity and HA discovery identifiers; changing it requires removing obsolete retained
HA discovery records yourself. The on-device storage paths remain the same for each physical unit.

`extended_controls` enables qualified live image/audio and motion settings. `cold_controls` enables
staged resolution/profile controls, Apply and Discard. Apply saves a bounded next-boot trial and
reboots; only matching actual stream metadata and sustained real health confirm it. Failed trials
fall back to the last confirmed graph on a subsequent clean boot. Watchdog must stay enabled.

Prepare directories and stream the private file without putting credentials in the shell command:

```sh
ssh root@CAMERA_HOST 'umask 077; mkdir -p /mnt/data/mi360 /mnt/data/mi360-network; chmod 700 /mnt/data/mi360 /mnt/data/mi360-network'
ssh root@CAMERA_HOST 'umask 077; cat > /mnt/data/mi360/cam.toml' < /path/to/private/cam.toml
```

## Wi-Fi configuration

`/mnt/data/mi360-network/wpa.conf` is root-owned 0600 under a 0700 directory.
Use the real working WPA network configuration retained during initial setup. Do not execute it as
shell code. This is a `wpa_supplicant` file, for example:

```text
network={
    ssid="YOUR_WIFI_SSID"
    psk="YOUR_WIFI_PASSWORD"
    key_mgmt=WPA-PSK
}
```

Use only a 2.4 GHz network supported by the RTL8188FU radio. Preserve security parameters required by
your actual network rather than blindly copying this minimal shape.

```sh
ssh root@CAMERA_HOST 'umask 077; cat > /mnt/data/mi360-network/wpa.conf' < /path/to/private/wpa.conf
```

No broker account, ACL, firewall, DNS or Home Assistant configuration is changed by the firmware.
MQTT uses the configured local broker without TLS; place it on a network appropriate for that policy.
RTSP digest authentication authenticates access but does not encrypt media. No Internet exposure is
needed for either service.

## State files

`settings.json`, `video-settings.json`, `presets.json` and the optional `position3d.json` remain private
under `/mnt/data/mi360`. Settings save only confirmed hardware state. Presets save actual stopped,
homed coordinates with final approach direction. Do not restore an entire DATA partition merely to
change one setting: it also contains Wi-Fi, SSH and other device-specific state.
