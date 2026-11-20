# Meld

Meld is a language/toolchain where one `.meld` file can switch language
mid-file and pass typed values between regions. Coverage is open-ended: adding
a language costs a Lang Pack (manifest, grammar, runner), not a core change.

## Quick start

```sh
cargo build --workspace
./target/debug/meld check examples/09-smoke.meld
./target/debug/meld run examples/09-smoke.meld
```

The smoke example prints:

```text
inline sum: 5
raw: ok
fence: ok
js: 20,40,60
lua: 3 values
```

Run the third-party pack example with:

```sh
MELD_PACKS=tests/fixtures/pack-third-party \
  ./target/debug/meld run examples/08-shell-pack.meld
```

The language-expansion example parses 15 additional languages and executes the
Bash region:

```sh
./target/debug/meld check examples/10-more-languages.meld
./target/debug/meld run examples/10-more-languages.meld
# prints: bash-ok
```

## Syntax at a glance

```meld
#meld python

@python as producer {
    values = [1, 2, 3]
    export values as nums
}

@javascript as consumer {
    import nums from producer
    globalThis.doubled = nums.map(x => x * 2);
    console.log(globalThis.doubled.join(","));
}

print(⟦python: 2 + 3⟧)
```

See `spec/syntax.md` for block, inline, fence, and raw-delimiter forms.

## Layout

- `spec/` — normative specs (syntax, value model, Lang Pack, T2 protocol, T3,
  LSP/tooling, registry).
- `crates/` — Rust core, grammars, T2/T3 backends, CLI, and LSP server.
- `packs/` — 28 Lang Packs. `python`, `javascript`, `lua` are L2 execution
  packs; `bash` is L1; the other 24 are strict L0 syntax packs.
- `tools/` — grammar sync, pack tooling, reference harnesses.
- `tests/` — fixtures and conformance data.
- `research/composition/` — Phase 6 go/no-go study.
- `status/` — phase-by-phase completion notes.

## Languages

Bundled packs: `bash`, `c`, `cpp`, `csharp`, `css`, `dart`, `go`, `haskell`,
`html`, `java`, `javascript`, `json`, `lua`, `markdown`, `ocaml`, `php`,
`python`, `r`, `ruby`, `rust`, `scala`, `solidity`, `sql`, `toml`, `tsx`,
`typescript`, `yaml`, `zig`.

All 15 packs added in the 2026-11-20 expansion (`typescript`, `tsx`, `bash`,
`ruby`, `php`, `yaml`, `toml`, `csharp`, `haskell`, `ocaml`, `scala`, `zig`,
`r`, `dart`, `solidity`) are strict, pinned, signed, and generated through
`tools/gen_pack.py --strict`; `bash` additionally ships a T2 L1 worker. See
`status/language-expansion.md` for versions and judgment calls.

## Verification

```sh
cargo test --workspace
python3 -m unittest discover -s tools/tests -v
python3 tools/fuzz_splitter.py
python3 tools/meld_check_ref.py
python3 tools/highlight_ref.py
python3 tools/t2_ref.py
python3 tools/sync_grammars.py --check
```

Toolchains: Rust (with `wasm32-unknown-unknown` for regenerating the T3
component and a C compiler for building the expanded grammar set), Python
3.11+, Node.js, Lua, and `sh` for the built-in Bash pack. `meld doctor` reports
what is present; it never installs anything.

See `status/phase-1.md` through `status/phase-7.md` and
`status/language-expansion.md` for implementation details and judgment calls.
