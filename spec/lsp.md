# Meld Tooling Specification: LSP, Formatter, REPL (Phase 5)

Status: normative for Phase 5+. Derived from `project.md` §7 (tooling) and §13
(thin glue language). Implemented by `crates/meld-lsp` and `meld-cli`; every
**JUDGMENT CALL** below is binding until a later phase changes it.

The Phase 5 done-when criterion is "editor completion works inside a
non-default-language region". The proof is
`crates/meld-lsp/tests/completion.rs`, which requests completion inside
`@mocklang as js { ... }` and receives an external server result mapped back to
the original `.meld` position.

## 1. Crate and transport

`meld-lsp` uses `tower-lsp-server 0.23.0` over stdio; the binary is `meld-lsp`.
Version was re-verified on 2026-11-20: `0.23.0` is the newest stable release
(`0.24.0-rc.1` is a prerelease, deliberately not used). The original
`tower-lsp` is treated as legacy.

Declared capabilities: full-document sync, publish diagnostics, and
completion. Hover, rename and incremental sync are future work. **JUDGMENT
CALL:** the smallest honest virtual-document forwarding server is preferable
to a broad but untested one; each added feature needs a source-map test.

## 2. Pack-owned editor metadata

The core does not contain a language-id table. Each pack may ship `lsp.toml`:

```toml
[lsp]
language_id = "python"          # virtual doc language id / external server id
extension = "py"                # virtual doc URI suffix
command = "python3"             # optional external stdio LSP
args = ["vendor/server.py"]
keywords = ["print", "len"]     # deterministic fallback completions
```

`command` is optional. Without it the server returns `keywords` plus known glue
import/export names. With it the server launches one external process per
language, sends `initialize`/`initialized`, mirrors `didOpen`/`didChange`, and
forwards `textDocument/completion`. Paths are rebased project-root first, then
pack-directory by basename, like `runner.toml` (`spec/lang-pack.md` §4.1).
`MELD_PACKS` extends discovery exactly as it does for the CLI, and `MELD_ROOT`
overrides the project root used for rebasing.

**JUDGMENT CALL:** static keywords are not a fake language server; they are an
explicit L0-grade completion source that works offline and makes the package
contract testable without installing one server per language.

## 3. Virtual documents

For each relevant region, `VirtualDocument::new` builds an isolated file:

```
<newline> * global_line
<space>   * global_column
sanitized_source[region.start..region.end]
```

`sanitize_source` blanks Meld glue lines byte-for-byte, so the slice remains
aligned with the original file and language servers still see syntactically
valid code. The URI is `meld-virtual:///<region-id>.<extension>`; the region id
comes from `@lang as id`, so a mock region aliased as `js` becomes
`meld-virtual:///js.js`.

**JUDGMENT CALL:** padding the virtual file to the region's global line/column
makes ordinary block regions keep identical LSP coordinates in both files.
This avoids rewriting every language server's position handling while still
leaving exact offsets for inline islands. Language servers that dislike leading
blank lines are outside the tested set; raw region bodies are the fallback.

Positions are UTF-16 columns as LSP requires. `byte_offset`/`position_at_byte`
clamp to character boundaries, and a test covers a multibyte `é` before the
cursor. Region-local completion positions are sent to the external server;
ranges in its response (`textEdit`, `insertAndReplace`, `replace`) are rewritten
back to `.meld` coordinates before the response reaches the editor.

## 4. Diagnostics

On `didOpen`/`didChange`, the server splits the document, runs the bundled
tree-sitter grammars per region, and publishes warnings for regions whose parse
has an error. Header, untagged, and `text` regions are skipped. A document that
cannot be split at all gets one error diagnostic at `(0,0)`.

**JUDGMENT CALL:** diagnostics use region start as the anchor instead of the
tree-sitter error node because included ranges and glue blanking can make a
child node's byte offset ambiguous; the region id and language are always named
in the message so the user can jump to the right island.

## 5. Formatter

`meld fmt` formats glue lines itself and delegates region bodies to
`formatter.toml`:

```toml
[formatter]
command = "python3"
args = ["vendor/fmt.py", "{src}", "{out}"]
```

Meld writes the dedented, glue-stripped body to `{src}`, runs the pack argv
(no shell interpolation), and reads `{out}` back into the same region span. With
`--check`, it compares instead of rewriting. **JUDGMENT CALL:** Meld only
canonicalizes `export`/`import` glue; pretending to understand a hosted
language's formatting would duplicate every pack's formatter.

## 6. REPL

`meld repl` keeps one T2 worker per language. `:lang <id>` switches the active
language; `:exit` shuts workers down. Ordinary input is executed as a region in
the active language using the same pack discovery and `runner.toml` as `meld
run`.

**JUDGMENT CALL:** the language switch is a CLI command rather than an escape
sequence, so the REPL is scriptable and testable over plain stdin (`tests/cli.rs`
feeds `:lang javascript` and checks the output). No hidden global namespace is
shared across languages; the bridge remains explicit.

## 7. Verification

```sh
cargo test -p meld-lsp
cargo test -p meld-cli
```

Key assertions:

- completion inside `@mocklang as js` returns `mockFromVirtualDoc`;
- the item detail proves the external server saw `meld-virtual:///js.js`;
- the echoed virtual edit range is mapped back to global
  `{"line":3,"character":11}`;
- a pack with no external server returns deterministic keywords;
- a pack-owned formatter changes only the region body;
- the REPL keeps running across `:lang javascript`.

## 8. Judgment call index

1. Use stable `tower-lsp-server 0.23.0`; keep the first server small and tested (§1).
2. Pack-owned `lsp.toml` with optional external command and keyword fallback (§2).
3. Pad virtual documents to global coordinates; slice sanitized bodies; UTF-16 mapping (§3).
4. Anchor diagnostics at the region, not an ambiguous child node (§4).
5. Meld formats glue, packs format regions via argv placeholders (§5).
6. Scriptable `:lang`/`:exit` REPL with one worker per language (§6).
