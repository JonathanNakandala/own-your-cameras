# Recovery

What brings the vendor firmware back, from the gentlest step to the last resort, and what each one has and has not
been shown to do. How the boot chooses between `cam` and the vendor stack: [architecture.md](architecture.md#boot).

| Route | Needs | Status |
|---|---|---|
| Remove the flag | SSH | used routinely |
| Three failed starts | nothing | host-tested; the counter and its reset ran on the camera |
| Hold reset at power-on | physical access | by U-Boot's design; erases `/mnt/para` |
| Stage `stock.spub` from Linux | SSH and a healthy NAND | **not rehearsed** |
| UART and TFTP | opening the camera | **not rehearsed** |

## Remove the flag

```sh
ssh root@CAMERA_IP rm /mnt/para/cam.enabled
ssh root@CAMERA_IP reboot
```

Without `/mnt/para/cam.enabled` the boot runs the untouched vendor block of `start_app`. The choice is made once
per boot, so removing the flag takes effect on the **next** boot, never in the running one. Create the flag again
(`touch /mnt/para/cam.enabled`) to go back. Your `cam-*` files and `cam.toml` stay where they are.

## A 3D noise reduction start that never proved healthy

`/mnt/para/cam-3dnr.trial` exists from the moment a start with `isp_3dnr` on builds its graph until both streams
have delivered for 180 s. While it exists the next start runs the plain graph (the failed attempt costs one
reset, never the three counted starts that select the vendor stack). Remove it by turning `isp_3dnr` off and
rebooting, or `rm /mnt/para/cam-3dnr.trial` over SSH. Details: [settings.md](settings.md#3d-noise-reduction-isp_3dnr).

## Three failed starts

`/mnt/para/cam.crashes` counts unsuccessful attempts plus the running one, `0` to `3`. Each boot with the flag
reserves an attempt before anything starts. An attempt that stays healthy for 180 s resets the count to `1`. With
three counted attempts the next clean boot selects the vendor stack, as does a corrupt counter or a leftover
`cam.crashes.tmp`. Nothing re-enables the budget by itself. After finding the cause:

```sh
ssh root@CAMERA_IP rm -f /mnt/para/cam.crashes /mnt/para/cam.crashes.tmp
```

The counter is written with temp file, `fsync`, rename and directory `fsync` on YAFFS2. Interrupting that sequence
at every point was not tested.

## Hold reset at power-on

U-Boot erases the whole `para` partition (`/mnt/para`) when the reset button is held at power-on. That removes the
flag, so the vendor stack boots, and also everything else stored there:

- `cam-wifi.conf`, `cam-rtsp.auth`, `cam.toml`, `cam-settings.toml` and the counter;
- the vendor's own settings (Wi-Fi, users, presets): the camera starts as from the factory and needs the Reolink
  app setup again;
- the SSH host key in `/mnt/para/root-ssh/`. Your authorized key is in the image, and the root-SSH hook generates
  a new host key at boot, so expect a changed-host-key warning [I].

The button never reflashes anything. The installed image (with `cam`) stays, so the flag can be created again once
SSH works.

## Restore stock from Linux: `stock.spub` (not rehearsed)

On **every** boot U-Boot looks at mtd9 (`download`, flash `0x2480000`, `0x1b80000` bytes). If it starts with a valid
SPUB container, U-Boot rewrites the partitions the container lists and clears it. The image build writes a
`stock.spub` holding the stock kernel, rootfs and app; staging it in mtd9 would return the camera to stock 4417.
This route has never been run on a camera. It is described so that someone who must use it knows the risks.

```sh
python3 -I scripts/spub.py build STOCK_PAK -o stock.spub
python3 -I scripts/spub.py --check stock.spub   # layout and CRCs only
```

The builder takes only kernel, rootfs and app (never loader, fdt or U-Boot), refuses settings and calibration
sections, unknown or duplicate sections and malformed records, and is byte-for-byte reproducible. `--check`
verifies names, addresses and partition sizes, CRCs, block alignment and that it fits in mtd9. It does not prove
that the payload boots or that the NAND is healthy.

The layout was recovered from the stock `upgrade` binary, because no captured vendor container exists:

| Part | Rule | Where in `upgrade` |
|---|---|---|
| header, 24 B | `"SPUB"` (`0x42555053`), CRC, `update=1`, `num`, `0`, `ext_len=0` | `0x3f188` |
| header CRC | CRC-32 (poly `0xEDB88320`, start 0, no final xor) over bytes `8 .. 24+60·num` | `0x3f680` |
| entry, 60 B | `name[32]`, `size`, `start`, `crc`, `addr`, `space`, `ext_type=0`, `ext_len=0` | `0x3f524..0x3f558` |
| `start` | `0x2480000` + staging offset | `0x3f5bc` |
| staging offset | first section at block 8 (block 0 holds the descriptor, 1–7 stay `0xFF`); each next section starts on the next 128 KiB block | `0x4a3c0`, loop at `0x45630` |
| `crc` | the same CRC-32 over exactly `size` bytes of the section | `0x458c4` |
| `addr`, `space` | partition start and size from `upgrade`'s table (`.data 0x9c37c`: kernel `0x180000`, rootfs `0x580000`, app `0x1480000`, …) | `0x43df4`, `0x45adc` |

The vendor writer skips bad blocks inside `download` (`0x4dc84`), which would shift its offsets; this builder
assumes fixed physical offsets. **Any bad block in mtd9, an unknown bad-block state or an ECC error rules this route
out.**

If you have to try it:

1. Confirm SSH works, `/proc/mtd` matches, mtd9 currently has zero bad blocks and ECC is clean. Have the stock PAK,
   your own NAND backups and a working UART connection ready.
2. U-Boot consumes mtd9 on **any** boot, including a watchdog reset. Plan power and watchdog so that a partial write
   is never followed by a blind reboot.
3. Erase mtd9 only (`flash_eraseall /dev/mtd9`; BusyBox has no `flash_erase`), then stream the image over SSH into
   `nandwrite -p /dev/mtd9`. Check both exit statuses and stderr. `/mnt/tmp` (20 MiB) is too small to hold it.
4. Before rebooting, read the whole of mtd9 back to the host (data only, ECC on, no OOB), require exactly
   28,835,840 bytes, compare the first 26,214,400 with `stock.spub` and require the rest erased (`0xFF`).
5. Only then reboot, watching U-Boot's log on UART.

Restores kernel, rootfs and app to stock 4417, so `cam` and the boot flag are gone, and with them root SSH (the
stock rootfs does not start it). Keeps `para`, `sp` (MAC, UID and the lens calibration), `ext_para` and the
bootloader partitions.

## UART and TFTP (not rehearsed)

The serial console is `ttyS0`, 115200 8N1, on pads inside the case; the pinout is not documented here. U-Boot has
`bootdelay=0`: press **Ctrl-C** during power-up to reach its prompt. U-Boot provides `nvt_update_all` over TFTP with
the default addresses `ipaddr=192.168.1.99`, `serverip=192.168.1.11`. Its exact arguments were not verified, and an
all-partition update could overwrite protected partitions.

## Never erase or write

`sp` (mtd7), `ext_para` (mtd8), `loader` (mtd0), `fdt` (mtd1), `uboot` (mtd2), and `/dev/mtd10` (`all`, the whole chip).
`sp` holds the unit's MAC, UID and the per-unit lens calibration, which exist nowhere else: back up your NAND
([own-your-cameras](https://github.com/jaxer/own-your-cameras/blob/main/docs/devices/reolink-e1-zoom/firmware-format.md))
before changing anything.
