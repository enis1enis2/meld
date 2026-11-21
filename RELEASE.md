# Release process

The current stable release target is `v0.2.0`. Meld publishes stable releases
only, using `vMAJOR.MINOR.PATCH` tags; the stable-tag rule is enforced by the
release workflow, so do not create `-rc.N`, `-alpha`, or `-beta` tags. Meld
follows SemVer 2.0.0 ([SEMVER.md](SEMVER.md)). This document describes what CI
does and what a human must verify before a tag is considered good. Publication
is automated from stable tags after every gate below passes. The historical
`v0.2.0-rc.5` tag remains published as history: `v0.2.0-rc.1`, `v0.2.0-rc.2`
and `v0.2.0-rc.3` were never published, `rc.4` was published but superseded by
`rc.5` after real Windows testing found an installer pack-directory bug, and
the stable line is prepared from this worktree.

## Preconditions

1. `CHANGELOG.md` has an entry for the stable version and the version passes the
   SemVer 2.0.0 regex used by `scripts/generate_release_manifest.py`. That regex
   validates SemVer syntax only (it still accepts pre-release/build suffixes);
   the release workflow, not the script, enforces the stable
   `vMAJOR.MINOR.PATCH` tag and the Cargo/tag match.
2. All required docs, licenses and Lang Pack data are present; the packager
   fails closed when any are missing.
3. Dependency policy files (`deny.toml`, `audit.toml`, `about.toml`) and grammar
   pins (`grammar-sources.json`) pass their checks. Any `TODO_VERIFY` value
   fails the grammar checker by design.
4. The workspace version is the stable version being tagged (for example
   `0.2.0`), matching the tag and the release manifest. The release workflow
   rejects any tag whose version differs from `Cargo.toml`. Lockfiles,
   third-party notices and package metadata are regenerated as part of the
   bump; no version mismatch is shipped.
5. There is no standard Linux aarch64 GitHub-hosted runner. `install.sh` fails
   clearly on Linux aarch64 instead of downloading a target that was never
   published. Do not hand-edit a target into the manifest or installer map.

## CI pipeline (`.github/workflows/release.yml`)

A stable `vMAJOR.MINOR.PATCH` tag triggers, in order:

1. **unified CI gates** — `.github/workflows/ci.yml` is called with `uses`; it
   runs locked fmt/clippy/test/doc, Python/grammar/schema/reference gates, pack
   verification, cross-target clippy and the supply-chain audit on the tag.
2. **docs gates** — `.github/workflows/docs.yml` is called with `uses`; it
   builds rustdoc/mdbook and runs markdownlint against `.markdownlint.json`.
3. **release tag gates** — the stable tag shape and Cargo/tag version match are
   enforced, then grammar, schema and checked-in pack verification run again
   against the exact tagged tree.
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
12. **release** — `gh release create`, always stable and marked latest, with all
    archives, checksums, manifest, SBOM, signatures, bundles and attestations
    attached.

## Local dry run

Use the stable version from `Cargo.toml` (currently `0.2.0`); the example below
matches it and must be updated when the workspace version changes.

```sh
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
target=$(rustc -vV | sed -n 's/^host: //p')
version=0.2.0
sh scripts/build_matrix.sh --version "$version" --targets "$target"
sh scripts/package_release.sh --version "$version" --target "$target"
sh scripts/check_reproducibility.sh --version "$version" --target "$target"
sh scripts/generate_sbom.sh --version "$version" --out "dist/release/meld-lang-$version-sbom.cdx.json"
python3 scripts/generate_release_manifest.py --version "$version" \
  --artifacts-dir dist/release \
  --sbom "dist/release/meld-lang-$version-sbom.cdx.json" \
  --commit "$(git rev-parse HEAD)" \
  --source-date-epoch "$SOURCE_DATE_EPOCH" \
  --expected-target "$target" \
  --out "dist/release/meld-lang-$version.release-manifest.json"
sh scripts/generate_checksums.sh --dir dist/release --out dist/release/SHA256SUMS
sh scripts/verify_artifacts.sh --dir dist/release --offline
```

Signing requires a real OIDC identity and is intentionally not part of a local
dry run. `scripts/sign_artifacts.sh` exits when no identity token is available,
and now requires `--tag` (or an explicit identity override) so verification is
never tied to a wildcard tag regex.

## Verification by consumers

Use the exact tag; do not accept a broad `refs/tags/.*` identity regex. The
example uses the stable `v0.2.0` tag; substitute the exact stable `vX.Y.Z` tag
for future releases:

```sh
sh scripts/verify_artifacts.sh --dir <release-dir> \
  --tag v0.2.0 \
  --issuer https://token.actions.githubusercontent.com \
  --attestation meld-lang-0.2.0.intoto.jsonl
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

- Homebrew: `Formula/meld-lang.rb` — per-architecture checksums are rewritten
  from the signed `SHA256SUMS` with
  `python3 tools/generate_channel_metadata.py --dist-dir dist/release --version 0.2.0`;
  the placeholder guard fails until then. Install wrappers point at the
  installed `libexec/bin/*` binaries and packs at `share/meld-lang/packs`.
- Scoop: `bucket/meld-lang.json` — uses `env_set` for `MELD_ROOT`; the checked-in
  hash must come from the signed release via the same generator before
  publishing. Autoupdate/checkver are disabled because no automatic source
  verifies cosign signatures and trusted hashes.
- Debian: `packaging/deb/` — `dpkg-buildpackage -us -uc -b` is an unsigned source
  package build and is not part of the signed artifact set; verify upstream
  release artifacts separately.
- Nix: `flake.nix` / `package.nix` — run `nix flake lock` and commit the real
  `flake.lock`. No `flake.lock` is present in this worktree, so this channel is
  a release blocker until it is generated; do not hand-write `narHash` or
  invent lock hashes.
- Docker: `Dockerfile` — built from a release tag or checkout; the image is
  outside the signed artifact set. Pinning base images by digest and using a
  reproducible exporter remain future work.

All channels ship the three binaries, Lang Pack data, licenses and docs, and
set a pack discovery variable (`MELD_PACKS`, or `MELD_ROOT` on Windows where the
colon-separated `MELD_PACKS` format conflicts with drive letters). None installs
a language runtime.

## After publication

- Record the release URL, tag, commit, artifact hashes from `SHA256SUMS`,
  attestation links and cosign identity in
  `status/production-readiness.md` (owned by that workstream).
- Run `python3 tools/generate_channel_metadata.py --dist-dir dist/release
  --version <v> --check` before merging channel metadata, and confirm the
  Homebrew/Scoop hashes match the signed release.
- Confirm installers on clean machines, including macOS and Windows; this
  cannot be verified from a Linux authoring environment.

## Release record (preparation baseline)

At the audit baseline `main` was `3d4d378d68f5e455bafb861cd642dce64623a071`. A
read-only query of the public Actions API for that SHA showed 23 completed
checks, all `success`, across CI, Docs, MSRV, Fuzz, supply-chain audit and
CodeQL. That evidence covers this repository's own gates only: eight external
suites were pending at audit time and are not claimed as passed here. No `v0.2.0`
release has been published (the latest GitHub release is the historical
`v0.2.0-rc.5`), so channel hashes remain placeholders until generated from
signed `SHA256SUMS`.
