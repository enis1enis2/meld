# Lang Libraries and Terminal Installer — completion evidence (2026-11-20)

Authority: `../project.md` §4, §7, §9 and §11. The normative design is in
`../spec/libraries.md`; this file records what was implemented, the judgment
calls, and the verification performed. No decision from `project.md` was
replaced. Every judgment call is also inline in the affected spec/code file.

## Scope delivered

- `crates/meld-lib` — shared `library.toml` manifest, installed index, project
  lock, canonical JSON, safe paths, and `verify_pack` mirroring `packtool`.
- `crates/meld-installer` — `meld-pack` terminal app with `install`, `remove`,
  `import`, `unimport`, `list`, `remote-list`, `verify`, `self-install` and
  `self-uninstall`.
- `crates/meld-cli` — imported-library discovery, `meld lib`, extended
  `meld doctor`, and runner rebasing so an imported worker wins over a
  same-named built-in worker.
- All 28 bundled packs plus `tests/fixtures/pack-third-party` carry
  `library.toml`; missing `version` fields were added, checksums regenerated,
  and all 29 packs signed with the out-of-repo development HMAC key.
- `tools/gen_pack.py` emits `library.toml` for every future pack.
  `tools/make_libraries.py` derives manifests/checksums for existing packs and
  deliberately leaves signing as an explicit step with an external key.
- Docs: new `spec/libraries.md`, updates to `spec/README.md`,
  `spec/lang-pack.md`, `spec/registry.md`, and the root `README.md`.

## Judgment calls

1. **Library means data, not a shared object.** A Lang Library is a Lang Pack
   plus `library.toml`, in the package-manager sense of `project.md` §4/§11.
   Compiling arbitrary downloaded grammar code would violate the §9 trust
   model, so no `cdylib` is loaded at discovery time.
2. **`exports` is a stable data surface.** It must contain safe relative paths
   that exist, but the installer copies the whole verified pack. This keeps the
   integration promise small without adding a partial-copy failure mode.
3. **Shared install root, per-project lock.** `<root>/index.json` records one
   active version per id; `meld.lock` records the exact version a project uses.
   `install`/`remove` mutate bytes and import, while `import`/`unimport` edit
   only the lock.
4. **Canonical metadata with no timestamps.** Index, lock, checksums and
   signatures are byte-reproducible; repeated identical installs produce no
   diff beyond intended state.
5. **Git CLI source acquisition.** `meld-pack` accepts `owner/repo`, GitHub
   URLs, `github:` slugs, and local git repositories, then uses `git init`,
   shallow `fetch` and `checkout FETCH_HEAD` in a temporary clone. Git is already
   a development dependency and understands credentials; pack code is never
   executed.
6. **Verify twice.** Each library is verified in the checkout and again at its
   final destination. Identical existing installs are no-ops; changed bytes at
   the same coordinates require `--force` or a version bump.
7. **Runtime drift is non-fatal.** Missing imported directories are skipped and
   reported as `MISSING`; a malformed lock is a warning. Imported libraries are
   inserted after built-ins so a project lock wins, and runner paths are
   rebased pack-first.
8. **HMAC remains local and symmetric.** Keys are at least 16 bytes,
   distributed out of band, and never stored in a pack or repository. Strict
   packs always require a signature file; keyed verification is optional but
   checked against a key-id first.

## Dependency re-verification

crates.io was queried again on 2026-11-20 before relying on these libraries:

| crate | current stable | pin used | note |
|---|---|---|---|
| `serde` | 1.0.229 | `=1.0.229` | derive feature used by `meld-lib` |
| `serde_json` | 1.0.154 | `=1.0.154` | canonical JSON and index/lock parsing |
| `toml` | 1.2.1+spec-1.1.0 | `1.2` | resolves to the current stable release |
| `sha2` | 0.11.1 | `=0.11.1` | upgraded from the earlier `0.10.9` pin |

`sha2 0.11.1` declares `rust-version 1.88`, which fits the workspace's
`rust-version 1.90`. Its only use in `meld-lib` is `Sha256::digest`, and the
RFC 4231 HMAC vector test still passes after the upgrade. `sha2 0.10.9` remains
elsewhere in `Cargo.lock` as a transitive dependency of existing crates;
`meld-lib` no longer uses it.

The user override allowed installing the `rustfmt` component so
`cargo fmt --all` could normalize the new Rust code. Formatting-only changes to
unrelated crates were reverted before staging.

## Verification performed

| Check | Result |
|---|---|
| `cargo test --workspace` | 28 tests passed, 0 failed |
| `python3 -m unittest discover -s tools/tests -v` | 8 tests passed |
| `python3 tools/fuzz_splitter.py` | PASS (1000 cases) |
| `python3 tools/meld_check_ref.py` | 0 problems |
| `python3 tools/highlight_ref.py` | PASS (15 mixed-language spans) |
| `python3 tools/t2_ref.py` | 0 problems |
| `CARGO_HOME=... python3 tools/sync_grammars.py --check` | 0 stale query files |
| Keyed `packtool verify` over all packs + fixture | 29 packs OK |
| `meld-pack verify packs/bash --key <dev-key>` | 14 files verified, signature verified |
| `python3 packs/bash/tests/run_conformance.py` | 5 cases OK |
| `meld check` + `meld run` for `examples/01`–`10` | all 10 passed |
| Installer CLI test | local git install/verify/list/remove/self-install/self-uninstall passed |
| CLI test `imported_library_overrides_a_same_named_builtin_pack` | passed |
| Sensitive-string scan | no private key bytes or key filename in the tree; key-id appears only in expected signature/status files |

The `sync_grammars.py` check must be run with the isolated `CARGO_HOME`
(`/mnt/data/supervisor/share/.cargo-home`) because the tool defaults to a
repository-local Cargo cache; with the correct environment it reports zero
stale queries.

## Known limits

- HMAC is symmetric, so key holders can forge signatures. Public-key signing
  and key distribution remain future work, as recorded in `spec/registry.md`.
- Installing another version of the same id replaces the index entry but leaves
  the old version directory on disk for explicit removal.
- `meld-pack remote-list` scans library manifests but does not verify every
  remote pack; verification happens for the libraries selected by `install`.

## Live GitHub smoke test

After pushing commit `245e591` to `origin/main`:

- `meld-pack remote-list --source enis1enis2/meld --ref main` listed 29
  libraries (28 bundled packs plus the `shell` fixture), including id/version,
  tier/level and display name.
- `meld-pack install bash ts ... --root <temp>/libs --project <temp>/project`
  fetched the public repository, verified and installed both libraries, and
  wrote both ids to `meld.lock`. `ts` resolved through the TypeScript alias.
- `meld-pack list --root <temp>/libs` reported both installs as `ok` with source
  `github:enis1enis2/meld@main`; `MELD_ROOT=<temp>/project MELD_LIB_ROOT=<temp>/libs meld lib` showed the same installed/imported split.
- `meld-pack verify <temp>/libs/typescript/0.1.0 --key <out-of-repo dev key>`
  verified 8 files and reported `signature: verified (key-id 32e400239df0f52a)`.
  The keyed check was local; the public install used checksum plus signature
  presence, since the symmetric development key is intentionally not published.
- `meld-pack remove sh ...` resolved the `sh` alias to `bash`, removed its files
  and lock entry, and the follow-up `list` showed only `typescript` remaining.

All live smoke-test state was kept under
`/mnt/data/opencode/tmp/opencode/meld-github-smoke`, outside the repository.
