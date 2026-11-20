# Phase 2 status

## Done
- T2 protocol extended with `fetch` (glue-declared exports) and an explicit
  `exports:[{name,value}]` shape for worker-pushed values
  (`spec/t2-protocol.md`).
- Rust host (`crates/meld-t2`): `T2Backend::fetch`, shared bridge, export
  fetch after `execute`, structured error mapping.
- `meld-cli`: `meld run` now strips glue, dedents region bodies, keeps one
  `Bridge` for the whole plan, and lazily spawns one worker per language.
- Python pack: `worker.py` + `meld_marshal.py`, manifest, runner/toolchain
  config, end-scanner stub, conformance cases.
- JavaScript pack: `worker.js` + `marshal.js`, manifest, runner/toolchain
  config, end-scanner stub, conformance cases.
- `tools/t2_ref.py`: real workers over stdio; stdout capture and ordering,
  error propagation, and value round-trips.
- `examples/05-t2-run.meld` executed by the real `meld` binary:

  ```
  python region one
  python region two
  javascript region one
  javascript region two
  ```

- `cargo test --workspace` passes, including
  `crates/meld-t2/tests/worker_bridge.rs` which spawns the Python worker.

## Judgment calls
- Glue exports are authoritative: host calls `fetch(name)` and publishes under
  the `as` bridge name; worker-pushed `exports` are an optional shortcut.
- Worker exceptions are response-level structured errors so the worker stays
  alive; Meld `result<T,E>` values are ordinary encoded values.
- One worker per language, spawned lazily; no worker pool until Phase 3+.
- Region bodies are dedented before execution (Python indentation).
- The Python codec module is `meld_marshal.py` to avoid Python's built-in
  `marshal` module.
- JS exporting regions assign to `globalThis.<name>` because `vm.runInContext`
  discards top-level `const`/`let` bindings.

## Blockers
None. Toolchains were installed after an explicit user override; B1/B2 from
Phase 1 are resolved (see `spec/README.md`).
