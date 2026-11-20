# Phase 5 status — tooling

## Done
- New `crates/meld-lsp` crate with a `meld-lsp` stdio binary built on
  `tower-lsp-server 0.23.0` (re-verified as the newest stable release).
- Region virtual documents pad lines/columns to the original `.meld`
  coordinates, slice the byte-preserving glue-sanitized body, and use
  `meld-virtual:///<region-id>.<extension>` URIs.
- UTF-16 LSP position conversion with boundary clamping; a multibyte test
  proves `é` before the cursor does not desynchronize source maps.
- Full-document sync, per-region tree-sitter diagnostics, and completion.
- Pack-owned `lsp.toml`: optional external stdio language server plus
  deterministic keyword/binding fallback. A small client performs initialize,
  didOpen/didChange, completion and shutdown; response ranges are mapped back
  to `.meld` coordinates.
- Pack-owned `formatter.toml`: `meld fmt` canonicalizes glue itself and runs a
  pack argv command for region bodies, with `{src}`/`{out}` placeholders and
  path rebasing.
- `meld repl` switches languages with `:lang <id>` and exits with `:exit`,
  keeping one T2 worker per language.
- Deterministic fixtures: `tests/fixtures/pack-lsp-mock/`,
  `tests/fixtures/lsp-mock/server.py`, `tests/fixtures/pack-fmt-mock/`, and
  `tests/fixtures/fmt_mock.py`.
- Spec added: `spec/lsp.md`; `spec/lang-pack.md` documents all four runtime
  metadata files.

## Verification
- `cargo test -p meld-lsp`: 4 tests pass. The decisive one requests completion
  inside `@mocklang as js` and asserts:
  - label `mockFromVirtualDoc`;
  - detail `uri=meld-virtual:///js.js`;
  - echo range mapped back to `{"line":3,"character":11}`.
- `cargo test -p meld-cli`: 3 tests pass — pack-owned formatter, REPL language
  switch, and third-party shell pack execution.

## Judgment calls
- Keep the first LSP narrow and fully source-map-tested; hover/rename/incremental
  sync are future work.
- Virtual files are padded to global coordinates so ordinary block regions need
  no position rewriting in the language server.
- Static keywords are an explicit offline completion source, not a pretend
  language server; `command` opts into real forwarding.
- Diagnostics anchor at the region because glue blanking can make child-node
  offsets ambiguous.
- Meld formats glue only; packs own language formatting and argv, never shell
  strings.
- The REPL's language switch is a plain command so it can be tested over stdin.

## Blockers
None.

## Done-when assessment
"Editor completion works inside a non-default-language region": satisfied by
the external mock LSP test with global coordinate mapping.
