# Meld Value Model Specification (Phase 0)

Status: normative for Phases 3+. Derived from `project.md` §6. The wire encoding
below is the T2 JSON envelope; WIT is reused as the IDL per project.md §13.

## 1. Type set

| Meld type | JSON wire form | WIT equivalent | Notes |
|---|---|---|---|
| `null` | `null` | `option<none>` sentinel | single value |
| `bool` | `true`/`false` | `bool` | |
| `int` | decimal string, e.g. `"42"` | `s64`/`u64` when in range | arbitrary precision; string avoids JSON f64 loss |
| `float` | JSON number | `float64` | `NaN`/`Inf` serialized as strings `"NaN"`, `"Inf"`, `"-Inf"` |
| `string` | JSON string | `string` | UTF-8, invalid surrogate rejected |
| `bytes` | base64 string tagged `{"$bytes":"..."}` | `list<u8>` | explicit tag distinguishes from string |
| `list<T>` | JSON array | `list<T>` | homogeneous |
| `map<string,V>` | JSON object with `{"$map":{...}}` | `list<tuple<string,V>>` | WIT has no native map |
| `record{...}` | JSON object | `record` | field order preserved |
| `variant` | `{"$variant":{"case":"x","value":...}}` | `variant` | |
| `option<T>` | `null` or T, tagged `{"$some":T}` when T is `null` | `option<T>` | |
| `result<T,E>` | `{"$ok":T}` / `{"$err":E}` | `result<T,E>` | errors cross as values, never host exceptions |
| `handle<R>` | `{"$handle":{"id":"h7","resource":"R","borrow":true}}` | `resource` / `borrow<R>` | owned handle dropped on region exit |
| `fn` | `{"$fn":"region-id:export-name"}` | `func` | callable handle into another region |

**JUDGMENT CALL:** JSON was chosen for the T2 wire because every language with a
CLI can emit it; the canonical ABI/WIT mapping is documented so T3 can later
replace the envelope without changing the bridge semantics (project.md §5/§6).

## 2. Numeric rules (documented, not hidden)

- `int` larger than target width raises `result::err(NumericOverflow)` in the
  receiver; never wraps silently.
- `float → int` requires exact integral value; otherwise `result::err(NotIntegral)`.
- `NaN`/`Inf` are typed strings on the wire; a receiver without IEEE-754 must
  surface them as `result::err(NonFinite)` rather than coercing.
- Narrowing `s64 → u32` checks range. Big ints beyond `s64` stay strings and may
  only be consumed by packs declaring `int: arbitrary`.

## 3. Copy and lifetime

- Default is copy-by-value across region boundaries (project.md §6, Component
  Model precedent). No shared mutable state.
- `bytes` ≥ `BULK_THRESHOLD` (default 1 MiB) should be sent as a shared-buffer
  handle when the pack declares `bulk-buffer`; otherwise copied.
- Owned handles are invalidated at region exit; the scheduler emits a `drop`
  call for each. Borrowed handles are only valid during a synchronous `call`.
- Handle ids are scoped per execution plan, never global.

## 4. Scoping and bridge namespace

Regions have private namespaces. `export x as y` places `y` in the bridge
namespace; `import y from <region-id>` binds it locally. Name collisions are a
check-time error (`E_BRIDGE_COLLISION`). No implicit leakage between regions.

## 5. Error taxonomy (wire codes)

`ParseError`, `SplitError`, `UnknownLang`, `ToolchainMissing`,
`WorkerCrashed`, `NumericOverflow`, `NotIntegral`, `NonFinite`,
`TypeMismatch`, `HandleExpired`, `BridgeCycle`, `SandboxDenied`.
Every error object carries a source-map entry back to the `.meld` location.

## 6. conformance cases (Phase 3)

Generated matrix `A → B` for `A,B ∈ {python, javascript, lua}` at level L2,
using the values above. Generated, not hand-written (project.md §8).

## Sources

- `project.md` §6 and §13 (design proposals).
- WebAssembly Component Model / WIT reference and Canonical ABI (verified current).
