# Build-only library interfaces

These JSON manifests describe the link interfaces of the hash-pinned camera libraries. They contain
symbol metadata, library identities, versions and architecture attributes, with no implementations.
The Mi360 (MIPS) and DS-KD8003-IME1 (ARM) manifests describe stock uClibc 0.9.33.2 and its matching
runtime; Reolink describes HDAL 2.00.000.

`scripts/link_stubs.py` uses only Python's standard library to generate deterministic, non-installable
ELF32 interfaces. They have no program headers or executable payload. Native builds link through these
files and load the real libraries already on the supported camera.

Keep function/object/TLS types, sizes, binding/visibility, symbol versions and MIPS floating-point ABI
attributes intact. Reolink's undefined `ioctl` import is needed to preserve executable interposition.
A new runtime/library ABI needs fresh inspection and native/device qualification. This metadata does
not describe C function signatures or establish behavioral compatibility by itself.

Generated files stay under ignored build directories. Release packaging uses an explicit runtime-file
allowlist and rejects ELF inputs without loadable program headers. Never install a generated interface.
