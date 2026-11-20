# Changelog

All notable changes to Meld are documented here. Meld follows
[Semantic Versioning 2.0.0](SEMVER.md).

## [Unreleased]

- Release engineering assets: signed, reproducible release pipeline, install
  channels, SBOM/provenance, dependency auditing and bounded libFuzzer targets.
- Release now calls the unified CI and docs workflows as gates, while keeping
  tag-specific grammar, schema and pack verification.
- Native release matrix gained Intel macOS (`x86_64-apple-darwin`) and asserts
  the exact four-target set instead of a hard-coded pair count. Linux aarch64
  is explicitly not published; installers fail clearly on that host.
- The release manifest is generated before `SHA256SUMS` and no longer embeds a
  stale signature list; strict checksum coverage is verified after signing.
- Sigstore identities are anchored to the exact release tag with escaped regex
  metacharacters, and release verification checks the GitHub attestation bundle
  with `gh attestation verify --bundle` before publishing.
- Package-channel guards were tightened: Scoop uses `env_set`, Debian wording
  no longer implies a signed source package, and release docs require a real
  committed `flake.lock`.
- No source or wire-format changes are planned for `0.2.0-rc.1`; the release
  candidate exists to validate the release process end to end.

## [0.2.0-rc.1] - planned first signed candidate

This is the first candidate intended to be signed with Sigstore keyless
signing and published from CI. It is a release candidate, not a final release.
The workspace `Cargo.toml` still declares `0.1.0`; bumping the workspace
version requires a separate authorized change and is not done here.

### Added

- Native Linux x86_64, macOS ARM64, macOS x86_64 and Windows x86_64 release
  binaries: `meld`, `meld-lsp`, `meld-pack`.
- Bundled Lang Pack data, licenses and documentation in every archive.
- Deterministic `.tar.gz` (Unix) and `.zip` (Windows) archives built with
  `--locked` and a fixed `SOURCE_DATE_EPOCH`.
- CycloneDX 1.6 SBOM, SHA256SUMS, deterministic release manifest, cosign bundles
  and GitHub build-provenance attestations.
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
- GitHub build attestations are not a SLSA level certification; no SLSA level
  is claimed.
