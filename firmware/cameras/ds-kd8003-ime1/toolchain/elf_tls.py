#!/usr/bin/env python3
"""Move the PT_TLS program header after every other entry of an ARM ELF32 executable.

The device's uClibc 0.9.33 loader reads the program headers in order and faults when PT_TLS
precedes PT_DYNAMIC/PT_GNU_RELRO; rustc emits it first. Reordering the header table changes no
code, data or offsets. Run as a script to rewrite a file in place, or import `reorder_tls_last`.
"""
import struct
import sys
from collections import Counter
from pathlib import Path


def reorder_tls_last(data: bytes) -> bytes:
    if len(data) < 52 or data[:7] != b'\x7fELF\x01\x01\x01':
        raise ValueError('ELF32 little-endian only')
    if struct.unpack_from('<HH', data, 16) != (2, 40):
        raise ValueError('ARM ET_EXEC required')
    offset = struct.unpack_from('<I', data, 28)[0]
    size, count = struct.unpack_from('<HH', data, 42)
    if offset < 52 or size != 32 or not 1 <= count <= 64 or offset + size * count > len(data):
        raise ValueError('program header bounds')
    entries = [data[offset + i * size:offset + (i + 1) * size] for i in range(count)]
    kinds = [struct.unpack_from('<I', row)[0] for row in entries]
    if kinds.count(7) != 1 or kinds.count(2) != 1 or kinds.count(6) != 1 or kinds.count(3) != 1:
        raise ValueError('expected exactly one PT_TLS, PT_DYNAMIC, PT_PHDR and PT_INTERP')
    first_load = kinds.index(1)
    if kinds.index(6) > first_load or kinds.index(3) > first_load:
        raise ValueError('PHDR/INTERP must precede LOAD')
    loads = [struct.unpack('<8I', row) for row in entries if struct.unpack_from('<I', row)[0] == 1]
    if [row[2] for row in loads] != sorted(row[2] for row in loads):
        raise ValueError('LOAD address order')
    for row in entries:
        fields = struct.unpack('<8I', row)
        if fields[1] + fields[4] > len(data):
            raise ValueError('file extent')
    ordered = [row for row in entries if struct.unpack_from('<I', row)[0] != 7]
    ordered += [row for row in entries if struct.unpack_from('<I', row)[0] == 7]
    assert Counter(entries) == Counter(ordered)
    output = bytearray(data)
    output[offset:offset + size * count] = b''.join(ordered)
    return bytes(output)


def main(argv):
    if len(argv) != 2:
        print('usage: elf_tls.py EXECUTABLE', file=sys.stderr)
        return 2
    path = Path(argv[1])
    path.write_bytes(reorder_tls_last(path.read_bytes()))
    return 0


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
