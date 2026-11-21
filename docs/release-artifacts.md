# Release artifacts

A signed candidate publishes these files for each target triple:

```text
meld-lang-<version>-<target>.tar.gz        # Linux/macOS
meld-lang-<version>-<target>.zip           # Windows
SHA256SUMS                                  # every archive, SBOM and manifest input
meld-lang-<version>.release-manifest.json   # deterministic signed manifest
meld-lang-<version>-sbom.cdx.json           # CycloneDX 1.6 SBOM
<each-of-the-above>.sig                     # cosign detached signature
<each-of-the-above>.bundle                  # cosign Sigstore bundle
*.intoto.jsonl                              # GitHub build-provenance attestation
```

The system package is `meld-lang`. The binaries inside are `meld`, `meld-lsp`
and `meld-pack`. Every archive contains:

```text
bin/                        three binaries
packs/                      all bundled Lang Pack data and queries
share/doc/meld-lang/        README, CHANGELOG, RELEASE, SBOM, SECURITY,
                            THIRD-PARTY-NOTICES, LICENSE-* and NOTICE
share/meld-lang/examples/   example .meld sources
```

## Layout note

Archives place docs under `share/doc/meld-lang`. Installers adapt this to the
host layout and expose pack data via `MELD_PACKS` (POSIX) or `MELD_ROOT`
(Windows, because `MELD_PACKS` is colon-separated and Windows paths contain a
colon). The archives contain no runtimes; the shell and PowerShell installers
detect missing language runtimes and install them by default, with an opt-out
flag. `meld` itself and `meld doctor` never install anything.

## Verify a download

The example uses the stable `v0.2.0` tag. For future releases, substitute the
exact stable `vX.Y.Z` tag; pre-release tags are rejected by the release workflow.

```sh
# 1. Integrity
sha256sum -c --strict SHA256SUMS

# 2. Signature and manifest + archive payload checks
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --tag v0.2.0 \
  --issuer https://token.actions.githubusercontent.com \
  --attestation meld-lang-0.2.0.intoto.jsonl

# 3. Installed files
sh scripts/verify_install.sh --root <install-prefix>
```

`--offline` omits cosign verification; it never claims that signatures were
checked. Without cosign, only checksums are verified.

## Install channels

| Channel | File | Notes |
| --- | --- | --- |
| Shell | `install.sh` | Linux/macOS; verifies SHA256SUMS and cosign when available. |
| PowerShell | `install.ps1` | Windows; SHA256SUMS and optional cosign. |
| Homebrew | `Formula/meld-lang.rb` | Formula named `meld-lang`; generate per-arch SHA256 from signed `SHA256SUMS`. |
| Scoop | `bucket/meld-lang.json` | Generate hash from signed `SHA256SUMS`; autoupdate disabled. |
| Debian | `packaging/deb/` | Source package `meld-lang`; profile.d exports MELD_PACKS. |
| Nix | `flake.nix`, `package.nix` | Commit a real `flake.lock` (missing in this tree); wrappers export MELD_PACKS. |
| Docker | `Dockerfile` | Copies binaries, packs, docs and licenses; sets MELD_PACKS. |

## Honesty about verification scope

Native macOS and Windows builds, the public GitHub release upload, and public
verification of signed artifacts must run in CI. They have not been run in the
Linux authoring environment where these files were created. Placeholder
checksums in installer manifests are explicitly marked `TODO_VERIFY_SHA256` and
cause failures until replaced with values from the signed release.
