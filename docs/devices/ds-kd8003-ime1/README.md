# DS-KD8003-IME1

<img src="device.png" alt="Hikvision DS-KD8003-IME1" width="280">

[Official datasheet (PDF)](https://www.hikvision.com/content/dam/hikvision/products/S000000001/S000000083/S000000129/S000000131/OFR002104/M000001550/Data_Sheet/DS-KD8003-IME1_Module-Door-Staion_Datasheet_V1.0.0_20210827.pdf) · [Hikvision image / mirrored datasheet](https://www.avdsecurity.nl/File/2985)

Tested: original non-B hardware, DS-17040 board, V2.2.62 build 230204.

1. Get a root shell and inspect hardware below.
2. [Build firmware with a startup hook](firmware.md).
3. [Install the native application](../../../firmware/cameras/ds-kd8003-ime1/README.md), which replaces
   the vendor application through that hook.

[Package format and verification evidence](firmware-format.md).

## Get a root shell

Enable maintenance SSH through the device's supported configuration. Log in as
`root` using the existing administrator password, **without a terminal**:

```sh
ssh -T root@CAMERA_HOST
# Type into the session:
id
exit
```

Expected: `uid=0`. For a short check:

```sh
printf 'id\nexit\n' | ssh -T root@CAMERA_HOST
```

Why it works: `/etc/passwd` starts `/bin/sh`; the profile starts the restricted
`psh` shell as a child. Without a terminal, `psh` exits and the parent shell reads
your commands. A `psh` banner or `stty` warning can still appear. A normal
interactive terminal stays restricted. This method does not apply to the G3/G5
cameras, where `psh` is the login shell itself.

## Platform and storage

| Part | Observed value |
|---|---|
| SoC / CPU | Hi3516CV300 / ARM926EJ-S, ARMv5TEJ soft float |
| OS | Linux 3.18.20, uClibc 0.9.33.2 |
| RAM | 192 MB physical; about 184 MiB usable |
| Flash | 16 MiB SPI NOR, Winbond W25Q128 |
| Persistent writable files | `/home/config`, 1.5 MiB JFFS2, shared with vendor settings |
| Vendor firmware | `/home/hik`, read-only cramfs |
| Temporary files | RAM root and `/home/app`; lost at reboot |

Flash offsets: bootloader `0x000000–0x060000`, config `0x060000–0x1E0000`,
firmware `0x1E0000–0x1000000`. The bootloader was not backed up in this research.

## Hardware map

| Function | Interface |
|---|---|
| Camera | AR0237, I2C0 `0x36`, MIPI, 27 MHz clock |
| Video pipeline | `/dev/vi`, `/dev/vpss`, `/dev/venc`, `/dev/isp_dev` |
| Microphone | `/dev/ai`, `/dev/aenc`, internal hi_acodec |
| Speaker | `/dev/ao`, `/dev/adec`, `/dev/DDM/sound` |
| Speaker amplifier | GPIO 13, high = on |
| Voice DSP / echo cancellation | FM2018, I2C1 `0x60` |
| Call button | GPIO 35, active low; `/dev/event0`, EV_KEY code 5 |
| Lock output COM1 | GPIO 39, high = electrically unlocked |
| Second relay COM2 | GPIO 38 |
| Door reed input AI1 | GPIO 34 |
| Exit-button input AI3 | GPIO 37 |
| Other alarm inputs | GPIO 36 / 32 |
| Tamper | GPIO 60 |
| LEDs | GPIO 57/58/59; additional GPIO 29 |
| IR illumination | PWM-related pins 50/30/67 |
| DS-KD-KP keypad | `/dev/ttyS1`, RS-485 half duplex, 19200 8N1 |
| Watchdog | `/dev/DDM/wdt`, about 28 seconds |

For the door input, open `/dev/DDM/alarmV2` and call ioctl `0x80044201` with a
**64-byte buffer**. Bit 0 set means door open, after polarity correction. The
nominal ioctl size says four bytes, but the driver writes more; do not allocate
just four. GPIO 39 reports electrical lock drive, not mechanical latch engagement.

Evdev can have several readers. A UART cannot be observed the same way: readers
split its bytes. Media devices also share global channels even when multiple
opens succeed. No standard ALSA or V4L2 interface was found.

## Speaker and microphone

Use ISAPI Digest authentication with the administrator account. `curl` prompts
for the password when `--user admin` is given without a password. Enable channel 1:

```sh
curl --digest --user admin --request PUT \
  --header 'Content-Type: application/xml' --data-binary @- \
  'http://CAMERA_HOST/ISAPI/System/TwoWayAudio/channels/1' <<'XML'
<TwoWayAudioChannel><id>1</id><enabled>true</enabled><audioCompressionType>G.711ulaw</audioCompressionType></TwoWayAudioChannel>
XML
```

Then send **raw 8 kHz mono G.711 µ-law**, without a WAV header:

```sh
curl --digest --user admin --request PUT \
  --header 'Content-Type: application/octet-stream' \
  --data-binary @speech.ulaw \
  'http://CAMERA_HOST/ISAPI/System/TwoWayAudio/channels/1/audioData'
```

This plays through the speaker. The vendor handles amplifier power. Enable again
before each new body: this build auto-disables the channel. The endpoint is
`audioData`, not `audio`. `GET .../audioData` returned HTTP 400; listen through RTSP
channel 101 instead. Its audio was G.711 µ-law.

## Running your own program

Build for ARMv5TE soft float and the matching uClibc runtime. Stock firmware does not run a
script just because it is placed in `/home/config`; the [startup hook](firmware.md) runs one at
every boot, and the [native application](../../../firmware/cameras/ds-kd8003-ime1/README.md) uses
it to replace the vendor application: RTSP, two-way audio, keypad PIN access and MQTT.
