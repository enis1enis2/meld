# Schemas

Stable in 0.2.x: `pack.toml`, `meld-lang-library/1`,
`meld-lang-library-index/1`, `meld-project-libraries/1`,
`meld-pack-registry/1`, `meld-grammar-sources/1`,
`meld-release-manifest/1`, value wire tags, the T2 envelope and the T3 WIT
world. Unknown schema identifiers are rejected. Migrations are explicit via
`tools/migrate_lock.py` and `tools/migrate_index.py`.

See the canonical [schema-stability.md](https://github.com/enis1enis2/meld/blob/main/docs/schema-stability.md)
and [SEMVER.md](https://github.com/enis1enis2/meld/blob/main/SEMVER.md).
