#!/usr/bin/env python3
"""Bounded, deterministic fuzz smoke for packtool's external-input parsers.

The Rust fuzz targets cover splitter, values, sandbox manifests/grants and T2
envelopes. This harness covers the Python-side release/decoder boundaries that
cannot be reached from Rust: trust roots, revocations, mirrors, registry index,
checksum files and signature files.

Only `PackToolError` is an expected rejection. Any other exception means a
malformed input escaped the decoder and is a bug.
"""

from __future__ import annotations

import argparse
import json
import random
import sys
import tempfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import packtool  # noqa: E402

ALPHABET = bytes(
    b"abcxyz0123456789 \t\r\n[]{}=,:-\"'#/\\.\x00\x7f"
)


def random_blob(rng: random.Random) -> bytes:
    size = rng.randrange(0, 513)
    return bytes(rng.choice(ALPHABET) for _ in range(size))


def random_doc(rng: random.Random, depth: int = 0) -> object:
    """Nested JSON-shaped data biased toward security-manifest key names."""
    keys = ["schema", "keys", "key_id", "public_key", "revoked", "mirrors", "packs", "id", "path"]
    scalars = [None, True, False, 0, 1, -1, 3.5, "", "meld-trust-roots/1", "abc"]
    if depth >= 3 or rng.random() < 0.45:
        return rng.choice(scalars)
    if rng.random() < 0.5:
        return [random_doc(rng, depth + 1) for _ in range(rng.randrange(0, 5))]
    return {
        rng.choice(keys): random_doc(rng, depth + 1)
        for _ in range(rng.randrange(0, 6))
    }


def expect_decoder_rejection_or_success(fn, *args) -> None:
    try:
        fn(*args)
    except packtool.PackToolError:
        pass


def run(iterations: int, seed: int) -> None:
    rng = random.Random(seed)
    with tempfile.TemporaryDirectory(prefix="meld-fuzz-packtool-") as tmp:
        root = Path(tmp)
        blob_path = root / "blob"
        pack_dir = root / "pack"
        pack_dir.mkdir()

        for _ in range(iterations):
            blob = random_blob(rng)
            blob_path.write_bytes(blob)

            expect_decoder_rejection_or_success(packtool.load_trust_roots, blob_path)
            expect_decoder_rejection_or_success(packtool.load_revocations, blob_path)
            expect_decoder_rejection_or_success(packtool.load_mirrors, blob_path)

            try:
                doc = json.loads(blob)
            except (UnicodeDecodeError, json.JSONDecodeError):
                doc = None
            if isinstance(doc, dict):
                expect_decoder_rejection_or_success(packtool.load_trust_roots_doc, doc)
                expect_decoder_rejection_or_success(packtool.load_registry_from_doc, doc)
            expect_decoder_rejection_or_success(packtool.load_revocations_from_doc, doc)

            (pack_dir / packtool.CHECKSUM_FILE).write_bytes(blob)
            expect_decoder_rejection_or_success(packtool.read_checksums, pack_dir)
            (pack_dir / packtool.SIGNATURE_FILE).write_bytes(blob)
            expect_decoder_rejection_or_success(packtool._read_signature, pack_dir)

            arbitrary = random_doc(rng)
            expect_decoder_rejection_or_success(packtool.load_revocations_from_doc, arbitrary)
            if isinstance(arbitrary, dict):
                expect_decoder_rejection_or_success(packtool.load_trust_roots_doc, arbitrary)
                expect_decoder_rejection_or_success(packtool.load_registry_from_doc, arbitrary)

    print(f"fuzz_packtool: PASS ({iterations} cases, seed {seed})")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--iterations", type=int, default=2000)
    parser.add_argument("--seed", type=int, default=20261120)
    args = parser.parse_args()
    run(args.iterations, args.seed)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
