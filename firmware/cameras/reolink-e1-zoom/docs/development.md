# Development

[Build instructions](build.md), host tests and trying code on a camera. Code rules:
[coding standards](../../../../CODING_STANDARDS.md).

## Make targets

| Target | What it does | Needs a camera |
|---|---|---|
| `make tools` | installs a pinned `cargo-zigbuild` into the workspace `firmware/.tools` (nothing global) | no |
| `make check` | the Python script tests, `scripts/check-tree` (no vendor bytes are tracked), `scripts/check-links` (relative links resolve), `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` | no |
| `make build` | cross build with `cargo zigbuild` (glibc 2.30, `-C target-cpu=cortex-a9`), then `make udiv-guard` | no |
| `make udiv-guard` | fails if `cam` or `cam-launch` contains `udiv`/`sdiv`, which SIGILL on this Cortex-A9 | no |

Host tests never touch a camera, a broker or host hardware: every hardware interaction has a fake. `make check`
also runs the boot-launcher matrix ([../image/README.md](../image/README.md#tests)). With your stock PAK at hand,
`STOCK_PAK=STOCK_PAK make test-scripts` adds the real-PAK checks of the image and `stock.spub` builders.

## Trying code on a camera

These targets copy the binary to `/mnt/tmp` (RAM) and never install anything:

| Target | What it shows |
|---|---|
| `make device-test CAMERA=<ssh host>` | `cam self-test` next to the running vendor stack: the binary runs on the Cortex-A9 and `libhdal.so` loads. Expect `hd_gettime_ms=0` and `hd_common not init` (it deliberately makes no HDAL init call). |
| `make device-probe CAMERA=…` | HDAL init and the sensor: `nvt_sen_os05a10`, 2592×1944, at least 25 fps |
| `make device-dump CAMERA=…` | both encoders for `DUMP_SECONDS`, decoded with `ffprobe` into `target/dump/` |
| `make device-serve CAMERA=… AUTH=FILE` | `cam serve --diagnostic` on port 8554 for `SERVE_SECONDS`; `AUTH` is a local `user:password` file |
| `make device-zoom CAMERA=… AUTH=FILE` | lens init, both zoom ends, autofocus, frames into `target/zoom/` |
| `make device-ir CAMERA=… AUTH=FILE` | day, night, day, the status LED and the auto switch, frames into `target/ir/` |

Every target except `device-test` runs inside `scripts/device-session`, because **HDAL must never run beside the
vendor `device`**: it refuses if a `cam` already runs, stops the vendor daemons, holds the watchdog with `cam wdt`
(the vendor `router` fed it), runs the commands and then **always reboots** the camera and waits for the vendor stack.
A RAM session leaves nothing behind. Never run the production `cam serve --attempt` beside `cam wdt`.

## The command line

```
cam self-test | probe-videocap | dump [--seconds N] [--out DIR]
cam serve --attempt TOKEN | cam serve --diagnostic [--port N] [--auth FILE | --no-auth] [--seconds N] [--config FILE] [--retune SECONDS:WxH@FPS@KBPS[@gdr][@slices]]...
cam preflight
cam health-watch --attempt TOKEN --pid PID [--dir DIR]
cam ptz [--dev PATH] [--cfg PATH] home | move <pan> <tilt> | stop | preset <n>
cam zoom [--dev PATH] [--sp PATH] <pos> | info | init
cam focus [--dev PATH] [--sp PATH] [<pos>]
cam ir day | night | status | auto [--to-night N] [--to-day N] [--seconds N]
cam led on | off
cam wdt
cam net [--check | --seed-vendor]
```

`--retune 60:2592x1944@10@4096` (add `@gdr` for gradual decoder refresh, `@slices` for slice split) makes a diagnostic run ask for that main
stream 60 s after it starts (repeatable),
a way to try live changes on a camera without a broker. The diagnostic run reads `/mnt/tmp/cam-settings.toml`, so a
RAM-only experiment sets `isp_3dnr = true` and the main size there; its trial marker is `/mnt/tmp/cam-3dnr.trial`.

`cam serve --attempt` is the production daemon and is only ever started by `cam-launch`. `cam serve --diagnostic`
is the same pipeline without the watchdog handoff, network or health file; its config is optional and its
settings live in `/mnt/tmp/cam-settings.toml`. The control commands (`ptz`, `zoom`, `focus`, `ir`, `led`) need
the vendor stack stopped, because it owns the same chips; zoom, focus and IR also need frames flowing
(`cam serve --diagnostic` in the background). `cam ptz stop` reaches a running daemon's hardware owner instead.
Hardware details per command: [hardware/](hardware/README.md).
