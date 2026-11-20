# Artifacts and verification

Download an archive, `SHA256SUMS`, the release manifest, the SBOM and their
`.sig`/`.bundle` files. Then:

```sh
sha256sum -c --strict SHA256SUMS
sh scripts/verify_artifacts.sh --dir <release-dir>
sh scripts/verify_install.sh --root <install-prefix>
```

Homebrew, Scoop, Debian, Nix and Docker channels all ship the three binaries,
Lang Pack data, licenses and docs. Placeholder checksums marked
`TODO_VERIFY_SHA256` must be replaced from the signed release before a channel
is published.

See the canonical [release-artifacts.md](https://github.com/enis1enis2/meld/blob/main/docs/release-artifacts.md).
