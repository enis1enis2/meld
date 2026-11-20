# Lua T2 pack (L2)

- `worker.lua`: Lua 5.4 worker using `load(code, name, "t", namespace)`.
- `marshal.lua`: Meld Value codec with explicit wrappers for ambiguous Lua
  values (`Meld.bytes`, `Meld.map`, `Meld.variant`, ...).
- `region_end.lua`: pack-owned scanner stub; long strings/comments need raw
  hash delimiters until Wasm scanners are wired.
- `tests/conformance.json`: behavioral cases.

**JUDGMENT CALL:** plain arrays encode as Meld lists; other plain tables encode
as `$map`. Decimal ints beyond 64 bits stay decimal strings to preserve
arbitrary precision.

Verification: `python3 tools/t2_ref.py` passes all Lua cases and the full 3×3
value round-trip matrix.
