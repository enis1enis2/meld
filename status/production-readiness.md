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

CI run links: pending first push of `production/baseline`.

## 2. Security

Status: NOT STARTED.

Planned evidence: threat model, OS sandbox enforcement tests, escape-attempt
suite, Ed25519 signing verification, revocation/rotation tests, cargo audit/deny
output, fuzz targets for every parser/decoder boundary.

## 3. Execution coverage

Status: NOT STARTED.

Planned evidence: per-language support matrix with executed conformance cases,
T3 generalization, T1/T4 written decisions, pairwise L2 round-trip matrix.

## 4. Parsing and correctness

Status: NOT STARTED.

Planned evidence: pack-owned scanner registration, heredoc/indentation/raw
string fixtures, extended fuzz corpus, differential testing, Miri/sanitizers.

## 5. Tooling

Status: NOT STARTED.

## 6. Performance and reliability

Status: NOT STARTED.

## 7. Documentation

Status: NOT STARTED.

## 8. Final audit

Status: NOT STARTED.
