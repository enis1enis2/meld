# Changelog

All notable changes to Meld are documented here. Meld follows
[Semantic Versioning 2.0.0](SEMVER.md).

## [Unreleased]

- Release engineering assets: signed, reproducible release pipeline, install
  channels, SBOM/provenance, dependency auditing and bounded libFuzzer targets.
- No source or wire-format changes are planned for `0.2.0-rc.1`; the release
  candidate exists to validate the release process end to end.

## [0.2.0-rc.1] - planned first signed candidate

This is the first candidate intended to be signed with Sigstore keyless
signing and published from CI. It is a release candidate, not a final release.
The workspace `Cargo.toml` still declares `0.1.0`; bumping the workspace
version requires a separate authorized change and is not done here.

### Added

- Native Linux, macOS and Windows release binaries: `meld`, `meld-lsp`,
  `meld-pack`.
- Bundled Lang Pack data, licenses and documentation in every archive.
- Deterministic `.tar.gz` (Unix) and `.zip` (Windows) archives built with
  `--locked` and a fixed `SOURCE_DATE_EPOCH`.
- CycloneDX 1.6 SBOM, SHA256SUMS, signed release manifest, cosign bundles and
  GitHub build-provenance attestations.
- Install channels: shell installer, PowerShell installer, Homebrew formula
  (`meld-lang`), Scoop bucket, Debian source (`meld-lang`), Nix flake and Docker
  image.

### Changed

- The system package name is `meld-lang` to avoid a collision with the existing
  `meld` diff tool. Binary names remain unchanged.

### Notes

- macOS/Windows builds, the public release upload and public signature
  verification are CI obligations; they are not claimed to have run in the
  environment where these assets were authored.
