# Reolink E1 Zoom — IPC_566SD664M5MP

[Build and install key-only root SSH](root-access.md) ·
[PAK, UBI and NAND reference](firmware-format.md) ·
[Mic and speaker frequency response](../../shared/camera-audio-response.md)

Own firmware user-space built on this root access (local RTSP, two-way audio, PTZ,
Home Assistant over MQTT): [Reolink replacement firmware](../../../firmware/cameras/reolink-e1-zoom/README.md).

**Tested:** the 5 MP `IPC_566SD664M5MP` hardware running
`v3.1.0.4417_2412122178`. Other E1 Zoom revisions are outside this recipe.

| Part | Observed value |
|---|---|
| CPU | ARM Cortex-A9; use `-mcpu=cortex_a9` |
| OS | Linux 4.19.91, vendor glibc userspace |
| Flash | 64 MiB NAND; 128 KiB eraseblocks, 2048-byte pages, 64-byte OOB |
| Root filesystem | SquashFS 4.0 / XZ in a dynamic UBI `rootfs` volume |
| Persistent configuration | `/mnt/para`, YAFFS2, 4 MiB |
| Startup | `/etc/init.d/rcS` → `S99_Sysctl` → `start_app` |

A stock-derived image with a static Dropbear server booted on this hardware and
accepted a real public-key login as UID 0. Password authentication was unavailable.
The running binary matched the locally built Cortex-A9 binary. Eleven selected
configuration responses matched their pre-flash values. This establishes SSH
access; independent audio/PTZ control and duplex voice quality remain untested.

The included tools are a generalized export of that procedure. They require your
own stock firmware, administrator credentials and SSH public key. Firmware,
private data and compiled workers are not distributed here. The exported build
and upload code is checked offline; it has not been used for another live flash.

**CPU lesson:** a Cortex-A7 build booted the vendor services but failed to start
SSH. Exact Cortex-A9 emulation reproduced an unsupported `UDIV` / SIGILL.
An ARMv7 ELF header or a successful test on Cortex-A15 is insufficient.

Keep the vendor services and watchdog running while investigating hardware.
Root access alone does not establish safe ownership of drivers or devices.
