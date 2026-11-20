#!/usr/bin/env python3
"""Migrate legacy registry/library indexes to their current canonical schemas."""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

REGISTRY = "meld-pack-registry/1"
LIBRARY = "meld-lang-library-index/1"
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


def canonical(data: dict) -> str:
    return json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def safe_path(value: str, root: pathlib.Path | None = None, *,
              allow_parent: bool = True) -> str:
    """Validate an index path with the same rules as packtool registries.

    JUDGMENT CALL: packtool accepts ``../pack`` registry paths only when the
    index is attested, but still rejects absolute paths, backslashes and any
    path that resolves outside the registry's allowed root. Symlink components
    are rejected so an index cannot redirect a migration outside the tree.
    """
    if not isinstance(value, str) or not value or "\x00" in value:
        raise SystemExit(f"migrate_index.py: unsafe relative path {value!r}")
    if "\\" in value or re.match(r"^[A-Za-z]:", value):
        raise SystemExit(f"migrate_index.py: unsafe relative path {value!r}")
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or (not allow_parent and ".." in path.parts):
        raise SystemExit(f"migrate_index.py: unsafe relative path {value!r}")
    if root is not None:
        base = pathlib.Path(root).resolve()
        # Mirrors packtool.resolve_registry_pack(..., attested=True): an
        # attested index may reach sibling pack directories, but nothing else.
        allowed_root = base.parent.parent if allow_parent else base
        current = base
        for part in path.parts:
            if part in ("", "."):
                continue
            if part == "..":
                current = current.parent
                continue
            current = current / part
            if current.is_symlink():
                raise SystemExit(f"migrate_index.py: symlink in path {value!r}")
        try:
            resolved = current.resolve()
        except OSError as exc:
            raise SystemExit(f"migrate_index.py: unresolvable path {value!r}: {exc}") from exc
        if not resolved.is_relative_to(allowed_root.resolve()):
            raise SystemExit(f"migrate_index.py: path escapes allowed root: {value!r}")
    return value


def migrate_registry(data: dict, root: pathlib.Path | None = None) -> dict:
    schema = data.get("schema")
    if schema not in (None, "meld-pack-registry/0", REGISTRY):
        raise SystemExit(f"migrate_index.py: unsupported registry schema {schema!r}")
    raw = data.get("packs", [])
    if not isinstance(raw, list):
        raise SystemExit("migrate_index.py: registry packs must be a list")
    entries = []
    for entry in raw:
        if isinstance(entry, str):
            entry = {"id": entry, "version": "0.0.0"}
        if not isinstance(entry, dict) or not NAME_RE.match(str(entry.get("id", ""))):
            raise SystemExit(f"migrate_index.py: invalid registry entry {entry!r}")
        version = str(entry.get("version", "0.0.0"))
        if not NAME_RE.match(version):
            raise SystemExit(f"migrate_index.py: invalid version {version!r}")
        normalized = {
            "id": entry["id"],
            "version": version,
            "aliases": list(entry.get("aliases", [])),
            "tier": entry.get("tier", ""),
            "level": entry.get("level", ""),
            "strict": bool(entry.get("strict", False)),
            "path": safe_path(
                str(entry.get("path", f"packs/{entry['id']}/{version}")),
                root, allow_parent=True,
            ),
            "checksums_sha256": entry.get("checksums_sha256"),
            "signature": entry.get("signature"),
        }
        entries.append(normalized)
    entries.sort(key=lambda e: (e["id"], e["version"]))
    return {"packs": entries, "schema": REGISTRY}


def migrate_library(data: dict, root: pathlib.Path | None = None) -> dict:
    schema = data.get("schema")
    if schema not in (None, "meld-lang-library-index/0", LIBRARY):
        raise SystemExit(f"migrate_index.py: unsupported library index schema {schema!r}")
    raw = data.get("libraries", {})
    if isinstance(raw, list):
        entries = {}
        for entry in raw:
            if not isinstance(entry, dict) or "id" not in entry:
                raise SystemExit("migrate_index.py: legacy library entries need an id")
            entries[entry["id"]] = entry
    elif isinstance(raw, dict):
        entries = raw
    else:
        raise SystemExit("migrate_index.py: unrecognized library index layout")
    libraries: dict[str, dict] = {}
    for key, entry in entries.items():
        if not NAME_RE.match(str(key)) or not isinstance(entry, dict):
            raise SystemExit(f"migrate_index.py: invalid library entry {key!r}")
        libraries[key] = {
            "aliases": list(entry.get("aliases", [])),
            "id": key,
            "level": entry.get("level", ""),
            "path": safe_path(str(entry.get("path", key)), root, allow_parent=False),
            "source": str(entry.get("source", "")),
            "tier": entry.get("tier", ""),
            "version": str(entry.get("version", "0.0.0")),
        }
    return {"libraries": libraries, "schema": LIBRARY}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", nargs="?", default="packs/registry/index.json")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--in-place", action="store_true")
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    path = pathlib.Path(args.file)
    original = path.read_text(encoding="utf-8")
    data = json.loads(original)
    if not isinstance(data, dict):
        raise SystemExit("migrate_index.py: index root must be an object")
    schema = data.get("schema", "")
    index_root = path.parent.resolve()
    if "registry" in str(schema) or "packs" in data:
        migrated = canonical(migrate_registry(data, index_root))
    else:
        migrated = canonical(migrate_library(data, index_root))
    if args.check:
        if original != migrated:
            print(f"migrate_index.py: {path} needs migration", file=sys.stderr)
            return 1
        print(f"migrate_index.py: PASS {path} is canonical")
        return 0
    if args.in_place:
        if args.output:
            raise SystemExit("migrate_index.py: --in-place and --output are mutually exclusive")
        backup = path.with_name(path.name + ".bak")
        backup.write_text(original, encoding="utf-8")
        path.write_text(migrated, encoding="utf-8")
        print(f"migrate_index.py: wrote {path} (backup {backup})")
    elif args.output:
        args.output.write_text(migrated, encoding="utf-8")
        print(f"migrate_index.py: wrote {args.output}")
    else:
        sys.stdout.write(migrated)
    return 0


if __name__ == "__main__":
    sys.exit(main())
