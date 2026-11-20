# Software bill of materials

Every release archive set is accompanied by one CycloneDX 1.6 JSON SBOM:

```text
meld-lang-<version>-sbom.cdx.json
```

In the examples below the file is named `sbom.cdx.json` inside the release
directory.

## Generation

```sh
sh scripts/generate_sbom.sh --version 0.2.0-rc.1 --out dist/release/sbom.cdx.json
```

The script invokes the real `cargo sbom` tool (pinned to 0.10.0 in CI) against
the workspace and fails when the tool is absent. It then normalizes volatile
fields so the document is reproducible:

- `serialNumber` becomes a deterministic UUIDv5 derived from the product,
  version, commit and `SOURCE_DATE_EPOCH`;
- `metadata.timestamp` becomes the UTC time of `SOURCE_DATE_EPOCH`;
- `metadata.tools` is replaced with a fixed tool entry;
- `meld:*` properties record the release version, epoch and commit;
- keys are sorted and output ends with one newline.

The SBOM is covered by `SHA256SUMS`, referenced by the signed release manifest,
and cosign-signed like the archives.

## What it contains

The SBOM lists Cargo workspace crates and their resolved dependencies as
reported by `cargo metadata`, using the committed `Cargo.lock`. It does **not**
claim to enumerate system libraries, language runtimes installed by users, or
files inside third-party Lang Packs; those are described in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) and per-pack manifests.

## Verification

```sh
sha256sum -c --strict SHA256SUMS
python3 -c 'import json; d=json.load(open("sbom.cdx.json")); assert d["bomFormat"]=="CycloneDX"; assert d["specVersion"]=="1.6"'
cosign verify-blob --bundle sbom.cdx.json.bundle \
  --certificate-identity-regexp '^https://github\.com/enis1enis2/meld/\.github/workflows/release\.yml@refs/tags/v0\.2\.0-rc\.1$' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  sbom.cdx.json
```

`sh scripts/verify_artifacts.sh --dir <release-dir> --tag v0.2.0-rc.1` performs
these checks together with an anchored, tag-specific identity (use `--offline`
for integrity checks only). No SBOM, hash or signature is fabricated by the
scripts; each is produced by a real tool and verified before publication.
