# Meld

<p align="center">
  <img src="docs/assets/logo.svg" alt="Meld mark" width="96" />
</p>

> One file. Many languages. Typed values.

![Meld social preview](docs/assets/social-preview.svg)

[![CI](https://github.com/enis1enis2/meld/actions/workflows/ci.yml/badge.svg)](https://github.com/enis1enis2/meld/actions/workflows/ci.yml)
[![Docs](https://github.com/enis1enis2/meld/actions/workflows/docs.yml/badge.svg)](https://github.com/enis1enis2/meld/actions/workflows/docs.yml)
[![MSRV](https://github.com/enis1enis2/meld/actions/workflows/msrv.yml/badge.svg)](https://github.com/enis1enis2/meld/actions/workflows/msrv.yml)
[![Supply-chain audit](https://github.com/enis1enis2/meld/actions/workflows/audit.yml/badge.svg)](https://github.com/enis1enis2/meld/actions/workflows/audit.yml)
[![Fuzz](https://github.com/enis1enis2/meld/actions/workflows/fuzz.yml/badge.svg)](https://github.com/enis1enis2/meld/actions/workflows/fuzz.yml)

Meld is a language/toolchain where one `.meld` file can switch language
mid-file and pass typed values between regions. Coverage is open-ended: adding
a language costs a Lang Pack (manifest, grammar, runner), not a core change.

**Docs:** [SemVer](SEMVER.md) · [SBOM](SBOM.md) · [Security policy](SECURITY.md) ·
[Lang Pack spec](spec/lang-pack.md) · [Lang Library spec](spec/libraries.md) ·
[book](docs/src/SUMMARY.md) · [schema stability](docs/schema-stability.md) ·
[release artifacts](docs/release-artifacts.md)

**Community:** [Code of Conduct](CODE_OF_CONDUCT.md) ·
[Contributing](CONTRIBUTING.md) · [Support](SUPPORT.md) ·
[Governance](GOVERNANCE.md) · [Issue templates](.github/ISSUE_TEMPLATE)

## Install

The system package name is `meld-lang`; it provides three binaries: `meld`,
`meld-lsp` and `meld-pack`. Signed release archives are published on the
[releases page](https://github.com/enis1enis2/meld/releases) for stable
`vMAJOR.MINOR.PATCH` tags (the current stable target is `v0.2.0`); pre-release
tags are rejected by the release workflow. Archives can be installed with
`install.sh` (Linux/macOS) or `install.ps1` (Windows). Both installers verify
`SHA256SUMS` and, when `cosign` is available, the Sigstore signature. They
detect missing language runtimes and install them by default; pass
`--skip-dependencies` or `-SkipDependencies` to opt out.

### Building from source

```sh
cargo build --workspace          # dev profile, unoptimized (fast iteration)
cargo rbuild                     # optimized release profile, binaries in target/release/
```

`rbuild` is a cargo alias for `cargo build --workspace --release --locked` and
uses the tuned `[profile.release]` settings in the root `Cargo.toml`. On Windows
the optimized binary is `target\release\meld.exe`; on Linux and macOS it is
`target/release/meld`. Other aliases: `rtest`, `rcheck` and `rclippy`.

The source build places the binaries under `./target/debug/` (or
`./target/release/` with `rbuild`). Commands below use the dev path; substitute
the release path or `meld`, `meld-lsp` and `meld-pack` from `PATH` when a
packaged installation is available.

## Quick start

```sh
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

Every Lang Pack is also an installable Lang Library. Browse and install packs
from GitHub with the terminal installer:

```sh
cargo build -p meld-installer
./target/debug/meld-pack remote-list
./target/debug/meld-pack install bash typescript
./target/debug/meld-pack list
./target/debug/meld lib
```

`install` verifies the pack data against public Ed25519 trust roots and writes
the exact version to `meld.lock`; `remove`, `import`, `unimport`, `verify`,
`self-install` and `self-uninstall` are also available. Remote installs read
`packs/registry/trust-roots.toml` and `revocations.json` from the fetched tree
by default, so an unsigned or unknown-key pack is rejected. See
[spec/libraries.md](spec/libraries.md) for the full workflow.

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

See [spec/syntax.md](spec/syntax.md) for block, inline, fence, and raw-delimiter
forms, and [examples/README.md](examples/README.md) for runnable examples.

## Layout

- `spec/` — normative specs (syntax, value model, Lang Pack, T2 protocol, T3,
  LSP/tooling, registry, Lang Libraries).
- `crates/` — Rust core, grammars, T2/T3 backends, CLI, LSP server, Lang Library
  format (`meld-lib`) and terminal installer (`meld-installer`).
- `packs/` — 28 Lang Packs / Lang Libraries plus `packs/registry/` metadata
  (index, public trust roots, revocations, mirrors and a signed attestation).
  `python`, `javascript`, `lua` are L2 execution packs; `bash` is L1; the other
  24 are strict L0 syntax packs.
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
`tools/gen_pack.py --strict`; `bash` additionally ships a T2 L1 worker. All 28
packs have a `library.toml`, so any of them can be installed with `meld-pack`.
See [status/language-expansion.md](status/language-expansion.md) for versions and
judgment calls, and [status/language-libraries.md](status/language-libraries.md)
for installer evidence.

## Verification

Install the Python test dependency first. CI requests a floating Python `3.12`
patch because an exact patch such as `3.12.9` can be absent from runner images;
`requirements-ci.txt` pins the only Python dependency, `cryptography==50.0.2`.

```sh
python -m pip install -r requirements-ci.txt
cargo test --workspace
python3 -m unittest discover -s tools/tests -v
python3 tools/fuzz_splitter.py
python3 tools/meld_check_ref.py
python3 tools/highlight_ref.py
python3 tools/t2_ref.py
python3 tools/sync_grammars.py --check
./target/debug/meld-pack verify packs/bash
./target/debug/meld-pack verify packs/bash \
  --trust-roots packs/registry/trust-roots.toml \
  --revocations packs/registry/revocations.json
python3 tools/packtool.py verify-registry packs/registry \
  --trust-roots packs/registry/trust-roots.toml \
  --revocations packs/registry/revocations.json --verify-packs
python3 tools/packtool.py verify packs/bash \
  --trust-roots packs/registry/trust-roots.toml \
  --revocations packs/registry/revocations.json
./target/debug/meld-pack list
```

`meld-pack verify` without trust roots checks checksums and signature shape;
strict packs always require `signature.ed25519`, and passing the public trust
roots proves the Ed25519 signature. The private key is kept outside the repo
and is never used for verification. Set `MELD_LIB_ROOT` to isolate installed
libraries in tests.

Toolchains: Rust (with `wasm32-unknown-unknown` for regenerating the T3
component and a C compiler for building the expanded grammar set), Python
3.11+, Node.js, Lua, and `sh` for the built-in Bash pack. `meld doctor` reports
what is present; it never installs anything.

See [status/production-readiness.md](status/production-readiness.md) for the
final evidence snapshot, and [status/phase-1.md](status/phase-1.md) through
[status/phase-7.md](status/phase-7.md),
[status/language-expansion.md](status/language-expansion.md), and
[status/language-libraries.md](status/language-libraries.md) for implementation
details and judgment calls.
