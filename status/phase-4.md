# Phase 4 status — compiled tier

> **Historical phase record.** The Wasmtime version below is the Phase 4
> snapshot, not the current pin. Current authority is judgment call 26 (Ed25519
> pack trust), judgment call 28 (MSRV `1.90`, Wasmtime `=36.0.23`) and judgment
> call 45 (floating Python `3.12`) in `../spec/README.md`, with final evidence
> in [production-readiness.md](production-readiness.md).

## Done
- `crates/meld-t3/wit/meld.wit` reuses WIT directly as the cross-language IDL
  (`meld:demo/ops@0.1.0`, `add`/`mul` over `s64`).
- `crates/meld-t3/guest/` is a real `no_std` Rust guest compiled to core Wasm and
  wrapped as a Component Model component by `tools/build_t3_guest.sh`. The
  committed artifact is `crates/meld-t3/component/demo.component.wasm`.
- **Historical (Phase 4):** `T3Backend` instantiated the component with
  Wasmtime 50.0.3, resolved nested interface exports, and called typed
  `(i64, i64) -> (i64,)` functions. The current pin is Wasmtime `=36.0.23`
  with MSRV `1.90` (judgment call 28); see
  [production-readiness.md](production-readiness.md).
- `call_value` maps `meld:t3/add` / `meld:t3/mul` from Meld Values with checked
  `s64` narrowing; oversized integers fail as typed errors, never wrap.
- Python `meld_call` sends negative-id `__host_call` callbacks to the Rust host;
  the host resolves them through T3 while the original worker request is still
  in flight.
- `meld run` loads one `T3Backend` per execution plan and shares it with every
  T2 worker through a host callback.
- `T5Backend` compiles/runs a region from pack-declared argv templates with
  `{src}`, `{out}`, `{dir}` placeholders and an `@meld` JSON result line. It
  never installs or downloads a toolchain.
- Spec added: `spec/t3-component.md`.

## Verification
- `cargo test -p meld-t3`: `rust_component_function_is_callable_with_typed_args`,
  `t5_toolchain_fallback_compiles_and_runs_a_region`, and
  `python_meld_call_reaches_rust_wit_component` pass.
- `meld run examples/07-t3-bridge.meld` prints:

  ```
  add(20, 22) = 42
  mul(6, 7) = 42
  ```

## Judgment calls
- WIT is reused rather than inventing an IDL; scalar `s64` keeps the Canonical
  ABI boundary explicit before richer values are mapped.
- The component binary is committed so `cargo test --workspace` does not require
  the wasm target or `wasm-tools`; the build script is the regeneration path.
- Componentized exports and unary result tuples are resolved explicitly instead
  of assuming flat core-Wasm exports.
- One shared T3 engine/component per run; reserved `meld:t3/*` callback names.
- T5 uses argv vectors and a final typed result line to stay compatible with
  the ordered T2 stdout contract.

## Blockers
None.

## Done-when assessment (historical Phase 4)
"Rust function called from Python with typed args": satisfied at that time by
the real Python worker → T2 callback → Wasmtime → WIT component test and the CLI
example. Final toolchain and trust evidence is in
[production-readiness.md](production-readiness.md).
