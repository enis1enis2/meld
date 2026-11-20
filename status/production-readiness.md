# Production readiness evidence

This is the living evidence file for the production-readiness workstream. It is
updated after each workstream with commands, observed output, and links to CI
runs. It is not a claim of completion until every checklist item is marked
`DONE` with evidence.

Authority: `../project.md` design decisions remain authoritative. Changes that
improve production properties but amend implementation judgment calls are
recorded in `../spec/README.md` and inline in the affected spec/code.

## 0. Baseline (before production work)

Command: `git status --short --branch` on `main`

```text
## main...origin/main
```

Command: `cargo test --workspace`

```text
test result: ok. 28 passed; 0 failed
```

Additional baseline checks:

| Command | Result |
|---|---|
| `python3 -m unittest discover -s tools/tests -v` | 8 passed |
| `python3 tools/fuzz_splitter.py` | PASS (1000 cases) |
| `python3 tools/meld_check_ref.py` | 0 problems |
| `python3 tools/highlight_ref.py` | PASS (15 mixed-language spans) |
| `python3 tools/t2_ref.py` | 0 problems |
| `python3 tools/sync_grammars.py --check` | 0 stale query files |
| keyed `packtool verify` over 28 packs + fixture | 29 OK |
| `meld-pack verify packs/bash --key <dev-key>` | 14 files verified, signature verified |
| examples `01`–`10` check/run | all pass |

Baseline limitations recorded before this workstream are in
`language-libraries.md`, `phase-6.md`, and the production-readiness section of
the project report delivered on 2026-11-20.

## 1. Release hygiene / baseline gates

- [x] Correct stale `project.md` status line.
- [x] Add `LICENSE-MIT`.
- [x] Add `LICENSE-APACHE`.
- [x] Add `NOTICE`.
- [x] Add `deny.toml` with explicit license/source policy.
- [x] Add `.github/workflows/ci.yml` for Linux/macOS/Windows, stable + MSRV,
      fmt, clippy, tests, docs, Python tests, fuzz smoke, pack verification and
      supply-chain audits.
- [x] Make `cargo clippy --workspace --all-targets -- -D warnings` pass locally
      (three pre-existing warnings fixed in `meld-core`, `meld-installer`,
      `meld-cli`, and `meld-core/tests`).

Local clippy result after fixes:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s)
```

CI run links for the final branch are recorded in §1.2 below.

### 1.1 Production reruns (interim evidence)

The following commands were rerun in the current production worktree before
merging the hardening branches. These are real, local results and are not a
claim that the final gate has passed.

| Command | Observed result |
|---|---|
| `cargo fmt --all -- --check` | PASS after `a573999` applied rustfmt across the workspace. |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS in the current environment. |
| `cargo install cargo-audit cargo-deny --locked` | PASS: `cargo-audit 0.23.0` and `cargo-deny 0.22.0` installed. |
| `cargo audit` | PASS after restoring DNS for the advisory database: loaded 1550 advisories, scanned 250 crate dependencies, no findings, exit 0. |
| `cargo deny check` | `advisories ok, bans FAILED, licenses ok, sources ok`. The failure is from `wildcards = "deny"` treating private workspace path dependencies as wildcards. `deny.toml` now sets `allow-wildcard-paths = true`; a final rerun is still required after workspace crates are confirmed private. |
| `cargo test --workspace` | Initial rerun exposed local tool wrappers missing `cc`/`ar`, `node`, and a chroot-visible `TMPDIR`. After adding those wrappers, CLI tests pass; the installer integration test is being rerun with `TMPDIR` under `/mnt/data`. Final result pending. |

A local compiler wrapper was required because this environment has no native
linker on `PATH`: `/mnt/data/opencode/bin/cc` and `ar` invoke the isolated
Debian chroot toolchain while preserving the caller's working directory. Node
and Lua wrappers were added for the same reason. These wrappers are host
configuration only and are not committed to the repository.

The installer integration test initially failed because the host filesystem
was full (`No space left on device` during `git add`), not because of Meld
logic. After `cargo clean` freed 14.3 GiB and the test used a chroot-visible
`TMPDIR`, `cargo test -p meld-installer --test cli` passed with
`1 passed; 0 failed`.

Release engineering assets from `production/release-eng` were merged. Local
re-verification in this worktree passed:

| Command | Result |
|---|---|
| `sh -n` on `install.sh`, `packaging/deb/rules`, `scripts/*.sh` | PASS |
| `python3 -m py_compile` on new Python tooling | PASS |
| JSON parse of `bucket/meld-lang.json`, `grammar-sources.json` | PASS |
| `cargo metadata --manifest-path fuzz/Cargo.toml --format-version 1 --locked` | PASS (targets: `splitter`, `value_decode`) |
| `python3 tools/check_grammar_sources.py` | PASS (27 pinned grammars) |
| `python3 tools/generate_third_party_notices.py` | PASS (notices current) |

Ed25519 and scanner integration were then merged and re-verified in the parent
worktree:

| Command | Observed result |
|---|---|
| `python3 -m unittest discover -s tools/tests -v` | PASS: 10 tests. |
| `python3 tools/packtool.py verify-registry ... --verify-packs` | PASS: 29 packs verified; attestation key-id `8ec9528bf1f0e31f`; `OK`. |
| keyed `verify` over 29 packs | PASS: `verified strict packs: 29`. |
| `python3 tools/make_libraries.py` + re-sign | PASS: all 29 libraries refreshed with `region_end.toml`, checksummed, and signed outside the repository. |
| `git grep` for HMAC/private-key material | PASS: no `signature.hmac`, `hmac-sha256`, `MELD_PACK_KEY`, PEM, seed, or `meld-keys` material. |
| `cargo deny check` after `publish = false` | PASS: `advisories ok, bans ok, licenses ok, sources ok`. |

The original OAuth credential lacked the `workflow` scope and the first push
was rejected verbatim as recorded in §5. It was replaced with a
repository-scoped fine-grained PAT (`Contents: write`, `Workflows: write`,
`Pull requests: write`, `Actions: read`); the successful push and triggered CI
runs are recorded in §1.2.

### 1.2 Public branch and CI runs

Command: `git push origin production/security-from-baseline`

```text
To https://github.com/enis1enis2/meld.git
 * [new branch]      production/security-from-baseline -> production/security-from-baseline
push exit: 0
```

The credential used is a fine-grained PAT limited to `enis1enis2/meld` with
`Contents: write`, `Workflows: write`, `Pull requests: write`, and
`Actions: read`. The token is stored only in the git credential store outside
the repository (`/mnt/data/supervisor/share/.git-home/.git-credentials`, mode
`0600`) and is never committed.

GitHub API
`GET /repos/enis1enis2/meld/actions/runs?branch=production%2Fsecurity-from-baseline`
returned `status=200`, `total_count: 5`. At the time of the check all five runs
were `in_progress` (conclusions pending):

| Run ID | Workflow | Status at check | Link |
|---|---|---|---|
| 44769509918 | Supply-chain audit | `in_progress` | https://github.com/enis1enis2/meld/actions/runs/44769509918 |
| 44769509948 | Fuzz | `in_progress` | https://github.com/enis1enis2/meld/actions/runs/44769509948 |
| 44769509968 | CI | `in_progress` | https://github.com/enis1enis2/meld/actions/runs/44769509968 |
| 44769510069 | Docs | `in_progress` | https://github.com/enis1enis2/meld/actions/runs/44769510069 |
| 44769510130 | MSRV | `in_progress` | https://github.com/enis1enis2/meld/actions/runs/44769510130 |

No green result is claimed here; native platform runs must finish before §2
platform gaps can be closed.

## 2. Security

Status: DONE for all controls locally runnable on this host; platform-specific
runtime gaps are recorded below and are not silently assumed complete.

- **Threat model:** `spec/threat-model.md` is normative. It records assets,
  trust boundaries, adversaries, deny-by-default invariants, tier enforcement,
  and residual platform risks. Judgment calls 26–35 in `spec/README.md` and
  inline comments bind the implementation (`MELD_TEST_UNSANDBOXED`, L1/L2
  boundary, backend attestation, canonical roots, extended fuzz coverage).
- **Deny-by-default sandbox:** `meld-sandbox` computes the authority as
  `pack maximum ∩ runner constraints ∩ file default grant ∩ region grant`. The
  file default grant is the canonical project root with read/write + stdio and
  never network; grants only narrow. A backend must attest the mechanisms the
  effective policy needs or launch fails closed. Backends: Linux namespaces +
  seccomp + optional Landlock/cgroups/OCI fallback, macOS Seatbelt
  (`sandbox-exec`), Windows Job Objects + AppContainer.
- **OS enforcement tests (local):**
  | Suite | Result |
  |---|---|
  | `cargo test -p meld-sandbox` (unit) | 27 passed; 1 ignored (needs namespace privileges and writable cgroup v2) |
  | `cargo test -p meld-sandbox --test escape_attempts` | 5 passed |
  | `cargo test -p meld-t2 --test t2_abuse` | 13 passed |
  | `cargo test -p meld-t2 --test worker_bridge` | 1 passed |
  | `meld doctor` | `sandbox backend: linux-namespaces`, `sandbox available: ok`, `Landlock syscall ENOSYS/unsupported: not enforced` |
  - A real teardown defect was found and fixed: `killpg` returning `ESRCH` was
    not mapped to `NotFound`; `crates/meld-sandbox/src/process.rs` now checks
    `raw_os_error() != Some(libc::ESRCH)`.
- **Ed25519 supply chain:** `python3 tools/packtool.py verify-registry
  packs/registry --verify-packs` → `packs: 29  trust roots: 1  revoked: 0
  mirrors: 1`, `packs verified: 29`, `attestation: verified (key-id
  8ec9528bf1f0e31f)`, `OK`. Rotation is additive trust roots, revocation is an
  explicit key-id list, and registry metadata is bound by a signed attestation.
  Installer CLI tests cover signer not in trust roots, revoked signer, tampered
  payload, tampered checksums, and missing/malformed trust metadata.
- **Dependency audits:** `cargo audit` loaded 1555 advisories, scanned 295
  dependencies, 1 allowed warning (`RUSTSEC-2025-0057`, `fxhash` unmaintained,
  no fixed upgrade available). `cargo deny check` → `advisories ok, bans ok,
  licenses ok, sources ok`.
- **Fuzz coverage:** five Rust libFuzzer targets (`splitter`, `value_decode`,
  `sandbox_manifest`, `sandbox_policy`, `t2_envelope`) each smoke-run 200 runs
  locally; `tools/fuzz_packtool.py` 2000 deterministic cases and
  `tools/fuzz_splitter.py` 1000 cases pass. The local stable builds are
  crash-freedom evidence only; CI uses `cargo +nightly fuzz` with 60 s/target.
- **Secret hygiene:** `git grep` found no PEM/seed/private-key material; the
  signing key lives at `/mnt/data/opencode/tmp/opencode/meld-keys/` (mode 0600)
  outside every repository and is never committed.
- **Platform gaps (honest):** macOS Seatbelt and Windows Job Objects /
  AppContainer cannot be runtime-verified on this Linux host; Landlock returns
  `ENOSYS` and the OCI fallback is unproven here; this host cannot create user
  namespaces, so CLI fixture tests use the debug-only
  `MELD_TEST_UNSANDBOXED=1` path (compiled out of release, warns loudly, never
  relaxes production launches). Native Linux (privileged), macOS, and Windows
  CI runs remain required to verify actual OS enforcement.

## 3. Execution coverage

Status: DONE.

- **Tier decisions:** `project.md` §5 selects T2 (worker process over stdio) as
  the universal default. Written rejection decisions: T1 in-process embedding
  (fragile ABI, crash risk) and T4 GraalVM/Truffle (heavy dependency) are
  optimizations only; T3 uses strict Wasm limits. All are in `project.md`.
- **Language matrix:** 29 signed packs — 25 L0 syntax packs plus 4 executable
  T2 packs: `bash` (L1, stdout-only), `javascript`, `python`, `lua` (L2).
  The full per-language pin table is in `status/language-expansion.md`.
- **Executed examples:** `examples/01`–`10` — 10 `meld check` and 10 `meld run`
  all pass. This includes the three-language value bridge
  (`python → javascript → lua`, observed `lua received 3 values`), T3 bridge,
  shell pack, inline `eval`, raw-hash delimiters, and fenced literate regions.
- **Pairwise L2 round-trip matrix:** `tools/t2_ref.py` exercised all 9 ordered
  pairs of `{python, javascript, lua}` × 15 wire values (scalars, tagged
  variants, handles, maps, lists, special floats) — all `round-trip ok`,
  `0 problem(s)`.
- **Conformance:** `packs/bash/tests/run_conformance.py` — initialize + 4
  cases pass (`status/language-libraries.md` records 5 cases in an earlier run).
- **T3:** `crates/meld-t3` pins Wasmtime `=36.0.23` with fuel/epoch interruption,
  pooling allocation, and threads/GC/tail-call/multi-memory disabled
  (`spec/t3-component.md`).

## 4. Parsing and correctness

Status: DONE for parser/decoder boundaries; Miri/sanitizers not run (gap below).

- **Scanner fixtures:** `crates/meld-core/tests/scanners.rs` has 15 tests
  covering Ruby heredocs and lookalikes, multiple heredocs FIFO order,
  shift-expression non-openers, Python triple strings/dedents, YAML block
  scalars, Lua long brackets, SQL dollar quotes, Rust raw strings/nested
  comments, C++ raw strings, OCaml/Haskell nested comments, JS template
  interpolation, PHP/Bash heredocs, raw hash delimiters, and metadata-only
  builtin selection (pack code never executed). Suite: 15 passed.
- **Scanner decision:** `spec/README.md` records NO-GO on SGLR/SDF3/Raku-slang
  composition and GO on pack-owned data scanners registered by
  `region_end.toml`; raw hash delimiters remain the escape hatch. Pack
  `region_end.*` code is never executed.
- **Fuzz/decoder corpus:** the five Rust targets listed in §2 plus Python
  `fuzz_splitter.py` (1000 cases) and `fuzz_packtool.py` (2000 cases) cover
  splitter/scanners, value decoding, sandbox manifests/policies/grants, T2
  envelopes, trust roots, revocations, mirrors, registry, checksums, and
  signatures.
- **Reference/differential checks:** `meld_check_ref.py` 0 problems,
  `highlight_ref.py` 15 mixed-language spans, `sync_grammars.py --check` 0
  stale query files, `check_grammar_sources.py` 27 pinned grammar crates,
  `check_schemas.py` PASS (one placeholder warning for
  `bucket/meld-lang.json`, expected until the signed release checksum is
  filled).
- **Composition benchmark:** `research/composition/benchmark.py` over a
  deterministic 240-case corpus (200 reps) measured 100% region-end accuracy
  for the composition-aware scanner on adversarial heredoc/lookalike/multiple
  categories vs 0% for the generic brace scanner, at a 2.035× cost
  (30.52 µs vs 15.00 µs per call, 48 000 calls each). This is a
  Python-prototype measurement from 2026-11-19, not a production Rust
  benchmark; see §6.
- **Gap:** Miri and sanitizers were not run. No nightly toolchain is installed
  on this host, and the tree-sitter/Wasmtime FFI dependency graph is not
  Miri-compatible. Nightly instrumented libFuzzer in CI is the continuous
  memory-safety gate; no Miri result is claimed here.

## 5. Tooling

Status: DONE.

- **CLI:** `meld check|run|fmt --check|repl|doctor|lib|pack`, with LSP via
  `meld-lsp`. `meld doctor` reports toolchain, sandbox backend/availability,
  and every discovered pack/tier (verified locally).
- **Library manager:** `meld-pack` supports `install`, `remove`, `import`,
  `unimport`, `list`, `remote-list`, `verify`, `self-install`, and
  `self-uninstall`; remote installs require a verified Ed25519 signature and
  pack code is never executed.
- **CI workflows (all committed):** `ci.yml` (Linux/macOS/Windows, stable +
  MSRV 1.90, fmt, clippy, tests, docs, Python tests, fuzz smoke, pack
  verification), `audit.yml`, `fuzz.yml` (5 Rust targets + packtool fuzz),
  `docs.yml` (rustdoc + mdbook), `msrv.yml`, and `release.yml` (gates,
  native matrix, reproducible packaging, CycloneDX SBOM, cosign keyless
  signing, build provenance, and final release gate).
- **Packaging/installers:** `install.sh`, `install.ps1`, `package.nix`,
  `flake.nix`, `Dockerfile`, `packaging/deb/`; system package `meld-lang`.
  `sh -n` on shell scripts, `python3 -m py_compile` on Python tooling, and
  JSON parse of `bucket/meld-lang.json` / `grammar-sources.json` passed
  (§1.1).
- **Test totals:** Rust workspace 102 passed / 0 failed / 1 ignored; Python 10
  tests passed. Formatter, clippy (`-D warnings`), check, tests, and docs all
  pass locally.
- **CI run links:** the branch was pushed successfully with a repository-scoped
  fine-grained PAT that includes `Workflows: write`; the five triggered runs are
  listed in §1.2. The earlier rejected attempt was verbatim:
  `! [remote rejected] production/security-from-baseline ->
  production/security-from-baseline (refusing to allow an OAuth App to create
  or update workflow '.github/workflows/audit.yml' without 'workflow' scope)`.

## 6. Performance and reliability

Status: DONE with an explicit measurement caveat.

- **Bounded execution:** 1 MiB protocol line cap, bounded request/stdout
  queues, absolute 10 s call deadlines, bounded stderr drain, host-call
  allowlist with 64-call and 2 s callback limits, and process-tree teardown.
  These are verified by the T2 abuse suite (13 tests) and worker bridge test.
- **Resource limits:** cgroups plus rlimits (address space, CPU, file size,
  process count, open files); enforced where the host permits, otherwise the
  backend fails closed.
- **Reliability defect fixed:** the `killpg` `ESRCH` mapping described in §2;
  escape-attempt suite (5 tests) and sandbox policy intersection tests guard
  the invariant.
- **Performance measurement:** the only measured figure is the composition
  benchmark in §4 (2.035× slowdown for composition-aware region-end discovery
  with 100% accuracy). No production Rust Criterion benchmark exists, so no
  latency/throughput SLO is claimed. Multi-tenant timing side channels are
  explicitly out of scope in `spec/threat-model.md` §6.

## 7. Documentation

Status: DONE.

- **User/project docs:** `README.md`, `CHANGELOG.md`, `RELEASE.md`, `SBOM.md`,
  `SECURITY.md`, `SEMVER.md`, `THIRD-PARTY-NOTICES.md`, `NOTICE`,
  `LICENSE-MIT`, `LICENSE-APACHE`.
- **Normative specs:** `spec/README.md` (35 numbered judgment calls),
  `spec/threat-model.md`, `spec/t2-protocol.md` (L1/L2 boundary, hostcall
  rules), `spec/t3-component.md`, `spec/registry.md`, `spec/libraries.md`,
  `spec/lang-pack.md`, `spec/syntax.md`, `spec/value-model.md`,
  `spec/lsp.md`.
- **Status/evidence:** baseline, phase 1–7, `language-expansion.md`,
  `language-libraries.md`, and this file.
- **Book:** `docs/src/` (overview, artifacts, release, schemas, security) plus
  `docs/release-artifacts.md` and `docs/schema-stability.md`. `cargo doc
  --workspace --no-deps` passes locally; mdbook is built by `docs.yml` in CI
  (mdbook is not installed on this host, so no local mdbook result is claimed).
- **Inline judgment calls:** affected spec and code files carry `JUDGMENT CALL`
  comments next to the decisions they describe.

## 8. Final audit

Status: DONE. All locally runnable gates were re-run after the final Lua pack
fix and registry re-attestation; results below are real command output.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| `cargo test --workspace` | 102 passed; 0 failed; 1 ignored |
| `cargo doc --workspace --no-deps` | PASS |
| `cargo audit` | 1555 advisories, 295 deps, 1 allowed warning (`fxhash`) |
| `cargo deny check` | advisories/bans/licenses/sources ok |
| `python3 -m unittest discover -s tools/tests -v` | 10 tests OK |
| `fuzz_splitter.py` / `t2_ref.py` / `meld_check_ref.py` / `highlight_ref.py` | 1000 cases / 0 problems / 0 problems / 15 spans |
| `sync_grammars.py --check` / `check_grammar_sources.py` / `check_schemas.py` | 0 stale / 27 pinned / PASS |
| `fuzz_packtool.py --iterations 2000` | PASS |
| 5 Rust fuzz smoke targets (`-runs=200`) | PASS each (crash-freedom only) |
| `verify-registry packs/registry --verify-packs` | 29 packs, attestation verified, OK |
| `examples/01`–`10` check + run | 10/10 + 10/10 PASS |
| Windows workspace cross-check (`x86_64-pc-windows-gnu`) | PASS |
| `meld-sandbox` Darwin cross-check (`x86_64-apple-darwin`) | PASS |
| Secret scan | PASS: no private keys or seed files in the repo |

**Remaining verification that this host cannot perform (not hidden):** native
macOS (Seatbelt) and Windows (AppContainer/Job Objects) runtime tests;
privileged Linux namespace/cgroup enforcement tests (CLI fixtures here use the
debug-only unsandboxing switch); Landlock (`ENOSYS` here) and the OCI fallback;
full-workspace Darwin cross-build (no macOS cross C toolchain/SDK — only the
platform-sensitive `meld-sandbox` crate was cross-checked); nightly instrumented
fuzzing and Miri/sanitizers; and final CI conclusions (runs are triggered and
linked in §1.2, but were still in progress at the time of this check). These
are tracked as environment/CI gaps, not as passed checks.

**Final judgment:** every gate that can be executed in this environment passes,
and the code fails closed where a platform control cannot be attested. Meld is
ready for the first public signed candidate `v0.2.0-rc.1` once the native CI,
privileged-Linux, and platform-sandbox runs above are green; this file records
those as pending rather than asserting them.
