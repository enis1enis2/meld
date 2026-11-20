#!/usr/bin/env python3
"""Reference `meld check` used while the Rust binary cannot be compiled."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference_splitter import split  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    paths = [Path(p) for p in sys.argv[1:]] or sorted((ROOT / "examples").glob("*.meld"))
    problems = 0
    for path in paths:
        src = path.read_text(encoding="utf-8")
        try:
            doc = split(src)
        except ValueError as exc:
            print(f"{path}: SPLIT ERROR: {exc}")
            problems += 1
            continue
        print(f"{path.name}: {len(doc['regions'])} regions (default={doc['default'] or 'text'})")
        for r in doc["regions"]:
            flag = " LOW_CONFIDENCE" if r["confidence"] < 0.75 else ""
            print(f"  {r['id']:>8} {r['lang']:<12} {r['kind']:<9} conf={r['confidence']:.2f}{flag}")
            if flag:
                problems += 1
    print(f"meld check reference: {problems} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
