# Camera firmware

Replacement userspace for camera models. Each camera runs its own small native daemon on top of the
vendor kernel and drivers; all of them share the RTSP/RTP implementation in `crates/`. What each build
needs, and how it is installed and recovered, is in that camera's guide.

| Camera | Guide |
|---|---|
| Reolink E1 Zoom | [cameras/reolink-e1-zoom](cameras/reolink-e1-zoom/README.md) |
| Xiaomi Mi 360 1080p | [cameras/mi360](cameras/mi360/README.md) |
| Hikvision DS-KD8003-IME1 | [cameras/ds-kd8003-ime1](cameras/ds-kd8003-ime1/README.md) |

Start with your model's [root-access guide](../docs/devices/README.md). Keep original firmware and
recovery backups private.

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

What a camera build or image needs beyond the pinned toolchains is in its guide. A successful host gate
or cross-build does not establish new hardware acceptance.

## Binary releases

[Build, download and use a binary/builder bundle](RELEASES.md). Model tags produce GitHub releases;
manual workflow runs produce build previews. Private images are assembled locally.

## Source layout

- `crates/camera-control`: versioned motion and image-position request parsers; no hardware or clock.
- `crates/camera-transport`: H.264 framing, RTSP authentication/RTP, Opus microphone framing and RTP,
  the talkback mixer and MQTT adapters. Video and connection limits are supplied per camera instance.
- `crates/camera-opus`: libopus 1.5.2 (`opus.json`, fetched by `scripts/fetch-opus.py`), built static with
  the pinned Zig for each camera; the encoder and decoder behind the transport's codec traits.
- `cameras/<model>`: one directory per camera with its daemon, hardware adapters and tools; its README
  and `docs/` describe the platform, limits and procedures.

## Contributing

Follow the [coding standards](../CODING_STANDARDS.md). Run `make fast` for policy changes and `make gate` before merging. Shared-source changes must pass
every camera's tests. Add isolated boundary/fault tests for ownership, cancellation and persistence
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
