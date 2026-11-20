# Third-party components

Meld bundles translated Lang Packs whose queries are copied from pinned
upstream tree-sitter grammar crates. The authoritative pin list is
[`../grammar-sources.json`](../grammar-sources.json), generated from
`packs/*/pack.toml` and verified against Cargo's checksums in `Cargo.lock`.

- `grammars.toml` — machine-readable pin list used by the vendor tool.
- `overrides.toml` — upstream crate substitutions and mechanical query fixups.
- `vendor/` — optional offline vendor output; see [vendor/README.md](vendor/README.md).

Every entry records an exact version and a `cargo_checksum`. The checker
(`tools/check_grammar_sources.py`) fails when a value is `TODO_VERIFY`, when a
pin disagrees with a pack manifest or `crates/meld-grammars/Cargo.toml`, or when
a checked-in query file no longer matches its recorded hash. No hash is
invented: release checksums come from the signed artifacts, and crate checksums
come from Cargo's verified lockfile.

Rust dependency licenses are summarized in
[`../THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md), generated from the
same lockfile with `tools/generate_third_party_notices.py`.
