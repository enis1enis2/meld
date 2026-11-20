# Meld fuzz workspace

This is a standalone Cargo workspace (the empty `[workspace]` table in
`Cargo.toml`) so the main workspace lockfile and normal builds are unaffected.
Run a target from this directory, for example:

```sh
cd fuzz
cargo +nightly fuzz run splitter -- \
  -max_total_time=60 -max_len=4096 -timeout=5 -artifact_prefix=artifacts/
```

## Targets

- `splitter` — split/render byte-exactness and region identity stability.
- `value_decode` — canonical `Value` wire round-trip and source-literal renderers.
- `sandbox_manifest` — pack/runner manifest validation and audit fingerprint.
- `sandbox_policy` — grants only narrow; re-intersection is fingerprint-stable.
- `t2_envelope` — NDJSON request/response/notification identity round-trips.

`tools/fuzz_packtool.py` covers Python release/decoder boundaries that the Rust
targets cannot reach. `tools/fuzz_splitter.py` is a bounded Python
reference/differential model, not a Rust fuzzer.

## Sanitizer evidence

**The CI `Fuzz` workflow's nightly/default-sanitizer runs are the authoritative
sanitizer evidence.** `cargo-fuzz` requires a nightly toolchain, and the
workflow runs the targets with the default sanitizer enabled.

A local run built with `--sanitizer none` is a coverage/smoke check **only**. It
is not AddressSanitizer evidence and must not be reported as such. On at least
one host the default ASAN startup failed with
`heap size 40000002000 exceeds max user virtual address 7fffffffff`; that is a
virtual-address-space constraint of the environment, not a Meld defect.

Failing inputs are uploaded as CI artifacts (`fuzz/artifacts`, `if: failure()`)
and are never committed. The generated `corpus/`, `artifacts/`, `coverage/` and
`target/` directories are ignored by this workspace's `.gitignore`.

## Deferred

Typed `Arbitrary`-derived inputs for structured targets are deferred: adding
the `derive` feature would require manifest and lockfile changes
(`derive_arbitrary` is not currently in `fuzz/Cargo.lock`), which is outside
this safe hardening pass. The existing raw `&[u8]` targets already cover the
decoders' panic-freedom and round-trip invariants.
