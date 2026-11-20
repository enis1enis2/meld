# JavaScript T2 pack (L2)

- `worker.js`: Node worker using `vm.runInContext` with a persistent namespace.
- `marshal.js`: Meld Value codec; plain objects encode as `$map`.
- `region_end.js`: pack-owned scanner stub (host uses generic scanner).
- `tests/conformance.json`: behavioral cases.

**JUDGMENT CALL:** `vm.runInContext` discards top-level `const`/`let` bindings
after a script completes, so regions that export a value must assign it to
`globalThis.<name>`. L1 regions that only print need no convention.

Verification: `python3 tools/t2_ref.py` passes all JavaScript cases and the
full 3×3 value round-trip matrix.
