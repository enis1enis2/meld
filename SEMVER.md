# Semantic versioning policy

Meld declares [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
Given `MAJOR.MINOR.PATCH`, pre-release identifiers follow `-` and build
metadata follows `+`. The first published signed candidate is `v0.2.0-rc.3`;
`rc.1` is a pre-release identifier, so it ranks below `0.2.0`.

## Public contract

The following are part of the versioned contract:

| Surface | Schema / location | Stability |
| --- | --- | --- |
| CLI commands and exit codes | `meld` | see [docs/schema-stability.md](docs/schema-stability.md) |
| Language server capabilities | `meld-lsp` | see docs/schema-stability.md |
| Lang Pack manifest | `pack.toml` | stable in 0.2.x |
| Library manifest | `library.toml`, `meld-lang-library/1` | stable in 0.2.x |
| Registry index | `meld-pack-registry/1` | stable in 0.2.x |
| Project lock | `meld-project-libraries/1` | stable in 0.2.x |
| Value wire tags | `spec/value-model.md` | stable in 0.2.x |
| T2 job envelope | `spec/t2-protocol.md` | stable in 0.2.x |
| T3 component interface | `spec/t3-component.md`, `wit/meld.wit` | stable in 0.2.x |
| Release manifest | `meld-release-manifest/1` | stable in 0.2.x |

Internal module paths, test fixtures, examples, scripts and status files are
not part of the contract.

## Increment rules

- **MAJOR** — incompatible change to any stable surface above, including
  removal of a CLI command, a wire tag, or a schema field required to read old
  data. While `MAJOR` is `0`, breaking changes increment `MINOR` and are
  avoided in release candidates.
- **MINOR** — backward-compatible functionality: new CLI subcommand or flag,
  new optional schema field with a default, new language pack, new LSP
  capability.
- **PATCH** — backward-compatible fixes, dependency pin updates that do not
  change behavior, documentation and packaging fixes.
- **Pre-release** — `alpha`, `beta`, `rc` identifiers may change without the
  stability guarantees above, but a candidate must not introduce a breaking
  change silently. Changes to the contract after `rc.1` require a new candidate
  with a higher pre-release identifier.

## Dependency pins

Pinned grammar versions in `grammar-sources.json` are part of the packaged data
contract for a given `MINOR`. Changing a pin is a MINOR change unless the pack
schema or query format changes, in which case it is MAJOR.

## Tagging

Release tags are `vMAJOR.MINOR.PATCH` for final releases and
`vMAJOR.MINOR.PATCH-rc.N` for candidates. The tag must match the release
manifest `version` field (without the leading `v`) and the signed checksums.
