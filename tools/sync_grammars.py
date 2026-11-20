#!/usr/bin/env python3
"""Copy the published highlight/injection queries for each pinned grammar.

JUDGMENT CALL: packs do not hand-write highlight queries. The upstream grammar
crate is the source of truth; the pack records the exact crate version in
`pack.toml` and this tool makes a reproducible copy into `queries/`. For
Markdown, the block grammar's queries are used because Meld regions are block
regions; the inline grammar remains available to the syntax layer later.
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# JUDGMENT CALL: two upstream crates in the original manifest were stuck on
# tree-sitter 0.19. These replacements provide the same languages with 0.27
# bindings; the mapping is also recorded in crates/meld-grammars/Cargo.toml.
CRATE_OVERRIDES = {
    "tree-sitter-markdown": "tree-sitter-md",
    "tree-sitter-sql": "tree-sitter-sequel",
}

# tree-sitter-md keeps its queries in two vendored subdirectories.
QUERY_SUBDIRS = {
    "tree-sitter-md": "tree-sitter-markdown",
}


def source_dir(registry_src: Path, crate: str, version: str) -> Path:
    matches = list(registry_src.glob(f"{crate}-{version}"))
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one registry source for {crate}=={version}, got {matches}"
        )
    return matches[0]


def query_path(src: Path, crate: str, name: str) -> Path | None:
    sub = QUERY_SUBDIRS.get(crate, "")
    candidates = [src / sub / "queries" / name, src / "queries" / name]
    for p in candidates:
        if p.is_file():
            return p
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    default_registry = (
        Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home")) / "registry" / "src"
    )
    ap.add_argument("--registry-src", type=Path, default=default_registry)
    ap.add_argument("--check", action="store_true", help="verify copies without writing")
    args = ap.parse_args()

    registry_src = args.registry_src
    # JUDGMENT CALL: Cargo puts crates under a registry-hash subdirectory.
    # Accept both that root and the concrete hash directory.
    indexed = next(
        (p for p in registry_src.glob("index.crates.io-*") if p.is_dir()), None
    )
    if indexed is not None:
        registry_src = indexed

    count = 0
    for manifest in sorted((ROOT / "packs").glob("*/pack.toml")):
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        ts = data.get("tree_sitter")
        if not ts:
            continue
        crate = CRATE_OVERRIDES.get(ts["crate"], ts["crate"])
        version = str(ts["rev"]).lstrip("=")
        src = source_dir(registry_src, crate, version)
        qdir = manifest.parent / "queries"
        qdir.mkdir(parents=True, exist_ok=True)
        for name in ("highlights.scm", "injections.scm"):
            upstream = query_path(src, crate, name)
            if upstream is None:
                continue
            target = qdir / name
            expected = upstream.read_bytes()
            if args.check:
                if not target.is_file() or target.read_bytes() != expected:
                    print(f"STALE: {target.relative_to(ROOT)}")
                    count += 1
                continue
            if not target.is_file() or target.read_bytes() != expected:
                shutil.copyfile(upstream, target)
                print(f"copied {crate}@{version} {name} -> {target.relative_to(ROOT)}")
                count += 1
    if args.check:
        print(f"sync-grammars check: {count} stale query file(s)")
        return 1 if count else 0
    print(f"sync-grammars: {count} query file(s) copied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
