# Configure the DS-KD8003-IME1

Two files sit next to `start.sh` on the config partition, both mode 0600:
[`cam.conf`](../firmware/cam.conf.example) and [`pins.conf`](../firmware/pins.conf.example).
`cam.conf` is read once at start; an unknown key or an out-of-range value stops the application
(and counts as an unhealthy boot).

## `cam.conf`

| Key | Default | Meaning |
|---|---|---|
| `mqtt_host` | required | broker address |
| `mqtt_port` | 1883 | |
| `mqtt_user`, `mqtt_password` | empty | broker login |
| `mqtt_prefix` | `ds-kd8003-ime1` | topic prefix, `[A-Za-z0-9_/-]` |
| `mqtt_node` | `ds_kd8003_ime1` | Home Assistant device id and unique-id prefix, `[A-Za-z0-9_-]` |
| `device_name` | `DS-KD8003-IME1` | device name in Home Assistant, printable ASCII without `"` or `\` |
| `rtsp_user`, `rtsp_password` | required | RTSP digest credentials |
| `rtsp_port` | 554 | |
| `open_seconds` | 5 | strike pulse on an opening, 1..120 s |
| `codec_out_db` | 0 | DAC output level, -30..6 dB |
| `clip_db` | 0 | digital gain on clips, -40..12 dB |
| `talk_db` | 6 | gain on talkback after the mixer, 0..18 dB (clipped) |
| `mic_gain_db` | 0 | microphone ADC gain, 0..40 dB |
| `keypad_key` | `/home/config/custom/keypad.key` | 16-byte keypad bootstrap key |
| `pins` | `/home/config/custom/pins.conf` | PIN table |
| `clips` | `/home/app/custom/native/clips` | directory of `<name>.pcm` clips |
| `http_port` | 0 | read-only diagnostics (`GET /state`, `GET /log?after=N`); 0 is off |
| `onvif_port` | 80 | [ONVIF](../../../../docs/shared/onvif.md) with the RTSP credentials; 0 is off |
| `ntp` | empty | NTP servers, comma-separated (up to four); empty leaves the clock alone |

Changing `mqtt_prefix` or `mqtt_node` later makes Home Assistant see a new device. The diagnostics
port has no credentials and its log names who opened the door; enable it only on a trusted network.

## `pins.conf`

One code per line: `<digits> <name> [clip]`, `#` starts a comment. Codes are 4–8 digits, names and
clip names `[A-Za-z0-9_-]`, and the clip defaults to `door_opened`. No code may contain another
one: codes open without `#`, so the longer one could never be told from the shorter one inside it.

## PIN policy

- A code opens as soon as its digits are the last ones typed; `#` after the digits also submits,
  and a submission that matches nothing is a refusal.
- Digits typed before a 20 s pause, or before an opening, do not count towards the next code.
- A code is not judged again while the strike it released is still open: the keypad re-sends its
  buffer, and `#` after a matched code is the same visitor.
- Eight digits that matched nothing are a refusal and start a fresh entry, so a long run of digits
  is not free guessing.
- A refusal plays `denied.pcm` (or a built-in tone) and raises the `access` event `denied` with the
  consecutive-failure count. There is no lockout; Home Assistant can alert on a streak.
- Decisions are logged by name, never by digits.

## Clips

`<name>.pcm` files, raw s16le mono 16 kHz, packed as `clips.tar`:

```sh
ffmpeg -i input.wav -ac 1 -ar 16000 -f s16le door_opened.pcm
```

`door_opened.pcm` is the fallback for any code whose clip is missing; an optional `denied.pcm`
replaces the refusal tone. A clip plays at most its first 10 s.
