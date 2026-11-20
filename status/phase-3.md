# Phase 3 status

## Done
- Full Meld Value Model codecs for Python, JavaScript and Lua
  (null, bool, arbitrary-precision int, float, string, bytes, list, map,
  record-as-map, variant, option, result, handle, fn).
- Import/export through the shared bridge, verified with the real Rust CLI:
  `meld run examples/06-bridge.meld` prints
  `js doubled: 2,4,6` and `lua received 3 values`.
- Structured error propagation verified for all three workers.
- Generated 3×3 round-trip matrix in `tools/t2_ref.py`, 15 canonical values per
  pair, all green:

  ```
  python->python / javascript / lua: ok
  javascript->python / javascript / lua: ok
  lua->python / javascript / lua: ok
  t2 reference harness: 0 problem(s)
  ```

- Lua L2 pack (worker, codec, manifest, runner/toolchain, conformance cases).
- `cargo test --workspace`: 8 tests passed (2 splitter, 1 T2 bridge, 5 value).

## Judgment calls
- Plain dict/object/table encodes as `$map` in every pack so wire shapes are
  stable across languages; explicit wrapper classes/tables cover the tagged
  types.
- Decimal-digit strings decode as `int`; numeric-looking Meld strings are a
  documented wire ambiguity and are excluded from the test vectors.
- Lua integers are 64-bit: decimal ints that cannot be represented exactly stay
  decimal strings, preserving arbitrary precision.
- Lua byte strings need `Meld.bytes(...)` wrapper because Lua has no distinct
  byte-string type.
- Execution remains sequential in file order (project.md §13 default); async
  regions are still reserved for after Phase 3.

## Blockers
None. All three interpreters and the Rust toolchain are installed and exercised.

## Done-when assessment
"Round-trip matrix green for those 3": satisfied — 9 pairs × 15 values, plus
real cross-language execution through `meld run`. Stopping after Phase 3 as
instructed.
