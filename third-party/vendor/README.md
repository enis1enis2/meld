# Offline grammar vendor directory

Vendored grammar sources are optional build inputs, not committed source of
truth. Generate them from a populated Cargo registry cache:

```sh
python3 tools/vendor_grammars.py --registry-src "$CARGO_HOME/registry/src"
```

The tool reads `../grammars.toml`, verifies each crate directory against the
recorded Cargo checksum where possible, copies the grammar sources and queries,
and writes a deterministic `vendor-manifest.json` with per-file SHA256 values.
Nothing is downloaded implicitly; no placeholder archive is treated as real.

Do not edit files here by hand. Change the pin in `packs/<lang>/pack.toml`,
`crates/meld-grammars/Cargo.toml`, `grammar-sources.json` and
`third-party/grammars.toml` together, then regenerate.
