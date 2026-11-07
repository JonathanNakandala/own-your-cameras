"""The stock V2.2.62 build 230204 package, rebuilt from the panel's own flash.

The firmware partition (mtd1) holds the package's CramFS at offset 0, followed by erased bytes. The
SWKH package around it is the CramFS plus per-model constants, so the exact stock DAV can be rebuilt
from a flash read and checked against the pinned stock SHA-256: no downloaded firmware is needed.
"""
import hashlib
import io
import lzma
import struct
import tarfile

from build import STOCK_SHA, CramFS, pack, parse, sha

# SWKH header fields of the stock package (offsets into the 108-byte header, after the XOR mask).
# The byte sums at 4 and 104 and the CramFS length at 100 are computed by build.pack.
HEADER_FIELDS = {
    0: b"SWKH",
    8: struct.pack("<I", 108),  # header length
    12: struct.pack("<I", 1),  # header format
    16: struct.pack("<I", 1),  # one file in the package
    20: struct.pack("<I", 0x3F1),  # device class
    24: b"\xff" * 20,  # language and region masks: all
    56: bytes([0x02, 0x02, 0x00, 0x3E]),  # version 2.2.62
    60: bytes([0x00, 0x17, 0x02, 0x04]),  # build 230204 (0, year 23, month 02, day 04)
    64: b"cramfs.img".ljust(32, b"\0"),  # the single file's name
    96: struct.pack("<I", 108),  # the file's offset
}
# The compatible device-type list, as stored (obfuscated like the header), in stock order.
COMPATIBLE_DEVICES = (
    0x2CAA, 0x2CA9, 0x2CAF, 0x2F85, 0x2FB7, 0x2FBA, 0x2FBB, 0x2FB8, 0x2FB9, 0x2FBE, 0x2FB4, 0x2FBD,
    0x2FB2, 0x2FB3, 0x2FAB, 0x2FA8, 0x2FAD, 0x2FAC, 0x2FAF, 0x2FAE, 0x2FA9, 0x2FA2, 0x2F93, 0x2F90,
)
CRAMFS_MAGIC = 0x28CD3D45
# The keypad bootstrap key lives in hicore (inside visdoor.tar.lzma) at a fixed offset of the pinned
# build; only its hash is recorded here.
KEYPAD_KEY_FILE = ("visdoor.tar.lzma", "hicore")
KEYPAD_KEY_OFFSET = 7105260
KEYPAD_KEY_SHA256 = "3d0b94c1098e595f6b3d4327345a98ea5476c5c357f83e1288829ac2745657a2"


def cramfs_from_flash(mtd: bytes) -> bytes:
    """The CramFS image at the start of a firmware-partition read; the rest must be erased."""
    magic, size = struct.unpack_from("<II", mtd, 0)
    if magic != CRAMFS_MAGIC or not 0 < size <= len(mtd):
        raise ValueError("The firmware partition does not start with a CramFS image")
    if mtd[size:].strip(b"\xff"):
        raise ValueError("Unexpected data after the CramFS image in the firmware partition")
    return mtd[:size]


def stock_dav(cramfs: bytes) -> bytes:
    """Rebuild the stock DAV around a CramFS image; raises unless it is exactly the pinned stock."""
    header = bytearray(108)
    for offset, value in HEADER_FIELDS.items():
        header[offset : offset + len(value)] = value
    tail = bytes(16) + struct.pack(f"<I{len(COMPATIBLE_DEVICES)}H", len(COMPATIBLE_DEVICES), *COMPATIBLE_DEVICES)
    dav = pack(header, cramfs, tail)
    if sha(dav) != STOCK_SHA:
        raise ValueError(
            "The panel's firmware is not the unmodified V2.2.62 build 230204 "
            "(already modified, or another build); nothing was changed"
        )
    parse(dav)
    return dav


def keypad_key(cramfs: bytes) -> bytes:
    """The 16-byte keypad bootstrap key from the stock hicore inside the CramFS image."""
    archive, member = KEYPAD_KEY_FILE
    fs = CramFS(io.BytesIO(cramfs))
    node = next(i for i in fs.root.iterdir() if i.name == archive)
    tar = tarfile.open(fileobj=io.BytesIO(lzma.decompress(node.open().read(), format=lzma.FORMAT_ALONE)))
    data = tar.extractfile(tar.getmember(member)).read()
    key = data[KEYPAD_KEY_OFFSET : KEYPAD_KEY_OFFSET + 16]
    if hashlib.sha256(key).hexdigest() != KEYPAD_KEY_SHA256:
        raise ValueError("The keypad key is not where the pinned build keeps it")
    return key
