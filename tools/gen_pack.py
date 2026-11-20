#!/usr/bin/env python3
"""Bootstrap a minimal L0 syntax-only Lang Pack (project.md §4).

The generator writes references only; it never downloads or executes grammar
code. JUDGMENT CALL: generated grammar revs are explicit placeholders and
`strict` mode rejects them until a human pins a real revision.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

TEMPLATE_MANIFEST = '''id = "{lang}"
aliases = []
extensions = []
tier = "L0"
level = "L0"
capabilities = ["syntax"]
grammar_rev = "generated-placeholder"
strict = false

[tree_sitter]
crate = "tree-sitter-{lang}"
rev = "generated-placeholder"

[sandbox]
network = false
fs = []
'''

TEMPLATE_HIGHLIGHT = '''; Auto-generated L0 highlight query for {lang}.
; Replace with the published grammar's queries/highlights.scm before strict mode.
(identifier) @variable
(string) @string
(comment) @comment
(number) @number
'''

TEMPLATE_END = ''';; Auto-generated end-scanner stub. Use raw hash delimiters until a
;; real scanner is supplied. Phase 1 fallback is the generic brace scanner.
(func $find_region_end (param i32 i32) (result i32)
  i32.const -1)
'''


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("lang")
    ap.add_argument("--grammar", default=None, help="pinned grammar reference (optional)")
    args = ap.parse_args()
    lang = args.lang
    if not lang.replace("-", "_").isalnum():
        print(f"invalid language id: {lang}", file=sys.stderr)
        return 2
    pack = ROOT / "packs" / lang
    if pack.exists():
        print(f"pack already exists: {pack}", file=sys.stderr)
        return 1
    (pack / "grammar").mkdir(parents=True)
    (pack / "queries").mkdir()
    (pack / "tests").mkdir()
    rev = args.grammar or "generated-placeholder"
    (pack / "pack.toml").write_text(
        TEMPLATE_MANIFEST.format(lang=lang).replace("generated-placeholder", rev),
        encoding="utf-8",
    )
    (pack / "grammar" / "REFERENCE.md").write_text(
        f"# Grammar reference for {lang}\n\nPinned rev: `{rev}`\n",
        encoding="utf-8",
    )
    (pack / "queries" / "highlights.scm").write_text(
        TEMPLATE_HIGHLIGHT.format(lang=lang), encoding="utf-8"
    )
    (pack / "queries" / "injections.scm").write_text(
        "; No injections in an auto-generated L0 pack.\n", encoding="utf-8"
    )
    (pack / "region_end.wat").write_text(TEMPLATE_END, encoding="utf-8")
    (pack / "tests" / "feed.meld").write_text(
        f"@python as p {{ print('host') }}\n@{lang} {{\n  # generated feeder fixture\n}}\n",
        encoding="utf-8",
    )
    print(f"generated L0 pack: {pack}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
