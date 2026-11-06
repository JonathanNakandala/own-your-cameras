# Binding a library without its headers

The camera's media stack is Novatek HDAL: `libhdal.so` in user space over the `kflow_*`/`nvt_*` kernel modules.
The camera runs HDAL **2.00.000** (`/usr/lib/libhdal.so`, the hash recorded in `firmware/abi/reolink-e1-zoom/hdal.json`). Its headers
were never published. The closest public headers are HDAL **3.02.000** for the NT9833x
([../hdal-sys/HEADERS.md](../hdal-sys/HEADERS.md) has the source and pin). This page says how `hdal-sys` is made
to match the 2.00 library anyway, what is proven and what is not.

## The method

1. **Maintain the qualified interface.** `hdal-sys/src/abi.rs` holds the Rust declarations;
   `src/arm_layout.rs` retains the original ARM size/alignment/offset literals. ARM builds compile
   every retained assertion; host pointers retain host width while C scalar widths stay pinned.
2. **Check declaration contracts.** `hdal-sys/tests/contracts.rs` checks scalar widths, memory function
   signatures and layout-assertion coverage. `tests/foundations.rs` checks constants and type ranges.
   These checks detect interface drift; they do not prove compatibility with another SDK binary.
3. **Read what the library really copies.** Each HDAL get/set takes a parameter id and a pointer. Inside
   `libhdal.so` the id selects a case through an ARM jump table, and the case `memset`s and `memcpy`s a fixed
   number of bytes between the caller's buffer and the ioctl envelope. That byte count, read from the
   disassembly, is the struct size the 2.00 library expects. Where it differs from the 3.02 header, the header is
   patched (below).
4. **Pin the parameters actually used.** `hdal-sys/abi/libhdal-2.00-param-sizes.tsv` lists the (unit, op, id)
   rows the daemon calls with their byte counts; `hdal-sys/tests/abi.rs` fails if a header size drifts from it and
   checks that every startup parameter has a row.
5. **Exercise each call on the camera** with the vendor stack stopped, and reboot afterwards.

The tools for step 3 are in `hdal-sys/abi/` ([../hdal-sys/abi/README.md](../hdal-sys/abi/README.md)): they draft an
inventory of jump-table cases and copy lengths. Their output is a list of candidates for a human reading, never a
certificate: a jump-table index is not proven to be the parameter id, copy lengths may belong to stack temporaries,
and a missing observation means unknown. The table's rows marked `legacy-*` were chosen with the expected header
size in hand, so they are leads, not independent proof; `retracted-word` marks a row that was wrong.

## Header changes

The maintained declarations inherit the following corrections from the qualified reference translation.

Two structs, where the 2.00 library keeps the codec configs in a union:

| Struct | 3.02 header | 2.00 library | Evidence |
|---|---|---|---|
| `HD_VIDEOENC_OUT` (`OUT_ENC_PARAM`) | 52 bytes | 44 | `hd_videoenc_get` case `0x2ce38`: `memset(p, 0, 44)`, reads the codec word, then `memcpy(p+4, tmp+4, codec == 0 ? 8 : 40)`; `hd_videoenc_set` case `0x2df18`: `memset 44`. h26x (40 B) and jpeg (8 B) share offset 4. |
| `HD_VIDEOENC_OUT2` (`OUT_ENC_PARAM2`) | 64 | 44 | get case `0x2c638`, set case `0x2d97c`: `memcpy 44` (4 + union of h26x 40 and jpeg2 20). |

Field names are unchanged, so `out.h26x.…` still compiles; only the jpeg view now overlays the h26x one.

Four source defects in the SDK, not new functionality:

| Definition | Correction |
|---|---|
| `HD_AUDIODEC_0_IN_20` | port 20 instead of a duplicate of port 10 |
| `HD_VIDEO_PXLFMT_TYPE` | balanced parentheses |
| `HD_VIDEO_PXLFMT_MAKE_NRX` | cast class 15 to `UINT32` before shifting by 28 (no signed overflow) |
| `VENDOR_VIDEO_PXLFMT_YUV420_MB3` | refers to the declared `HD_VIDEO_PXLFMT_YUV420_MB3`, not an undefined `CIF_` name |

## Differences that are not layouts

- Ids the 2.00 library has no case for (its dispatch tables end below them): videocap `OUT_MASK_ATTR`,
  `IN_PALETTE_TABLE`, `DRV_CONFIG3` (13..15); videoproc `IN_PALETTE_TABLE`, `IN3`, `STATUS` (26..28); videoenc
  `OUT_TRIG_SNAPSHOT3`, `OUT_VIDEO_PACK_NUM` (29, 30). videocap get of `OUT_NOTIFY_HANDLER` (9) and
  `OUT_MD_STATUS` (10) and videoproc get/set of `OUT_CROP_PSR` (12) land on the error stub. Do not call them.
- videoenc `RATE_CONTROL` (id 8) is 64 bytes as in the header: the 80-byte `memset` in `hd_videoenc_get` is a
  temporary for `RATE_CONTROL2`.
- `hd_common_mem_cache_sync` is declared but not exported; `hd_common_dmacopy` always returns −1. No `vendor_*`
  function is exported, and `wrapper.h` does not generate them.

## Memory

The kernel reserves about 47.5 MiB at `0x05080000` as the `nvtmpp` media pool (of 80 MiB DDR; Linux sees
76.6 MB). The daemon asks for four blocks of one full 2592×1944 YUV420 frame each (`cam::pipeline::PipelineConfig::e1_zoom`);
the encoder rings come from the same pool. The main encoder's ring is sized at startup for 2592×1944 at
8.192 Mbit/s (about 12 MiB) so a later resolution or bitrate raise never re-opens it. Each ring is capped at
16 MiB and both at 20 MiB; rings are mapped, never copied, and every copied access unit is bounded by
`MAX_AU_BYTES` (7 MiB).

From the library's disassembly (SHA-256 in the HDAL link manifest):

- `hd_common_mem_mmap` (`0x92b8`) rejects zero size and an unaligned physical address and turns mmap's
  all-ones failure into **null** (`0x9340` → `0x937c`). The wrapper still rejects both null and all-ones and
  checks the address extent.
- `hd_common_mem_alloc` (`0x9904`): nonzero means failure, and early errors need not write the outputs. On
  failure it releases the pool and clears the outputs (`0x9ad8..0x9b00`); on success it publishes nonzero
  addresses only after mmap succeeded (`0x9a38..0x9a80`). Callers initialise the outputs, check the status first,
  then validate both extents. Null or all-ones is never passed to `mem_free`, and an impossible success with
  invalid outputs is an error.

Rules the `hdal` adapters keep:

- Encoder paths validate nonempty, non-wrapping 32-bit physical and host extents. Video rejects zero or excessive
  pack counts and validates **all** packs before any cache operation or slice. Every successful pull is released,
  also when validation or cache maintenance fails; frames are copied before the release and a failure never
  returns a partial frame.
- One common HDAL session per process. Every hardware call requires its successful owner; a failed cleanup or
  release makes the session terminal (remaining cleanup is still attempted). Mapped encoders are closed before
  reconfiguration. A new process does not replace the reboot a failed session needs.
- The speaker staging buffer is reusable after a push because the kernel copies it synchronously
  ([hardware/audio.md](hardware/audio.md#speaker)).

These checks assume the pinned library and kernel honour their own success contracts; they cannot prove driver
correctness or DMA coherency on failing hardware.

## ISP payloads

The lens, IR and picture controls bypass HDAL and use `/dev/nvt_isp` ioctls directly. The ioctl number's size
field is the pointer size (4), not the payload, so the kernel copies **its own** item size into a 4 KiB buffer.
The safe entry points therefore allow only these items, with the size read from each module's size table:

| Type / item | Direction | Kernel bytes | Source |
|---|---|---|---|
| common 30 (VA) | get | 2052 | `nvt_isp.ko` `ispt_api_get_item_size` (`0xc870`) reads `.data + 0xf14 + item*8 + 4` |
| common 41/42 (motors), incl. 42 cmds 2/6/7 | get/set | 20 | same |
| common 43 (motor misc) | get/set | 132 | same |
| common 44 (mirror/flip) | get/set | 12 | `nvt_isp.ko` table at `0x12128 + 44*8` |
| IQ 10–15, 20 (picture levels, day/night) | get/set | 8 | `nvt_iq.ko` `iqt_api_get_item_size` (`0x1beac`), `.data + 0x5a24 + item*8 + 4` |
| AE 12, 14 (anti-flicker, EV) | get/set | 8 | `nvt_ae.ko` `aet_api_get_item_size`, `.data + 0x2de8 + item*8 + 4` |

Module hashes: `nvt_isp.ko` `4f9ee1200cde7c65cca0cda87eda6e07629f9170871b7175438572d80681fcd8`, `nvt_iq.ko`
`c435ceda37c335386eb2168498263175c36d6d77066359592c9d62adf267d25e`. Arbitrary item numbers (including high-bit
aliases) and unknown item/direction pairs are refused; a new item needs its own size evidence. The IQ and AE set
paths return 0 even when they ignore a value, so every set is read back ([hardware/isp-iq.md](hardware/isp-iq.md)).

## Descriptors

I²C and ISP share `hardware_fd::open_rw` (atomic `O_RDWR | O_CLOEXEC`). Rust `File` opens and the networking and
process descriptors are close-on-exec already; the logging pipe uses `pipe2(O_CLOEXEC)`. Only the intentional
stdout/stderr `dup2` targets are inherited.

## Seeing kernel errors: the ioctl capture

HDAL reports most kernel failures as one generic error. `hdal::sys::ioctl_capture` exports an `ioctl` symbol on the
ARM build that interposes the process's own calls: it forwards the three ABI words unchanged to the syscall and,
only inside a capture scope on the calling thread, records the first failing command, its errno and item. A
failure HDAL already reports keeps its meaning; an unreported one becomes `-65536 - errno`. Other threads pass
straight through.

`hdal/examples/ioctl-capture-replay.rs` and `hdal-sys/abi/audio-config-fault.c` are RAM-only experiments that
replace one payload pointer and call the real kernel, to see the failure path end to end (EFAULT with no result
write). Run them only with the vendor stack stopped and reboot afterwards; they are never part of the product.

## Not proven

- Rows with a struct in the header but no byte count tied to it by a reading: videoproc get/set `FUNC_CONFIG`,
  `LL_CONFIG` and set `OUT_CROP`; videoenc get/set `SYSCAPS`, set `RATE_CONTROL`; audioout get `OUT`/`IN`; the
  stamp, mask, mosaic and pattern ids (`_hd_osg_*`); videocap's ctrl-path `PATH_CONFIG`. Code that uses them
  re-checks on the camera: `hd_videocap_get(SYSINFO)` must report 2592×1944 at 30 fps.
- The audio startup rows (capture `DEV_CONFIG` 172 bytes, `DRV_CONFIG` 148 bytes and the ctrl-path `BUFINFO` get,
  id 10) are in the table and were exercised on the camera, but their directions, offsets and extents have no
  independent reading. The audio encoder unit is no longer used.
- No allocation or cache failure was injected on the camera.
