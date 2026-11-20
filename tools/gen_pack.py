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
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXACT_VERSION_RE = re.compile(r"^=?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")
PINNED_REF_RE = re.compile(r"^pinned:[A-Za-z0-9][A-Za-z0-9._-]*$")

TEMPLATE_MANIFEST = '''id = "{lang}"
aliases = []
extensions = []
version = "{version}"
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
version = "{version}"
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

TEMPLATE_LIBRARY = '''# JUDGMENT CALL: every generated pack is immediately an installable Lang
# Library (spec/libraries.md). `exports` lists the relative files a consumer
# may rely on; it is data, not executable code.
api = "meld-lang-library/1"
kind = "lang"
id = "{lang}"
name = "{name}"
version = "{version}"
aliases = [{aliases}]
exports = [{exports}]
'''


def _toml_strings(items: list[str]) -> str:
    return ", ".join(f'"{item}"' for item in items)


def _local_crate_exists(root: Path, crate: str, ref: str) -> bool:
    """Return true when a pinned ref is already vendored or in the Cargo cache."""
    if (root / "third-party" / "vendor" / f"{crate}-{ref}").is_dir():
        return True
    home = Path(os.environ.get("CARGO_HOME", root / ".cargo-home")) / "registry" / "src"
    roots = [p for p in home.glob("index.crates.io-*") if p.is_dir()] or [home]
    return any((base / f"{crate}-{ref}").is_dir() for base in roots)


def validate_grammar_rev(rev: str, crate: str, root: Path = ROOT) -> str:
    """Fail closed on malformed pins; verify exact-version pins when offline data exists.

    Opaque ``pinned:<ref>`` values cannot be resolved without network access or a
    local vendor/cache directory, so they are accepted only after strict format
    validation. Exact versions are additionally checked against
    ``grammar-sources.json`` or the local Cargo registry cache when available.
    """
    if not isinstance(rev, str) or not rev:
        raise ValueError("grammar reference must not be empty")
    if rev == "generated-placeholder":
        return rev
    if rev.startswith("pinned:"):
        if not PINNED_REF_RE.fullmatch(rev):
            raise ValueError(
                f"malformed pinned grammar ref {rev!r}; expected 'pinned:<ref>' "
                "with a non-empty ref matching [A-Za-z0-9._-]+"
            )
        ref = rev[len("pinned:"):]
        if len(ref) > 128:
            raise ValueError(f"pinned grammar ref is too long: {rev!r}")
        if _local_crate_exists(root, crate, ref):
            return rev
        if EXACT_VERSION_RE.fullmatch(ref.lstrip("=")):
            version = ref.lstrip("=")
            sources_path = root / "grammar-sources.json"
            documented: set[str] = set()
            if sources_path.is_file():
                doc = json.loads(sources_path.read_text(encoding="utf-8"))
                documented = {
                    str(entry["version"])
                    for entry in doc.get("crates", [])
                    if isinstance(entry, dict) and entry.get("crate") == crate
                    and isinstance(entry.get("version"), str)
                }
            if version not in documented:
                raise ValueError(
                    f"pinned grammar ref {rev!r} is not documented in grammar-sources.json "
                    "or available in the local Cargo cache"
                )
        return rev
    if EXACT_VERSION_RE.fullmatch(rev.lstrip("=")):
        return rev
    raise ValueError(
        f"unsupported grammar reference {rev!r}; use an exact version or 'pinned:<ref>'"
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("lang")
    ap.add_argument("--grammar", default=None, help="pinned grammar reference (optional)")
    ap.add_argument("--crate", default=None, help="published grammar crate (default: tree-sitter-<lang>)")
    ap.add_argument("--aliases", default="", help="comma-separated aliases")
    ap.add_argument("--extensions", default="", help="comma-separated file extensions")
    ap.add_argument("--keywords", default="", help="comma-separated LSP fallback keywords")
    ap.add_argument("--language-id", default=None, help="LSP language id (default: pack id)")
    ap.add_argument("--name", default=None, help="human-readable library name")
    ap.add_argument("--version", default="0.1.0", help="library version (default: 0.1.0)")
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
    try:
        validate_grammar_rev(rev, crate, ROOT)
    except (ValueError, OSError, json.JSONDecodeError) as exc:
        print(f"invalid grammar reference: {exc}", file=sys.stderr)
        return 2
    if args.strict and rev == "generated-placeholder":
        print("--strict requires --grammar with an exact version or pinned:<ref>", file=sys.stderr)
        return 2
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
            version=args.version,
            crate=crate,
            rev=rev,
        )
    else:
        manifest = TEMPLATE_MANIFEST.format(lang=lang, version=args.version).replace(
            "generated-placeholder", rev
        )
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
    # JUDGMENT CALL: generated L0 packs are libraries from birth. A later T2
    # upgrade adds runner.toml and the update tool extends `exports`.
    exports = [
        "pack.toml",
        "grammar/REFERENCE.md",
        "queries/highlights.scm",
        "queries/injections.scm",
        "region_end.wat",
        "tests/feed.meld",
    ]
    (pack / "library.toml").write_text(
        TEMPLATE_LIBRARY.format(
            lang=lang,
            name=args.name or lang.title(),
            version=args.version,
            aliases=_toml_strings(aliases),
            exports=_toml_strings(exports),
        ),
        encoding="utf-8",
    )
    print(f"generated {'strict L0' if args.strict else 'L0'} library: {pack}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
