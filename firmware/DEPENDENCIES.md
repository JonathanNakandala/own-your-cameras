# Dependency review

Release recipes use the workspace `Cargo.lock`. Dependency updates must pass host tests and the
platform's native build, image-size and device acceptance checks; the MIPS/uClibc target has a custom
startup/linker boundary.

libopus is the upstream 1.5.2 release tarball, pinned by SHA-256 in `opus.json` and unpacked by
`scripts/fetch-opus.py` into the ignored `.tools/` directory (BSD-3-Clause). `crates/camera-opus` compiles
it with the pinned Zig as a float build: NEON intrinsics for the Reolink Cortex-A9, plain MIPS32r2 for the
Mi360, native for host tests. No DNN/DRED code is built.

Tokio is locked to patched 1.51.6 for
[RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html). Its mio dependency requires
libc bindings at least 0.2.183; the workspace uses 0.2.188, for the shared transport and Mi360 application. The later pin includes the upstream fix for
[uClibc constants](https://github.com/rust-lang/libc/issues/5140) required by socket2.

OSV also reports block-buffer 0.10.4 under
[GHSA-qwgh-2vcv-g2f7](https://github.com/advisories/GHSA-qwgh-2vcv-g2f7). The described inline-cursor
failure differs from 0.10.4's separate position field. The applications do not catch and reuse a
panicking digest, and release builds abort on panic. This is a documented applicability assessment,
not a claim that the database finding has been fixed upstream. Recheck the exact implementation and
call path when changing SHA-256 dependencies or panic policy.
