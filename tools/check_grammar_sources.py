#!/usr/bin/env python3
"""Fail closed on unpinned, stale or unverifiable grammar sources.

The checker compares grammar-sources.json with pack manifests, the meld-grammars
dependency pins, the Cargo-verified checksums in Cargo.lock, checked-in query
files and third-party/grammars.toml. Any TODO_VERIFY marker is an error.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import tomllib

PLACEHOLDER = "TODO_VERIFY"


def fail(message: str) -> None:
    print(f"check_grammar_sources.py: {message}", file=sys.stderr)


def contains_placeholder(value: object) -> bool:
    if isinstance(value, str):
        return PLACEHOLDER in value
    if isinstance(value, dict):
        return any(contains_placeholder(k) or contains_placeholder(v) for k, v in value.items())
    if isinstance(value, (list, tuple)):
        return any(contains_placeholder(item) for item in value)
    return False


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parent.parent)
    parser.add_argument("--sources", type=pathlib.Path, default=None)
    args = parser.parse_args()
    root = args.root.resolve()
    sources_path = (args.sources or root / "grammar-sources.json").resolve()

    errors: list[str] = []
    sources = json.loads(sources_path.read_text(encoding="utf-8"))
    if contains_placeholder(sources) or PLACEHOLDER in sources_path.read_text(encoding="utf-8"):
        errors.append(f"{sources_path.name} still contains {PLACEHOLDER}")

    if sources.get("schema") != "meld-grammar-sources/1":
        errors.append("unexpected grammar source schema")

    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    lock_by_name = {pkg["name"]: pkg for pkg in lock["package"]}
    grammars_manifest = tomllib.loads((root / "crates/meld-grammars/Cargo.toml").read_text(encoding="utf-8"))
    dep_pins = grammars_manifest["dependencies"]

    pack_pins: dict[str, dict[str, str]] = {}
    for manifest in sorted((root / "packs").glob("*/pack.toml")):
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        ts = data.get("tree_sitter")
        if ts:
            pack_pins.setdefault(ts["crate"], {})[manifest.parent.name] = str(ts["rev"]).lstrip("=")

    vendor_doc = tomllib.loads((root / "third-party/grammars.toml").read_text(encoding="utf-8"))
    vendor_by_name = {entry["crate"]: entry for entry in vendor_doc.get("grammar", [])}
    overrides = tomllib.loads((root / "third-party/overrides.toml").read_text(encoding="utf-8"))
    if contains_placeholder(vendor_doc) or contains_placeholder(overrides):
        errors.append("third-party config still contains a placeholder")

    entries = sources.get("crates", [])
    seen = set()
    for entry in entries:
        crate = entry.get("crate", "<missing>")
        seen.add(crate)
        version = str(entry.get("version", ""))
        expected_packs = pack_pins.get(crate)
        if not expected_packs:
            errors.append(f"{crate}: no pack manifest declares this grammar")
        else:
            if sorted(expected_packs) != sorted(entry.get("packs", [])):
                errors.append(f"{crate}: pack list differs from pack.toml")
            versions = set(expected_packs.values())
            if versions != {version}:
                errors.append(f"{crate}: version {version} differs from pack pins {versions}")

        pin = dep_pins.get(crate)
        if isinstance(pin, dict):
            pin = pin.get("version", "")
        if str(pin).lstrip("=") != version:
            errors.append(f"{crate}: meld-grammars pin {pin!r} differs from {version}")
        if entry.get("cargo_pin", "").lstrip("=") != version:
            errors.append(f"{crate}: grammar-sources cargo_pin differs from {version}")

        lock_entry = lock_by_name.get(crate, {})
        if lock_entry.get("version") != version:
            errors.append(f"{crate}: Cargo.lock version differs from {version}")
        checksum = entry.get("cargo_checksum", "")
        if len(checksum) != 64 or checksum != lock_entry.get("checksum"):
            errors.append(f"{crate}: cargo_checksum does not match Cargo.lock")

        v = vendor_by_name.get(crate)
        if not v:
            errors.append(f"{crate}: missing from third-party/grammars.toml")
        elif v.get("version") != version or v.get("cargo_checksum") != checksum:
            errors.append(f"{crate}: third-party/grammars.toml entry is stale")

        for relative, expected_hash in entry.get("query_sha256", {}).items():
            path = root / "packs" / relative
            if not path.is_file():
                errors.append(f"{crate}: missing query file {relative}")
            elif sha256(path) != expected_hash:
                errors.append(f"{crate}: query file {relative} hash mismatch")

    for crate in sorted(set(pack_pins) - seen):
        errors.append(f"{crate}: pinned in packs but missing from grammar-sources.json")
    for crate in sorted(set(vendor_by_name) - seen):
        errors.append(f"{crate}: present in grammars.toml but missing from grammar-sources.json")

    if errors:
        for error in errors:
            fail(error)
        print(f"check_grammar_sources.py: FAIL ({len(errors)} problem(s))", file=sys.stderr)
        return 1
    print(f"check_grammar_sources.py: PASS ({len(entries)} pinned grammar crate(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
