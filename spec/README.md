# Meld Phase 0 Specs — review record

## Reviewed deliverables

- `syntax.md` — region forms, glue syntax, diagnostics.
- `value-model.md` — types, wire encoding, numeric/lifetime rules.
- `lang-pack.md` — pack contract, tiers, conformance levels, generator.
- `t2-protocol.md` — universal worker protocol.
- `../examples/*.meld` — three hand-written example files.

The plan `../project.md` was read in full. It was not present on disk; the full
text supplied in the task prompt was persisted verbatim as `project.md` before
work began. **JUDGMENT CALL:** using prompt text as the authoritative file rather
 than halting, per the "flag blocker and continue with unblocked work" rule.

## Library verification (done before Phase 0 sign-off)

Verified against crates.io API on 2026-11-19:

| crate | current stable | rust-version | note |
|---|---|---|---|
| `tree-sitter` | 0.27.2 | 1.90 | edition 2024; `wasm` feature exists |
| `wasmtime` | 50.0.3 | 1.97 | `component-model` is a default feature; T3 target |
| `tower-lsp-server` | 0.23.0 | 1.85 | maintained fork (`tower-lsp` original is legacy); Phase 5 |
| `serde_json` | 1.0.154 | 1.71 | T2 envelope |

Capability claims from project.md §2 were re-checked: tree-sitter docs index
still documents multi-language documents; Component Model/WIT docs still define
WIT as the IDL and the Canonical ABI as the copy boundary. Claims in §3–§11
remain design proposals, not sourced fact, and are labeled as such.

## Judgment calls made (all inline where applied)

1. `#meld` is the only header directive; no `#lang` alias.
2. Raw hash delimiters are available in Phase 1 alongside `{...}`.
3. Inline regions do not nest in Phase 1.
4. Auto-detect confidence threshold 0.75, non-configurable in Phase 1.
5. T2 JSON line protocol with decimal-string integers to avoid JSON f64 loss.
6. `tower-lsp-server` (community fork) is the Phase 5 LSP crate.
7. Grammar revisions must be pinned even in generated packs.
8. Rust core remains the stack (project.md §10); no alternative substituted.

## Disagreements with the plan

None that change direction. One environment constraint is flagged below.

## Blockers

- **B1 — no Rust toolchain** (RESOLVED). The user later granted an explicit
  override to the no-install rule. Rust 1.100.0 was installed via rustup with
  `CARGO_HOME`/`RUSTUP_HOME` under the writable share; `cargo test --workspace`
  now passes, including a real T2 worker integration test. The Python reference
  harness remains as a fast proxy but is no longer the only verification.
- **B2 — no Node.js or Lua interpreter** (RESOLVED). Node 18.20.4, Lua 5.4.4 and
  lua-cjson were installed in the isolated Debian root. All three workers are
  exercised by `tools/t2_ref.py`, and Python+JS+Lua execution is verified by
  `meld run examples/06-bridge.meld`.
- **B3 — `project.md` not found on disk** (resolved by persisting prompt text).

## Phase 0 done-when assessment

Specs reviewed: yes (this file). Three example `.meld` files: yes. Therefore
Phase 0's stated criteria are met; B1/B2 affect Phases 1–3 verification only.

## Later specs added (Phases 4–7)

- `t3-component.md` — WIT as the IDL, committed Wasm Component, typed host
  calls, Python → T2 → T3 callback, and the T5 toolchain fallback.
- `lsp.md` — virtual documents, source maps, pack-owned editor metadata,
  completion forwarding, formatter delegation, and the scriptable REPL.
- `registry.md` — pack checksums, local HMAC signing, registry format, and the
  publish/list/install workflow that lets a third party add a language.
- `libraries.md` — installable Lang Packs: `library.toml`, git-backed
  `meld-pack`, installed index, and project `meld.lock`.
- `lang-pack.md` now documents `runner.toml`, `inline.toml`, `lsp.toml`,
  `formatter.toml`, copied queries, and the two accepted spellings of a pinned
  grammar rev.

Library verification was refreshed on 2026-11-20: `tower-lsp-server 0.23.0`
is still the newest stable release (`0.24.0-rc.1` is a prerelease), and the
locked versions in `Cargo.lock` are `tree-sitter 0.27.2`, `wasmtime 50.0.3`,
`tokio 1.54.1`, `serde_json 1.0.154`, and `ls-types 0.0.6`. No unverified
preview dependency was adopted.

Additional judgment calls (all also inline in the relevant spec/code):

9. WIT is reused directly; a single component result is typed `(i64,)` in
   Wasmtime; `meld:t3/*` is a reserved callback namespace.
10. The component binary is committed so tests do not need `wasm-tools`.
11. LSP virtual documents are padded to global line/column coordinates and
    pack `lsp.toml` supplies either keywords or an external stdio server.
12. `meld fmt` formats glue in core and delegates region bodies to the pack.
13. Pack signing uses local HMAC-SHA256; an exact crate version or
    `pinned:<ref>` both satisfy strict grammar pinning.
14. Phase 6 composition research is a NO-GO for SGLR/SDF3 and a GO for
    pack-owned scanner upgrades and raw delimiters.

Phase completion evidence is in `../status/phase-4.md` through
`../status/phase-7.md`.

## Language expansion (2026-11-20)

Fifteen strict Lang Packs were added without a core redesign: `typescript`,
`tsx`, `bash`, `ruby`, `php`, `yaml`, `toml`, `csharp`, `haskell`, `ocaml`,
`scala`, `zig`, `r`, `dart`, and `solidity`. crates.io was queried again before
selection; exact versions are recorded in `crates/meld-grammars/Cargo.toml`
and each `pack.toml`, and copied upstream queries are checked by
`tools/sync_grammars.py --check`.

Additional judgment calls (also inline in the affected files):

15. `tree-sitter-perl 1.1.2` was rejected because it keeps `tree-sitter 0.26`
    as a normal dependency, which Cargo refuses alongside the core's 0.27
    `links = "tree-sitter"` library. It is deferred, not claimed as supported.
16. TypeScript/TSX share one pinned crate and PHP/OCaml use their published
    implementation-grammar constants; pack ids are aliases over those choices.
17. `bash` is L1 (statements + stdout + structured errors) and deliberately not
    L2: the proven third-party shell worker has no value bridge.
18. The upstream Solidity 1.2.13 query contains a trailing `.` anchor rejected
    by tree-sitter 0.27 `Query`; `tools/sync_grammars.py` normalizes it in both
    copy and check modes rather than maintaining a divergent hand-written pack
    query.
19. New L0 fixtures use `##"... "##` raw delimiters; indentation-sensitive
    languages never rely on the generic brace scanner.

Evidence and verification results are in `../status/language-expansion.md`.

## Lang Libraries (2026-11-20)

`libraries.md` is normative for installable/importable Lang Packs. Every
checked-in pack and the third-party fixture now carries `library.toml`;
`tools/gen_pack.py` emits it for all future packs, and
`tools/make_libraries.py` refreshed existing packs and their checksums. The
terminal installer is `crates/meld-installer` (`meld-pack`), the shared format
and verification live in `crates/meld-lib`, and `meld` reads `meld.lock` for
project imports. No `project.md` decision was replaced.

Additional judgment calls (also inline in `libraries.md` and the code):

20. A Lang Library is a Lang Pack plus a manifest, package-manager style, not a
    compiled `cdylib`; downloading arbitrary grammar code stays outside the
    §9 trust model.
21. `exports` is a stable relative data surface; traversal and missing paths
    fail validation, but the whole verified pack is copied.
22. Installed libraries live under a shared root with canonical `index.json`,
    while `meld.lock` records each project's exact imported versions.
23. `meld-pack` uses the `git` CLI with a shallow fetch of a git tree, accepting
    GitHub slugs/URLs and local git repos through the same code path.
24. Missing imports are skipped at runtime, and imported libraries are
    discovered last so a project lock overrides a same-named built-in pack.
25. Temp clones honor `TMPDIR`/`MELD_PACK_TMP` and are visibility-checked after
    git checkout; a chrooted git otherwise writes outside the Rust process's
    view and looks like an empty catalog.

Dependency versions were re-checked against crates.io on 2026-11-20 before
implementation: `serde 1.0.229`, `serde_json 1.0.154`, and
`toml 1.2.1+spec-1.1.0` were current stable. `sha2 0.11.1` had become current
stable (the earlier workspace pin was `0.10.9`), so `meld-lib` was moved to the
new exact pin; it declares `rust-version 1.88`, compatible with this workspace's
`1.90`, and the RFC 4231 HMAC vector still passes. The checked-in
`Cargo.lock` records the resolved graph.

Evidence and verification results are in `../status/language-libraries.md`.

## Production hardening amendments (2026-11-20)

`threat-model.md` is normative for the production hardening workstream. It
records the assets, trust boundaries, adversaries, deny-by-default invariants,
tier enforcement rules, and residual platform limitations. It does not claim
completion; runtime evidence is tracked separately in
`../status/production-readiness.md`.

Additional judgment calls (also inline in the affected specs/code):

26. Local HMAC pack signing is replaced by a hard cutover to Ed25519. A
    symmetric dev key cannot be a public trust root; trust is anchored in
    `packs/registry/trust-roots.toml`, rotation adds roots, revocation is
    explicit in `revocations.json`, and registry metadata is bound by a signed
    attestation. Private keys remain outside all repositories and CI artifacts.
27. Untrusted region authority is an intersection
    (`pack maximum ∩ runner constraints ∩ file default grant ∩ region grant`).
    Grants can only narrow, unknown capabilities/traversal are hard errors, and
    an unenforceable declared policy fails closed. T2's protocol requests are
    not enforcement; T3 is hardened independently with no WASI.
28. The workspace MSRV remains 1.90. Wasmtime is pinned to `=36.0.23`
    (declared `rust-version 1.86`) instead of adopting Wasmtime 50, which
    declares `rust-version 1.97.0`. This is a security-compatible downgrade
    that preserves strict T3 limits without raising the supported toolchain.
29. SGLR/SDF3/Raku-slang composition remains a NO-GO. Production parsing uses
    pack-owned, data-only scanner profiles selected by `region_end.toml` and
    implemented as built-in Rust algorithms. Pack `region_end.*` code is never
    executed; raw hash delimiters remain the normative escape hatch.
30. The first public signed release candidate is `v0.2.0-rc.1`, versions follow
    SemVer 2.0.0, and the system package name is `meld-lang` to avoid collision
    with the existing Meld diff tool. Binaries remain `meld`, `meld-lsp`, and
    `meld-pack`.
31. OS sandbox backends are selected per host: Linux uses namespaces, seccomp,
    Landlock where available, cgroups, and a probed OCI fallback; macOS uses
    Seatbelt via `sandbox-exec`; Windows uses Job Objects plus AppContainer. A
    backend must attest the mechanisms required by the effective policy or the
    launch fails closed. Landlock, OCI, Seatbelt, and AppContainer are not
    runtime-proven on this Linux host and require native CI.
32. L1 T2 workers are accepted only for stdout-only `execute`. Value imports,
    exports, inline `eval`, `fetch`, and worker-initiated host calls are L2-only
    and are rejected by the host before dispatch. L1 must advertise `stdout`
    only; L2 must advertise `values`. Host calls received before initialization
    are denied unconditionally because no worker level is trusted yet.
33. `MELD_TEST_UNSANDBOXED=1` is a debug-only escape hatch for fixture tests on
    hosts without namespace/cgroup privileges. It is compiled out of release
    builds, prints a loud warning, and never changes production launch behavior:
    production still probes and enforces the OS policy or fails closed.
34. Manifest roots are canonicalized before policy parsing, so a `MELD_ROOT`
    containing `..` cannot create a path-validation ambiguity. CLI and installer
    temp helpers honor `TMPDIR`/`MELD_TEST_TMP` and do not embed host-specific
    temporary paths.
35. Fuzz coverage is extended to five Rust libFuzzer targets (`splitter`,
    `value_decode`, `sandbox_manifest`, `sandbox_policy`, `t2_envelope`) plus a
    deterministic Python harness for packtool trust roots, revocations, mirrors,
    registry, checksums, and signatures. Stable local smoke runs are evidence
    of crash-freedom only; instrumented nightly CI remains the required
    continuous gate.
36. A stray `⟧` outside an inline island is literal text, not a splitter
    boundary. Only `⟦` starts an inline region; the inline parser locates its
    own close marker. The nightly fuzzer found that the old boundary rule could
    yield a zero-length untagged region and hang on the three-byte input
    `⟧`; the splitter now also guarantees forward progress for untagged runs.
37. Delimiter probes are total over indices at or past the end of input. A Lua
    `--` comment ending at EOF used to make `lua_long(b, i + 2)` read `b[len]`;
    both Lua and SQL delimiter helpers now return `None` at EOF, so a final
    comment is consumed by line-end handling instead of panicking. This came
    from fuzz artifact `crash-151c648844c8db74133fee74eaee420d9c8aabee`
    (Fuzz run 44774908545).
38. cargo-audit policy is single-sourced in `.cargo/audit.toml`, the only config
    path cargo-audit 0.23.0 actually loads from a checkout. CI keeps
    `--deny warnings`; `RUSTSEC-2025-0057` is explicitly ignored until
    2027-02-20 because `fxhash` is reachable only through the deliberately
    pinned `wasmtime 36.0.23 -> fxprof-processed-profile 0.6.0` path. The ignore
    is removed when that pin changes or a runtime-validated dependency update
    eliminates the crate.
39. Windows canonical paths start with a verbatim volume prefix such as
    `\\?\D:`. That prefix is a device object, so `symlink_metadata` on it fails
    with `ERROR_INVALID_FUNCTION` (raw OS error 1). The safe-root walk now skips
    only the `Prefix` and `RootDir` components and still checks every real
    directory component for junctions/symlinks; all other I/O errors remain
    hard errors. This was confirmed from Windows diagnostic job
    136448518092 (workflow run 44782478274).
40. The Windows backend no longer claims AppContainer enforcement. It created
    a profile and DACL entries but launched the child with an ordinary
    `Command::spawn`, so the token was never restricted. Policies needing
    network denial, filesystem isolation or syscall filtering now fail closed
    on Windows instead of running unsandboxed. Job Objects still provide real
    resource limits and process-tree teardown, and the debug-only unsandboxed
    test switch is unchanged.
41. The OCI fallback is a privileged runtime boundary. `runc` is accepted only
    from an absolute `MELD_OCI_RUNC` path; PATH lookup is removed. The probe
    independently checks seccomp, each required namespace, cgroup v2
    cpu/memory/pids controllers, and that `runc kill --all` is advertised, and
    it fails closed if any check is missing. The generated spec now declares
    namespaces explicitly and runs at the caller's uid/gid instead of root.
    Bundles use random 0700 exclusive-create directories; every runc invocation
    is bounded; teardown deletes the container before removing the bundle and
    returns failures through `SpawnedProcess::kill_tree`; all spawn error paths
    attempt cleanup. Privileged runtime tests assert a distinct network
    namespace, empty `CapEff`, active seccomp (`Seccomp: 2`), and invisibility
    of an ungranted host file. Linux CI gates the `oci` feature with Clippy and
    tests. These checks remain ignored in ordinary CI because they need a
    privileged host.
42. macOS exposes `/var` and `/tmp` as OS-managed symlinks into `/private`;
    `std::env::temp_dir()` returns a `/var/folders/...` path. `SafeRoot::new`
    now rewrites only those exact first components before symlink validation,
    so system temp roots work while pack-controlled symlinks in later components
    still fail closed (CI job 136453004367).
43. `MELD_PACKS` is parsed with `std::env::split_paths`, the host path-list
    separator. Splitting on a literal ':' dropped Windows drive letters, so a
    fixture pack in `%TEMP%` on `C:` was searched on `D:` and its scanner
    metadata was silently ignored (CI job 136453004332). The LSP discovery path
    uses the same parser.
44. Windows worker environments retain `SYSTEMROOT`, `TEMP` and `TMP` from the
    explicit allowlist. With a cleared environment, Node could not initialize
    its CSPRNG and aborted with `ncrypto::CSPRNG(nullptr, 0)`. These are OS
    locators, not secrets; all other host variables remain blocked.
45. CI requests a floating Python 3.12 patch instead of 3.12.9, which is absent
    from the Ubuntu 26.04 runner image (CI job 136453004450). The Python
    dependency itself remains pinned in `requirements-ci.txt`.
46. The CI supply-chain job pins `taiki-e/install-action` to the same commit and
    tool versions as `audit.yml` (`cargo-audit@0.23.0`, `cargo-deny@0.22.0`).
    The floating `@cargo-deny` branch installed 0.21.0, which rejects the
    unmaintained `fxhash` advisory even though `deny.toml` is unchanged; the
    pinned version honours the config and passes. No advisory exception was
    added and `--deny warnings` stays in force.
47. The process kill-hook test spawns the current test executable with `--list`
    rather than `/bin/true`. macOS keeps `true` under `/usr/bin` and Windows has
    no `/bin`; because CI aborts at the first failing test binary, this test had
    never actually run on Windows. Using `current_exe()` makes it portable and
    still exercises fail-closed teardown.
48. Windows child argv and cwd are stripped of the `\\?\` extended-length prefix
    that `std::fs::canonicalize` adds, at the OS spawn boundary only. Node 22
    called `realpathSync` on the bare drive component and aborted with
    `EISDIR: illegal operation on a directory, lstat 'D:'` before it could answer
    `initialize`; the T2 10 s deadline was never the cause. Policy checks still
    use the canonical `\\?\` paths for symlink validation. Diagnostic commit
    `ae3527f` captured the worker stderr; `\\?\UNC\` is mapped back to
    `\\server\share`.
49. The 10 s T2 idle/call deadline is deliberately unchanged. The Windows
    worker exited in under 5 s with a deterministic `EISDIR`, so a longer
    timeout would only delay the same failure.
50. Local pack sources are canonicalized for validation, but the verbatim
    `\\?\` prefix is stripped from the string handed to `git`. Git parsed
    `\\?\C:\...` as a URL with host `?`, so Windows installer tests failed with
    `hostname contains invalid characters` before any signature check ran.
    `\\?\UNC\` is mapped back to `\\server\share`. This mirrors judgment call 48
    at the spawn boundary and leaves filesystem checks on canonical paths.
51. Temporary pack clones set `core.autocrlf=false` before checkout. Git for
    Windows otherwise rewrites `checksums.sha256` and `signature.ed25519` to
    CRLF, making the verifier parse paths with a trailing `\r` and report signed
    files as missing. Pack bytes are signed, so no host may normalize them.
52. The installer test fixture repository also sets `core.autocrlf=false` before
    `git add`. `packtool` signs the working-tree bytes; Git normalization on
    commit changed CRLF fixtures to LF in the object store, so a byte-preserving
    clone produced checksum and signature mismatches. Both sides now preserve
    exactly the bytes that were signed.
53. `checksums.sha256` parsing accepts uniformly LF or uniformly CRLF line
    endings and validates the normalized canonical text. Python writes CRLF in
    text mode on Windows and signs those exact bytes; the Rust verifier split
    only on `\n`, so paths became `README.md\r` and were reported missing.
    Mixed line endings and bare carriage returns are rejected rather than
    silently normalized. The signed checksum bytes are still verified byte for
    byte, so this does not weaken signature enforcement.
54. `packtool sign` now recomputes payload digests and refuses a canonical but
    stale `checksums.sha256` instead of publishing a pack that fails on every
    verifier. The installer test fixture refreshes checksums after copying the
    pack so the Ed25519 signature covers the exact working-tree bytes stored by
    Git, regardless of host line endings.
55. The T5 fallback test uses Python for both external stages, and
    `T5Backend::fill` normalizes substituted paths to forward slashes. Windows
    has no `/bin/sh`, cannot execute a shebang script directly, and Git Bash
    consumed backslash paths before crashing on the run stage. Python is
    installed by every CI job (`python` on Windows, `python3` elsewhere), so the
    test still performs a real build/run round trip through an external
    toolchain without host-specific shell behavior. The previous hardcoded shell
    path stayed latent only because earlier failing test binaries stopped the
    run before this test executed.
56. Checked-in pack trees are marked `-text` in `.gitattributes`. Signatures and
    checksums cover exact bytes, but a Windows checkout with `core.autocrlf=true`
    rewrote every payload file to CRLF and made the fixture pack fail with all
    digests mismatched. A repository attribute travels with the code and applies
    to contributors as well as CI; relying on runner-local Git config did not.
57. Python tests write registry JSON through `packtool.canonical_json_bytes`.
    `Path.write_text` emitted CRLF on Windows, so registry canonicality checks
    rejected test fixtures before the intended checksum/attestation assertions
    could run. Writing the same canonical bytes as the production tool keeps the
    test about verification behavior, not host newline policy.
58. `SpawnedProcess::drain_stderr` waits up to a fixed 50 ms grace for the I/O
    pump's `Data`/`Eof` packets after a worker is reaped. The previous
    `try_recv` loop raced the pump and intermittently omitted the stderr tail
    from crash reports (observed as a Windows-only failure in
    `stderr_tail_is_included_in_crash_report`). The wait is bounded, so error
    reporting remains non-blocking within a fixed budget.
59. CLI integration tests take a shared test-only mutex. Several fixtures spawn
    real Node/Python/shell workers; parallel cold starts on Windows made the
    REPL JavaScript worker intermittently exceed the unchanged 10 s T2
    initialize deadline. Serializing these fixtures removes runner contention
    without changing an assertion, a timeout, or a sandbox policy.

Dependency/toolchain versions used by these amendments were re-checked against
official sources: `ed25519-dalek 3.0.0` and `rand_core 0.10.1` both declare
`rust-version 1.85` and are not yanked; Python `cryptography 50.0.2` supports
Python 3.9+ excluding 3.9.0/3.9.1; `wasmtime 36.0.23` declares
`rust-version 1.86.0`, while `wasmtime 50.0.3` declares `1.97.0`.
