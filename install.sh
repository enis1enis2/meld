#!/bin/sh
# Install a signed Meld release (meld, meld-lsp, meld-pack plus pack data,
# licenses and docs). No language runtime is installed or downloaded.
set -eu

REPO=${MELD_REPO:-enis1enis2/meld}
VERSION=${MELD_VERSION:-v0.2.0-rc.1}
PREFIX=${MELD_PREFIX:-$HOME/.local}
TARGET=${MELD_TARGET:-}
VERIFY_SIGNATURES=${MELD_VERIFY_SIGNATURES:-auto}
IDENTITY='https://github.com/enis1enis2/meld/.github/workflows/release.yml@refs/tags/.*'
ISSUER='https://token.actions.githubusercontent.com'

usage() {
    cat <<'EOF'
usage: install.sh [--version TAG] [--prefix DIR] [--target TRIPLE]
                  [--skip-signature-verification]

Environment: MELD_VERSION, MELD_PREFIX, MELD_TARGET, MELD_REPO,
MELD_VERIFY_SIGNATURES (auto|1|0), MELD_PACKS (extra pack roots, preserved).
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --prefix) PREFIX=${2:?--prefix needs a value}; shift 2 ;;
        --target) TARGET=${2:?--target needs a value}; shift 2 ;;
        --skip-signature-verification) VERIFY_SIGNATURES=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'install.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

case "$VERSION" in v*) TAG=$VERSION; VERSION_NO_V=${VERSION#v} ;; *) TAG=v$VERSION; VERSION_NO_V=$VERSION ;; esac

if [ -z "$TARGET" ]; then
    os=$(uname -s)
    arch=$(uname -m)
    case "$os:$arch" in
        Linux:x86_64|Linux:amd64) TARGET=x86_64-unknown-linux-gnu ;;
        Linux:aarch64|Linux:arm64) TARGET=aarch64-unknown-linux-gnu ;;
        Darwin:x86_64) TARGET=x86_64-apple-darwin ;;
        Darwin:arm64|Darwin:aarch64) TARGET=aarch64-apple-darwin ;;
        *) printf 'install.sh: no release artifact mapping for %s/%s; set MELD_TARGET\n' "$os" "$arch" >&2; exit 1 ;;
    esac
fi
case "$TARGET" in *windows*) printf 'install.sh: use install.ps1 on Windows\n' >&2; exit 1 ;; esac

BASE="https://github.com/$REPO/releases/download/$TAG"
ARCHIVE="meld-lang-$VERSION_NO_V-$TARGET.tar.gz"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/meld-install.XXXXXX")
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

download() {
    url=$1
    dest=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$dest" "$url"
    else
        printf 'install.sh: curl or wget is required\n' >&2
        exit 1
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        printf 'install.sh: sha256sum or shasum is required\n' >&2
        exit 1
    fi
}

printf 'install.sh: downloading %s\n' "$ARCHIVE"
download "$BASE/$ARCHIVE" "$TMP/$ARCHIVE"
download "$BASE/SHA256SUMS" "$TMP/SHA256SUMS"

expected=$(awk -v name="$ARCHIVE" '$2 == name {print $1}' "$TMP/SHA256SUMS" | head -n 1)
[ -n "$expected" ] || { printf 'install.sh: %s is not in SHA256SUMS\n' "$ARCHIVE" >&2; exit 1; }
actual=$(sha256_of "$TMP/$ARCHIVE")
[ "$actual" = "$expected" ] || {
    printf 'install.sh: checksum mismatch: expected %s got %s\n' "$expected" "$actual" >&2
    exit 1
}
printf 'install.sh: PASS SHA256SUMS verification\n'

if [ "$VERIFY_SIGNATURES" = 1 ] || { [ "$VERIFY_SIGNATURES" = auto ] && command -v cosign >/dev/null 2>&1; }; then
    command -v cosign >/dev/null 2>&1 || {
        printf 'install.sh: signature verification requested but cosign was not found\n' >&2
        exit 1
    }
    download "$BASE/$ARCHIVE.bundle" "$TMP/$ARCHIVE.bundle"
    cosign verify-blob --bundle "$TMP/$ARCHIVE.bundle" \
        --certificate-identity-regexp "$IDENTITY" \
        --certificate-oidc-issuer "$ISSUER" \
        "$TMP/$ARCHIVE"
    printf 'install.sh: PASS Sigstore signature verification\n'
else
    printf 'install.sh: signature verification skipped (integrity checksum only)\n'
fi

mkdir -p "$TMP/extract" "$PREFIX/bin" "$PREFIX/lib/meld-lang" "$PREFIX/share/meld-lang"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP/extract"
PAYLOAD=$(find "$TMP/extract" -mindepth 1 -maxdepth 1 -type d | head -n 1)
[ -n "$PAYLOAD" ] || { printf 'install.sh: archive has no payload directory\n' >&2; exit 1; }

rm -rf "$PREFIX/lib/meld-lang/bin" "$PREFIX/share/meld-lang/packs" "$PREFIX/share/doc/meld-lang"
mkdir -p "$PREFIX/lib/meld-lang/bin" "$PREFIX/share/meld-lang" "$PREFIX/share/doc"
cp -R "$PAYLOAD/bin/." "$PREFIX/lib/meld-lang/bin/"
cp -R "$PAYLOAD/packs/." "$PREFIX/share/meld-lang/packs/"
cp -R "$PAYLOAD/share/doc/meld-lang" "$PREFIX/share/doc/meld-lang"
if [ -d "$PAYLOAD/share/meld-lang/examples" ]; then
    mkdir -p "$PREFIX/share/meld-lang/examples"
    cp -R "$PAYLOAD/share/meld-lang/examples/." "$PREFIX/share/meld-lang/examples/"
fi

# Wrappers keep the install layout relocatable while telling every binary where
# the bundled Lang Packs live. Existing MELD_PACKS values are preserved.
for bin in meld meld-lsp meld-pack; do
    wrapper="$PREFIX/bin/$bin"
    cat > "$wrapper" <<EOF
#!/bin/sh
# Generated by Meld install.sh; bundled packs are discovered via MELD_PACKS.
MELD_PACKS="$PREFIX/share/meld-lang/packs\${MELD_PACKS:+\$MELD_PACKS}"
export MELD_PACKS
exec "$PREFIX/lib/meld-lang/bin/$bin" "\$@"
EOF
    chmod 755 "$wrapper"
done

printf 'install.sh: installed Meld %s to %s\n' "$TAG" "$PREFIX"
printf 'install.sh: binaries: %s/bin/meld %s/bin/meld-lsp %s/bin/meld-pack\n' "$PREFIX" "$PREFIX" "$PREFIX"
printf 'install.sh: add %s/bin to PATH if it is not already there\n' "$PREFIX"
