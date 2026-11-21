#!/usr/bin/env python3
"""Unit tests for deterministic PE/COFF metadata normalization."""
from __future__ import annotations

import pathlib
import struct
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import normalize_pe  # noqa: E402

EPOCH = 1_795_292_305
SEED = "0.2.0/x86_64-pc-windows-msvc"
GUID = bytes(range(16))


def synthetic_pe(*, magic: int = 0x20B, timestamp: int = 0x11111111) -> bytearray:
    """Build a minimal PE image with one debug directory and a CodeView record."""
    data = bytearray(0x1000)
    pe = 0x80
    struct.pack_into("<I", data, 0x3C, pe)
    data[pe : pe + 4] = b"PE\0\0"
    struct.pack_into("<H", data, pe + 6, 1)  # one section
    struct.pack_into("<I", data, pe + 8, timestamp)
    optional_size = 240 if magic == 0x20B else 224
    struct.pack_into("<H", data, pe + 20, optional_size)
    optional = pe + 24
    struct.pack_into("<H", data, optional, magic)
    if magic == 0x20B:
        struct.pack_into("<I", data, optional + 108, 16)
        directories = optional + 112
    else:
        struct.pack_into("<I", data, optional + 92, 16)
        directories = optional + 96
    debug_rva, debug_size = 0x2000, 28
    struct.pack_into("<II", data, directories + 6 * 8, debug_rva, debug_size)

    section = optional + optional_size
    data[section : section + 8] = b".rdata\0\0"
    struct.pack_into("<IIII", data, section + 8, 0x1000, 0x2000, 0x1000, 0x400)

    entry = 0x400
    struct.pack_into("<I", data, entry + 4, timestamp)
    struct.pack_into("<II", data, entry + 12, normalize_pe.CODEVIEW, 32)
    struct.pack_into("<I", data, entry + 24, 0x500)
    data[0x500 : 0x504] = b"RSDS"
    data[0x504 : 0x514] = GUID
    struct.pack_into("<I", data, 0x514, 1)
    data[0x518 : 0x520] = b"fake.pdb"
    return data


class NormalizePeTests(unittest.TestCase):
    def test_normalizes_every_volatile_field(self):
        data = synthetic_pe()
        changed = normalize_pe.normalize(data, epoch=EPOCH, seed=SEED, name="meld.exe")
        self.assertEqual(changed, 3)
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        self.assertEqual(struct.unpack_from("<I", data, pe + 8)[0], EPOCH)
        self.assertEqual(struct.unpack_from("<I", data, 0x404)[0], EPOCH)
        self.assertEqual(bytes(data[0x504 : 0x514]), normalize_pe.deterministic_guid(SEED, "meld.exe", EPOCH))

    def test_idempotent_and_file_roundtrip(self):
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td) / "meld.exe"
            path.write_bytes(synthetic_pe())
            cmd = [sys.executable, str(ROOT / "scripts" / "normalize_pe.py"),
                   "--epoch", str(EPOCH), "--seed", SEED, str(path)]
            import subprocess

            first = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(first.returncode, 0, first.stderr)
            once = path.read_bytes()
            second = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertEqual(path.read_bytes(), once)
            self.assertIn("normalized 0 field(s)", second.stdout)

    def test_pe32_is_supported(self):
        data = synthetic_pe(magic=0x10B)
        self.assertEqual(normalize_pe.normalize(data, epoch=EPOCH, seed=SEED, name="meld.exe"), 3)
        self.assertEqual(struct.unpack_from("<I", data, 0x88)[0], EPOCH)

    def test_guid_is_stable_and_seed_sensitive(self):
        one = normalize_pe.deterministic_guid(SEED, "meld.exe", EPOCH)
        self.assertEqual(one, normalize_pe.deterministic_guid(SEED, "meld.exe", EPOCH))
        self.assertNotEqual(one, normalize_pe.deterministic_guid("other", "meld.exe", EPOCH))
        self.assertNotEqual(one, normalize_pe.deterministic_guid(SEED, "meld-lsp.exe", EPOCH))

    def test_malformed_image_fails_closed(self):
        with self.assertRaises(ValueError):
            normalize_pe.normalize(bytearray(64), epoch=EPOCH, seed=SEED, name="bad.exe")


if __name__ == "__main__":
    unittest.main()
