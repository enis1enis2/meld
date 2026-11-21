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
- The bundled Python pack now launches through `py -3` or `python` on Windows
  when `python3` is absent, without changing signed pack bytes. `meld doctor`
  reports the interpreter that will actually be used.
- Added a tuned release profile and `cargo rbuild`/`rtest`/`rcheck`/`rclippy`
  aliases so optimized binaries are a single command away on every platform.
- Both installers now detect and install missing language runtimes and cosign
  by default, with `--skip-dependencies`/`-SkipDependencies` to opt out.

## [0.2.0-rc.3] - 2026-11-21

This is the first published candidate signed with Sigstore keyless signing and
published from CI. It is a release candidate, not a final release. The workspace
version, lockfiles, notices and package metadata all report `0.2.0-rc.3`.

- The `v0.2.0-rc.1` and `v0.2.0-rc.2` tags were created but never published.
  `rc.1` failed before uploading any asset because the tag gates did not install
  the pinned `cryptography` dependency and Windows packaging used an unsupported
  `os.utime(follow_symlinks=False)` call. `rc.2` fixed those but its archives
  lacked the top-level payload directory, so install-layout verification failed.
  Both fixes are included here, so `rc.3` supersedes `rc.1` and `rc.2`.

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

- Native macOS, Windows and Linux builds, the public release upload and public
  signature verification are performed by the release workflow, not by the
  local dry run documented in [RELEASE.md](RELEASE.md).
- GitHub build attestations are not a SLSA level certification; no SLSA level
  is claimed.
