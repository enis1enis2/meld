#!/usr/bin/env python3
"""Fail closed when a release binary embeds a local build path.

Rust stores `file!()` locations for panic messages in released binaries. Without
`--remap-path-prefix`, generated sources and dependency sources can leak the
checkout, Cargo target root or registry location. Those strings are not code,
but they make two otherwise identical builds differ and disclose host layout.
"""
from __future__ import annotations

import argparse
import pathlib
import sys


def encoded_variants(prefix: str) -> set[bytes]:
    """Return byte encodings for native and slash-normalized path spellings."""
    variants = {prefix, prefix.replace("\\", "/")}
    out: set[bytes] = set()
    for value in variants:
        out.add(value.encode("utf-8"))
        out.add(value.encode("utf-16-le"))
        out.add(value.encode("utf-16-be"))
    return out


def find_leaks(data: bytes, prefixes: list[str]) -> list[str]:
    leaks: list[str] = []
    for prefix in prefixes:
        for needle in encoded_variants(prefix):
            if needle and needle in data:
                leaks.append(prefix)
                break
    return sorted(set(leaks))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--prefix", action="append", default=[], help="forbidden local path prefix")
    parser.add_argument("files", nargs="+", type=pathlib.Path, help="release binaries to inspect")
    args = parser.parse_args(argv)
    if not args.prefix:
        parser.error("at least one --prefix is required")

    failed = False
    for path in args.files:
        leaks = find_leaks(path.read_bytes(), args.prefix)
        if leaks:
            failed = True
            for prefix in leaks:
                print(f"check_binary_paths.py: {path}: embeds local path prefix {prefix!r}", file=sys.stderr)
    if failed:
        return 1
    print(f"check_binary_paths.py: PASS ({len(args.files)} file(s), {len(args.prefix)} prefix(es))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
