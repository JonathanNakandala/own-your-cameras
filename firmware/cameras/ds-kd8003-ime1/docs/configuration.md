# Configure the DS-KD8003-IME1

Two files sit next to `start.sh` on the config partition, both mode 0600:
[`cam.conf`](../firmware/cam.conf.example) and [`pins.conf`](../firmware/pins.conf.example).
`cam.conf` holds only secrets, identity and paths and is read once at start; an unknown key or an
invalid value stops the application (and counts as an unhealthy boot). Everything else (door open
time, volumes, streams, image) is a [live setting](home-assistant.md#settings) in Home Assistant,
stored in `settings.json` in the same directory.

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
| `keypad_key` | `/home/config/custom/keypad.key` | 16-byte keypad bootstrap key |
| `pins` | `/home/config/custom/pins.conf` | PIN table |
| `clips` | `/home/app/custom/native/clips` | directory of `<name>.pcmd` (as installed) or `<name>.pcm` clips |
| `http_port` | 0 | read-only diagnostics (`GET /state`, `GET /log?after=N`); 0 is off |
| `call_port` | 8089 | direct intercom link for the indoor panel (TCP, authenticated with the RTSP account); 0 is off |
| `onvif_port` | 80 | [ONVIF](../../../../docs/shared/onvif.md) with the RTSP credentials; 0 is off |
| `ntp` | empty | NTP servers, comma-separated (up to four); empty leaves the clock alone. While in sync, `lock/state` also carries `relock_at` |

`settings.json` (mode 0600) is written at most every 10 s after a change and survives a
reinstall. Deleting it and restarting starts over as a first run: defaults, image settings as the
ISP reports them.

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

Give the installer `<name>.pcm` files, raw s16le mono 16 kHz:

```sh
ffmpeg -i input.wav -ac 1 -ar 16000 -f s16le door_opened.pcm
```

It trims each and levels it: the silence or room noise before the first sound and after the last
goes (the ringback `call_dial_tone` keeps its length, its pause is part of the loop), and every clip
is set to the same loudness, -12 dBFS RMS over the audible part, with a limiter holding peaks at
-1 dBFS. The built-in tones play at the same level, so with Announcement gain at 0 every sound is as
loud as it gets without clipping; a higher gain clips the peaks. It stores each as `<name>.pcmd` in
`clips.tar` (the levelled samples, losslessly in a form that packs smaller). The application reads both `.pcmd` and plain `.pcm` files. Below, `<name>.pcm` stands for
either. `door_opened.pcm` is the fallback for any code whose clip is missing; an optional `denied.pcm`
replaces the refusal tone. A clip plays at most its first 10 s. `call_dial_tone.pcm` is the
[intercom call](home-assistant.md#intercom-call)'s ringback: one period of the tone and its pause
(for example 2 s tone, 2 s silence), looped while it rings; without it the call rings silently at
the gate.
