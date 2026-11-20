#!/usr/bin/env python3
"""Migrate legacy meld.lock shapes to meld-project-libraries/1 canonical JSON."""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

CURRENT = "meld-project-libraries/1"
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
VERSION_RE = re.compile(r"^[0-9A-Za-z][0-9A-Za-z.+-]*$")


def canonical(data: dict) -> str:
    return json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def migrate(data: object) -> dict:
    schema = data.get("schema") if isinstance(data, dict) else None
    if schema not in (None, "meld-project-libraries/0", CURRENT):
        raise SystemExit(f"migrate_lock.py: unsupported schema {schema!r}")
    raw = data.get("libraries", data.get("libs", data)) if isinstance(data, dict) else data
    result: dict[str, str] = {}
    if isinstance(raw, dict):
        items = raw.items()
    elif isinstance(raw, list):
        items = []
        for entry in raw:
            if not isinstance(entry, dict) or "id" not in entry or "version" not in entry:
                raise SystemExit("migrate_lock.py: legacy list entries need id and version")
            items.append((entry["id"], entry["version"]))
    else:
        raise SystemExit("migrate_lock.py: unrecognized lock layout")
    for key, value in items:
        if not isinstance(key, str) or not NAME_RE.match(key):
            raise SystemExit(f"migrate_lock.py: invalid library id {key!r}")
        if not isinstance(value, str) or not VERSION_RE.match(value):
            raise SystemExit(f"migrate_lock.py: invalid version for {key}: {value!r}")
        if key in result:
            raise SystemExit(f"migrate_lock.py: duplicate library id {key}")
        result[key] = value
    return {"libraries": result, "schema": CURRENT}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", nargs="?", default="meld.lock")
    parser.add_argument("--check", action="store_true", help="fail unless the file is already current")
    parser.add_argument("--in-place", action="store_true")
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    path = pathlib.Path(args.file)
    original = path.read_text(encoding="utf-8")
    data = json.loads(original)
    migrated = canonical(migrate(data))
    if args.check:
        if original != migrated:
            print(f"migrate_lock.py: {path} needs migration", file=sys.stderr)
            return 1
        print(f"migrate_lock.py: PASS {path} is already {CURRENT}")
        return 0
    if args.in_place:
        if args.output:
            raise SystemExit("migrate_lock.py: --in-place and --output are mutually exclusive")
        backup = path.with_name(path.name + ".bak")
        backup.write_text(original, encoding="utf-8")
        path.write_text(migrated, encoding="utf-8")
        print(f"migrate_lock.py: wrote {path} (backup {backup})")
    elif args.output:
        args.output.write_text(migrated, encoding="utf-8")
        print(f"migrate_lock.py: wrote {args.output}")
    else:
        sys.stdout.write(migrated)
    return 0


if __name__ == "__main__":
    sys.exit(main())
