#!/usr/bin/env python3
"""Copy the published highlight/injection queries for each pinned grammar.

JUDGMENT CALL: packs do not hand-write highlight queries. The upstream grammar
crate is the source of truth; the pack records an exact crate version or a
documented ``pinned:<ref>`` in `pack.toml`, and this tool makes a reproducible
copy into `queries/`. Crate substitutions, query subdirectories and mechanical
fixups come from `third-party/overrides.toml`; accepted pinned versions come
from `grammar-sources.json`.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXACT_VERSION_RE = re.compile(r"^=?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")
PINNED_REF_RE = re.compile(r"^pinned:[A-Za-z0-9][A-Za-z0-9._-]*$")


def load_overrides(root: Path = ROOT) -> tuple[dict[str, str], dict[str, str],
                                              dict[str, dict[str, list[tuple[str, str]]]]]:
    """Read crate/subdir/fixup rules from third-party/overrides.toml."""
    path = root / "third-party" / "overrides.toml"
    doc = tomllib.loads(path.read_text(encoding="utf-8"))
    crates = {str(k): str(v) for k, v in doc.get("crates", {}).items()}
    subdirs = {str(k): str(v) for k, v in doc.get("query_subdirs", {}).items()}
    fixups: dict[str, dict[str, list[tuple[str, str]]]] = {}
    for entry in doc.get("fixups", []):
        crate = str(entry["crate"])
        name = str(entry["file"])
        fixups.setdefault(crate, {}).setdefault(name, []).append(
            (str(entry["from"]), str(entry["to"]))
        )
    return crates, subdirs, fixups


def load_pinned_versions(root: Path = ROOT) -> dict[str, set[str]]:
    """Read documented immutable versions from grammar-sources.json."""
    doc = json.loads((root / "grammar-sources.json").read_text(encoding="utf-8"))
    versions: dict[str, set[str]] = {}
    for entry in doc.get("crates", []):
        crate = entry.get("crate")
        if not isinstance(crate, str):
            continue
        accepted = versions.setdefault(crate, set())
        for value in (entry.get("version"), entry.get("cargo_pin")):
            if isinstance(value, str) and value:
                accepted.add(value.lstrip("="))
    return versions


def resolve_rev(rev: object, crate: str, pinned: dict[str, set[str]]) -> str:
    """Accept exact versions and `pinned:<ref>` values documented locally."""
    if not isinstance(rev, str) or not rev:
        raise SystemExit(f"missing tree_sitter.rev for {crate}")
    if rev.startswith("pinned:"):
        if not PINNED_REF_RE.fullmatch(rev):
            raise SystemExit(
                f"malformed pinned grammar ref {rev!r}; expected 'pinned:<ref>' "
                "with a non-empty ref matching [A-Za-z0-9._-]+"
            )
        ref = rev[len("pinned:"):]
        if ref not in pinned.get(crate, set()):
            raise SystemExit(f"no documented grammar source for {crate} pinned ref {rev!r}")
        return ref
    version = rev.lstrip("=")
    if EXACT_VERSION_RE.fullmatch(version):
        return version
    raise SystemExit(
        f"unsupported tree_sitter.rev {rev!r} for {crate}; "
        "use an exact version or a documented 'pinned:<ref>'"
    )


def normalize_query(crate: str, name: str, data: bytes,
                    fixups: dict[str, dict[str, list[tuple[str, str]]]]) -> bytes:
    text = data.decode("utf-8")
    for old, new in fixups.get(crate, {}).get(name, []):
        if old not in text:
            raise SystemExit(f"expected query fixup not found in {crate} {name}: {old!r}")
        text = text.replace(old, new)
    return text.encode("utf-8")


def source_dir(registry_src: Path, crate: str, version: str) -> Path:
    matches = list(registry_src.glob(f"{crate}-{version}"))
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one registry source for {crate}=={version}, got {matches}"
        )
    return matches[0]


def query_path(src: Path, crate: str, name: str, subdirs: dict[str, str]) -> Path | None:
    sub = subdirs.get(crate, "")
    candidates = [src / sub / "queries" / name, src / "queries" / name]
    for path in candidates:
        if path.is_file():
            return path
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", type=Path, default=ROOT)
    default_registry = (
        Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home")) / "registry" / "src"
    )
    ap.add_argument("--registry-src", type=Path, default=default_registry)
    ap.add_argument("--check", action="store_true", help="verify copies without writing")
    args = ap.parse_args()

    root = args.root.resolve()
    crate_overrides, query_subdirs, query_fixups = load_overrides(root)
    pinned_versions = load_pinned_versions(root)

    registry_src = args.registry_src
    # JUDGMENT CALL: Cargo puts crates under a registry-hash subdirectory.
    # Accept both that root and the concrete hash directory.
    indexed = next(
        (p for p in registry_src.glob("index.crates.io-*") if p.is_dir()), None
    )
    if indexed is not None:
        registry_src = indexed

    count = 0
    for manifest in sorted((root / "packs").glob("*/pack.toml")):
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        ts = data.get("tree_sitter")
        if not ts:
            continue
        declared_crate = str(ts["crate"])
        crate = crate_overrides.get(declared_crate, declared_crate)
        version = resolve_rev(ts["rev"], crate, pinned_versions)
        src = source_dir(registry_src, crate, version)
        qdir = manifest.parent / "queries"
        if not args.check:
            qdir.mkdir(parents=True, exist_ok=True)
        for name in ("highlights.scm", "injections.scm"):
            upstream = query_path(src, crate, name, query_subdirs)
            if upstream is None:
                continue
            target = qdir / name
            expected = normalize_query(crate, name, upstream.read_bytes(), query_fixups)
            if args.check:
                if not target.is_file() or target.read_bytes() != expected:
                    print(f"STALE: {target.relative_to(root)}")
                    count += 1
                continue
            if not target.is_file() or target.read_bytes() != expected:
                # JUDGMENT CALL: write the normalized bytes, not the raw upstream
                # bytes, so --check and copy mode agree even with fixups.
                target.write_bytes(expected)
                print(f"copied {crate}@{version} {name} -> {target.relative_to(root)}")
                count += 1
    if args.check:
        print(f"sync-grammars check: {count} stale query file(s)")
        return 1 if count else 0
    print(f"sync-grammars: {count} query file(s) copied")
    return 0


if __name__ == "__main__":
    sys.exit(main())
