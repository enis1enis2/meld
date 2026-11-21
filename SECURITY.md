# Security policy

## Supported versions

| Version | Status |
| --- | --- |
| `0.2.x` (release candidates) | Security fixes accepted |
| `0.1.x` | Development history; no security support |

Only signed release artifacts from the upstream repository are supported.
Locally built binaries and third-party repackaging are not covered by these
signatures.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting on
<https://github.com/enis1enis2/meld/security/advisories/new>. Include:

- affected version/commit and platform;
- reproduction input (a `.meld` file, pack, wire payload or archive);
- impact and suggested severity.

Do not open a public issue for an unfixed vulnerability. Acknowledge and triage
is targeted within 7 days; a fix or mitigation plan within 30 days for issues
affecting signed releases. Maintainers may ask for a minimal reproducer.

## Trust boundaries

- Lang Packs are data, not trusted code execution during install. `meld-pack`
  verifies canonical SHA-256 checksums and Ed25519 signatures against public
  trust roots and revocations before copying files.
- Release archives are signed with Sigstore cosign keyless identities bound to
  the release workflow and tag. `SHA256SUMS` and the release manifest are signed
  too, so a swapped archive cannot pass verification.
- Pack Ed25519 signatures and release Sigstore signatures are different
  controls. A pack signature authenticates pack data, not the binaries; a
  binary signature does not authenticate a third-party pack.
- Verification configuration includes `MELD_PACK_TRUST_ROOTS`,
  `MELD_PACK_REVOCATIONS`, `MELD_LIB_ROOT` and `MELD_ROOT`. Treat these as
  configuration, never as secrets accepted from untrusted input. Private
  signing keys are not environment variables and are never checked in.
- Fuzzing targets cover the region splitter and value decoder; findings are
  handled through the same private reporting channel.

## Signature identity

Expected identity regexp for the release being verified (example tag
`v0.2.0-rc.1`; replace with the exact tag you are verifying):

```text
^https://github\.com/enis1enis2/meld/\.github/workflows/release\.yml@refs/tags/v0\.2\.0-rc\.1$
```

Issuer: `https://token.actions.githubusercontent.com`.

Verify with:

```sh
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --tag v0.2.0-rc.1 \
  --issuer https://token.actions.githubusercontent.com \
  --attestation meld-lang-0.2.0-rc.1.intoto.jsonl
```

The exact tag binds verification to one signed release. Never accept a broad
`refs/tags/.*` identity regexp; `scripts/verify_artifacts.sh` constructs the
anchored regexp from `--tag` and fails closed when no exact tag is supplied
unless `--offline` is explicitly requested. For a staging workflow, use an
explicitly reviewed anchored regexp and record why it differs.

## Supply chain claims

CI runs `cargo audit`, `cargo deny`, exact grammar pins, reproducible builds,
SBOM generation, cosign signing and GitHub build-provenance attestations. Meld
does **not** claim SLSA Build L3 or any other SLSA Build level. GitHub artifact
attestations alone provide SLSA v1.0 Build Level 2, and build provenance is an
attestation, not a certification or a security guarantee.

## Dependency and key rotation

- Add advisories to `audit.toml` ignores only with a linked exception rationale.
- Release signing uses keyless OIDC; there is no long-lived release private key
  to rotate. Pack signing uses Ed25519 private keys generated outside the repo;
  rotate them by adding a new public root and revoking the old key id.
- Compromised release artifacts require deleting the GitHub release, revoking
  the OIDC identity used, publishing an advisory and incrementing the patch or
  minor version per [SEMVER.md](SEMVER.md).
