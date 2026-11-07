# Architecture

One process, `ds-kd8003-ime1-cam`, replaces the vendor application at run time. The boot supervisor
`firmware/start.sh` starts it after the vendor application, the application stops the vendor
processes and owns the camera, microphone, speaker, strike relay, inputs, call-button light and
keypad.

## Layers

| Layer | Where | Rule |
|---|---|---|
| Domain | `src/access.rs`, `pin.rs`, `config.rs`, `discovery.rs`, `keypad_frames.rs`, `mqtt/` | pure, host-tested; time and hardware state are passed in, decisions come back as values |
| Shared transport | `../../crates/camera-transport`, `camera-opus`, `keypad-protocol` | RTSP/RTP, PCMU and Opus framing, the talkback mixer, keypad bus envelopes (AES-ECB + CRC) and the staged key handoff |
| Adapters | `src/runtime/` (ARM only) | vendor stop, ioctls, the media bridge FFI, the keypad UART session, MQTT thread, speaker and capture loops, diagnostics HTTP |
| C bridge | `sdk/media_bridge.c`, `sdk/hisi_v3_types.h` | the HiSilicon MPP V1 pipeline over the vendor libraries loaded at run time; structure layouts from OpenIPC Divinus (MIT) |
| Loader shims | `toolchain/` | startup and symbols uClibc 0.9.33 lacks, the PT_TLS reorder, `libloader_compat.so` |
| Supervisor | `firmware/start.sh` | unpack, boot counter and fallback, network address keeper, reboot on exit |

`src/runtime/mod.rs` is the wiring: it reads devices, hands snapshots to the domain modules and acts
on what they return. The access rule is an example: the keypad thread reports buffer snapshots,
`access::Access` decides `Grant`, `Deny` or nothing from the snapshot, the clock and the strike
state, and the main loop pulses the relay, plays the clip and publishes the event only after the
relay confirmed the pulse; a pulse that failed plays the refusal instead.

## Threads

Every thread is named, so `/proc/<pid>/task/*/comm` shows what uses the CPU: `wdt` (feeds the
watchdog while the main loop beats), `video0`, `video1`, `mic`, `speaker`, `rtsp`, `button`,
`keypad`, `mqtt`, `http` (when enabled), `signals` and `beat` (only while media starts). The main loop runs every 50 ms: strike
pulse end and release retries, inputs, call button, remote commands, keypad decisions, day/night
and the boot-health mark.

## Safety rules

- The strike pulse is bounded by `open_seconds` and never exceeds 120 s; a failed release is retried
  every pass, and twenty consecutive failures (one second) make the process exit so the watchdog resets the board.
- A video or audio lease that became uncertain stops media; the process exits for a reboot once the
  strike is off.
- PIN digits stay in zeroized buffers and never reach a log, `Debug` output or MQTT.
