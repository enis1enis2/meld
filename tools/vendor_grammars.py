#!/usr/bin/env python3
"""Vendor pinned grammar sources from a local Cargo registry cache.

The tool never downloads anything. It reads third-party/grammars.toml, locates
each exact crate version in CARGO_HOME/registry/src (or verifies and unpacks the
cached .crate archive), copies the grammar sources and queries into
third-party/vendor/, and writes a deterministic per-file SHA256 manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import sys
import tarfile
import tempfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
COPY_NAMES = ("grammar.js", "src/parser.c", "src/scanner.c", "src/scanner.cc", "src/node-types.json")
PLACEHOLDER = "TODO_VERIFY"


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def registry_roots(explicit: pathlib.Path | None) -> list[pathlib.Path]:
    if explicit:
        return [explicit]
    home = pathlib.Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home"))
    src = home / "registry" / "src"
    results = [p for p in src.glob("index.crates.io-*") if p.is_dir()]
    return results or [src]


def find_source(roots: list[pathlib.Path], crate: str, version: str, expected: str,
                cache_dir: pathlib.Path) -> pathlib.Path | None:
    for root in roots:
        candidate = root / f"{crate}-{version}"
        if candidate.is_dir():
            return candidate
    home = pathlib.Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home"))
    for cache in sorted((home / "registry" / "cache").glob("index.crates.io-*")):
        archive = cache / f"{crate}-{version}.crate"
        if not archive.is_file():
            continue
        actual = sha256(archive)
        if actual != expected:
            raise SystemExit(f"vendor_grammars.py: checksum mismatch for cached {archive.name}: {actual}")
        with tempfile.TemporaryDirectory(prefix="meld-vendor-") as tmp:
            with tarfile.open(archive, "r:gz") as tf:
                tf.extractall(tmp, filter="data")
            found = pathlib.Path(tmp) / f"{crate}-{version}"
            if not found.is_dir():
                raise SystemExit(f"vendor_grammars.py: unexpected archive layout for {crate}")
            # Unpack into a tool-owned cache; never mutate CARGO_HOME.
            target = cache_dir / f"{crate}-{version}"
            if not target.exists():
                cache_dir.mkdir(parents=True, exist_ok=True)
                shutil.copytree(found, target)
            return target
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=pathlib.Path, default=ROOT / "third-party/grammars.toml")
    parser.add_argument("--sources", type=pathlib.Path, default=ROOT / "grammar-sources.json")
    parser.add_argument("--registry-src", type=pathlib.Path, default=None)
    parser.add_argument("--dest", type=pathlib.Path, default=ROOT / "third-party/vendor")
    parser.add_argument("--check", action="store_true", help="verify existing vendor tree without writing")
    parser.add_argument("--write-config", action="store_true", help="rewrite grammars.toml from grammar-sources.json")
    args = parser.parse_args()

    if PLACEHOLDER in args.config.read_text(encoding="utf-8"):
        raise SystemExit(f"vendor_grammars.py: {args.config} contains a placeholder; fill verified checksums first")

    sources = json.loads(args.sources.read_text(encoding="utf-8"))
    if args.write_config:
        lines = [
            "# Generated from grammar-sources.json (verified against Cargo.lock).",
            "# Regenerate with: python3 tools/vendor_grammars.py --write-config",
            "# Schema: meld-third-party-grammars/1",
            'schema = "meld-third-party-grammars/1"',
            "",
        ]
        for entry in sources["crates"]:
            lines += [
                "[[grammar]]",
                f'crate = "{entry["crate"]}"',
                f'version = "{entry["version"]}"',
                f'cargo_checksum = "{entry["cargo_checksum"]}"',
                "packs = [" + ", ".join(f'"{p}"' for p in entry["packs"]) + "]",
                "",
            ]
        args.config.write_text("\n".join(lines), encoding="utf-8")
        print(f"vendor_grammars.py: wrote {args.config}")
        return 0

    config = tomllib.loads(args.config.read_text(encoding="utf-8"))
    roots = registry_roots(args.registry_src)
    manifest: dict[str, object] = {
        "schema": "meld-vendor-manifest/1",
        "source": "local Cargo registry cache (no network access)",
        "grammars": [],
    }
    missing: list[str] = []
    for grammar in config["grammar"]:
        crate, version, checksum = grammar["crate"], grammar["version"], grammar["cargo_checksum"]
        src = find_source(roots, crate, version, checksum, args.dest / ".cache")
        if src is None:
            missing.append(f"{crate}-{version}")
            continue
        record: dict[str, object] = {
            "crate": crate,
            "version": version,
            "cargo_checksum": checksum,
            "files": [],
        }
        candidates: list[pathlib.Path] = [src / name for name in COPY_NAMES]
        candidates += sorted((src / "queries").glob("*.scm"))
        for path in candidates:
            if not path.is_file():
                continue
            relative = path.relative_to(src).as_posix()
            digest = sha256(path)
            record["files"].append({"path": relative, "sha256": digest})  # type: ignore[union-attr]
            dest = args.dest / f"{crate}-{version}" / relative
            if not args.check:
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, dest)
            elif not dest.is_file() or sha256(dest) != digest:
                raise SystemExit(f"vendor_grammars.py: stale or missing vendored file {dest}")
        if not record["files"]:  # type: ignore[comparison-overlap]
            raise SystemExit(f"vendor_grammars.py: no files found for {crate}-{version} in {src}")
        manifest["grammars"].append(record)  # type: ignore[union-attr]
        print(f"vendor_grammars.py: {crate}-{version}: {len(record['files'])} file(s)")  # type: ignore[arg-type]

    if missing:
        raise SystemExit(
            "vendor_grammars.py: no local source/cache for " + ", ".join(missing)
            + ". Populate the Cargo registry cache first; this tool never downloads."
        )
    manifest_path = args.dest / "vendor-manifest.json"
    rendered = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    if args.check:
        if not manifest_path.is_file() or manifest_path.read_text(encoding="utf-8") != rendered:
            raise SystemExit(f"vendor_grammars.py: {manifest_path} is stale")
        print(f"vendor_grammars.py: PASS ({len(manifest['grammars'])} grammar crate(s))")
        return 0
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(rendered, encoding="utf-8")
    print(f"vendor_grammars.py: wrote {manifest_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
