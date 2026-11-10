# Hardware notes

What the daemon drives on the Reolink E1 Zoom (board `IPC_566SD664M5MP`, firmware `v3.1.0.4417_2412122178`),
and how each fact was established. Nothing here comes from a datasheet: the vendor publishes none for this
board, so every register, item and offset was read from the stock firmware or measured on a camera.

| Part | Chip or interface | Notes |
|---|---|---|
| SoC | Novatek NA51089, one Cortex-A9, 80 MiB DDR (about 47.5 MiB reserved as the `nvtmpp` media pool) | [../hdal-abi.md](../hdal-abi.md) |
| Flash | 64 MiB SPI-NAND, 128 KiB erase blocks | [../recovery.md](../recovery.md) |
| Sensor | OmniVision OS05A10, 2592×1944 at 30 fps over MIPI | [../architecture.md](../architecture.md#media-graph) |
| Pan/tilt | MS32006 dual stepper driver on I²C | [ptz.md](ptz.md) |
| Zoom/focus | YT3005 lens, AN41908 motor driver behind `/dev/nvt_isp` | [zoom-focus.md](zoom-focus.md), [position3d.md](position3d.md) |
| Day/night | IR-cut coil (on the MS32006), IR LEDs, light ADC, status LED | [ir.md](ir.md) |
| Picture | ISP image-quality and exposure items | [isp-iq.md](isp-iq.md) |
| Audio | codec microphone and speaker, a GPIO-switched speaker amplifier | [audio.md](audio.md) |
| Network | Realtek RTL8822CU Wi-Fi on USB, Ethernet fallback | [network.md](network.md) |
| Motion | `kdrv_md` hardware engine, driven by the vendor stack; not used by `cam` | [motion-detection.md](motion-detection.md) |
| AI detection | Person, vehicle and pet CNN (`ppvd.bin`) on the AI unit via `kdrv_ai`; not used by `cam` | [ai-detection.md](ai-detection.md) |

## Evidence tags

Each fact carries one tag:

- **[V]** verified on a camera: the register was written or read and the effect observed (frames, dmesg,
  sysfs read-back, motor travel).
- **[D]** read from the disassembly of the stock `device` binary from firmware 4417
  (SHA-256 `face96a6802e3725c8d4106c1e77a71f840bd380f7ad0ccc81b7791c51302dd9`). Addresses are VMAs; the file
  offset is VMA − 0x10000. Kernel module addresses are offsets in the named `.ko`.
- **[R]** read from the memory of the running stock `device` process (`/proc/<pid>/mem`).
- **[I]** inferred from the above, not observed directly.

A document without per-line tags states its evidence once at the top.
