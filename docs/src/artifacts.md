# Artifacts and verification

Download an archive, `SHA256SUMS`, the release manifest, the SBOM and their
`.sig`/`.bundle` files. Then:

```sh
sha256sum -c --strict SHA256SUMS
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --tag v0.2.0 \
  --issuer https://token.actions.githubusercontent.com \
  --attestation meld-lang-0.2.0.intoto.jsonl
sh scripts/verify_install.sh --root <install-prefix>
```

Use the exact stable tag; the workflow rejects pre-release tags.

Homebrew, Scoop, Debian, Nix and Docker channels all ship the three binaries,
Lang Pack data, licenses and docs. Placeholder checksums marked
`TODO_VERIFY_SHA256` must be replaced from the signed release before a channel
is published.

See the [Release pipeline](release.md) and [Security](security.md) chapters for
the signing identity and supply-chain context.

See the canonical [release-artifacts.md](https://github.com/enis1enis2/meld/blob/main/docs/release-artifacts.md).
