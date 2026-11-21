#!/usr/bin/env python3
"""Normalize volatile PE/COFF metadata so Windows release binaries are byte-stable.

MSVC/lld-link store the link time in the COFF header, in every debug-directory
entry, and in the CodeView PDB GUID. Those fields change on every rebuild even
when SOURCE_DATE_EPOCH is exported, so two otherwise identical release builds
produce different SHA-256 hashes. This script rewrites only those informational
fields to deterministic values derived from SOURCE_DATE_EPOCH and an explicit
seed. It does not touch code, data, imports or signatures, and is idempotent.

Run it after linking and before any checksum, signature or attestation is made.
"""
from __future__ import annotations

import argparse
import hashlib
import pathlib
import struct
import sys

CODEVIEW = 2
DEBUG_ENTRY_SIZE = 28
MAX_TIMESTAMP = 0xFFFFFFFF


def deterministic_guid(seed: str, name: str, epoch: int) -> bytes:
    """Derive a stable RFC-4122-shaped GUID for the CodeView record."""
    digest = hashlib.sha256(f"{seed}|{name}|{epoch}".encode("utf-8")).digest()
    guid = bytearray(digest[:16])
    guid[7] = (guid[7] & 0x0F) | 0x50  # version 5
    guid[8] = (guid[8] & 0x3F) | 0x80  # RFC 4122 variant
    return bytes(guid)


def normalize(data: bytearray, *, epoch: int, seed: str, name: str) -> int:
    """Patch a PE image in place; return the number of fields changed."""
    if len(data) < 0x40:
        raise ValueError("not a PE image: file is too small")
    if not 0 < epoch <= MAX_TIMESTAMP:
        raise ValueError(f"epoch {epoch} is outside the PE timestamp range")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if pe + 24 > len(data) or data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("missing PE signature")
    stamp = epoch & MAX_TIMESTAMP
    patches = 0

    if struct.unpack_from("<I", data, pe + 8)[0] != stamp:
        struct.pack_into("<I", data, pe + 8, stamp)
        patches += 1

    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    if optional_size < 96 or optional + optional_size > len(data):
        raise ValueError("truncated optional header")
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic == 0x20B:  # PE32+
        count_offset, directories = optional + 108, optional + 112
    elif magic == 0x10B:  # PE32
        count_offset, directories = optional + 92, optional + 96
    else:
        raise ValueError(f"unsupported optional-header magic {magic:#x}")
    if directories + 7 * 8 > optional + optional_size:
        raise ValueError("truncated data directories")
    directory_count = struct.unpack_from("<I", data, count_offset)[0]
    if directory_count < 7:  # no IMAGE_DIRECTORY_ENTRY_DEBUG
        return patches

    debug_rva, debug_size = struct.unpack_from("<II", data, directories + 6 * 8)
    if debug_rva == 0 or debug_size == 0:
        return patches
    if debug_size % DEBUG_ENTRY_SIZE:
        raise ValueError("malformed debug directory size")
    section_table = optional + optional_size
    if section_table + 40 * sections > len(data):
        raise ValueError("truncated section table")

    spans = []
    for index in range(sections):
        offset = section_table + 40 * index
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        spans.append((virtual_address, max(virtual_size, raw_size), raw_offset))

    def rva_to_offset(rva: int) -> int:
        for address, span, raw_offset in spans:
            if address <= rva < address + span:
                offset = raw_offset + (rva - address)
                if offset < len(data):
                    return offset
        raise ValueError(f"debug directory RVA {rva:#x} is outside all sections")

    table = rva_to_offset(debug_rva)
    if table + debug_size > len(data):
        raise ValueError("debug directory outside image")
    guid = deterministic_guid(seed, name, epoch)
    for index in range(debug_size // DEBUG_ENTRY_SIZE):
        entry = table + DEBUG_ENTRY_SIZE * index
        if struct.unpack_from("<I", data, entry + 4)[0] != stamp:
            struct.pack_into("<I", data, entry + 4, stamp)
            patches += 1
        kind, size = struct.unpack_from("<II", data, entry + 12)
        pointer = struct.unpack_from("<I", data, entry + 24)[0]
        if kind != CODEVIEW or size < 24 or pointer == 0:
            continue
        if pointer + size > len(data):
            raise ValueError("CodeView record outside image")
        if data[pointer : pointer + 4] != b"RSDS":
            continue
        if bytes(data[pointer + 4 : pointer + 20]) != guid:
            data[pointer + 4 : pointer + 20] = guid
            patches += 1
    return patches


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--epoch", type=int, required=True, help="SOURCE_DATE_EPOCH")
    parser.add_argument("--seed", required=True, help="stable seed, e.g. VERSION/TARGET")
    parser.add_argument("files", nargs="+", type=pathlib.Path, help="PE images to patch in place")
    args = parser.parse_args(argv)
    for path in args.files:
        data = bytearray(path.read_bytes())
        try:
            patches = normalize(data, epoch=args.epoch, seed=args.seed, name=path.name)
        except ValueError as error:
            raise SystemExit(f"normalize_pe.py: {path}: {error}") from error
        if patches:
            path.write_bytes(data)
        print(f"normalize_pe.py: {path.name}: normalized {patches} field(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
