# Contributing to Meld

Thanks for helping improve Meld. This document covers local development, the checks CI runs, and the conventions used for changes. By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- Read the [design plan](project.md), the [book](docs/src/SUMMARY.md) and the relevant files under [spec/](spec/) before changing behavior.
- Use the [issue forms](https://github.com/enis1enis2/meld/issues/new/choose) for bugs, features, docs and Lang Pack requests.
- Report security vulnerabilities privately via [SECURITY.md](SECURITY.md), never in a public issue.
- For usage help, see [SUPPORT.md](SUPPORT.md).

## Development setup

Meld builds with Rust 1.90 or newer (MSRV) and a Python 3.12 interpreter for the reference/tooling tests. The only pinned Python dependency is in `requirements-ci.txt`.

```sh
python3 -m pip install -r requirements-ci.txt
cargo fetch --locked
```

The third-party notices generator reads crate manifests from the Cargo registry cache. Populate the cache with `cargo fetch --locked` before running `--check`; an empty cache fails closed rather than inventing license metadata.

## Local gates

Run the same gates as CI before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo doc --workspace --no-deps --locked
python3 -m unittest discover -s tools/tests
python3 tools/fuzz_splitter.py
python3 tools/meld_check_ref.py
python3 tools/highlight_ref.py
python3 tools/check_grammar_sources.py
python3 tools/check_schemas.py
python3 tools/smoke_manual_tools.py
python3 tools/generate_third_party_notices.py --check
```

On Linux, also run the OCI-feature gates:

```sh
cargo clippy -p meld-sandbox --features oci --all-targets --locked -- -D warnings
cargo test -p meld-sandbox --features oci --all-targets --locked
```

Verify every checked-in Lang Pack and the registry attestation after changing pack data:

```sh
for pack in packs/* tests/fixtures/pack-third-party; do
  if [ -f "$pack/pack.toml" ]; then
    python3 tools/packtool.py verify "$pack" --trust-roots packs/registry/trust-roots.toml --require-trusted
  fi
done
python3 tools/packtool.py verify-registry packs/registry --verify-packs --gha-workflow <workflow-ref>
```

`verify-registry --verify-packs` and `install-pack --verify-packs` share the same index metadata and Sigstore identity checks. In CI the workflow reference is the full immutable `https://github.com/<owner>/<repo>/.github/workflows/release.yml@refs/tags/<tag>` of the release; locally it is the trusted pack signing workflow. Pack signatures are bound to the pinned identity, the tag, and the exact archive digest; a pack that fails any check is rejected.

Some privilege-dependent and native-platform checks are intentionally skipped locally. CI matrices and release jobs remain the source of truth for those; document any skip in the pull request.

## Release verification

Release artifacts are checked with `scripts/verify_artifacts.sh`, which requires an anchored `--identity-regexp` matching the exact release workflow identity and tag, and enforces Sigstore attestations. The manifest records the expected target triples, so files from other builds are rejected.

Channel metadata is generated from the verified dist directory rather than written by hand. Before publishing a release, run:

```sh
python3 tools/generate_channel_metadata.py --dist-dir dist/release --version <version> --check
```

The T3 demo guest is reproducible. `tools/build_t3_guest.sh --check` verifies that the committed component matches a rebuild when the pinned `wasm32-unknown-unknown` target is installed; `python3 tools/check_t3_guest_digest.py --check` additionally checks the recorded digest. Release CI gates on both checks and fails closed on drift.

## Lang Packs and signing

- Pin grammar crates to exact versions and record the source in `grammar-sources.json`; see [the Lang Pack spec](spec/lang-pack.md).
- Do not weaken sandbox policy, `clippy`, tests, or the fail-closed verification path to make a change pass.
- Never commit signing keys, tokens, or generated private material. The fixture signing key is maintained outside the repository; ask a maintainer to re-sign after changing signed fixture bytes.
- Pack bytes are covered by checksums and Ed25519 signatures. If payload bytes change, regenerate `checksums.sha256` and the signature with the established key before verification.

## Commits and pull requests

- Use small conventional commits such as `fix:`, `feat:`, `docs:`, `ci:` and `test:`.
- Do not force-push to `main` or the `production/**` branches.
- Describe what changed, why, and how it was verified. Link the relevant issue when one exists.
- Update the design plan, spec, status evidence, or changelog when the change affects behavior or policy.
- Keep the markdownlint config intact; new Markdown must pass the existing linter rather than disabling rules.

## Governance

Maintainer and release authority are described in [GOVERNANCE.md](GOVERNANCE.md). The release process and the remaining external publication obligations are tracked in [RELEASE.md](RELEASE.md).
