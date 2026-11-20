# Meld Phase 0 Specs — review record

## Reviewed deliverables

- `syntax.md` — region forms, glue syntax, diagnostics.
- `value-model.md` — types, wire encoding, numeric/lifetime rules.
- `lang-pack.md` — pack contract, tiers, conformance levels, generator.
- `t2-protocol.md` — universal worker protocol.
- `../examples/*.meld` — three hand-written example files.

The plan `../project.md` was read in full. It was not present on disk; the full
text supplied in the task prompt was persisted verbatim as `project.md` before
work began. **JUDGMENT CALL:** using prompt text as the authoritative file rather
 than halting, per the "flag blocker and continue with unblocked work" rule.

## Library verification (done before Phase 0 sign-off)

Verified against crates.io API on 2026-11-19:

| crate | current stable | rust-version | note |
|---|---|---|---|
| `tree-sitter` | 0.27.2 | 1.90 | edition 2024; `wasm` feature exists |
| `wasmtime` | 50.0.3 | 1.97 | `component-model` is a default feature; T3 target |
| `tower-lsp-server` | 0.23.0 | 1.85 | maintained fork (`tower-lsp` original is legacy); Phase 5 |
| `serde_json` | 1.0.154 | 1.71 | T2 envelope |

Capability claims from project.md §2 were re-checked: tree-sitter docs index
still documents multi-language documents; Component Model/WIT docs still define
WIT as the IDL and the Canonical ABI as the copy boundary. Claims in §3–§11
remain design proposals, not sourced fact, and are labeled as such.

## Judgment calls made (all inline where applied)

1. `#meld` is the only header directive; no `#lang` alias.
2. Raw hash delimiters are available in Phase 1 alongside `{...}`.
3. Inline regions do not nest in Phase 1.
4. Auto-detect confidence threshold 0.75, non-configurable in Phase 1.
5. T2 JSON line protocol with decimal-string integers to avoid JSON f64 loss.
6. `tower-lsp-server` (community fork) is the Phase 5 LSP crate.
7. Grammar revisions must be pinned even in generated packs.
8. Rust core remains the stack (project.md §10); no alternative substituted.

## Disagreements with the plan

None that change direction. One environment constraint is flagged below.

## Blockers

- **B1 — no Rust toolchain** (RESOLVED). The user later granted an explicit
  override to the no-install rule. Rust 1.100.0 was installed via rustup with
  `CARGO_HOME`/`RUSTUP_HOME` under the writable share; `cargo test --workspace`
  now passes, including a real T2 worker integration test. The Python reference
  harness remains as a fast proxy but is no longer the only verification.
- **B2 — no Node.js or Lua interpreter** (RESOLVED). Node 18.20.4, Lua 5.4.4 and
  lua-cjson were installed in the isolated Debian root. All three workers are
  exercised by `tools/t2_ref.py`, and Python+JS+Lua execution is verified by
  `meld run examples/06-bridge.meld`.
- **B3 — `project.md` not found on disk** (resolved by persisting prompt text).

## Phase 0 done-when assessment

Specs reviewed: yes (this file). Three example `.meld` files: yes. Therefore
Phase 0's stated criteria are met; B1/B2 affect Phases 1–3 verification only.

## Later specs added (Phases 4–7)

- `t3-component.md` — WIT as the IDL, committed Wasm Component, typed host
  calls, Python → T2 → T3 callback, and the T5 toolchain fallback.
- `lsp.md` — virtual documents, source maps, pack-owned editor metadata,
  completion forwarding, formatter delegation, and the scriptable REPL.
- `registry.md` — pack checksums, local HMAC signing, registry format, and the
  publish/list/install workflow that lets a third party add a language.
- `libraries.md` — installable Lang Packs: `library.toml`, git-backed
  `meld-pack`, installed index, and project `meld.lock`.
- `lang-pack.md` now documents `runner.toml`, `inline.toml`, `lsp.toml`,
  `formatter.toml`, copied queries, and the two accepted spellings of a pinned
  grammar rev.

Library verification was refreshed on 2026-11-20: `tower-lsp-server 0.23.0`
is still the newest stable release (`0.24.0-rc.1` is a prerelease), and the
locked versions in `Cargo.lock` are `tree-sitter 0.27.2`, `wasmtime 50.0.3`,
`tokio 1.54.1`, `serde_json 1.0.154`, and `ls-types 0.0.6`. No unverified
preview dependency was adopted.

Additional judgment calls (all also inline in the relevant spec/code):

9. WIT is reused directly; a single component result is typed `(i64,)` in
   Wasmtime; `meld:t3/*` is a reserved callback namespace.
10. The component binary is committed so tests do not need `wasm-tools`.
11. LSP virtual documents are padded to global line/column coordinates and
    pack `lsp.toml` supplies either keywords or an external stdio server.
12. `meld fmt` formats glue in core and delegates region bodies to the pack.
13. Pack signing uses local HMAC-SHA256; an exact crate version or
    `pinned:<ref>` both satisfy strict grammar pinning.
14. Phase 6 composition research is a NO-GO for SGLR/SDF3 and a GO for
    pack-owned scanner upgrades and raw delimiters.

Phase completion evidence is in `../status/phase-4.md` through
`../status/phase-7.md`.

## Language expansion (2026-11-20)

Fifteen strict Lang Packs were added without a core redesign: `typescript`,
`tsx`, `bash`, `ruby`, `php`, `yaml`, `toml`, `csharp`, `haskell`, `ocaml`,
`scala`, `zig`, `r`, `dart`, and `solidity`. crates.io was queried again before
selection; exact versions are recorded in `crates/meld-grammars/Cargo.toml`
and each `pack.toml`, and copied upstream queries are checked by
`tools/sync_grammars.py --check`.

Additional judgment calls (also inline in the affected files):

15. `tree-sitter-perl 1.1.2` was rejected because it keeps `tree-sitter 0.26`
    as a normal dependency, which Cargo refuses alongside the core's 0.27
    `links = "tree-sitter"` library. It is deferred, not claimed as supported.
16. TypeScript/TSX share one pinned crate and PHP/OCaml use their published
    implementation-grammar constants; pack ids are aliases over those choices.
17. `bash` is L1 (statements + stdout + structured errors) and deliberately not
    L2: the proven third-party shell worker has no value bridge.
18. The upstream Solidity 1.2.13 query contains a trailing `.` anchor rejected
    by tree-sitter 0.27 `Query`; `tools/sync_grammars.py` normalizes it in both
    copy and check modes rather than maintaining a divergent hand-written pack
    query.
19. New L0 fixtures use `##"... "##` raw delimiters; indentation-sensitive
    languages never rely on the generic brace scanner.

Evidence and verification results are in `../status/language-expansion.md`.

## Lang Libraries (2026-11-20)

`libraries.md` is normative for installable/importable Lang Packs. Every
checked-in pack and the third-party fixture now carries `library.toml`;
`tools/gen_pack.py` emits it for all future packs, and
`tools/make_libraries.py` refreshed existing packs and their checksums. The
terminal installer is `crates/meld-installer` (`meld-pack`), the shared format
and verification live in `crates/meld-lib`, and `meld` reads `meld.lock` for
project imports. No `project.md` decision was replaced.

Additional judgment calls (also inline in `libraries.md` and the code):

20. A Lang Library is a Lang Pack plus a manifest, package-manager style, not a
    compiled `cdylib`; downloading arbitrary grammar code stays outside the
    §9 trust model.
21. `exports` is a stable relative data surface; traversal and missing paths
    fail validation, but the whole verified pack is copied.
22. Installed libraries live under a shared root with canonical `index.json`,
    while `meld.lock` records each project's exact imported versions.
23. `meld-pack` uses the `git` CLI with a shallow fetch of a git tree, accepting
    GitHub slugs/URLs and local git repos through the same code path.
24. Missing imports are skipped at runtime, and imported libraries are
    discovered last so a project lock overrides a same-named built-in pack.

Dependency versions were re-checked against crates.io on 2026-11-20 before
implementation: `serde 1.0.229`, `serde_json 1.0.154`, and
`toml 1.2.1+spec-1.1.0` were current stable. `sha2 0.11.1` had become current
stable (the earlier workspace pin was `0.10.9`), so `meld-lib` was moved to the
new exact pin; it declares `rust-version 1.88`, compatible with this workspace's
`1.90`, and the RFC 4231 HMAC vector still passes. The checked-in
`Cargo.lock` records the resolved graph.

Evidence and verification results are in `../status/language-libraries.md`.
