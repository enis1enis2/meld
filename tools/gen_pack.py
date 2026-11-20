#!/usr/bin/env python3
"""Bootstrap a minimal syntax-only Lang Pack (project.md §4).

The generator writes references only; it never downloads or executes grammar
code. JUDGMENT CALL: the legacy default keeps `generated-placeholder` revs and
`strict = false`, so `packtool verify` refuses to sign/publish an unfinished
pack. `--strict` is the reviewed path: it records an exact crate version (or
any `pinned:<ref>` passed via `--grammar`) and emits the integrity-ready layout
used by the language-expansion batch.
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

STRICT_MANIFEST = '''# JUDGMENT CALL: exact crate version pinned here and in
# crates/meld-grammars/Cargo.toml. Queries are copied from the published crate
# by tools/sync_grammars.py; this generator never hand-writes them.
id = "{lang}"
aliases = [{aliases}]
extensions = [{extensions}]
version = "0.1.0"
tier = "L0"
level = "L0"
capabilities = ["syntax"]
grammar_rev = "crate:{crate}@{rev}"
strict = true

[tree_sitter]
crate = "{crate}"
rev = "{rev}"

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

TEMPLATE_LSP = '''# JUDGMENT CALL: no external language server is bundled for generated L0
# packs. Keywords give the Meld LSP deterministic fallback completions while
# keeping the core free of language-id tables.
[lsp]
language_id = "{lang}"
extension = "{extension}"
keywords = [{keywords}]
'''


def _toml_strings(items: list[str]) -> str:
    return ", ".join(f'"{item}"' for item in items)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("lang")
    ap.add_argument("--grammar", default=None, help="pinned grammar reference (optional)")
    ap.add_argument("--crate", default=None, help="published grammar crate (default: tree-sitter-<lang>)")
    ap.add_argument("--aliases", default="", help="comma-separated aliases")
    ap.add_argument("--extensions", default="", help="comma-separated file extensions")
    ap.add_argument("--keywords", default="", help="comma-separated LSP fallback keywords")
    ap.add_argument("--language-id", default=None, help="LSP language id (default: pack id)")
    ap.add_argument("--strict", action="store_true", help="emit an integrity-ready pinned L0 pack")
    args = ap.parse_args()
    lang = args.lang
    if not lang.replace("-", "_").isalnum():
        print(f"invalid language id: {lang}", file=sys.stderr)
        return 2
    pack = ROOT / "packs" / lang
    if pack.exists():
        print(f"pack already exists: {pack}", file=sys.stderr)
        return 1

    rev = args.grammar or "generated-placeholder"
    crate = args.crate or f"tree-sitter-{lang}"
    aliases = [v.strip() for v in args.aliases.split(",") if v.strip()]
    extensions = [v.strip() for v in args.extensions.split(",") if v.strip()]
    keywords = [v.strip() for v in args.keywords.split(",") if v.strip()]

    (pack / "grammar").mkdir(parents=True)
    (pack / "queries").mkdir()
    (pack / "tests").mkdir()

    if args.strict:
        manifest = STRICT_MANIFEST.format(
            lang=lang,
            aliases=_toml_strings(aliases),
            extensions=_toml_strings(extensions),
            crate=crate,
            rev=rev,
        )
    else:
        manifest = TEMPLATE_MANIFEST.format(lang=lang).replace("generated-placeholder", rev)
    (pack / "pack.toml").write_text(manifest, encoding="utf-8")

    (pack / "grammar" / "REFERENCE.md").write_text(
        f"# Grammar reference for {lang}\n\nCrate: `{crate}`\nPinned rev: `{rev}`\n",
        encoding="utf-8",
    )
    (pack / "queries" / "highlights.scm").write_text(
        TEMPLATE_HIGHLIGHT.format(lang=lang), encoding="utf-8"
    )
    (pack / "queries" / "injections.scm").write_text(
        "; No injections in an auto-generated L0 pack.\n", encoding="utf-8"
    )
    (pack / "region_end.wat").write_text(TEMPLATE_END, encoding="utf-8")
    if args.strict:
        (pack / "lsp.toml").write_text(
            TEMPLATE_LSP.format(
                lang=lang,
                extension=extensions[0] if extensions else lang,
                keywords=_toml_strings(keywords),
            ),
            encoding="utf-8",
        )
    # JUDGMENT CALL: the fixture uses a raw hash delimiter because the generic
    # brace scanner cannot see indentation-terminated or template languages.
    # Raw delimiters are the deterministic fallback in spec/syntax.md §2.1.
    (pack / "tests" / "feed.meld").write_text(
        f"@{lang}##\"\n  # generated feeder fixture\n\"##\n",
        encoding="utf-8",
    )
    print(f"generated {'strict L0' if args.strict else 'L0'} pack: {pack}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
