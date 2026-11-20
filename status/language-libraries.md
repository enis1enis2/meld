# Lang Libraries and Terminal Installer — completion evidence (2026-11-20)

Authority: `../project.md` §4, §7, §9 and §11. The normative design is in
`../spec/libraries.md`; this file records what was implemented, the judgment
calls, and the verification performed. No decision from `project.md` was
replaced. Every judgment call is also inline in the affected spec/code file.

The signing layer was cut over from HMAC-SHA256 to Ed25519 in this worktree.
Historical HMAC-only evidence below is labelled as such; the current verification
table is authoritative for the code in this branch.

## Scope delivered

- `crates/meld-lib` — shared `library.toml` manifest, installed index, project
  lock, canonical JSON, safe paths, and `verify_pack` mirroring `packtool`,
  now with Ed25519 trust roots, revocations and strict signature parsing.
- `crates/meld-installer` — `meld-pack` terminal app with `install`, `remove`,
  `import`, `unimport`, `list`, `remote-list`, `verify`, `self-install` and
  `self-uninstall`. Remote installs require a verified Ed25519 signature.
- `crates/meld-cli` — imported-library discovery, `meld lib`, extended
  `meld doctor`, and runner rebasing so an imported worker wins over a
  same-named built-in worker.
- All 28 bundled packs plus `tests/fixtures/pack-third-party` carry
  `library.toml`; all 29 packs now carry `signature.ed25519` signed with the
  out-of-repo development/release key (`8ec9528bf1f0e31f`). Payloads and
  checksums were not regenerated.
- `packs/registry/` contains the 29-entry index, public trust roots, empty
  revocations, mirrors and a signed registry attestation. No private key or
  `meld-keys` directory is inside any repository.
- `tools/gen_pack.py` emits `library.toml` for every future pack.
  `tools/make_libraries.py` derives manifests/checksums for existing packs;
  signing with `packtool sign --private-key` remains an explicit step.
- Docs: `spec/registry.md`, `spec/libraries.md`, `spec/lang-pack.md`,
  `README.md`, `tests/fixtures/pack-third-party/README.md`, and the relevant
  status notes were updated for Ed25519. Parent-owned files were untouched.

## Judgment calls

1. **Library means data, not a shared object.** A Lang Library is a Lang Pack
   plus `library.toml`, in the package-manager sense of `project.md` §4/§11.
   Compiling arbitrary downloaded grammar code would violate the §9 trust
   model, so no `cdylib` is loaded at discovery time.
2. **`exports` is a stable data surface.** It must contain safe relative paths
   that exist, but the installer copies the whole verified pack.
3. **Shared install root, per-project lock.** `<root>/index.json` records one
   active version per id; `meld.lock` records the exact version a project uses.
   `install`/`remove` mutate bytes and import, while `import`/`unimport` edit
   only the lock.
4. **Canonical metadata with no timestamps.** Index, lock, checksums,
   signatures and registry metadata are byte-reproducible.
5. **Git CLI source acquisition.** `meld-pack` accepts `owner/repo`, GitHub
   URLs, `github:` slugs, and local git repositories, then uses `git init`,
   shallow `fetch` and `checkout FETCH_HEAD` in a temporary clone. Pack code is
   never executed.
6. **Verify twice.** Each library is verified in the checkout and again at its
   final destination. Identical existing installs are no-ops; changed bytes at
   the same coordinates require `--force` or a version bump.
7. **Runtime drift is non-fatal.** Missing imported directories are skipped and
   reported as `MISSING`; a malformed lock is a warning. Imported libraries are
   inserted after built-ins so a project lock wins, and runner paths are
   rebased pack-first.
8. **Ed25519 replaces HMAC.** Public trust roots let any verifier authenticate
   a pack without being able to forge one. Key ids are public-key hashes;
   revocation is an explicit key-id list; rotation is additive. Remote installs
   require `require_trusted` and read trust roots/revocations from the fetched
   tree unless pinned externally. `--key` remains only as an actionable removal
   error.

## Dependency re-verification

crates.io was queried on 2026-11-20 before relying on these libraries:

| crate | current stable | pin used | note |
|---|---|---|---|
| `serde` | 1.0.229 | `=1.0.229` | derive feature used by `meld-lib` |
| `serde_json` | 1.0.154 | `=1.0.154` | canonical JSON and index/lock parsing |
| `toml` | 1.2.1+spec-1.1.0 | `1.2` | resolves to the current stable release |
| `sha2` | 0.11.1 | `=0.11.1` | checksum hashing in `meld-lib` |
| `ed25519-dalek` | 3.0.0 | `=3.0.0` | `verify_strict`; `rust-version 1.85` |

`sha2 0.11.1` and `ed25519-dalek 3.0.0` fit the workspace's declared
`rust-version 1.90`. The wasmtime 50 tree already in `Cargo.lock` declares
`rust-version 1.97`, so full workspace tests were run with the available Rust
1.100 toolchain; the parent-owned `Cargo.toml` was not modified. Formatting was
limited to the touched crates with `cargo fmt -p meld-installer -p meld-cli`.

## Verification performed

| Check | Result |
|---|---|
| `python3 -m unittest discover -s tools/tests -v` | 10 tests passed |
| `cargo test -p meld-lib -p meld-installer` | 5 library + 4 installer CLI tests passed |
| `cargo test --locked -p meld-cli` (Rust 1.100) | 4 CLI tests passed |
| `packtool verify-registry packs/registry --verify-packs` | 29 packs verified, attestation verified |
| `packtool verify <pack> --trust-roots ... --revocations ...` loop | 29/29 `signature: verified` |
| `meld-pack verify packs/bash --trust-roots ... --revocations ...` | 14 files verified, signature verified |
| `python3 packs/bash/tests/run_conformance.py` | 5 cases OK |
| `meld check` + `meld run` for examples `01`–`10` | passed in earlier workspace runs; CLI test suite covers examples `08` |
| Sensitive-string scan | no private key bytes or PEM text in tracked files; key directory remains outside all repos |

Installer CLI coverage includes: tampered payload (stale checksums), tampered
checksums (stale signature), signer not in trust roots, revoked signer, missing
trust roots, malformed trust roots, missing revocations, install/verify/list/
remove round trip, and self-install/uninstall.

## Known limits

- Private-key custody is operational: the public trust root is in the repo, so
  adding a key to `trust-roots.toml` is a trust decision that must be reviewed.
  A compromised key must be added to `revocations.json` and the registry
  re-attested.
- Editing any registry metadata file requires re-running `packtool attest`;
  otherwise `verify-registry` reports an attestation mismatch by design.
- Installing another version of the same id replaces the index entry but leaves
  the old version directory on disk for explicit removal.
- `meld-pack remote-list` scans library manifests but does not verify every
  remote pack; verification happens for the libraries selected by `install`.

## Historical live GitHub smoke test (pre-cutover, HMAC)

Before the Ed25519 cutover, commit `245e591` was pushed to `origin/main` and
`meld-pack remote-list/install/verify/remove` were exercised against the public
repository with the then-current symmetric key. Those results validated the git
source path and lifecycle only; the symmetric-key assertions no longer describe
this branch. The current branch is verified with local git repositories and
Ed25519 keys as recorded above; no push was performed for this change.

## Follow-up: chrooted git temp-directory visibility

An SSH user reported `meld-pack remote-list` printing "no libraries found"
even though the public repository had 29 manifests. Reproduced with `TMPDIR`
unset: the sandbox `git` wrapper chroots into `/mnt/data/pkgmgr/debian-root`,
where the host's `/tmp` is not mounted. Git initialized and populated its own
`/tmp/meld-pack-...`, while the Rust process read the host's empty directory.

Fix:

- `meld-pack` uses `MELD_PACK_TMP` first, then `TMPDIR`, then
  `env::temp_dir()`, and verifies after `git checkout` that the clone's `.git`
  is visible to the Rust process.
- Installer CLI tests set `MELD_PACK_TMP` to a directory under `/mnt/data`, and
  all git invocations use explicit `-C` so the chrooted wrapper cannot
  accidentally initialize a repository at the chroot root.
