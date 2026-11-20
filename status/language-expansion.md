# Language Expansion — completion evidence (2026-11-20)

Authority: `../project.md` §1, §4, §5, §8, §9, §11. The expansion follows the
existing Lang Pack contract; no core design decision from `project.md` was
replaced. Every judgment call is also inline in the file where it applies.

## Scope

Added 15 strict, pinned, signed packs. Fourteen are L0 syntax packs; `bash` is
an executable T2 L1 pack reusing the protocol worker proven by the Phase 7
third-party shell fixture. Bundled grammars grew from 13 to 28, satisfying the
"cheap route to a long tail" in `project.md` §4 without changing the core.

| Pack | Tier/level | Crate (exact pin) | Aliases |
|---|---|---|---|
| `typescript` | L0 | `tree-sitter-typescript 0.23.2` | `ts` |
| `tsx` | L0 | `tree-sitter-typescript 0.23.2` | `react-tsx` |
| `bash` | T2/L1 | `tree-sitter-bash 0.25.1` | `sh`, `shell`, `posix-sh` |
| `ruby` | L0 | `tree-sitter-ruby 0.23.1` | `rb` |
| `php` | L0 | `tree-sitter-php 0.25.2` | — |
| `yaml` | L0 | `tree-sitter-yaml 0.7.2` | `yml` |
| `toml` | L0 | `tree-sitter-toml-ng 0.7.0` | — |
| `csharp` | L0 | `tree-sitter-c-sharp 0.24.0` | `cs`, `dotnet-csharp` |
| `haskell` | L0 | `tree-sitter-haskell 0.24.1` | `hs` |
| `ocaml` | L0 | `tree-sitter-ocaml 0.26.0` | `ml` |
| `scala` | L0 | `tree-sitter-scala 0.26.13` | `sc` |
| `zig` | L0 | `tree-sitter-zig 1.1.2` | — |
| `r` | L0 | `tree-sitter-r 1.3.0` | `rscript` |
| `dart` | L0 | `tree-sitter-dart 0.3.0` | — |
| `solidity` | L0 | `tree-sitter-solidity 1.2.13` | `sol` |

## Library verification (before selection)

crates.io API was queried on 2026-11-20 for newest stable versions and each
version's dependency kind. All selected grammars expose a `LanguageFn`
constant and keep `tree-sitter` as a dev-dependency only, so Cargo resolves a
single `tree-sitter 0.27.2` native library as required by `meld-core`.

`tree-sitter-perl 1.1.2` was evaluated and rejected: its `tree-sitter ^0.26.3`
dependency is *normal*, producing a `links = "tree-sitter"` conflict with the
core. This is recorded inline in `crates/meld-grammars/Cargo.toml` and in
`spec/README.md` judgment call 15.

## Judgment calls

1. `tools/gen_pack.py --strict` was added so reviewed packs carry exact pins,
   aliases/extensions, LSP fallback keywords, and raw-delimiter feeder tests.
   The legacy placeholder mode is unchanged.
2. Multi-grammar crates are reused for TSX, PHP, and OCaml rather than adding
   duplicate dependencies; the chosen constant is recorded in
   `crates/meld-grammars/src/lib.rs`.
3. The `bash` pack declares L1, not L2, because its worker implements
   `initialize`/`execute`/`shutdown` but no value bridge. It runs through `sh`
   so the `sh`/`shell`/`posix-sh` aliases are honest.
4. All new L0 fixtures and `examples/10-more-languages.meld` use
   `@lang##"..."##` raw delimiters. The generic brace scanner cannot see
   indentation-based region ends, and Phase 6 explicitly favored raw
   delimiters (`status/phase-6.md`).
5. `tree-sitter-solidity 1.2.13` ships a highlight query with a trailing `.`
   anchor that tree-sitter 0.27 rejects. `tools/sync_grammars.py` normalizes
   exactly that pattern in both copy and `--check` modes; no pack query is
   hand-maintained.
6. `meld doctor` now reports `sh` because the built-in Bash pack needs it.
   Nothing is auto-installed (`project.md` §9).
7. New grammar crates need a C compiler at build time. The isolated Debian
   root already provides `cc`; no package was installed.

## Verification

- `cargo test --workspace`: pass (23 tests, including every bundled grammar
  parsing a real snippet and every published query compiling).
- `python3 -m unittest discover -s tools/tests -v`: 8 tests pass.
- `python3 tools/fuzz_splitter.py`: 1000 cases pass.
- `python3 tools/meld_check_ref.py`: 0 problems across 11 examples.
- `python3 tools/highlight_ref.py`: 15 mixed-language spans pass.
- `python3 tools/t2_ref.py`: 0 problems (Node/Lua unavailable in this shell;
  Python L2 matrix passes).
- `CARGO_HOME=… python3 tools/sync_grammars.py --check`: 0 stale query files.
- `packtool verify … --trust-roots packs/registry/trust-roots.toml`: all 28
  built-in packs and the third-party fixture report `signature: verified` with
  Ed25519 key-id `8ec9528bf1f0e31f`. Payloads/versions in this table are
  unchanged; only the signature format was replaced.
- `packs/bash/tests/run_conformance.py`: initialize + 4 cases pass.
- `meld check examples/10-more-languages.meld`: 32 regions, 0 errors.
- `meld run examples/10-more-languages.meld`: prints `bash-ok`.
- Examples `01`–`10` pass under `drun`; `08` still uses
  `MELD_PACKS=tests/fixtures/pack-third-party`.

The Ed25519 private key remains outside every repository and worktree; only its
public key is checked in as a trust root. The GitHub token was never written
into a tracked file.
