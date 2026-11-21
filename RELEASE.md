# Release process

The first published signed candidate is `v0.2.0-rc.3`. Meld follows SemVer 2.0.0
([SEMVER.md](SEMVER.md)). This document describes what CI does and what a human
must verify before a tag is considered good. Publication is automated from `v*`
tags after every gate below passes. The earlier `v0.2.0-rc.1` and `v0.2.0-rc.2`
tags were never published and are superseded by `rc.3`.

## Preconditions

1. `CHANGELOG.md` has an entry for the candidate and the version passes the
   SemVer 2.0.0 regex used by `scripts/generate_release_manifest.py`.
2. All required docs, licenses and Lang Pack data are present; the packager
   fails closed when any are missing.
3. Dependency policy files (`deny.toml`, `audit.toml`, `about.toml`) and grammar
   pins (`grammar-sources.json`) pass their checks. Any `TODO_VERIFY` value
   fails the grammar checker by design.
4. The workspace version is `0.2.0-rc.3`, matching the candidate tag and the
   release manifest. Lockfiles, third-party notices and package metadata are
   regenerated as part of the bump; no version mismatch is shipped.
5. There is no standard Linux aarch64 GitHub-hosted runner. `install.sh` fails
   clearly on Linux aarch64 instead of downloading a target that was never
   published. Do not hand-edit a target into the manifest or installer map.

## CI pipeline (`.github/workflows/release.yml`)

A `v*` tag triggers, in order:

1. **unified CI gates** — `.github/workflows/ci.yml` is called with `uses`; it
   runs locked fmt/clippy/test/doc, Python/grammar/schema/reference gates, pack
   verification, cross-target clippy and the supply-chain audit on the tag.
2. **docs gates** — `.github/workflows/docs.yml` is called with `uses`; it
   builds rustdoc/mdbook and runs markdownlint against `.markdownlint.json`.
3. **release tag gates** — grammar, schema and checked-in pack verification run
   again against the exact tagged tree.
4. **audit / deny / notices**, **msrv** and **fuzz** remain separate reusable
   gates; fuzz runs are bounded (`60s`, `4096`-byte inputs) and failures are
   uploaded, never committed.
5. **native-matrix** — Ubuntu `x86_64-unknown-linux-gnu`, macOS ARM
   `aarch64-apple-darwin`, Intel macOS `x86_64-apple-darwin` (`macos-15-intel`)
   and Windows `x86_64-pc-windows-msvc` are built natively. Each runner asserts
   the host triple, builds/packages twice under one `SOURCE_DATE_EPOCH`, verifies
   the install layout, and uploads canonical archives plus both build pairs.
6. **reproducible** — every uploaded pair is compared again in a separate job;
   the job then asserts the exact target set (four distinct triples), rejecting
   missing, extra or duplicate targets. Hard-coded pair counts are gone.
7. **sbom** — `cargo-sbom` CycloneDX 1.6 output, normalized for determinism.
8. **manifest then checksums** — the release manifest is generated first and
   contains no `signatures` field. `SHA256SUMS` is generated afterwards and
   includes the manifest. This avoids a self-hash cycle and stale embedded
   signature lists.
9. **sign** — cosign keyless-signs `SHA256SUMS`, the manifest, the SBOM and each
   archive. The certificate identity is anchored to the exact
   `GITHUB_REF_NAME` tag; dots and regex metacharacters are escaped. Each
   signature is verified immediately.
10. **artifact verification** — `scripts/verify_artifacts.sh` enforces strict
    `SHA256SUMS` coverage, manifest consistency, archive payloads, and exactly
    one non-empty `.sig` and `.bundle` per signed artifact. It fails closed if
    cosign is required but unavailable; `--offline` is explicitly integrity-only.
11. **provenance** — GitHub build-provenance attestations for the core release
    files. The published release job re-verifies artifacts and checks the
    `.intoto.jsonl` bundle with `gh attestation verify --bundle`. This is **not**
    a SLSA L3 claim; no SLSA level is asserted.
12. **release** — `gh release create`, prerelease for `-rc` tags, with all
    archives, checksums, manifest, SBOM, signatures, bundles and attestations
    attached.

## Local dry run

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
target=$(rustc -vV | sed -n 's/^host: //p')
sh scripts/build_matrix.sh --version 0.2.0-rc.3 --targets "$target"
sh scripts/package_release.sh --version 0.2.0-rc.3 --target "$target"
sh scripts/check_reproducibility.sh --version 0.2.0-rc.3 --target "$target"
sh scripts/generate_sbom.sh --version 0.2.0-rc.3 --out dist/release/meld-lang-0.2.0-rc.3-sbom.cdx.json
python3 scripts/generate_release_manifest.py --version 0.2.0-rc.3 \
  --artifacts-dir dist/release \
  --sbom dist/release/meld-lang-0.2.0-rc.3-sbom.cdx.json \
  --commit "$(git rev-parse HEAD)" \
  --source-date-epoch "$SOURCE_DATE_EPOCH" \
  --expected-target "$target" \
  --out dist/release/meld-lang-0.2.0-rc.3.release-manifest.json
sh scripts/generate_checksums.sh --dir dist/release --out dist/release/SHA256SUMS
sh scripts/verify_artifacts.sh --dir dist/release --offline
```

Signing requires a real OIDC identity and is intentionally not part of a local
dry run. `scripts/sign_artifacts.sh` exits when no identity token is available,
and now requires `--tag` (or an explicit identity override) so verification is
never tied to a wildcard tag regex.

## Verification by consumers

Use the exact tag; do not accept a broad `refs/tags/.*` identity regex:

```sh
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --tag v0.2.0-rc.3 \
  --issuer https://token.actions.githubusercontent.com \
  --attestation meld-lang-0.2.0-rc.3.intoto.jsonl
```

`--attestation` requires a `gh` build that supports `gh attestation verify
--bundle`; the script fails closed if it does not. Omitting it still checks
checksums, manifest, archive payloads and cosign signatures, but prints a
notice that the GitHub attestation was not verified. `--offline` checks only
integrity and signature-file presence and never claims cryptographic trust.
The shell and PowerShell installers verify checksums by default and only claim
signature verification when cosign verifies a bundle against the anchored,
tag-specific identity.

## Package channels

- Homebrew: `Formula/meld-lang.rb` — the `TODO_VERIFY_SHA256` guard must be
  replaced from the signed `SHA256SUMS` before submitting.
- Scoop: `bucket/meld-lang.json` — uses `env_set` for `MELD_ROOT`; the checked-in
  hash must come from the signed release before publishing. Autoupdate does not
  verify cosign signatures, so its hash source must be trusted and hash changes
  reviewed before merge.
- Debian: `packaging/deb/` — `dpkg-buildpackage -us -uc -b` is an unsigned source
  package build and is not part of the signed artifact set; verify upstream
  release artifacts separately.
- Nix: `flake.nix` / `package.nix` — run `nix flake lock` and commit the real
  `flake.lock`. Do not hand-write `narHash` or invent lock hashes.
- Docker: `Dockerfile` — built from a release tag or checkout; the image is
  outside the signed artifact set. Pinning base images by digest and using a
  reproducible exporter remain future work.

All channels ship the three binaries, Lang Pack data, licenses and docs, and
set a pack discovery variable (`MELD_PACKS`, or `MELD_ROOT` on Windows where the
colon-separated `MELD_PACKS` format conflicts with drive letters). None installs
a language runtime.

## After publication

- Record the release URL, commit, attestation links and cosign identity in
  `status/production-readiness.md` (owned by that workstream).
- Confirm installers on clean machines, including macOS and Windows; this
  cannot be verified from a Linux authoring environment.
