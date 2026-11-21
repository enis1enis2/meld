# Pull request

## Summary

Describe what changed and why. Link the issue this addresses when one exists.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] `cargo doc --workspace --no-deps --locked`
- [ ] Python tool checks and `python3 tools/generate_third_party_notices.py --check`
- [ ] Pack and registry verification when pack data changed
- [ ] Linux OCI-feature clippy and tests when applicable
- [ ] markdownlint passed for new or changed Markdown

List any check that was skipped and why. Privilege-dependent and native-platform checks are tracked by CI even when they cannot run locally.

## Impact

- [ ] No sandbox policy, lint rule, test, or fail-closed verification path was weakened.
- [ ] No secret, signing key, or credential was added.
- [ ] Design plan, spec, status evidence, or changelog updated where behavior or policy changed.
- [ ] Signed pack bytes were regenerated and re-signed if their payload changed.

See [CONTRIBUTING.md](../CONTRIBUTING.md) for the full development guide and [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) for community expectations.
