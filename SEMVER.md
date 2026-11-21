# Semantic versioning policy

Meld declares [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
Given `MAJOR.MINOR.PATCH`, pre-release identifiers follow `-` and build
metadata follows `+`, but Meld no longer uses pre-release identifiers for
published release tags; the stable-tag rule is enforced by the release
workflow. The current stable target is `v0.2.0`. The historical
`v0.2.0-rc.5` release is retained as published history; under SemVer
`0.2.0-rc.5` ranks below `0.2.0`.

## Public contract

The following are part of the versioned contract:

| Surface | Schema / location | Stability |
| --- | --- | --- |
| CLI commands and exit codes | `meld` | see [docs/schema-stability.md](docs/schema-stability.md) |
| Language server capabilities | `meld-lsp` | see docs/schema-stability.md |
| Lang Pack manifest | `pack.toml` | stable in 0.2.x |
| Library manifest | `library.toml`, `meld-lang-library/1` | stable in 0.2.x |
| Library index | `meld-lang-library-index/1` | stable in 0.2.x; see [docs/schema-stability.md](docs/schema-stability.md) |
| Grammar sources | `meld-grammar-sources/1` | stable in 0.2.x; see [docs/schema-stability.md](docs/schema-stability.md) |
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
  avoided before stable publication.
- **MINOR** — backward-compatible functionality: new CLI subcommand or flag,
  new optional schema field with a default, new language pack, new LSP
  capability.
- **PATCH** — backward-compatible fixes, dependency pin updates that do not
  change behavior, documentation and packaging fixes.
- **Pre-release** — SemVer permits `alpha`, `beta` and `rc`, but Meld does not
  publish pre-release tags. Validate candidates in branches or workflow runs;
  do not create `-rc.N`, `-alpha`, or `-beta` tags.

## Dependency pins

Pinned grammar versions in `grammar-sources.json` are part of the packaged data
contract for a given `MINOR`. Changing a pin is a MINOR change unless the pack
schema or query format changes, in which case it is MAJOR.

## Tagging

Release tags are `vMAJOR.MINOR.PATCH` only. A tag containing `-`, `+`, or any
other pre-release/build suffix is rejected by the release workflow. The tag must
match the release manifest `version` field (without the leading `v`) and the
signed checksums. The regex in `scripts/generate_release_manifest.py` validates
SemVer 2.0.0 syntax only, including pre-release and build suffixes; the workflow
gate enforces the stable-tag policy. The historical `v0.2.0-rc.5` tag is
retained as published history, not a pattern for new releases.
