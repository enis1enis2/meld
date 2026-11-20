# Phase 7 status — ecosystem

## Done
- `tools/packtool.py` implements `verify`, `sign`, `publish`, `list`, `install`,
  `keygen`, `trust-roots`, `revocations`, `mirrors`, `attest` and
  `verify-registry` with canonical SHA-256 checksum files and Ed25519 public-key
  signatures. It never uses the network, installs toolchains, imports pack code,
  or spawns pack workers.
- `spec/registry.md` is normative. HMAC-SHA256 was removed; `signature.hmac`
  files are gone and every strict pack carries `signature.ed25519` over the
  exact message `scheme\nid\nversion\n<raw checksums.sha256 bytes>`.
- `packs/registry/` now ships a 29-entry `index.json` (28 bundled packs via
  `../<id>`, the third-party shell fixture via
  `../../tests/fixtures/pack-third-party`), `trust-roots.toml`,
  `revocations.json`, `mirrors.json` and a signed `attestation.ed25519`. The
  attestation binds all four metadata files; `../` paths are accepted only when
  it verifies.
- `tools/tests/test_packtool.py` has 10 unit/CLI tests: verification, tamper
  detection, Ed25519 signing, unknown/revoked keys, strict requirements,
  exact-version pins, id/alias resolution, publish/list/install, attestation
  binding, and fixture completeness. HMAC has no test except the `--key`
  removal error.
- Third-party fixture `tests/fixtures/pack-third-party/` is a complete `shell`
  pack with manifest, runner, toolchain, worker, end scanner, data-driven
  conformance cases, checksums and an Ed25519 signature.
- Runtime pack discovery reads `runner.toml` from `MELD_PACKS` roots, so
  `MELD_PACKS=tests/fixtures/pack-third-party meld run examples/08-shell-pack.meld`
  prints `hello from a third-party pack` with no Rust core change.
- All 28 built-in packs plus the fixture were re-signed with the out-of-repo
  development/release key (`key-id 8ec9528bf1f0e31f`). Payloads and checksums
  were not regenerated. The private key PEM stays in
  `/mnt/data/opencode/tmp/opencode/meld-keys/` (mode `0600`) and never enters a
  worktree, git index, or commit.
- `crates/meld-lib` verifies Ed25519 with `ed25519-dalek = "=3.0.0"`, loads
  trust roots and revocations, and reports `absent` /
  `present-unverified` / `verified`. `crates/meld-installer` requires verified
  signatures for remote installs and exposes `--trust-roots` /
  `--revocations` with `MELD_PACK_TRUST_ROOTS` / `MELD_PACK_REVOCATIONS` fallbacks.

## Verification
- `python3 -m unittest discover -s tools/tests -v`: 10 tests pass.
- `cargo test -p meld-lib -p meld-installer`: 5 library tests and 4 installer
  CLI tests pass (tampering, unknown key, revoked key, missing/malformed trust
  roots).
- `cargo test --locked -p meld-cli` with Rust 1.100 (the wasmtime 50 dependencies
  in the pre-existing lockfile need a newer compiler than the 1.90 wrapper):
  4 CLI tests pass, including the shell-pack and import-override tests.
- `tools/packtool.py verify-registry packs/registry --verify-packs`:
  29 packs verified, attestation verified.
- Loop `tools/packtool.py verify <pack> --trust-roots ... --revocations ...`:
  all 29 packs report `signature: verified`.

## Judgment calls
- Ed25519 replaced HMAC because a symmetric key cannot be a public trust root.
  Signatures commit to the exact checksum bytes, pack id and version; key ids
  are SHA-256 prefixes of raw public keys; revocation is checked first.
- Canonical checksums: lowercase hex, two spaces, POSIX-relative sorted paths,
  LF endings; caches and control files excluded; symlinks rejected.
- Checksums are always required; a signature is required only for strict packs.
  Both `pinned:<ref>` and exact crate versions count as pins because both are
  immutable and `sync_grammars.py` consumes the version form directly.
- Registry paths are relative; `../` is allowed only under a valid attestation.
  The index, trust roots, revocations and mirrors are canonical and timestamp
  free, and republishing identical bytes is idempotent.
- Exact ids win over aliases; otherwise an alias collision is an error. This is
  required because the registry ships both `bash` (alias `shell`) and the
  third-party `shell` pack.
- Remote installs require trust roots from the fetched tree by default; local
  checks may report `present-unverified` but never call it authentic.
- Install is a non-overwriting data copy and validates index metadata against
  the staged pack; no network, toolchain install, or pack-code execution.

## Blockers
None for the Ed25519 cutover. Note for CI owners: the pinned `wasmtime 50.0.3`
tree in the pre-existing lockfile declares `rust-version 1.97`; the 1.90 cargo
wrapper cannot compile it. This is unrelated to the signing change and the
parent-owned `Cargo.toml`/CI files were intentionally left untouched.

## Done-when assessment
"A third party adds a language without touching core": satisfied by the shell
pack fixture, which is discovered and executed through data-only pack metadata,
is listed in the attested registry, and can be installed with an Ed25519
verified signature.
