#!/usr/bin/env python3
"""Offline-only builder for the pinned DS-KD8003-IME1 2.2.62 SWKH package.
Never communicates with a camera. Preserve the stock image except start.sh and
checksums/lengths necessary to reference its replacement compressed blocks.
The input is either the stock DAV or a read of the panel's firmware partition
(/dev/mtd1); the stock DAV is rebuilt from the latter and checked by hash.
"""

import argparse
import hashlib
import io
import json
import math
import struct
import zlib
from pathlib import Path

from Crypto.Cipher import DES3
from dissect.cramfs.cramfs import CramFS
from elftools.elf.elffile import ELFFile

STOCK_SHA = "f5b080741e4b1360c13d0f46abd6af1b6df7e07cbf62c6a7c6435203bb6d107d"
# Fixed SWKH header obfuscation mask, not an owner credential.
HEADER_XOR_MASK = bytes.fromhex("bacdbcfed6caddd3bab9a3abbfcbb5be")
FLASH_SIZE = 0xE20000


def xor(b):
    return bytes(v ^ HEADER_XOR_MASK[(i + (i >> 4)) & 15] for i, v in enumerate(b))


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def put(b, o, v):
    struct.pack_into("<I", b, o, v)


def sha(b):
    return hashlib.sha256(b).hexdigest()


def parse(b):
    hlen = u32(xor(b[:16]), 8)
    assert hlen == 108
    h = xor(b[:hlen])
    assert h[:4] == b"SWKH" and u32(h, 12) == 1
    assert u32(h, 4) == sum(h[12:]) & 0xFFFFFFFF
    assert h[64:96].split(b"\0")[0] == b"cramfs.img" and u32(h, 96) == 108
    n = u32(h, 100)
    c = b[108 : 108 + n]
    assert len(c) == n
    assert u32(h, 104) == sum(c) & 0xFFFFFFFF
    tail = b[108 + n :]
    assert len(tail) >= 20 and tail[:16] == hashlib.md5(c).digest()
    count = u32(tail, 16)
    assert len(tail) == 20 + count * 2
    assert n <= FLASH_SIZE
    assert u32(c, 0) == 0x28CD3D45 and u32(c, 4) == len(c) and u32(c, 8) == 3
    crc = bytearray(c)
    put(crc, 32, 0)
    assert u32(c, 32) == zlib.crc32(crc)
    return h, c, tail


def pack(h, c, tail):
    h = bytearray(h)
    put(h, 100, len(c))
    put(h, 104, sum(c) & 0xFFFFFFFF)
    put(h, 4, sum(h[12:]) & 0xFFFFFFFF)
    return xor(h) + c + hashlib.md5(c).digest() + tail[16:]


def key_from_module(module):
    with io.BytesIO(module) as f:
        return ELFFile(f).get_section_by_name(".rodata").data()[:24]


def decrypt(b, key):
    assert (len(b) - 1) % 8 == 0 and 0 <= b[-1] < 8
    plain = DES3.new(key, DES3.MODE_ECB).decrypt(b[:-1])
    return plain[: len(plain) - b[-1]] if b[-1] else plain


def encrypt(b, key):
    pad = (-len(b)) % 8
    return DES3.new(key, DES3.MODE_ECB).encrypt(b + b"\0" * pad) + bytes([pad])


def replace_file(c, path, data):
    fs = CramFS(io.BytesIO(c))
    node = fs.get(path)
    inode = node.inode
    assert inode.mode & 0xF000 == 0x8000 and len(data) < 1 << 24
    result = bytearray(c)
    off = (len(result) + 3) & ~3
    result.extend(bytes(off - len(result)))
    blocks = [data[i : i + 4096] for i in range(0, len(data), 4096)]
    result.extend(bytes(4 * len(blocks)))
    for i, block in enumerate(blocks):
        result.extend(zlib.compress(block, 9))
        put(result, off + i * 4, len(result))
    result.extend(bytes((-len(result)) % 4096))
    assert len(result) <= FLASH_SIZE
    put(
        result, node.offset + 4, (u32(result, node.offset + 4) & 0xFF000000) | len(data)
    )
    put(
        result, node.offset + 8, (u32(result, node.offset + 8) & 63) | ((off // 4) << 6)
    )
    put(result, 4, len(result))
    put(result, 40, u32(c, 40) + len(blocks) - math.ceil(inode.size / 4096))
    put(result, 32, 0)
    put(result, 32, zlib.crc32(result))
    assert CramFS(io.BytesIO(result)).get(path).open().read() == data
    return bytes(result)


def file_manifest(c):
    fs = CramFS(io.BytesIO(c))
    out = {}

    def walk(node, path):
        # Materialize names before recursion: dissect iterators share their seek handle.
        entries = []
        for item in node.iterdir():
            entries.append((item.name, item))
        for name, item in entries:
            p = path + "/" + name
            i = item.inode
            row = {"mode": i.mode, "uid": i.uid, "gid": i.gid, "size": i.size}
            if i.mode & 0xF000 == 0x4000:
                walk(item, p)
            else:
                row["sha256"] = sha(item.open().read())
            out[p] = row

    walk(fs.root, "")
    return out


def stock_input(data):
    """The stock DAV from either the DAV itself or a firmware-partition read; text a shell printed
    before the read (a login banner) is skipped."""
    start = data.find(struct.pack("<I", 0x28CD3D45), 0, 4096)
    if not data.startswith(xor(b"SWKH")) and start >= 0:
        import stock  # noqa: PLC0415 (stock imports this module)

        return stock.stock_dav(stock.cramfs_from_flash(data[start:]))
    return data


def main():
    if not __debug__:
        raise SystemExit("Run without -O: validation assertions must remain enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "stock", type=Path, help="Stock V2.2.62 build 230204 DAV, or a read of /dev/mtd1"
    )
    parser.add_argument(
        "--output", type=Path, required=True, help="New local output directory"
    )
    args = parser.parse_args()
    build_hook(stock_input(args.stock.read_bytes()), args.output)


def build_hook(b, out):
    """Write the hook candidate and its evidence into the new directory out; returns the report."""
    assert sha(b) == STOCK_SHA, "Unexpected stock DAV; inspect new builds independently"
    h, c, t = parse(b)
    # Refuse existing destinations so repeated builds cannot overwrite prior evidence.
    out.mkdir(parents=True, exist_ok=False)
    original = pack(h, c, t)
    assert original == b
    (out / "stock-roundtrip.dav").write_bytes(original)
    fs = CramFS(io.BytesIO(c))
    enc = fs.get("start.sh").open().read()
    module = fs.get("digicapkeyArm.ko").open().read()
    key = key_from_module(module)
    plain = decrypt(enc, key)
    assert encrypt(plain, key) == enc, "Stock encryption padding differs"
    hook = Path(__file__).with_name("boot-hook.sh").read_bytes()
    assert b"OWNER_CUSTOM_BOOT_V1" not in plain
    newplain = plain.rstrip(b"\n") + b"\n\n" + hook
    newenc = encrypt(newplain, key)
    assert decrypt(newenc, key) == newplain
    patched = replace_file(c, "start.sh", newenc)
    candidate = pack(h, patched, t)
    h2, c2, t2 = parse(candidate)
    before = file_manifest(c)
    after = file_manifest(c2)
    changed = [p for p in before if before[p] != after[p]]
    assert before.keys() == after.keys() and changed == ["/start.sh"], changed
    assert h[:4] == h2[:4] and h[8:100] == h2[8:100] and t[16:] == t2[16:]
    (out / "ds-kd8003-ime1-2.2.62-hook.dav").write_bytes(candidate)
    (out / "candidate.cramfs").write_bytes(patched)
    (out / "stock.cramfs").write_bytes(c)
    (out / "start-stock.sh").write_bytes(plain)
    (out / "start-candidate.sh").write_bytes(newplain)
    report = {
        "base_sha256": sha(b),
        "candidate_sha256": sha(candidate),
        "candidate_bytes": len(candidate),
        "cramfs_bytes": len(patched),
        "partition_bytes": FLASH_SIZE,
        "partition_headroom": FLASH_SIZE - len(patched),
        "byte_identical_stock_roundtrip": True,
        "only_changed_file": changed,
        "stock_files": before,
        "candidate_files": after,
        "header_preserved_ranges": ["0:4", "8:100"],
        "device_type_list_preserved": True,
        "compatible_model_types": [
            struct.unpack("<H", xor(t[i : i + 2]))[0] for i in range(20, len(t), 2)
        ],
        "signing": "No private-key signature in the traced SWKH flow; byte sums + payload MD5 + model/version checks. See docs/devices/ds-kd8003-ime1/firmware-format.md for scope.",
    }
    (out / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
    for p in sorted(out.glob("*.dav")):
        print(sha(p.read_bytes()), p.name)
    print(
        "Changed files:", changed, "partition headroom:", report["partition_headroom"]
    )
    print("Candidate:", out / "ds-kd8003-ime1-2.2.62-hook.dav")
    print("Manifest:", out / "manifest.json")
    return report


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, ValueError, OSError) as error:
        raise SystemExit(
            f"Build failed: {error or 'input validation failed'}"
        ) from None
