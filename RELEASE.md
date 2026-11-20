# Release process

The first signed candidate is `v0.2.0-rc.1`. Meld follows SemVer 2.0.0
([SEMVER.md](SEMVER.md)). This document describes what CI does and what a human
must verify before a tag is considered good. Nothing here claims a release has
already been published.

## Preconditions

1. `CHANGELOG.md` has an entry for the candidate and the version passes the
   SemVer 2.0.0 regex used by `scripts/generate_release_manifest.py`.
2. All required docs, licenses and Lang Pack data are present; the packager
   fails closed when any are missing.
3. Dependency policy files (`deny.toml`, `audit.toml`, `about.toml`) and grammar
   pins (`grammar-sources.json`) pass their checks. Any `TODO_VERIFY` value
   fails the grammar checker by design.
4. The workspace version bump is a separate, authorized change. Until it lands,
   the candidate tag and manifest version describe the release while Cargo
   metadata still reports `0.1.0`; note this in release notes.

## CI pipeline (`.github/workflows/release.yml`)

A `v*` tag triggers, in order:

1. **gates** — `cargo fmt --check`, `cargo clippy --workspace --all-targets
   -- -D warnings`, `cargo test --workspace --locked`, `cargo doc`, Python unit
   tests, reference harnesses and pack verification.
2. **audit / deny / notices** — `cargo audit`, `cargo deny check`, and
   third-party notice generation check.
3. **msrv** — Rust 1.90 (the workspace `rust-version`) checks the workspace.
4. **fuzz** — real, bounded libFuzzer runs (`60s` per target, `4096` byte max
   input); failing inputs are uploaded as artifacts, never committed.
5. **native-matrix** — release binaries built on Ubuntu, macOS and Windows
   runners (not cross-compiled from Linux). Each runner builds and packages
   twice under one `SOURCE_DATE_EPOCH` and uploads a canonical archive plus
   the two build pairs.
6. **reproducible** — the uploaded build pairs are compared again in a separate
   job; a mismatch fails the release.
7. **sbom** — `cargo-sbom` CycloneDX 1.6 output, normalized for determinism.
8. **sign** — cosign keyless signatures and bundles for SHA256SUMS, the
   manifest, the SBOM and each archive; each signature is verified immediately.
9. **provenance** — GitHub build-provenance attestations for the release files.
   This is **not** a SLSA L3 claim; no SLSA level is asserted.
10. **release** — `gh release create`, prerelease for `-rc` tags, with all
    archives, checksums, manifest, SBOM, signatures, bundles and attestations
    attached.

## Local dry run

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
sh scripts/build_matrix.sh --version 0.2.0-rc.1 --targets "$(rustc -vV | sed -n 's/^host: //p')"
sh scripts/package_release.sh --version 0.2.0-rc.1 --target "$(rustc -vV | sed -n 's/^host: //p')"
sh scripts/check_reproducibility.sh --version 0.2.0-rc.1 --target "$(rustc -vV | sed -n 's/^host: //p')"
sh scripts/generate_checksums.sh --dir dist/packages --out dist/release/SHA256SUMS
sh scripts/generate_sbom.sh --version 0.2.0-rc.1 --out dist/release/sbom.cdx.json
python3 scripts/generate_release_manifest.py --version 0.2.0-rc.1 \
  --artifacts-dir dist/release --checksums dist/release/SHA256SUMS \
  --sbom dist/release/sbom.cdx.json --commit "$(git rev-parse HEAD)" \
  --source-date-epoch "$SOURCE_DATE_EPOCH" \
  --out dist/release/meld-lang-0.2.0-rc.1.release-manifest.json
sh scripts/verify_artifacts.sh --dir dist/release --offline
```

Signing requires a real OIDC identity and is intentionally not part of a local
dry run. `scripts/sign_artifacts.sh` exits when no identity token is available.

## Verification by consumers

See [docs/release-artifacts.md](docs/release-artifacts.md) for the exact
`sha256sum`, cosign and installer verification commands. The shell and
PowerShell installers default to checksum verification and only claim
signature verification when cosign actually verifies a bundle.

## Package channels

- Homebrew: `Formula/meld-lang.rb` — fill `TODO_VERIFY_SHA256` from the signed
  artifact before submitting.
- Scoop: `bucket/meld-lang.json` — fill `TODO_VERIFY_SHA256` before publishing.
- Debian: `packaging/deb/` — build with `dpkg-buildpackage`.
- Nix: `flake.nix` / `package.nix` — generate `flake.lock` with `nix flake lock`.
- Docker: `Dockerfile` built from a release tag or source checkout.

All channels ship the three binaries, Lang Pack data, licenses and docs, and
set a pack discovery variable (`MELD_PACKS`, or `MELD_ROOT` on Windows where the
colon-separated `MELD_PACKS` format conflicts with drive letters). None installs
a language runtime.

## After publication

- Record the release URL, commit, attestation links and cosign identity in
  `status/production-readiness.md` (owned by that workstream).
- Confirm installers on clean machines, including macOS and Windows; this
  cannot be verified from a Linux authoring environment.
