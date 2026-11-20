# Phase 7 status — ecosystem

## Done
- `tools/packtool.py` implements `verify`, `sign`, `publish`, `list` and
  `install` with canonical SHA-256 checksum files and local HMAC-SHA256 signing
  over the checksum file. It never uses the network, installs toolchains,
  imports pack code, or spawns pack workers.
- `spec/registry.md` is normative; `packs/registry/index.json` is intentionally
  empty and schema-valid. Staged copies are created only by `publish`.
- `tools/tests/test_packtool.py` has 8 unit/CLI tests: verification, tamper
  detection, signing, strict requirements, exact-version pins, publish/list/
  install round-trip, unsigned packs, index tamper, and fixture completeness.
- Third-party fixture `tests/fixtures/pack-third-party/` is a complete `shell`
  pack with manifest, runner, toolchain, worker, end scanner, datadriven
  conformance cases, checksums and signature.
- Runtime pack discovery reads `runner.toml` from `MELD_PACKS` roots, so
  `MELD_PACKS=tests/fixtures/pack-third-party meld run examples/08-shell-pack.meld`
  prints `hello from a third-party pack` with no Rust core change.
- All strict built-in packs have regenerated `checksums.sha256` and
  `signature.hmac` files after `inline.toml` / `lsp.toml` were added. Their
  exact crate versions are accepted as immutable grammar pins.
- A local development HMAC key is kept outside the pack tree and outside the
  repository; key holders can run `packtool verify --key <path>` for a MAC
  check, while ordinary `verify` checks payload integrity and signature shape.

## Verification
- `python3 -m unittest discover -s tools/tests -v`: 8 tests pass.
- `packtool verify --key <dev-key>` passes for every strict pack and the shell
  fixture; publish/list/install round-trip completes with a verified keyed
  install.
- `cargo test -p meld-cli` includes the third-party shell pack end-to-end test.

## Judgment calls
- HMAC-SHA256 with an out-of-band shared key, not public-key signing; key-id is
  the first 16 hex chars of SHA-256(key), and a keyed verify uses
  `hmac.compare_digest`.
- Canonical checksums: lowercase hex, two spaces, POSIX-relative sorted paths,
  LF endings; caches and control files excluded; symlinks rejected.
- Checksums are always required; a signature is required only for strict packs.
  Both `pinned:<ref>` and exact crate versions count as pins because both are
  immutable and `sync_grammars.py` consumes the version form directly.
- Registry paths are relative, the index is canonical JSON with no timestamps,
  and republishing identical bytes is idempotent; changed bytes require a new
  version.
- Install is a non-overwriting data copy and validates index metadata against
  the staged pack; alias collisions are errors rather than guesses.

## Blockers
None.

## Done-when assessment
"A third party adds a language without touching core": satisfied by the shell
pack fixture, which is discovered and executed through data-only pack metadata
and is publishable/installable through `packtool`.
