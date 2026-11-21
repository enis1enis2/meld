# Governance

This document describes how changes and releases are decided in the Meld repository.

## Maintainers

The repository owner, [@enis1enis2](https://github.com/enis1enis2), is the current maintainer and release authority. Additional maintainers may be added by the repository owner. Maintainer duties include issue triage, code review, CI upkeep, security response and release publication.

## Decision making

- Changes are proposed with issues and pull requests and are merged after the required CI checks pass.
- Design decisions are recorded in [project.md](project.md), the relevant files under [spec/](spec/), and inline `JUDGMENT CALL` notes where a trade-off is made in code or documentation.
- Security policy is defined in [SECURITY.md](SECURITY.md) and is changed only with explicit review.
- Community conduct is handled under [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- Disagreements should be resolved in the open where possible. Private information, security reports and conduct reports stay private.

## Releases

The release workflow, artifact set, signing identities and verification steps are defined in [RELEASE.md](RELEASE.md). Publication follows the authorized tag and workflow; no signing key or credential is stored in the repository. Release candidates follow [SEMVER.md](SEMVER.md), and changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## Amending this document

Propose changes to this file by pull request. Changes that affect maintainer authority or release publication require maintainer approval before merge.
