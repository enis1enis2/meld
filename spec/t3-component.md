# T3 Component and T5 Fallback Specification (Phase 4)

Status: normative for Phase 4+. Derived from `project.md` §5 (execution tiers),
§6 (value model / WIT IDL), §9 (sandboxing), and §13 (reuse WIT). Implemented by
`crates/meld-t3`; every **JUDGMENT CALL** below is binding until a later phase
changes it.

The Phase 4 done-when criterion is "a Rust function is called from Python with
typed arguments". The proof is `crates/meld-t3/tests/python_bridge.rs` and
`examples/07-t3-bridge.meld`, which print `42` for both `add` and `mul`.

## 1. Layout

```
crates/meld-t3/
  wit/meld.wit                 # the IDL; source of truth for export names
  guest/                       # no_std Rust guest -> core wasm
  component/demo.component.wasm # committed component used by tests
  src/lib.rs                   # Wasmtime host + T5 runner
  tests/                       # typed calls, T5, Python -> T2 -> T3 bridge
```

`tools/build_t3_guest.sh` is the reproducible regeneration path. It requires
`wasm32-unknown-unknown` and `wasm-tools`; `WASM_TOOLS` can override the binary
location. **JUDGMENT CALL:** the component binary is committed so
`cargo test --workspace` needs neither the wasm target nor `wasm-tools`
installed. Tests use `include_bytes!` and instantiate it directly.

## 2. WIT is the IDL

```wit
package meld:demo@0.1.0;

interface ops {
  add: func(a: s64, b: s64) -> s64;
  mul: func(a: s64, b: s64) -> s64;
}

world guest {
  export ops;
}
```

**JUDGMENT CALL:** WIT is reused directly instead of inventing a Meld IDL
(`project.md` §6/§13). The demo starts with `s64` scalars so the Canonical ABI
boundary is visible without an allocator; richer Meld Values map to WIT
records/variants later. The hand-written guest uses canonical export
attributes rather than `wit-bindgen`, but the boundary contract is identical:
a production Rust pack generated with `wit-bindgen` would expose the same names.

The core module is embedded with `wasm-tools component embed` and wrapped with
`wasm-tools component new`; `wasm-tools validate` is the final build check.

## 3. Host API and calling convention

`T3Backend::from_bytes` decodes any component; `T3Backend::demo` loads the
committed one. `call_i64_pair(export, a, b)` instantiates a fresh `Store` per
call and uses Wasmtime's typed Component API.

Two details are easy to get wrong and are therefore pinned here:

- **JUDGMENT CALL:** componentizing a WIT interface nests functions under an
  interface export. The host resolves `meld:demo/ops@0.1.0#add` in two steps:
  first the interface index, then `add` inside it. Flat core-Wasm export lookup
  is not sufficient.
- **JUDGMENT CALL:** Wasmtime models a single component result as a named list.
  The typed function is `func(a: s64, b: s64) -> s64` in WIT but
  `get_typed_func::<(i64, i64), (i64,)>` in Rust; the unary tuple must be
  destructured.

`call_value("meld:t3/add", &[Value, Value])` is the bridge-facing entry point.
It checks arity and uses `Value::as_i64_checked`, so a big Meld int fails with a
typed `TypeMismatch` rather than wrapping silently (`spec/value-model.md` §2).
**JUDGMENT CALL:** the `meld:t3/*` namespace is reserved so a pack cannot
accidentally bind an ordinary export to a host callback.

## 4. Python → T2 → T3 bridge

The Python pack exposes `meld_call(name, *args)`. It sends a worker-initiated
`__host_call` with negative id while the original `execute` is still in flight
(`spec/t2-protocol.md` §3.1). The Rust host handles the callback inline through
the shared `T3Backend` and continues waiting for the worker's response.

`meld run` loads one `T3Backend` per execution plan and shares an `Arc` with
every T2 worker through `T2Backend::set_host_call`. **JUDGMENT CALL:** one shared
engine/component avoids repeated component instantiation and keeps T2 workers
independent of Wasmtime; no worker needs the wasm target or host libraries.

## 5. T5 toolchain fallback

`T5Backend::new(lang, build, run, cwd)` takes argv vectors, not shell strings.
`{src}`, `{out}` and `{dir}` placeholders are substituted with a per-call temp
directory; JSON arguments are appended as one trailing argv value so quotes and
spaces cannot be reinterpreted. A final stdout line beginning `@meld ` is parsed
as a Meld Value; remaining lines are ordinary ordered stdout.

**JUDGMENT CALL:** this keeps T5 compatible with the T2 stdout contract while
giving compiled regions a typed return path. T5 never downloads or installs a
toolchain; `meld doctor` only reports what is present (project.md §9).

## 6. Security

T3 gives real Wasm sandboxing: no filesystem or network access unless the host
adds imports, which the demo component has none. T5 and T2 rely on OS-level
confinement declared in `runner.toml`; the Component Model copy boundary does
not apply to them (project.md §9). Errors cross as structured values, never
raw host panics.

## 7. Verification

```sh
cargo test -p meld-t3
cargo run -p meld-cli -- run examples/07-t3-bridge.meld
```

Expected output:

```
add(20, 22) = 42
mul(6, 7) = 42
```

`tools/build_t3_guest.sh` regenerates the binary after any guest or WIT change;
the committed artifact must be rebuilt in the same commit as its source.

## 8. Judgment call index

1. Reuse WIT directly; start with `s64` scalars (§2).
2. Commit the component so tests are toolchain-independent (§1).
3. Resolve componentized interface exports and unary result tuples explicitly (§3).
4. Reserve `meld:t3/*` for checked host callbacks and share one T3 backend per run (§3–§4).
5. T5 uses argv vectors, placeholder substitution and an `@meld` result line (§5).
