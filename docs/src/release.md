# Release pipeline

A `v*` tag runs gates (fmt, clippy, tests, docs), pack verification, cargo
audit/deny, MSRV 1.90 checks, bounded libFuzzer runs, native Linux/macOS/Windows
builds, reproducible packaging, SBOM generation, cosign signing, build
provenance and release creation.

The pipeline produces signed archives, `SHA256SUMS`, a signed release manifest,
a CycloneDX 1.6 SBOM and signature bundles. It does not assert an SLSA Build
level. Native macOS/Windows builds and public release verification happen in
CI; they are not claimed to have run in the authoring environment.

Next: [Artifacts and verification](artifacts.md) and [Security](security.md).

See the canonical [RELEASE.md](https://github.com/enis1enis2/meld/blob/main/RELEASE.md).
