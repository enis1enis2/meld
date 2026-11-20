#!/usr/bin/env python3
"""Proxy check for 'highlights mixed file correctly' (Phase 1 done-when).

JUDGMENT CALL: tree-sitter grammars are not compiled in this environment, so
this verifies that every region in a mixed file resolves to a generated L0 pack
with a highlight query and produces a deterministic scope span. It does not
validate tree-sitter node shapes; that is deferred to Rust tests.
"""
from __future__ import annotations

import sys
from pathlib import Path

from fuzz_splitter import split

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    fixture = ROOT / "examples" / "04-highlight.meld"
    doc = split(fixture.read_text(encoding="utf-8"))
    langs = {r["lang"] for r in doc["regions"] if r["kind"] == "fence"}
    if len(langs) < 10:
        print(f"fixture has only {len(langs)} fenced languages")
        return 1
    missing = []
    for lang in sorted(langs):
        pack = ROOT / "packs" / lang
        query = pack / "queries" / "highlights.scm"
        if not query.is_file() or "@" not in query.read_text(encoding="utf-8"):
            missing.append(lang)
        else:
            print(f"{lang}: L0 pack + highlight query ok")
    if missing:
        print(f"missing highlight packs: {', '.join(missing)}")
        return 1
    # Every fenced region maps to a scope span in source order.
    spans = [(r["id"], r["lang"], r["full_start"], r["full_end"])
             for r in doc["regions"] if r["kind"] == "fence"]
    assert spans == sorted(spans, key=lambda s: s[2]), "spans not in source order"
    print(f"highlight proxy: PASS ({len(spans)} mixed-language spans)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
