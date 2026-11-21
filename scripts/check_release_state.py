#!/usr/bin/env python3
"""Compare an existing GitHub release against the desired published state.

Used by the release workflow to make publication idempotent: it verifies the
release is stable, has the expected notes and asset set, and reports only the
assets that still need uploading. It never changes the release's latest flag.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, required=True, help="JSON from `gh release view --json`")
    parser.add_argument("--notes", type=Path, required=True, help="expected release notes file")
    parser.add_argument("--expect-latest", choices=("yes", "no"), required=True)
    parser.add_argument("--asset", action="append", default=[], dest="assets")
    args = parser.parse_args()

    try:
        state = json.loads(args.state.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"check_release_state.py: unreadable release state: {exc}", file=sys.stderr)
        return 1
    try:
        expected_notes = args.notes.read_text(encoding="utf-8").strip()
    except OSError as exc:
        print(f"check_release_state.py: unreadable notes: {exc}", file=sys.stderr)
        return 1

    if state.get("isPrerelease") is not False:
        print("check_release_state.py: existing release is marked prerelease", file=sys.stderr)
        return 1
    expect_latest = args.expect_latest == "yes"
    if bool(state.get("isLatest")) != expect_latest:
        print(
            f"check_release_state.py: existing release isLatest={state.get('isLatest')}, expected {expect_latest}",
            file=sys.stderr,
        )
        return 1
    actual_notes = str(state.get("body") or "").strip()
    if actual_notes != expected_notes:
        print("check_release_state.py: existing release notes do not match the changelog section", file=sys.stderr)
        return 1

    expected_assets = set(args.assets)
    if len(expected_assets) != len(args.assets):
        print("check_release_state.py: duplicate asset names", file=sys.stderr)
        return 1
    raw_assets = state.get("assets")
    if not isinstance(raw_assets, list):
        print("check_release_state.py: release state has no assets list", file=sys.stderr)
        return 1
    existing_assets = {asset["name"] for asset in raw_assets if isinstance(asset, dict) and isinstance(asset.get("name"), str)}
    unexpected = sorted(existing_assets - expected_assets)
    if unexpected:
        print(f"check_release_state.py: unexpected existing assets: {unexpected}", file=sys.stderr)
        return 1

    for name in sorted(expected_assets - existing_assets):
        print(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
