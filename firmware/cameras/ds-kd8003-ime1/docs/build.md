# Build the DS-KD8003-IME1 application

Complete the [host setup](../../../README.md#host-development). Run from the repository root.

## Application

```sh
make -C firmware build-ds-kd8003-ime1
```

The binary links against link-only interfaces of the stock uClibc 0.9.33.2, generated from
[`firmware/abi/ds-kd8003-ime1`](../../../abi/ds-kd8003-ime1) by `scripts/link_stubs.py`; no files
from a device are needed, and the real libraries load on the panel. The build needs Rust 1.95.0 with
`rust-src` (the standard library is built for the tier-3 `armv5te-unknown-linux-uclibceabi` target),
`clang` and `ld.lld`. Output in `firmware/cameras/ds-kd8003-ime1/dist/`:

| File | Role |
|---|---|
| `ds-kd8003-ime1-cam` | the application |
| `libloader_compat.so` | loaded before libc: VB pool-info ioctl translation and the `pthreadSpawn` helpers the vendor ISP library imports |
| `build.json` | sizes and SHA-256 sums |

Two things make a plain Rust binary run on this loader: `toolchain/start.S` and `toolchain/compat.c`
provide the startup, `getauxval` and `pthread_setname_np` that uClibc 0.9.33 lacks, and
`toolchain/elf_tls.py` moves the PT_TLS program header last, which the old loader requires.

## Maintenance SSH

The vendor SSH daemon accepts the root password and drops into a restricted shell. Build the
key-only replacement from the Dropbear 2026.94 release tarball with the pinned Zig:

```sh
python3 tools/ds-kd8003-ime1/build_dropbear.py dropbear-2026.94.tar.bz2 --output-dir /path/to/new-dir
```

Output: `dropbearmulti-arm926` (static musl, Ed25519 only, no passwords, no forwarding;
`sync_atomics.c` supplies the ARMv5 atomic helpers through the kernel's `__kuser_cmpxchg`).

## Host checks

`make -C firmware gate` runs the host tests of the pure modules (`config`, `pin`, `access`,
`discovery`, `mqtt`, `keypad_frames`, `crates/keypad-protocol`) and `toolchain/test_elf_tls.py`.
