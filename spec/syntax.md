# Meld Syntax Specification (Phase 0)

Status: normative for Phases 1–3. Derived from `project.md` §3.1 and §3.2.
Everything marked **JUDGMENT CALL** is an implementation decision made by the lead
engineer inside the bounds of the plan; it is revisable but must be honored until
a later phase explicitly changes it.

## 1. Lexical model

A `.meld` file is UTF-8. All offsets in the IR are **byte offsets**; line/column
are 1-based Unicode scalar counts for diagnostics. A region is a contiguous byte
range with a language tag and a stable id.

The file always begins with at most one default-language declaration:

```meld
#meld python
```

If absent, the default language is `text` (an L0 passthrough region) and `meld
check` emits a warning, not an error. **JUDGMENT CALL:** no `#lang` alias; the
plan's Racket precedent is honored with `#meld`, keeping one directive name.

## 2. Region forms

All forms lower to the same `Region { id, lang, start, end, body, kind }`.

### 2.1 Block switch

```text
@<lang> { <body> }
@<lang> as <id> { <body> }
@<lang>#"<hash>" <body> "#<hash>    (raw delimiter)
```

`{ ... }` is the Phase 1 delimiter. The raw form uses Rust-style hash counts so
the closing marker can never collide with nested braces; that branch is parsed
before any scanner is consulted and remains the normative escape hatch (§3.2 of
project.md). For the plain form, pack-owned `region_end.toml` metadata selects a
built-in Rust scanner profile by name. `find_region_end` returns
`RegionEnd { body_end, full_end, close_marker }` so glue stripping and source
maps retain exact byte offsets; `find_end` remains as a compatibility shim.

**JUDGMENT CALL:** Phase 6 end scanners are data-only. A pack file names a
built-in algorithm (`brace`, `heredoc`, `indent`, `delimited`, `counted`,
`template`); Meld never loads, spawns, or compiles `region_end.py/js/lua/sh/wat`
from a pack. `kind = "wasm"`, an unknown kind, an unknown schema, or invalid
TOML warns and falls back to `brace`. Raw hash delimiters are always accepted
even for packs that declare a scanner, and bypass the registry entirely.

### 2.2 Inline switch

```text
⟦<lang>: <expression>⟧
```

Inline regions are expressions and may appear inside another region's body.
They are still split out as separate regions by the splitter; the containing
region's source map records a hole. **JUDGMENT CALL:** inline regions do not
nest inside other inline regions in Phase 1; a nested `⟦` is literal text. This
keeps the splitter O(n) and is enough for Phase 2/3 examples. **JUDGMENT CALL:**
only `⟦` starts a region boundary; a `⟧` outside an already-open inline island
is literal text. The inline parser finds its own closing marker, so treating a
stray close as a boundary could produce a zero-length untagged region and an
infinite split loop.

### 2.3 Fence switch

````text
```<lang>
<body>
```
````

Fences are the literate-file form. A fence must start at the beginning of a
line. Closing fence must be alone on its line. **JUDGMENT CALL:** fences are
translated to block regions with generated ids `f0`, `f1`, ... so the binding
resolver sees one region model.

### 2.4 Auto-detect fallback

Untagged text (the default region) is classified by, in order: shebang,
`#meld` default, pack extension hints, then token/grammar heuristics. Every
classification carries `confidence: f32`. Confidence `< 0.75` is surfaced as a
diagnostic and never executed under `meld run` without `--assume-detected`.
**JUDGMENT CALL:** the threshold is a constant in `meld-core` and is tested; it
is not configurable in Phase 1 to avoid silent guessing.

## 3. Glue syntax (thin language)

Only three glue forms exist; Meld does not invent host semantics (project.md
§13 default).

```text
export <name> [as <bridge-name>]        # publishes from current region
import <name> from <region-id>           # binds into current region
```

`export`/`import` lines may appear anywhere in a region body but are stripped
from the code handed to the runner and replaced with caller-side bridge calls.
The bridge namespace is flat and explicit; no implicit globals.

Execution order is file order (project.md §13). A future `async` modifier is
reserved but unimplemented through Phase 3.

## 4. Grammar sketch (EBNF)

```ebnf
file          := header? region* ;
header        := '#meld' WS lang-id NEWLINE ;
region        := block | inline | fence | untagged ;
block         := '@' lang-id ('as' region-id)? WS
                 ('{' body '}' | hash-delim body hash-delim) ;
inline        := '⟦' lang-id ':' expr '⟧' ;
fence         := '```' lang-id NEWLINE body NEWLINE '```' ;
hash-delim    := '#'+ '"' | '"' '#'+ ;
lang-id       := [A-Za-z_][A-Za-z0-9_-]* ;
region-id     := [A-Za-z_][A-Za-z0-9_-]* ;
body          := raw-bytes ;
```

## 4.1 Region body preparation

Before a region body is handed to a runner, Meld strips glue lines
(`export`/`import`) and removes the common leading whitespace. **JUDGMENT
CALL:** block regions are indented inside the Meld file, but indentation is
significant in Python, so dedenting is required for the body to be valid
standalone source. Implemented in `meld-core::scheduler::strip_glue`.

## 5. Diagnostics contract

Every parse/split error reports `{file, byte_start, byte_end, line, col,
region_id, lang, code, message}`. Source maps are maintained from Meld byte
offsets to region-local offsets and back (project.md §7). Formatter and LSP
consume the same maps.

## 6. Verification checklist for Phase 1

- [ ] Splitter fuzz suite: closing markers inside strings/comments/heredocs.
- [ ] Round-trip: split → render → split is idempotent for all forms.
- [ ] Source map: injected error in region maps to exact `.meld` line/col.
- [ ] ≥10 syntax-only packs generated and highlighting mixed file correct.

## Sources

- `project.md` §3.1, §3.2 (design proposals, not external facts).
- tree-sitter multi-language documents (verified available in current docs index).
