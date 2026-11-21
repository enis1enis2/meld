#!/usr/bin/env python3
"""Fail closed when a cargo-audit exception has no review deadline or has expired.

`cargo audit` honours `.cargo/audit.toml`, but an `ignore` entry stays in force
forever unless someone remembers to remove it. This gate makes every ignored
RUSTSEC id carry an explicit `EXPIRY: YYYY-MM-DD` comment and fails once that
date passes. It is deliberately stdlib-only so every CI job can run it.
"""
from __future__ import annotations

import argparse
import datetime as dt
import pathlib
import re
import sys
import tomllib

PLACEHOLDER_ROOT = pathlib.Path(__file__).resolve().parent.parent
EXPIRY_RE = re.compile(r"EXPIRY:\s*(\d{4}-\d{2}-\d{2})\b")


def check(root: pathlib.Path, today: dt.date | None = None) -> list[str]:
    path = root / ".cargo" / "audit.toml"
    if not path.is_file():
        return [f"{path}: missing audit configuration"]
    raw = path.read_text(encoding="utf-8")
    try:
        doc = tomllib.loads(raw)
    except tomllib.TOMLDecodeError as exc:
        return [f"{path}: invalid TOML ({exc})"]
    ignored = (doc.get("advisories") or {}).get("ignore") or []
    if not ignored:
        return []
    expiry = EXPIRY_RE.search(raw)
    if not expiry:
        return [f"{path}: ignore list is non-empty but has no EXPIRY: YYYY-MM-DD comment"]
    try:
        deadline = dt.date.fromisoformat(expiry.group(1))
    except ValueError:
        return [f"{path}: invalid EXPIRY date {expiry.group(1)!r}"]
    today = today or dt.date.today()
    if today > deadline:
        return [
            f"{path}: ignored advisories expired on {deadline.isoformat()}; "
            "review and remove the exception or document a reviewed renewal"
        ]
    return []


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=PLACEHOLDER_ROOT)
    parser.add_argument("--today", help="YYYY-MM-DD (test override)")
    args = parser.parse_args()
    today = dt.date.fromisoformat(args.today) if args.today else None
    errors = check(args.root.resolve(), today)
    for error in errors:
        print(f"check_audit_expiry.py: {error}", file=sys.stderr)
    if errors:
        return 1
    path = args.root.resolve() / ".cargo" / "audit.toml"
    print(f"check_audit_expiry.py: PASS audit exceptions are within their review deadline ({path})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
