# Camera firmware

Replacement userspace for two qualified camera models. Each camera runs its own small native daemon;
both applications use the same RTSP/RTP implementation and motion command contract.
Vendor kernels, drivers and SDK implementations stay on the cameras. Native builds use local interface
declarations and generated link-only metadata; they need no vendor firmware, libraries or header downloads.

| Camera | Platform | Build and installation |
|---|---|---|
| Reolink E1 Zoom, IPC_566SD664M5MP, stock 4417 | Novatek NA51089, ARM Cortex-A9, glibc | [Reolink guide](cameras/reolink-e1-zoom/README.md) |
| Xiaomi Mi 360 1080p, MJSXJ10CM, stock 4.3.4_0376, SC2336 | Ingenic T31, MIPS32r2, uClibc | [Mi360 guide](cameras/mi360/README.md) |

Start with your model's [root-access guide](../docs/devices/README.md). Keep original firmware and
recovery backups private. The two image formats and recovery procedures are different.

## Host development

From the repository root, with Python 3.12+, a host C compiler installed:

```sh
rustup toolchain install 1.95.0 --component rustfmt --component clippy --component rust-src
python3 firmware/scripts/fetch-zig.py
python3 firmware/scripts/fetch-opus.py
export PATH="$PWD/firmware/.tools/zig:$PATH"
make gate
```

The workspace pins Rust in `rust-toolchain.toml`. The Zig and libopus installers verify release hashes and write only to `firmware/.tools/`.
On macOS, `brew install llvm` supplies the ARM disassembler; on Debian/Ubuntu, install `clang llvm`.
The gate checks local interface pins, compiles the production ABI assertions, and runs isolated
host tests. It needs no camera, broker, Home Assistant, microphone, speaker or proprietary libraries.

Native builds need only the pinned toolchains and source dependencies. Personalized images still need
your stock firmware/backup and SSH public key; follow the model guide. A successful host gate
or cross-build does not establish new hardware acceptance.

## Binary releases

[Build, download and use a binary/builder bundle](RELEASES.md). Model tags produce GitHub releases;
manual workflow runs produce build previews. Private images are assembled locally.

## Source layout

- `crates/camera-control`: versioned motion and image-position request parsers; no hardware or clock.
- `crates/camera-transport`: H.264 framing, RTSP authentication/RTP, Opus microphone framing and RTP,
  the talkback mixer and the Mi360 MQTT adapter. Video and connection limits are supplied per camera instance.
- `crates/camera-opus`: libopus 1.5.2 (`opus.json`, fetched by `scripts/fetch-opus.py`), built static with
  the pinned Zig for each camera; the encoder and decoder behind the transport's codec traits.
- `cameras/reolink-e1-zoom`: daemon, HDAL wrappers, Novatek hardware adapters, image and recovery tools.
- `cameras/mi360`: daemon, IMP C bridges, T31 hardware adapters, network recovery and NOR image tools.

The Reolink profile permits 7 MiB access units, 8 MiB shared retained video, 32 queued frames and eight
connections. Mi360 uses 1 MiB access units, 2 MiB retained video, four queued frames, six connections
and 96 KiB RTSP worker stacks. These are independent runtime policies, not Cargo feature switches.

## Contributing

Follow the [coding standards](../CODING_STANDARDS.md). Run `make fast` for policy changes and `make gate` before merging. Shared-source changes must pass
both applications' tests. Add isolated boundary/fault tests for ownership, cancellation and persistence
changes. Keep hardware calls behind their existing adapter interfaces. Do not add camera access to
host tests or treat requested settings as measured state.

A firmware release identifies the model and source revision. Validate the corresponding native build,
image preservation/size and real device behavior separately. Use model-specific release tags so a
release for one camera does not imply new qualification for another.

## License and security

Original firmware source is [MIT licensed](LICENSE). This license applies to `firmware/`, not to vendor
SDKs, owner firmware dumps or the repository's separately sourced documentation and artwork.
Dependency licenses remain their own. The protocol contract includes no proprietary firmware bytes.

Report security issues privately using GitHub's **Report a vulnerability** facility when available.
Avoid posting credentials, device identifiers, logs or recordings in public issues. See
[dependency review](DEPENDENCIES.md) for the current advisory assessment.
