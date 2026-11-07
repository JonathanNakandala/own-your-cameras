# DS-2CD2T87G2-L — G3

<img src="device.png" alt="Hikvision DS-2CD2T87G2-L" width="177">

[Official datasheet (PDF)](https://www.hikvision.com/content/dam/hikvision/products/S000000001/S000000002/S000000003/S000000025/OFR000043/M000041483/Data_Sheet/DS-2CD2T87G2-L_Datasheet_V5.5.154_20201202.pdf) · [Hikvision image / mirrored datasheet](https://www.doriscctv.co.uk/downloads/DS-2CD2T87G2-L_Datasheet_V5.5.154_20220114.pdf)

Tested: original non-C G3 hardware, fixed 8 MP ColorVu camera.
Root access used V5.5.154 build 200928; the managed upgrade installed official
V5.5.820 build 231108. Start with [root and persistent SSH](../../shared/root-access.md).

## Hardware

| Part | Observed value |
|---|---|
| SoC | Hi3516CV500, dual Cortex-A7 |
| ABI | ARMv7-A softfp, NEON / VFPv4 |
| OS | Linux 4.9.37; runtime files named `ld-2.24.so` and `libc-2.24.so` |
| RAM | About 246 MiB usable |
| Application storage | `/dav`, UBIFS, about 187 MiB |
| Configuration | `/config` and `/davinci`, small shared settings volume; SQLite |
| Temporary storage | Root and `/home`, RAM-backed |
| Neural inference | `/dev/nnie`, owned by `hikdsp` |
| Image processing | `/dev/ive`, owned by `hikdsp` |
| White illumination | GPIO 81; IR/white PWM0/PWM1 |
| IR-cut | GPIO 72/73 |
| Speaker amplifier / microphone select | GPIO 52 / 53 |
| Status indicators | GPIO 74/77 |
| PHY reset / peripheral MCU reset | GPIO 62 / 91 |
| Sensor | I2C `0x36`, 8 MP MIPI, board code HIK188; exact sensor part unconfirmed |

Use ARMv7 softfp builds, not the DS-KD8003-IME1's ARMv5/uClibc binary assumptions.
Do not kill `davinci` or `hikdsp`: watchdog and `guard.sh` behavior can send the
camera into recovery. `/home/webLib` is HTTP-served; keep private files elsewhere.

## Upgrade without losing access

This documents the tested sequence, not a complete installation recipe. Supply
your own SSH worker and loader; use the [offline decoder](../../shared/firmware-decoding.md)
to extract the reference startup script.

**Requires a working root SSH session and installed key-only access.** The tested
updater preserved extra `/dav` files but replaced `/dav/initrun.sh`, removing the
startup entry point. Keep the old root session alive throughout these steps.

1. Back up settings and the existing access files. Obtain the matching official
   [G3 firmware archive](https://assets.hikvision.com/prd/public/all/files/202312/Firmware__V5.5.820_231108_S3000539625.zip).
2. Check the extracted DAV SHA-256 against the value below.
3. Upload through `PUT /ISAPI/System/updateFirmware`. The tested upload returned
   success with reboot required; **do not send the reboot yet**.
4. Read the new `/dav/initrun.sh` through the existing root shell. Compare it with
   the script extracted from that exact package. If it differs, stop.
5. Restore the call to your access loader at the end of the new script, check with
   `/bin/sh -n /dav/initrun.sh`, and read it back to verify the exact change.
6. Reboot using `PUT /ISAPI/System/reboot`. Verify the firmware version, pinned SSH
   host key, `uid=0`, fresh images and the settings you need.

The restored entry point was a call to `/dav/custom_initrun.sh`; the loader started
the access worker. Unlike the G5 build, this G3 build does not call that loader
unless you add the call. The tested process used byte-for-byte comparisons before
and after appending it. Firmware extraction is included; upload and access-installation tools are not.

```text
Official DAV SHA-256:
0887eac3807f9afcf1912c0faba5b580874000de27b0f184ed5ef17bd67b42dc
```

The reboot retained key-only root access. Fresh snapshots passed, and 80 comparable
settings responses were unchanged. Cross-version backup restore and downstream
recording/automation were not independently tested. Future firmware may behave
differently; unattended updates can remove the startup hook.

## Recognition hardware

IVE supplies background subtraction, morphology and connected components through
`libhms.so` / `HI_MPI_IVE_*`. NNIE runs neural models inside `hikdsp`.
`/dav/G3_model.bin` contains a YOLOv2-labelled human/vehicle detector among other
models. Supported region/line events can expose human/vehicle classification via
ISAPI without running another detector.

`libPicDetect.so` exports `PICDETECT_CreateHandle`, `PICDETECT_Inspect` and
`PICDETECT_DestroyHandle`. These are investigation leads, not a tested independent
SDK.

NNIE requires a compatible mapper/runtime, supported operators and quantization.
No custom model or benchmark is included; copying an ONNX file onto the camera
is not enough. For an integration, start with vendor ISAPI region/line events
and configure linkage on the exact firmware.

## Inspect vendor firmware

[Decode the G3 file table and payloads offline](../../shared/firmware-decoding.md)
with the included G3 seeds. No camera connection or root access is needed for
extraction. This does not sign modified packages or establish stock-updater acceptance.
