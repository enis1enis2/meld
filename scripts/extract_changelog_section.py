#!/usr/bin/env python3
"""Extract the exact release section for a version from CHANGELOG.md.

Release notes must describe only the tag being published, not the entire
changelog. This script selects the ``## [VERSION] ...`` heading and emits the
content up to the next second-level heading, failing closed on missing,
duplicate or empty sections.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

HEADING = re.compile(r"^## \[([^\]]+)\]")


def extract(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    matches = []
    for i, line in enumerate(lines):
        match = HEADING.match(line)
        if match and match.group(1) == version:
            matches.append(i)
    if not matches:
        raise ValueError(f"no changelog section for {version}")
    if len(matches) > 1:
        raise ValueError(f"duplicate changelog sections for {version}")

    start = matches[0] + 1
    end = len(lines)
    for i in range(start, len(lines)):
        if lines[i].startswith("## "):
            end = i
            break

    section = "".join(lines[start:end]).strip("\n")
    if not section:
        raise ValueError(f"empty changelog section for {version}")
    return section + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="version without the leading v")
    parser.add_argument("--input", default="CHANGELOG.md", help="changelog path")
    parser.add_argument("--output", required=True, help="file to write the notes to")
    args = parser.parse_args()

    try:
        text = Path(args.input).read_text(encoding="utf-8")
        notes = extract(text, args.version)
    except (OSError, ValueError) as exc:
        print(f"extract_changelog_section.py: {exc}", file=sys.stderr)
        return 1

    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(notes, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
