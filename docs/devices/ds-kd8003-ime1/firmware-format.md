# DS-KD8003-IME1 firmware format and evidence

[DS-KD8003-IME1 overview](README.md)

Reference for **DS-KD8003-IME1 non-B, V2.2.62 build 230204**.
Start with the [build and hook instructions](firmware.md).

## Image assembly


The [builder](../../../tools/ds-kd8003-ime1/build.py) performs these steps:

1. Require the pinned stock hash and decode the XOR-obfuscated SWKH header.
   Validate the header and payload byte sums, payload MD5, compatibility-tail
   shape, CramFS header and filesystem CRC. Repack stock and require byte equality.
2. Read encrypted `start.sh` and `digicapkeyArm.ko` from stock CramFS. For this
   exact module, the first 24 bytes of ELF `.rodata` supply the 3DES key. The key
   stays in memory; no key value is embedded in the public source or report.
3. Decrypt the script using 3DES ECB. Its format is zero-padded ciphertext followed
   by one clear byte recording padding length. Require stock decrypt/re-encrypt
   to reproduce the original bytes.
4. Append the hook and re-encrypt. Split the replacement into 4096-byte chunks,
   zlib-compress each, and append a new CramFS block-pointer table and data.
   Redirect only the script inode's size and data offset. Preserve its mode,
   owner and group, and every other member's data offsets. Old script data remains
   unreachable slack; this is not a complete filesystem repack.
5. Align the filesystem to 4096 bytes, update its size and block count, then
   recompute its CRC with the CRC field zeroed. Enforce the `0xe20000`-byte
   (14.125 MiB) firmware partition capacity.
6. Update DAV payload length and byte sum, header byte sum, and payload MD5.
   Preserve model/language/OEM/version/compatibility fields and the device-type
   list. Parse the result again and require the same file inventory with only
   `/start.sh` changed.

The SWKH header here is 108 bytes: a 64-byte base and one 44-byte file entry for
`cramfs.img`. CramFS follows it. The stock tail is 68 bytes: 16 bytes of payload
MD5, a little-endian 32-bit count, and 24 encoded 16-bit model compatibility IDs.
These are model IDs, not unit serial numbers. Header offset 32 is a compatibility
field, **not a package length**; a generic Hikvision packer using that interpretation
can corrupt this format.

The fixed header XOR mask is format obfuscation. The traced stock updater's
selected SWKH path checks sums, MD5, model compatibility and minimum version;
no private-key signature was required on that path. This finding does not apply
to all Hikvision update formats. Checksums alone do not prove that another device
will accept an image, that its boot chain accepts it, or that recovery will work.

## Verification scope

| Check | Original research image | Generic public candidate |
|---|---|---|
| Exact stock roundtrip; only `/start.sh` changed | Passed | Checked by builder |
| Script decrypt/re-encrypt; package/CRCs/capacity | Passed | Checked by builder |
| Original ARM decrypt and updater pre-write checks | Passed; ten invalid cases rejected | Harness not included |
| Independent CramFS and original ARM shell checks | Passed | Must check separately |
| Flash and boot marker after reboot | Verified | Not device-tested |
| Snapshot, RTSP and configuration after flash | Verified | Not device-tested |
| Power-loss or serial recovery | Untested | Untested |

The public hook has generic paths and a different marker. Its output is not the
original flashed image. A marker proves execution, not working audio or locks.

The research emulator stopped before target lookup and writes. It did not emulate
the HTTP server, kernel, bootloader, power loss or hardware. The original ARM
runtime checks and emulator are not distributed here; they are historical evidence,
not hidden prerequisites for running this builder.

## Runtime limits

- RAM-disk init mounts config and firmware, loads `digicapkeyArm.ko`, decrypts
  `/home/hik/start.sh` to `/tmp/start.sh` with `dec`, and executes it.
- `/etc` and `/home/app` are volatile. Config is a separate 1.5 MiB JFFS2 partition
  shared with vendor settings; firmware headroom does not enlarge it.
- The hook starts in the background. Vendor services may still be starting.
- Existing `sdbg=k` and `sdbg=d` debug exits skip the hook.
- `umask 077`, `ulimit -c 0` and `ulimit -f 128` restrict file permissions, core
  dumps and regular-file size. They provide no CPU/memory isolation, log rotation,
  restart supervision or watchdog.
- Keep `hicore` running; it owns media, access control and watchdog behavior.
