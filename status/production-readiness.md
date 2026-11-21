# Production readiness evidence — final snapshot

This is a one-time evidence snapshot for the production-readiness workstream.
Every result below was actually run; no pending check is marked complete.
`../project.md` remains authoritative. Amendments to implementation judgment
calls are recorded inline and in `../spec/README.md` (59 numbered calls; the
final hardening chain adds 31–59).

## Scope and branch state

Command:

```text
git status --short && echo STATUS_CLEAN && git rev-parse main HEAD
```

Observed before this evidence file was written:

```text
STATUS_CLEAN
3189c1557292c71fe3c65175fcfdf609ae7e5e60
225dce46c571cba16b9d332a12e063d6ba846cb1
```

- Branch: `production/security-from-baseline`.
- Final code SHA verified by all five workflows:
  `225dce46c571cba16b9d332a12e063d6ba846cb1`.
- `main` stayed at
  `3189c1557292c71fe3c65175fcfdf609ae7e5e60`.
- All pushes were fast-forward; no force-push was used.
- The private Ed25519 signing key stayed outside the repository at
  `/mnt/data/opencode/tmp/opencode/meld-keys/ed25519-private.pem` (mode `0600`).
  Credentials are stored only in the git credential store outside the
  repository (mode `0600`) and are never committed.
- The earlier evidence commit `34086d6…` was superseded: its retriggered CI
  exposed two Windows test flakes, which were diagnosed and fixed in
  `cb15cfe…` and `225dce4…`. This snapshot is written only after the final code
  SHA went green.
- Committing this document creates a documentation-only commit and retriggers
  the five workflows. The observed results for that evidence commit are
  reported in the final session report, not back-filled here, so this snapshot
  remains a single real record.

## Final CI — full-SHA query

Abbreviated-SHA filtering gave false `0/5` results, so all checks used the
full commit SHA. Command:

```text
python3 /mnt/data/opencode/tmp/opencode/check-final-runs.py 225dce46c571cba16b9d332a12e063d6ba846cb1
```

Observed output:

```text
head_sha query total: 5
  sha-query  44822484998    Supply-chain audit    completed    success
  sha-query  44822485023    MSRV    completed    success
  sha-query  44822485305    Docs    completed    success
  sha-query  44822485137    CI    completed    success
  sha-query  44822484969    Fuzz    completed    success
branch query exact-SHA matches: 5
  branch     44822484998    Supply-chain audit    completed    success    https://github.com/enis1enis2/meld/actions/runs/44822484998
  branch     44822485023    MSRV    completed    success    https://github.com/enis1enis2/meld/actions/runs/44822485023
  branch     44822485305    Docs    completed    success    https://github.com/enis1enis2/meld/actions/runs/44822485305
  branch     44822485137    CI    completed    success    https://github.com/enis1enis2/meld/actions/runs/44822485137
  branch     44822484969    Fuzz    completed    success    https://github.com/enis1enis2/meld/actions/runs/44822484969
```

| Run ID | Workflow | Conclusion |
| --- | --- | --- |
| 44822485137 | CI | `success` |
| 44822485305 | Docs | `success` |
| 44822485023 | MSRV | `success` |
| 44822484998 | Supply-chain audit | `success` |
| 44822484969 | Fuzz | `success` |

The CI workflow includes the Linux/macOS/Windows stable + MSRV matrix, fmt,
clippy `-D warnings`, workspace tests, OCI-feature checks on Linux, docs,
Python unit tests, splitter fuzz smoke, reference harnesses, and verification of
every checked-in pack.

## Final local gates on the exact code tree

Rust command:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo clippy -p meld-sandbox --features oci --all-targets -- -D warnings
cargo test -p meld-sandbox --features oci --all-targets
cargo doc --workspace --no-deps
```

Real output excerpts:

```text
=== cargo fmt --all -- --check ===
=== cargo clippy --workspace --all-targets -- -D warnings ===
    Finished `dev` profile [unoptimized + debuginfo] target(s)
=== cargo test --workspace --all-targets ===
test t5_toolchain_fallback_compiles_and_runs_a_region ... ok
test stderr_tail_is_included_in_crash_report ... ok
test repl_switches_languages_and_keeps_running ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s
=== cargo clippy -p meld-sandbox --features oci --all-targets -- -D warnings ===
=== cargo test -p meld-sandbox --features oci --all-targets ===
test result: ok. 37 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 5.01s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
=== cargo doc --workspace --no-deps ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.93s
   Generated .../target/doc/meld/index.html and 9 other files
=== ALL CARGO GATES EXIT 0 ===
```

The 24 workspace test binaries summed to **108 passed, 0 failed, 1 ignored**.
The OCI-feature suite's three ignored tests require `MELD_OCI_RUNC` plus a
privileged runc runtime; they are not counted as local passes.

Python, supply-chain, and pack command:

```text
PYTHONPATH=/mnt/data/opencode/tmp/opencode/pydeps python3 -m unittest discover -s tools/tests -v
python3 tools/fuzz_splitter.py
python3 tools/meld_check_ref.py
python3 tools/highlight_ref.py
python3 tools/fuzz_packtool.py --iterations 2000
python3 tools/check_grammar_sources.py
python3 tools/generate_third_party_notices.py --check
PATH=/mnt/data/supervisor/share/.cargo-home/bin:$PATH \
CARGO_HOME=/mnt/data/supervisor/share/.cargo-home \
RUSTUP_HOME=/mnt/data/supervisor/share/.rust-home \
cargo audit --deny warnings
... cargo deny --all-features check
for pack in packs/* tests/fixtures/pack-third-party; do python3 tools/packtool.py verify "$pack"; done
```

Real output excerpts:

```text
Ran 11 tests in 6.914s

OK
splitter reference fuzz: PASS (1000 cases)
meld check reference: 0 problem(s)
highlight proxy: PASS (15 mixed-language spans)
fuzz_packtool: PASS (2000 cases, seed 20261120)
check_grammar_sources.py: PASS (27 pinned grammar crate(s))
generate_third_party_notices.py: PASS notices are current
    Loaded 1555 security advisories (from /mnt/data/supervisor/share/.cargo-home/advisory-db)
    Scanning Cargo.lock for vulnerabilities (295 crate dependencies)
advisories ok, bans ok, licenses ok, sources ok
=== verify every checked-in pack ===
... 28 packs + tests/fixtures/pack-third-party ...
OK (each)
=== ALL PYTHON/SUPPLY/PACK GATES EXIT 0 ===
```

`cargo audit --deny warnings` exited 0 with no RUSTSEC findings and no
unmaintained-crate warning under the pinned tool/config; `cargo deny` exited 0
with all four categories ok. `Cargo.lock` pins Wasmtime `=36.0.23`; the
workspace MSRV remains `1.90`.

Fuzz command (with the host's chroot toolchain wrappers on `PATH`):

```text
cargo-fuzz build splitter --sanitizer none
<fresh-binary> /mnt/data/opencode/tmp/opencode/fuzz-artifact/crash-151c648844c8db74133fee74eaee420d9c8aabee
<fresh-binary> -max_total_time=60 -max_len=4096 -timeout=5 -artifact_prefix=...
```

Real output excerpts:

```text
=== cargo-fuzz build splitter --sanitizer none ===
    Finished `release` profile [optimized + debuginfo] target(s) in 0.56s
=== exact crash artifact repro ===
Executed /mnt/data/opencode/tmp/opencode/fuzz-artifact/crash-151c648844c8db74133fee74eaee420d9c8aabee in 0 ms
=== bounded 60s fuzz session ===
Done 1442633 runs in 61 second(s)
=== ALL FUZZ GATES EXIT 0 ===
```

The local binary used `--sanitizer none`: this host's default ASAN startup fails
with `heap size 40000002000 exceeds max user virtual address 7fffffffff`. The
Fuzz workflow's nightly/default-sanitizer runs are green in CI and are the
authoritative sanitizer evidence.

Targeted regression loops on the final code tree:

```text
cargo test -q -p meld-t2 --test t2_abuse stderr_tail_is_included_in_crash_report -- --exact
  -> pass=20 fail=0
cargo test -q -p meld-cli --test cli -- --test-threads=6
  -> pass=20 fail=0
```

Secret scan:

```text
git grep -n -E 'BEGIN (RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY' -- . || echo NO_PRIVATE_KEY_BLOCKS
git ls-files | grep -E '(^|/)(meld-keys|.*private.*\.pem|seed)' || echo NO_KEY_OR_SEED_FILES
```

```text
NO_PRIVATE_KEY_BLOCKS
NO_KEY_OR_SEED_FILES
```

## Task-by-task findings

### 1. Release hygiene, CI, and dependency policy

- **Root cause:** the baseline had stale status, no license/notice/deny policy,
  no cross-platform CI matrix, unpinned Python/tooling, and clippy warnings that
  defeated `-D warnings`.
- **Commits:** `28c915ed…`, `a573999b…`, `b69718d5…`, `62f75db3…`,
  `d802039a…`, `14e3168e…`, `005001f7…`, `944541da…`, `176b2e8b…`.
  The final pinning fix is
  `944541da07ed5e6db3666f1e4827a5a85b1c5fef` (`fix(ci): pin cargo audit and deny
  tool versions`); `176b2e8b5fc80c11cca1f1f9eb08a047a368ebb5` regenerated notices.
- **Verification:** CI run `44822485137` success; local `cargo clippy
  --workspace --all-targets -- -D warnings` finished 0; `cargo audit --deny
  warnings` loaded 1555 advisories over 295 deps and exited 0; `cargo deny
  --all-features check` reported all categories ok; notices check passed.
- **Unresolved:** none for the committed policy. Floating Python `3.12` patch is
  an accepted judgment call (45); `cryptography` remains pinned to `50.0.2`.

### 2. Security and deny-by-default sandbox

- **Root cause:** worker launch needed a single authority computed from four
  narrowing layers, and any backend that could not attest a required mechanism
  had to fail closed rather than silently run unsandboxed. Windows AppContainer
  was claimed without enforceable evidence; OCI needed an explicit privileged
  boundary. A final race also let a reaped worker's stderr pump publish after
  the crash-report reader had already given up.
- **Commits:** `6cca6228…`, `d3562c6d…`, `2f9afa73…`, `54a61399…`,
  `35c0fead…`, `cc3aace2…`, `ebe5f59a…`, `6295a278…`, `30894687…`,
  `a4779142…`, `e32fcff5…`, `cb15cfea…`.
  Final fixes include
  `cc3aace2d2bf8bf9b19de9c7408a0d5db62fce29` (withdraw AppContainer claim),
  `ebe5f59a0423eb810f542a4317aec76ad06d5084` (privileged OCI boundary),
  `30894687ed87c2f1304994b2c2d8b41c729c381d` (Windows symlink walk),
  `a477914250342478aa8744992fe8f79fbe5fa23e` (portable kill-hook test),
  `e32fcff528ffc302f2742c68542103f889ef4269` (strip Windows `\\\\?\\` from child
  argv), and
  `cb15cfeafb7a77a38452757c1f18371af5c99721` (bounded stderr drain, judgment 58).
- **Authority rule:** `pack maximum ∩ runner constraints ∩ file default grant
  ∩ region grant`. The file default grant is the canonical project root with
  read/write + stdio, never network; grants only narrow.
- **Backends:** Linux namespaces/seccomp with Landlock/cgroups/OCI fallback;
  macOS Seatbelt; Windows Job Objects only.
- **Verification:** `cargo test -p meld-sandbox --features oci --all-targets`
  gave `37 passed; 0 failed; 3 ignored`, and escape-attempt tests gave
  `5 passed; 0 failed`. The previously flaky
  `stderr_tail_is_included_in_crash_report` passed 20/20 targeted reruns and in
  the full workspace run. CI stable/MSRV Windows matrix jobs passed in run
  `44822485137`. Previously, with a privileged runc, `oci::tests::oci_runtime_spawn`
  and `oci::tests::oci_runtime_confinement_facts` each reported `1 passed; 0
  failed`; that environment is not available on the final local rerun.
- **Unresolved:** native macOS Seatbelt enforcement, native Windows Job Object
  enforcement, privileged Linux namespace/cgroup spawn, Landlock (`ENOSYS` on
  this host), and reproducible privileged OCI runtime tests on the final tree
  are not claimed as local passes. `MELD_TEST_UNSANDBOXED` is a debug-only,
  loud, release-compiled-out fixture escape hatch and is not a production
  relaxation.

### 3. Supply chain and pack trust

- **Root cause:** the baseline used symmetric HMAC and mutable trust; packs
  needed an Ed25519 hard cutover, canonical trust roots, revocation, signed
  registry attestation, mirrors, and byte-exact verification.
- **Commits:** `6582ee85…`, `d4b1d184…`, `24b2df5e…`, `510c3f3c…`,
  `a39d7768…`, `f951fa0d…`, `d510e9e8…`, `b6af7e84…`, `a725a10a…`,
  `58ba5a3f…`, `4fc80cd4…`, `c3a1ddd2…`, `938a61d6…`, `a6616e8f…`.
- **Final Windows failure chain and fixes:**
  - `\\\\?\\C:\\...` was passed to `git remote add` →
    `hostname contains invalid characters`; fixed by
    `1ca7ce6ca15b0c27d4104e25d0e35fa46e36c373` (normalize local source paths
    for git, judgment 50).
  - Git normalized fixture/clone bytes → signature/checksum mismatches; fixed
    by `a725a10aba4f157b25fe07c8aadc4b6afade03d1` (temporary clones set
    `core.autocrlf=false`), `58ba5a3ff890ada38cb2ad6fe8f0204a0e705cbc` (fixture
    repository too), and `938a61d625c6b904787ea42946132c9947f0c550`
    (`.gitattributes` marks checked-in pack trees `-text`, judgment 56).
  - Signed CRLF `checksums.sha256` parsed with trailing `\\r` →
    `missing file: README.md`; fixed by
    `4fc80cd4c9bfcdb74fe78eed469476f66e6ea8aa` (uniform LF or CRLF, reject
    mixed/bare CR, judgment 53).
  - `packtool sign` accepted canonical but stale checksums → all digests
    mismatched after byte-preserving checkout; fixed by
    `c3a1ddd238827890ba990d8ade53d92a46111299` (recompute before signing,
    judgment 54).
  - Python tests wrote registry JSON in text mode → CRLF made indices
    non-canonical before the intended assertions; fixed by
    `a6616e8f108f4fc2ba70d1060fadb510a3516808` (canonical bytes, judgment 57).
- **Verification:** 11 Python tests `OK`, including
  `test_sign_refuses_stale_checksums`; all 28 packs plus the third-party fixture
  report `checksums verified` and `OK`; `git check-attr text -- packs/ruby/pack.toml
  packs/ruby/checksums.sha256 tests/fixtures/pack-third-party/worker.sh`
  returned `text: unset` for each; secret scan found no private-key blocks or
  key/seed files.
- **Unresolved:** none for trust-chain enforcement. The AppContainer claim was
  withdrawn rather than silently assumed; `signature.hmac` is rejected and the
  dev HMAC key never became a trust root.

### 4. Tier/execution host

- **Root cause:** L1 T2 workers were safe only for stdout-only `run`; value
  imports/exports, inline `eval`, `fetch`, and host calls required the stronger
  L2 worker, and pre-init host calls had to be denied. Two Windows flakes also
  surfaced in the final chain: a stderr-pump publication race and contention
  during parallel external-runtime cold starts.
- **Commits:** `30fb5848…`, `710bdb21…`, `4aee8fa3…`, `c91eb640…`,
  `c98a9c10…`, `8d132fe8…`, `ae3527fe…`, `95538f28…`, `59240209…`,
  `bcd00f6…`, `ce33c48e…`, `752089ce…`, `f9e43aba…`, `cb15cfea…`,
  `225dce4…`.
- **Final T5 failure chain:** `/bin/sh` does not exist on Windows and shebang
  scripts cannot be spawned directly (`ce33c48e…`); Git Bash then consumed
  backslashes in `-c` scripts (`752089ce…` made placeholders portable); the
  run stage still crashed with `0xc0000005`, so
  `f9e43abaed2862c7f4d3c0746ac366639c928915` replaced the shell toolchain with
  `python` on Windows and `python3` elsewhere (judgment 55).
- **Final Windows flake fixes:**
  - `cb15cfeafb7a77a38452757c1f18371af5c99721` replaced `try_recv` with a
    bounded 50 ms `recv_timeout` grace so a crashed worker's stderr tail cannot
    be lost to pump scheduling (judgment 58).
  - `225dce46c571cba16b9d332a12e063d6ba846cb1` added a test-only mutex around
    CLI integration cases. Parallel Node/Python/shell cold starts had made the
    REPL JavaScript worker intermittently exceed the unchanged 10 s T2
    initialize deadline; no assertion, timeout, or policy changed (judgment 59).
- **Verification:** `test t5_toolchain_fallback_compiles_and_runs_a_region ...
  ok`, `test stderr_tail_is_included_in_crash_report ... ok`, and
  `test repl_switches_languages_and_keeps_running ... ok` locally; both targeted
  20-run loops passed; CI Windows stable and 1.90 jobs passed at the final SHA;
  T3 WIT calls and T2 abuse/worker suites passed (13 + 1 tests).
- **Unresolved:** the 10 s T2 timeout remains deliberately unchanged (judgment
  49). Native Windows sandbox runtime enforcement is still a platform gap,
  although the test-toolchain and startup-contention issues are resolved.

### 5. Parsing and region-end scanning

- **Root cause:** generic brace matching failed on heredocs and lexer-like
  constructs; composition via SGLR/SDF3/Raku-slang was not viable.
- **Commits:** `43970a0a…`, `3a3eef35…`, `c98a9c10…`, `2bc68f29…`,
  `d4464ea6…`, `e384a84c…`, `d1549200…`, `da11ea4d…`.
- **Decision:** NO-GO on SGLR/SDF3/Raku-slang composition; GO on pack-owned
  data scanners registered by `region_end.toml`, beginning with Ruby heredocs.
  Pack `region_end.*` code is never executed.
- **Verification:** scanner suite `15 passed; 0 failed`; `cargo test
  --workspace --all-targets` passed; `tools/fuzz_splitter.py` passed 1000 cases;
  `meld_check_ref.py` reported `0 problem(s)`; `highlight_ref.py` passed 15
  mixed-language spans.
- **Unresolved:** Miri and sanitizer-instrumented local runs were not performed
  (see §“Unverified” below).

### 6. Tooling, installers, and docs

- **Root cause:** library discovery needed trust-root-aware install/verify;
  Windows path forms, worker env, and OS locators needed explicit handling;
  docs needed to match the shipped behavior.
- **Commits:** `22a7b945…`, `67ce7a71…`, `872c766f…`, `1ca7ce6c…`,
  `a725a10a…`, `58ba5a3f…`, `4fc80cd4…`, `bc741235…`, `ddcef662…`.
- **Verification:** installer CLI tests `6 passed; 0 failed`; CLI tests `15
  passed; 0 failed`; docs workflow `44822485305` success; local `cargo doc
  --workspace --no-deps` finished successfully.
- **Unresolved:** native macOS/Windows runtime behavior of installers is
  covered by CI tests but not by this Linux host's direct runtime execution;
  mdbook is not installed locally, so only CI builds the book.

### 7. Performance and reliability

- **Root cause:** bounded worker I/O, host-call limits, teardown, and deterministic
  timeout behavior had to hold under abuse and cross-platform kill hooks.
- **Commits:** `2ed5544d…`, `d3562c6d…`, `848acd4d…`, `a4779142…`,
  `ae3527fe…`, `95538f28…`, `cb15cfea…`, `225dce4…`.
- **Verification:** T2 abuse suite `13 passed; 0 failed`; worker bridge
  `1 passed; 0 failed`; kill-hook test passed after replacing its executable
  with a portable one; stderr-tail regression passed 20/20; CLI integration
  suite passed 20/20 with six test threads; no timeout diagnostics remain in
  the tree.
- **Unresolved:** no production Rust Criterion benchmark exists; the only
  measured performance figure remains the earlier Python composition prototype
  (100% accuracy at 2.035× cost), not an SLO.

### 8. Final audit

- **Root cause:** earlier status evidence was stale (35 judgment calls,
  AppContainer claim, pending OCI/CI) and had to be replaced with a single
  real snapshot after all fixes. An intermediate snapshot also became stale
  when the final evidence commit retriggered CI and exposed two Windows flakes.
- **Commit:** this evidence-only commit; final code SHA remains
  `225dce46c571cba16b9d332a12e063d6ba846cb1`.
- **Verification:** all five workflows `success` by full-SHA query (table
  above); local Rust/Python/supply-chain/pack/fuzz gates exited 0; `main`
  unchanged; secret scan clean.
- **Unresolved:** the evidence commit retriggers CI; its own run IDs and
  conclusions are verified in the session report after push.

## Unverified items (explicit)

These are not claimed as passed on this host:

1. Native macOS Seatbelt runtime enforcement.
2. Native Windows Job Object runtime enforcement. The AppContainer enforcement
   claim is withdrawn; Windows attests Job Objects only.
3. Privileged Linux namespace/cgroup runtime spawn (the test is ignored without
   privileges and a writable cgroup v2).
4. Landlock enforcement (host returns `ENOSYS`).
5. Reproducible privileged OCI runtime tests on the final tree (earlier
   privileged runc evidence exists but is not part of the final local rerun).
6. Full-workspace Darwin cross-build; only platform-sensitive crates are
   cross-checked in CI. Local Windows cross-builds of `meld-cli`/`meld-lsp` are
   blocked by `zstd-sys` passing `-m64` to this host's cross compiler, not by a
   Meld code failure.
7. Miri and sanitizer-instrumented local runs; default ASAN fuzz startup fails
   with an address-space limit on this host. CI's nightly fuzz workflow is
   green.
8. Native execution of macOS and Windows installers outside GitHub Actions.
9. mdbook local build (not installed here); Docs workflow is green.

## Final judgment

Every gate that can be executed on this host passes, the five workflows are
green on the final code SHA, and the code fails closed where a platform control
cannot be attested. The remaining items above are environment/platform gaps,
not hidden failures.

## Addendum — v0.2.0 release preparation (2026-11-21)

- Release tags are stable `vMAJOR.MINOR.PATCH` only (`v0.2.0` is the current
  target); the release workflow rejects pre-release tags.
- At the audit baseline `main` was
  `3d4d378d68f5e455bafb861cd642dce64623a071`. A read-only Actions API query for
  that SHA returned 23 completed checks, all `success`, covering CI, Docs, MSRV,
  Fuzz, supply-chain audit and CodeQL.
- Eight external suites were pending at audit time and are not claimed as passed
  here. No `v0.2.0` release has been published (the latest GitHub release is the
  historical `v0.2.0-rc.5`); channel hashes stay as placeholders until generated
  from signed `SHA256SUMS`.
- See [RELEASE.md](../RELEASE.md) §“Release record (preparation baseline)” for
  the same evidence and the post-publication record requirement.
