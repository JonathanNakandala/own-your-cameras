#!/usr/bin/env python3
"""Isolated fixture: a synthetic ELF32 header table, never a device binary."""
import struct
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from elf_tls import reorder_tls_last  # noqa: E402


def elf(kinds):
    header = bytearray(52)
    header[:7] = b'\x7fELF\x01\x01\x01'
    struct.pack_into('<HH', header, 16, 2, 40)
    struct.pack_into('<I', header, 28, 52)
    struct.pack_into('<HH', header, 42, 32, len(kinds))
    rows = b''.join(struct.pack('<8I', k, 0, i * 0x1000, 0, 0, 0, 0, 0) for i, k in enumerate(kinds))
    return bytes(header) + rows


class ReorderTls(unittest.TestCase):
    def test_tls_moves_last_and_nothing_else_changes(self):
        before = elf([6, 3, 7, 1, 1, 2])
        after = reorder_tls_last(before)
        kinds = [struct.unpack_from('<I', after, 52 + i * 32)[0] for i in range(6)]
        self.assertEqual(kinds, [6, 3, 1, 1, 2, 7])
        self.assertEqual(after[:52], before[:52])
        self.assertEqual(sorted(after[52:]), sorted(before[52:]))

    def test_refuses_foreign_layouts(self):
        for kinds in ([6, 3, 1, 2], [6, 3, 7, 7, 1, 2], [1, 6, 3, 7, 2]):
            with self.assertRaises(ValueError):
                reorder_tls_last(elf(kinds))
        with self.assertRaises(ValueError):
            reorder_tls_last(b'\x7fELF\x02' + bytes(60))


if __name__ == '__main__':
    unittest.main()
