#!/bin/sh
# Keyless-sign release artifacts with Sigstore cosign and verify each signature
# immediately. Requires a real OIDC identity (GitHub Actions id-token: write);
# this script deliberately fails when cosign or OIDC is unavailable.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/sign_artifacts.sh --dir DIR [--identity-regexp REGEXP] [--issuer URL]

Default identity is the release workflow of this repository and the default
issuer is the GitHub Actions OIDC provider. Override both when verifying.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIR=
IDENTITY='https://github.com/enis1enis2/meld/.github/workflows/release.yml@refs/tags/.*'
ISSUER='https://token.actions.githubusercontent.com'

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR=${2:?--dir needs a value}; shift 2 ;;
        --identity-regexp) IDENTITY=${2:?--identity-regexp needs a value}; shift 2 ;;
        --issuer) ISSUER=${2:?--issuer needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'sign_artifacts.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$DIR" ] || { usage >&2; exit 2; }
[ -d "$DIR" ] || { printf 'sign_artifacts.sh: missing directory %s\n' "$DIR" >&2; exit 1; }
command -v cosign >/dev/null 2>&1 || {
    printf 'sign_artifacts.sh: cosign is required (https://docs.sigstore.dev/cosign/installation/)\n' >&2
    exit 1
}
if [ -z "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] && [ -z "${COSIGN_IDENTITY_TOKEN:-}" ]; then
    printf 'sign_artifacts.sh: no OIDC identity token available; refusing to fake a signature\n' >&2
    exit 1
fi

count=0
find "$DIR" -maxdepth 1 -type f \( -name 'SHA256SUMS' -o -name '*release-manifest*.json' -o \
    -name '*.cdx.json' -o -name '*.tar.gz' -o -name '*.zip' \) -print | LC_ALL=C sort | while IFS= read -r file; do
    name=${file##*/}
    printf 'sign_artifacts.sh: signing %s\n' "$name"
    cosign sign-blob --yes \
        --oidc-issuer "$ISSUER" \
        --bundle "$file.bundle" \
        --output-signature "$file.sig" \
        "$file"
    cosign verify-blob \
        --bundle "$file.bundle" \
        --certificate-identity-regexp "$IDENTITY" \
        --certificate-oidc-issuer "$ISSUER" \
        "$file"
done
# The pipeline above runs in a subshell in POSIX sh; count only what was signed.
count=$(find "$DIR" -maxdepth 1 -type f -name '*.bundle' | wc -l | tr -d ' ')
[ "$count" -gt 0 ] || { printf 'sign_artifacts.sh: no signatures produced\n' >&2; exit 1; }
printf 'sign_artifacts.sh: signed and verified %s artifact(s)\n' "$count"
