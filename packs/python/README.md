# Python T2 pack (L2)

- `pack.toml` / `runner.toml` / `toolchain.toml`: manifest and launch profile.
- `worker.py`: newline-delimited JSON worker (spec/t2-protocol.md).
- `meld_marshal.py`: Meld Value codec. Named this way because Python's stdlib
  has a built-in `marshal` module that would shadow a same-named file.
- `region_end.py`: pack-owned scanner stub; the host still uses its generic
  balanced-brace scanner (see file header).
- `tests/conformance.json`: behavioral cases.

Verification: `python3 tools/t2_ref.py` (from the project root) passes all
Python cases and the full 3×3 value round-trip matrix.
