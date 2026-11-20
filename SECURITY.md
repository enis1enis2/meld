# Security policy

## Supported versions

| Version | Status |
|---|---|
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
  verifies checksums and HMAC signatures before copying files.
- Release archives are signed with Sigstore cosign keyless identities bound to
  the release workflow and tag. `SHA256SUMS` and the release manifest are signed
  too, so a swapped archive cannot pass verification.
- Pack HMAC signatures and release Sigstore signatures are different controls.
  A pack signature does not authenticate the binaries, and a binary signature
  does not authenticate a third-party pack.
- `MELD_PACKS`, `MELD_ROOT` and `MELD_PACK_KEY` influence discovery and
  verification. Treat them as configuration, not secrets from untrusted input.
- Fuzzing targets cover the region splitter and value decoder; findings are
  handled through the same private reporting channel.

## Signature identity

Expected identity regexp (override only when verifying an staging workflow):

```text
https://github.com/enis1enis2/meld/.github/workflows/release.yml@refs/tags/.*
```

Issuer: `https://token.actions.githubusercontent.com`.

Verify with:

```sh
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --identity-regexp 'https://github.com/enis1enis2/meld/.github/workflows/release.yml@refs/tags/.*' \
  --issuer 'https://token.actions.githubusercontent.com'
```

## Supply chain claims

CI runs `cargo audit`, `cargo deny`, exact grammar pins, reproducible builds,
SBOM generation, cosign signing and GitHub build-provenance attestations. Meld
does **not** claim SLSA Level 3 or any other SLSA level. Build provenance is an
attestation, not a certification.

## Dependency and key rotation

- Add advisories to `audit.toml` ignores only with a linked exception rationale.
- Release signing uses keyless OIDC; there is no long-lived release private key
  to rotate. The pack-development HMAC key must never be used as a release key.
- Compromised release artifacts require deleting the GitHub release, revoking
  the OIDC identity used, publishing an advisory and incrementing the patch or
  minor version per [SEMVER.md](SEMVER.md).
