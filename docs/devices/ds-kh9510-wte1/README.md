# DS-KH9510-WTE1

<img src="device.png" alt="Hikvision DS-KH9510-WTE1" width="280">

[Official datasheet (PDF)](https://assets.hikvision.com/prd/public/all/doc/m000020009/DS-KH9510-WTE1-Video-Intercom-Network-Indoor-Station_Datasheet_20230818.pdf) · [Product image: Hikvision](https://assets.hikvision.com/prd/public/all/doc/m000020009/DS-KH9510-WTE1-Video-Intercom-Network-Indoor-Station_Datasheet_20230818.pdf)

| Item | Observed value |
|---|---|
| Firmware | V2.2.28 build 230613 |
| Platform | R1 / rk312x |
| OS | Android 5.1.1, API 22 |
| Relevant media path | Android hardware AVC decoding |

Start with [root SSH and the local helper](root-access.md).
The original root procedure was verified on this build; the new minimal helper
has only been checked offline.

System-partition changes survived reboots, but firmware updates can replace them.
Inspected reference firmware did not exactly match the installed build; matching
individual binaries is not enough to choose a flash image.

## Platform and storage

| Component | Observed |
|---|---|
| CPU | Rockchip RK3128, four Cortex-A7 cores, ARMv7, NEON/VFPv4 |
| Clock | 216/408/600/696/816 MHz available; firmware ceiling 816 MHz |
| RAM | MemTotal 1,020,956 KiB; about 1 GiB, with 64 MiB zram |
| Storage | eMMC about 7.28 GiB; `/data` about 988 MiB; internal SD filesystem about 5 GiB |
| OS | Android 5.1.1 / API 22, Linux 3.10, Bionic; SELinux permissive in capture |
| Graphics | Mali-400 MP, GLES 2.0 / EGL 1.4; hardware composer enabled |
| Display | 1024×600, reported 57.481 Hz |
| Framebuffer | `/dev/graphics/fb0`, 32 bpp, virtual size 1024×1800 |

Three cores were online in the initial idle capture; four were present. This is
a power-state snapshot. Display refresh is not a measured application frame rate.

## Hardware map

| Function | Interface and evidence |
|---|---|
| Touch | `hik_ts`, initially `/dev/input/event2`; five multitouch slots, X 0–1024 / Y 0–600; physically tested |
| Physical button | Raw `EV_KEY` 249 on `hik_ts`, Android key 400; physically tested |
| Other input | `rk816_pwrkey` and `rk29-keypad` advertised; capabilities do not prove fitted buttons |
| LCD backlight | `/sys/class/backlight/rk28_bl/brightness`, maximum 255; brightness/fade physically tested |
| Microphone/speaker | ALSA card 0 `RK_RK312X`; PCM 0 hifi and PCM 1 voice advertise capture/playback; Android 16 kHz mono microphone and speaker playback physically tested |
| Wall light | Underside illumination, identified physically; GPIO 73 / vendor function 3 on tested firmware; on/off tested, dimming unproven |
| Video decode | `/dev/vpu_service`, `/dev/hevc_service`, Rockchip codec libraries; Android hardware AVC decoding used successfully; supported profiles/resolutions require separate testing |
| Vendor I/O | `/dev/hikio`, `/dev/hik_sysrq`, `/dev/alarmV2`, `/dev/led`, `/dev/noaec`; presence alone does not establish a usable ABI |

The wall light is separate from the LCD backlight. No separate button backlight
was established. Do not infer GPIO or ioctl compatibility with other revisions.

The firmware consumes some key-400 release/long-press events before Activity
dispatch. A working app observed raw `EV_KEY` 249 without an exclusive grab;
ordinary Activity callbacks did not provide a reliable press/release pair.
Root can inspect input devices; ordinary apps need a deliberate access grant.

## Inspect your device

First obtain [root SSH](root-access.md), then connect with a PTY:

```sh
ssh -tt -i OWNER_PRIVATE_KEY root@CAMERA_HOST
```

Run these read-only commands inside that shell:

```sh
getprop ro.build.version.release
getprop ro.build.version.sdk
cat /proc/version
getenforce
cat /proc/cpuinfo
cat /proc/meminfo
cat /proc/partitions
df
cat /sys/devices/system/cpu/present /sys/devices/system/cpu/online
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_frequencies
cat /sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq
cat /proc/bus/input/devices
getevent -pl
cat /proc/asound/cards /proc/asound/pcm
cat /sys/class/graphics/fb0/virtual_size /sys/class/graphics/fb0/bits_per_pixel
cat /sys/class/backlight/rk28_bl/brightness /sys/class/backlight/rk28_bl/max_brightness
dumpsys SurfaceFlinger
```

Compare against the tables above. Device numbers and online CPUs can differ.
Keep full output private: CPU and other diagnostic output can contain identifiers.

## Running your own program

- Target 32-bit Android ARMv7 and API 22. Desktop Linux binaries are not drop-in replacements.
- Start with an Android Activity/NativeActivity, GLES 2.0 and Android audio/media APIs.
- SurfaceFlinger owns framebuffer, Mali, ION and RGA resources. Extra framebuffer nodes do not prove independently usable overlays.
- Keep `com.hik.vis.indoor` running: its process owns `/dev/watchdog` and vendor control devices. Watchdog handover and disable semantics remain unverified.
- Keep SurfaceFlinger running: its init restart rule also restarts zygote.

Replacing the foreground UI was demonstrated while retaining stock services.
A fully independent hardware takeover, UART recovery and replacement firmware
have not been demonstrated. No new on-device checks were performed for this page.
