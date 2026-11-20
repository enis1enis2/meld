# Phase 1 status

> **Historical phase record.** Toolchain details below reflect the Phase 1/2
> environment at the time. Current authority is judgment call 26 (Ed25519 pack
> trust), judgment call 28 (MSRV `1.90`, Wasmtime `=36.0.23`) and judgment call
> 45 (floating Python `3.12`) in `../spec/README.md`, with final evidence in
> [production-readiness.md](production-readiness.md).

## Done
- `crates/meld-core`: splitter, end-scanner trait, syntax layer (tree-sitter included ranges), source maps, binding resolver, DAG plan, scheduler skeleton.
- `crates/meld-cli`: `meld check`, `meld run`, `meld doctor`, `meld pack new`.
- `tools/fuzz_splitter.py`: 1000 deterministic fuzz cases — **PASS**.
- `crates/meld-core/tests/splitter_fuzz.rs`: same suite authored in Rust.
- `tools/meld_check_ref.py`: check reference run over all examples — 0 problems.
- `tools/gen_pack.py`: generated 10 L0 syntax-only packs (rust, go, c, cpp, java, json, markdown, sql, html, css).
- `tools/highlight_ref.py`: mixed-file highlight proxy over those 10 packs — **PASS**.

## Judgment calls
- Raw hash delimiters available in Phase 1; generic balanced-brace scanner is the default fallback.
- Auto-detect confidence threshold 0.75, surfaced by `meld check`.
- Generated grammar revs are explicit `pinned:<lang>-v1` placeholders; `strict` mode will reject them until replaced with real commit hashes.
- Python reference harness stood in for `cargo test` while B1 applied (historical).

## Blockers (historical Phase 1 state)
- B1 (no Rust toolchain): `cargo test` and real tree-sitter parsing were not executed. The fuzz/highlight results were proxies, not substitutes.
- B2 (no Node/Lua): execution tiers for JS/Lua were not exercised.

## Resolution (historical, added during Phase 2)
Both blockers were cleared after the user explicitly overrode the no-install
rule. The then-available Rust 1.100.0, Node 18.20.4, Lua 5.4.4 and lua-cjson
were installed; the Rust splitter fuzz suite then passed natively
(`cargo test --workspace`). Fixing the compile also uncovered two real bugs (a
non-ASCII byte literal and a move-after-use in the binding resolver); both were
corrected. **Historical:** Rust 1.100.0 was a verification toolchain, not the
declared MSRV; current policy is MSRV `1.90` with Wasmtime `=36.0.23` (judgment
call 28), with final evidence in [production-readiness.md](production-readiness.md).

## Done-when assessment (historical Phase 1)
"Splitter fuzz suite passes; highlights mixed file correctly": satisfied in the
reference harness at that time; Rust-native verification was still blocked by
B1. Proceeding to Phase 2 with the blocker flagged. Final evidence is in
[production-readiness.md](production-readiness.md).
