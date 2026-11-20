#!/bin/sh
# Keyless-sign release artifacts with Sigstore cosign and verify each signature
# immediately. Requires a real OIDC identity (GitHub Actions id-token: write);
# this script deliberately fails when cosign or OIDC is unavailable.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/sign_artifacts.sh --dir DIR --tag TAG [--repo OWNER/REPO]
                                  [--identity-regexp REGEXP] [--issuer URL]

The default identity is anchored to the exact --tag and escapes regex
metacharacters. --identity-regexp overrides it for non-tag verification only.
No signature list is embedded in the release manifest; verification checks the
signature files on disk.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIR=
TAG=
REPO='enis1enis2/meld'
IDENTITY=
ISSUER='https://token.actions.githubusercontent.com'

escape_regex() {
    printf '%s' "$1" | sed -e 's/[][\.^$*+?(){}|]/\\&/g'
}

identity_from_tag() {
    printf '^https://github\\.com/%s/\\.github/workflows/release\\.yml@refs/tags/%s$' \
        "$(escape_regex "$REPO")" "$(escape_regex "$TAG")"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR=${2:?--dir needs a value}; shift 2 ;;
        --tag) TAG=${2:?--tag needs a value}; shift 2 ;;
        --repo) REPO=${2:?--repo needs a value}; shift 2 ;;
        --identity-regexp) IDENTITY=${2:?--identity-regexp needs a value}; shift 2 ;;
        --issuer) ISSUER=${2:?--issuer needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'sign_artifacts.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$DIR" ] || { usage >&2; exit 2; }
[ -d "$DIR" ] || { printf 'sign_artifacts.sh: missing directory %s\n' "$DIR" >&2; exit 1; }
if [ -z "$IDENTITY" ]; then
    [ -n "$TAG" ] || {
        printf 'sign_artifacts.sh: provide --tag (or --identity-regexp for tool testing)\n' >&2
        exit 2
    }
    IDENTITY=$(identity_from_tag)
fi
command -v cosign >/dev/null 2>&1 || {
    printf 'sign_artifacts.sh: cosign is required (https://docs.sigstore.dev/cosign/installation/)\n' >&2
    exit 1
}
if [ -z "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] && [ -z "${COSIGN_IDENTITY_TOKEN:-}" ]; then
    printf 'sign_artifacts.sh: no OIDC identity token available; refusing to fake a signature\n' >&2
    exit 1
fi

[ -f "$DIR/SHA256SUMS" ] || { printf 'sign_artifacts.sh: SHA256SUMS is missing\n' >&2; exit 1; }
manifest_count=$(find "$DIR" -maxdepth 1 -type f -name '*release-manifest*.json' | wc -l | tr -d ' ')
sbom_count=$(find "$DIR" -maxdepth 1 -type f -name '*.cdx.json' | wc -l | tr -d ' ')
archive_count=$(find "$DIR" -maxdepth 1 -type f \( -name '*.tar.gz' -o -name '*.zip' \) | wc -l | tr -d ' ')
[ "$manifest_count" -eq 1 ] || { printf 'sign_artifacts.sh: expected one release manifest, found %s\n' "$manifest_count" >&2; exit 1; }
[ "$sbom_count" -eq 1 ] || { printf 'sign_artifacts.sh: expected one SBOM, found %s\n' "$sbom_count" >&2; exit 1; }
[ "$archive_count" -gt 0 ] || { printf 'sign_artifacts.sh: no platform archives found\n' >&2; exit 1; }

manifest=$(find "$DIR" -maxdepth 1 -type f -name '*release-manifest*.json' | LC_ALL=C sort | head -n 1)
manifest_name=${manifest##*/}
awk -v name="$manifest_name" '$2 == name { found = 1 } END { exit !found }' "$DIR/SHA256SUMS" || {
    printf 'sign_artifacts.sh: %s is not covered by SHA256SUMS\n' "$manifest_name" >&2
    exit 1
}

stale=$(find "$DIR" -maxdepth 1 -type f \( -name '*.sig' -o -name '*.bundle' \) -print)
[ -z "$stale" ] || {
    printf 'sign_artifacts.sh: stale signature files present; clean the directory first\n' >&2
    exit 1
}

LIST=$(mktemp "${TMPDIR:-/tmp}/meld-sign-list.XXXXXX")
trap 'rm -f "$LIST"' EXIT HUP INT TERM
find "$DIR" -maxdepth 1 -type f \( -name 'SHA256SUMS' -o -name '*release-manifest*.json' -o \
    -name '*.cdx.json' -o -name '*.tar.gz' -o -name '*.zip' \) -print | LC_ALL=C sort > "$LIST"
expected_count=$(wc -l < "$LIST" | tr -d ' ')
[ "$expected_count" -gt 0 ] || { printf 'sign_artifacts.sh: no artifacts to sign\n' >&2; exit 1; }

while IFS= read -r file; do
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
done < "$LIST"

sig_count=$(find "$DIR" -maxdepth 1 -type f -name '*.sig' | wc -l | tr -d ' ')
bundle_count=$(find "$DIR" -maxdepth 1 -type f -name '*.bundle' | wc -l | tr -d ' ')
[ "$sig_count" -eq "$expected_count" ] && [ "$bundle_count" -eq "$expected_count" ] || {
    printf 'sign_artifacts.sh: expected %s signature+bundle pairs, got %s sig / %s bundle\n' \
        "$expected_count" "$sig_count" "$bundle_count" >&2
    exit 1
}
printf 'sign_artifacts.sh: signed and verified %s artifact(s)\n' "$expected_count"
