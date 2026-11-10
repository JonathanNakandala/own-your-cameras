# Architecture

The daemon is a member of the shared `firmware/` workspace. Its `rtsp` module adapts runtime progress
to [camera-transport](../../../crates/camera-transport/src/lib.rs); motion requests come from
[camera-control](../../../crates/camera-control/src/lib.rs). Hardware-specific optics, HDAL and boot
policy stay in this application.

One Rust daemon, `cam`, replaces the Reolink media and control processes. The stock kernel, the Novatek kernel modules,
`libhdal.so`, the root filesystem and `/mnt/app` stay as they are, both because the daemon uses them and so the
vendor stack can still boot.

```text
 U-Boot → kernel → rcS → S00_PreReady (feeds the watchdog) … S99 → /etc/init.d/start_app
                                                                     │
                     stock init (modules, sensor, lens driver) ◄─────┤
                                                                     │ flag + attempt reserved?
                              no ─► stock vendor block (router, device, …) as before
                              yes ─► cam-launch --serve ─► cam serve --attempt TOKEN
                                                             │
          ┌──────────────┬────────────────┬──────────────────┼───────────────┬────────────┐
       watchdog       network          media (main)        rtsp           mqtt ─► mqtt-hardware
      /dev/watchdog  wpa_supplicant   HDAL graph,         :554, 8 clients   broker   PTZ, lens, IR, ISP
                     udhcpc           audio in/out                          telemetry
```

## Crates

The application and hardware crates below consume the shared `camera-control` and `camera-transport`
crates described in the [workspace guide](../../../README.md#source-layout).

| Crate | What it is | Why it is separate |
|---|---|---|
| `hdal-sys` | maintained HDAL 2.00 interface declarations with frozen ARM layout assertions | all `unsafe` FFI in one place, and the layout checks ([hdal-abi.md](hdal-abi.md)) |
| `hdal` | a safe wrapper: the video graph, encoders, audio, common memory, all behind `Backend` traits | its `fake` feature is a host backend, so the daemon's media logic is tested without a camera |
| `cam` | the daemon and its command line (`cam serve`, `cam ptz`, …), plus the launcher `cam-launch` (`image/cam-launch.rs`) | model boot, ownership, network, MQTT and hardware policy; RTSP uses the shared transport |

Every hardware interaction in `cam` is a trait with a host fake as well (`ptz::Bus`, `zoom::Isp`, `ir::Hw`,
`net::Host`, `wdt::WatchdogDevice`, `telemetry::Wifi` and the shared `telemetry::Source`, `settings::Disk`). Keep system calls at these
adapter boundaries; see the [coding standards](../../../../CODING_STANDARDS.md).

## Boot

Three files are added to the stock image ([../image/README.md](../image/README.md)): `start_app`, `cam-launch` and
`cam`. The choice between `cam` and the vendor stack is made once per boot and never revised in that boot.

1. **`start_app`** creates the directory `/mnt/tmp/cam-boot-selected` (if it already exists, this boot was already
   decided and nothing more starts). It runs the byte-identical stock initialisation, then, if
   `/mnt/para/cam.enabled` exists and `cam-launch` is executable, continues in the background with standard streams
   on `/dev/null`, so the root-SSH hook that runs after it is never blocked. Otherwise it runs the stock vendor
   block unchanged.
2. **`cam-launch --reserve`** may only run the read-only `cam preflight` (two-second limit) and the counter write. It
   checks the binary, flag, configuration, counter and boot identity, then durably reserves attempt `count + 1`
   in `/mnt/para/cam.crashes`. `start_app` gives it ten seconds. Any other outcome, a refusal, a crash or the
   deadline, runs the stock vendor block instead. Nothing has touched hardware yet, so this is always safe.
3. **`cam-launch --serve`** runs only after a successful reservation, and from here on the only fallback is a
   reboot. It first starts an independent guard process (one-byte pulses over a private socket; five seconds
   without a pulse, or EOF, requests a reset) and marks `/mnt/tmp/cam-guard-up`; `start_app` resets the camera if
   that marker is missing after five seconds. It then starts `cam serve --attempt <boot id>-<count>`.
4. **Health watch.** The daemon writes a small health record to `/mnt/tmp`. The launcher requires advancing records
   within 15 s, never staler than 3 s, and 180 s of continuous health within 300 s. A qualified attempt resets the
   counter to `1`. Any failure after the start requests a reboot (`/sbin/reboot`, then `reboot -f` five seconds
   later); the next clean boot reserves the next attempt, and three counted attempts select the vendor stack.
5. **Watchdog handoff.** The stock `S00_PreReady` feeds `/dev/watchdog` from early boot. The daemon alone creates
   `/mnt/tmp/wdt_flag`, waits up to 10 s for S00 to release the device, sets a 60 s timeout and feeds it
   ([threads](#threads-and-the-watchdog)). The launcher never opens the watchdog.

The daemon also writes a one-byte boot latch into `/mnt/tmp/cam-runtime.lock` before the handoff; diagnostic
commands refuse to run after it, even once the daemon has exited. The fallbacks this gives you are in
[recovery.md](recovery.md).

## Threads and the watchdog

`cam serve --attempt` is one owner of everything, `cam::runtime::production`:

| Thread | Owns | Progress budget |
|---|---|---|
| `watchdog` | `/dev/watchdog` after the S00 handoff, and the health record | feeds only while every budget below is fresh |
| main | HDAL: the capture → videoproc → encoders graph, audio capture and output, common memory | main and sub encoder: a frame every 5 s |
| `network` | GPIO 133, the Wi-Fi modules, `wpa_supplicant`, `udhcpc` ([hardware/network.md](hardware/network.md)) | a completed tick every 30 s |
| `rtsp` | the listener on 554 and one thread per connection | 5 s |
| `mqtt` | the broker connection, discovery, commands, settings persistence | 10 s |
| `mqtt-hardware` | I²C (pan/tilt, IR-cut), `/dev/nvt_isp` (lens, ISP items), GPIOs, the light ADC | 60 s |
| `telemetry` | `/proc` and `iw` readings every 30 s, published through the shared change filter | through `mqtt` |
| `log-drain`, `log-write` | stdout/stderr into `/mnt/tmp/cam-serve.log`, 256 KiB cap | none |

Startup (graph, motor homing, services) has a 90 s grace. A worker that exits, unwinds or stops reporting progress
stops the feeding for good, and the armed watchdog resets the camera. Nothing ever disarms the watchdog, so even an
orderly exit ends in a reset. A service failure keeps the network up until then, so SSH stays reachable.

Diagnostic commands (`cam ptz`, `cam zoom`, `cam ir`, `cam serve --diagnostic`, …) share `/mnt/tmp/cam-runtime.lock`
with each other and are refused while production holds it exclusively; each also takes the one resource it needs
(media or controls). They exist for development with the vendor stack stopped ([development.md](development.md)).

## Media graph

The OS05A10 runs at its full 2592×1944, 30 fps, over MIPI into `hd_videocap` (with AE, AWB and AF statistics on).
`hd_videoproc` scales the whole frame (never a crop) to two outputs, each bound to an H.264 encoder:

| Stream | RTSP path | Default | Range |
|---|---|---|---|
| main | `main` | 1920×1440, 15 fps, 1024 kbit/s, 1 s GOP | 2592×1944, 1920×1440 or 1280×960; 5–30 fps; 256–8192 kbit/s |
| sub | `sub` | 640×480, 10 fps, 512 kbit/s, 1 s GOP | fixed |

The main stream's parameters are settings ([settings.md](settings.md)); a change restarts only the main path, and a
change that starves either stream is rolled back after 2 s. Audio capture pulls raw 48 kHz mono PCM from the
capture unit and encodes it to Opus in software (libopus), and the speaker output takes 48 kHz signed 16-bit samples
mixed from the backchannels ([hardware/audio.md](hardware/audio.md)). All of it shares one HDAL session and the
`nvtmpp` memory pool ([hdal-abi.md](hdal-abi.md#memory)). The main thread loop waits up to 10 ms for video, polls
every pending capture frame and every due mixed speaker block per turn.

## RTSP

`cam::rtsp` is a small RTSP 1.0 server: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN and GET_PARAMETER as keep-alive,
digest authentication with the single `user:password` in `/mnt/para/cam-rtsp.auth`, RTP interleaved on the TCP
connection or over UDP to the client's ports, at most eight connections and 60 s session timeout. Video limits:
7 MiB access units, 8 MiB shared retained video, 32 queued frames.

Each path offers H.264 video (track 0) and Opus audio (track 1, RFC 7587: PT 111, `opus/48000/2`,
`sprop-stereo=0`, 48 kHz mono, 64 kbit/s, one 20 ms frame per packet). A client that sends `Require:
www.onvif.org/ver20/backchannel`, as go2rtc does for two-way audio, also gets a `sendonly` Opus track 2 to talk on
(TCP interleaved only). Up to four clients may talk at once; a fifth SETUP of track 2 gets 453. Each talker has its
own decoder (at the speaker rate, 48 kHz here) and gain; a mixer sums the talkers on its own 20 ms output clock,
joins a talker after 100 ms of buffered audio (on the tested cameras Wi-Fi delivery stalled for more than 40 ms now
and then), caps each
talker at 200 ms, starts each burst 40 ms ahead so the speaker hardware keeps a lead, and plays out what a talker
already sent after it disconnects; that session keeps its talker slot until then, at most 200 ms
(`camera_transport::rtsp::talk`). Each finished session logs one line:
`talk: session ended received_ms=… played_ms=… underruns=… rebuffers=… overflow_ms=…`.

A phone or browser microphone reaches the backchannel far below the level the speaker needs to fill a room, while
the speaker hardware already runs at the vendor's full volume. Each talker's decoded packets therefore pass an
automatic gain (`camera_transport::rtsp::agc`): it follows the smoothed speech level towards -12 dBFS, boosts at
most +24 dB, starts a session at +12 dB, rises about 6 dB per second, falls about 1.4 dB per packet for loud input
and holds during pauses below -50 dBFS. One instant-attack limiter with 50 ms release after the mix keeps every
sample under -1 dBFS.

## MQTT

One client, the shared MQTT layer of all cameras (`camera_transport::mqtt`), to the broker in `cam.toml`, Home Assistant discovery for every control, setting and
diagnostic ([home-assistant.md](home-assistant.md)). Commands are validated on the MQTT thread and executed one at
a time by the `mqtt-hardware` thread, which is the only owner of the motors, lens, IR and ISP items; a stop request
from a second process reaches the same owner through `/mnt/tmp/cam-controls.stop`. Settings are applied by their
owner (media thread or hardware thread) and saved to `/mnt/para/cam-settings.toml` within 10 s by the shared settings
store (`camera_transport::settings::Values`, TOML codec). The learned focus
map ([hardware/zoom-focus.md](hardware/zoom-focus.md#autofocus-in-the-daemon)) is saved the same way to
`/mnt/para/cam-focus-map.toml`, within 60 s. A preset saved from Home Assistant is written at once to
`/mnt/para/cam-presets.toml`; the hardware thread owns the preset table (`cam.toml`'s presets with those saves
applied) and resolves every preset command against it.

## What the daemon talks to

It listens on RTSP 554 and, unless `[onvif] port = 0`, on ONVIF (HTTP and WS-Discovery); it connects to the
configured MQTT broker and, when `cam.toml` names servers, to NTP. `udhcpc` asks for a lease and `wpa_supplicant`
joins the access point. There is no cloud, P2P, push or vendor endpoint, and no web or Reolink app interface;
ONVIF is the shared Profile S service ([ONVIF](../../../../docs/shared/onvif.md)). Root SSH comes from the
image's root-SSH hook, not from `cam`.

## Hardware notes

What each register and item means, with its evidence: [hardware/README.md](hardware/README.md) ([PTZ](hardware/ptz.md),
[zoom and focus](hardware/zoom-focus.md), [tap-to-centre](hardware/position3d.md), [day/night](hardware/ir.md),
[picture](hardware/isp-iq.md), [audio](hardware/audio.md), [network](hardware/network.md)).

The shared [motion interface](motion-control.md) owns wire parsing and capability validation in
`cam::motion_control`. Pan/tilt holds retain separate axis trajectories; the device adapter combines
them with lens motion without serializing independent motors.
