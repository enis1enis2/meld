#!/bin/sh
# Install a signed Meld release (meld, meld-lsp, meld-pack plus pack data,
# licenses and docs). Missing language runtimes and cosign are installed by
# default; pass --skip-dependencies to report them without installing.
set -eu

REPO=${MELD_REPO:-enis1enis2/meld}
VERSION=${MELD_VERSION:-v0.2.0-rc.3}
PREFIX=${MELD_PREFIX:-$HOME/.local}
TARGET=${MELD_TARGET:-}
VERIFY_SIGNATURES=${MELD_VERIFY_SIGNATURES:-auto}
INSTALL_DEPENDENCIES=1
COSIGN_VERSION=2.5.3
ISSUER='https://token.actions.githubusercontent.com'

escape_regex() {
    printf '%s' "$1" | sed -e 's/[][\.^$*+?(){}|]/\\&/g'
}

usage() {
    cat <<'EOF'
usage: install.sh [--version TAG] [--prefix DIR] [--target TRIPLE]
                  [--skip-signature-verification] [--skip-dependencies]

Environment: MELD_VERSION, MELD_PREFIX, MELD_TARGET, MELD_REPO,
MELD_VERIFY_SIGNATURES (auto|1|0), MELD_SKIP_DEPENDENCIES (1 to opt out),
MELD_PACKS (extra pack roots, preserved).

Missing Python, Node.js, Lua and cosign are detected and installed by default
using the host package manager. Package installation may prompt for sudo.
There is no published Linux aarch64 artifact; auto-detection fails clearly on
that host. Only set MELD_TARGET explicitly for a target proven to be published.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --prefix) PREFIX=${2:?--prefix needs a value}; shift 2 ;;
        --target) TARGET=${2:?--target needs a value}; shift 2 ;;
        --skip-signature-verification) VERIFY_SIGNATURES=0; shift ;;
        --skip-dependencies) INSTALL_DEPENDENCIES=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'install.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "${MELD_SKIP_DEPENDENCIES:-0}" = 1 ]; then
    INSTALL_DEPENDENCIES=0
fi

case "$VERSION" in v*) TAG=$VERSION; VERSION_NO_V=${VERSION#v} ;; *) TAG=v$VERSION; VERSION_NO_V=$VERSION ;; esac
IDENTITY=$(printf '^https://github\\.com/%s/\\.github/workflows/release\\.yml@refs/tags/%s$' \
    "$(escape_regex "$REPO")" "$(escape_regex "$TAG")")

if [ -z "$TARGET" ]; then
    os=$(uname -s)
    arch=$(uname -m)
    case "$os:$arch" in
        Linux:x86_64|Linux:amd64) TARGET=x86_64-unknown-linux-gnu ;;
        Linux:aarch64|Linux:arm64)
            printf 'install.sh: no published Linux aarch64 release artifact; set MELD_TARGET only for a trusted published target\n' >&2
            exit 1
            ;;
        Darwin:x86_64) TARGET=x86_64-apple-darwin ;;
        Darwin:arm64|Darwin:aarch64) TARGET=aarch64-apple-darwin ;;
        *) printf 'install.sh: no release artifact mapping for %s/%s; set MELD_TARGET\n' "$os" "$arch" >&2; exit 1 ;;
    esac
fi
case "$TARGET" in
    aarch64-unknown-linux-gnu)
        printf 'install.sh: aarch64-unknown-linux-gnu is not published by the release matrix\n' >&2
        exit 1
        ;;
    *windows*) printf 'install.sh: use install.ps1 on Windows\n' >&2; exit 1 ;;
esac

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

have_any() {
    for candidate in "$@"; do
        command -v "$candidate" >/dev/null 2>&1 && return 0
    done
    return 1
}

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        return 1
    fi
}

package_manager() {
    if command -v apt-get >/dev/null 2>&1; then printf 'apt'
    elif command -v dnf >/dev/null 2>&1; then printf 'dnf'
    elif command -v yum >/dev/null 2>&1; then printf 'yum'
    elif command -v apk >/dev/null 2>&1; then printf 'apk'
    elif command -v pacman >/dev/null 2>&1; then printf 'pacman'
    elif command -v brew >/dev/null 2>&1; then printf 'brew'
    else printf ''
    fi
}

package_for() {
    tool=$1
    manager=$2
    case "$tool:$manager" in
        python3:apt|python3:dnf|python3:yum|python3:apk) printf 'python3' ;;
        python3:pacman) printf 'python' ;;
        python3:brew) printf 'python@3.12' ;;
        node:apt|node:dnf|node:yum|node:apk|node:pacman) printf 'nodejs' ;;
        node:brew) printf 'node' ;;
        lua:apt) printf 'lua5.4' ;;
        lua:dnf|lua:yum|lua:apk|lua:pacman|lua:brew) printf 'lua' ;;
        *) printf '' ;;
    esac
}

install_via_pm() {
    manager=$1
    package=$2
    case "$manager" in
        apt) as_root apt-get update -qq && as_root apt-get install -y "$package" ;;
        dnf) as_root dnf install -y "$package" ;;
        yum) as_root yum install -y "$package" ;;
        apk) as_root apk add --no-cache "$package" ;;
        pacman) as_root pacman -S --noconfirm "$package" ;;
        brew) brew install "$package" ;;
        *) return 1 ;;
    esac
}

ensure_runtime() {
    label=$1
    tool=$2
    shift 2
    if have_any "$@"; then
        printf 'install.sh: %s: ok\n' "$label"
        return 0
    fi
    hint=$(printf '%s not on PATH' "$*")
    if [ "$INSTALL_DEPENDENCIES" -eq 0 ]; then
        printf 'install.sh: %s missing (%s); install manually\n' "$label" "$hint" >&2
        return 0
    fi
    manager=$(package_manager)
    package=$(package_for "$tool" "$manager")
    if [ -z "$manager" ] || [ -z "$package" ]; then
        printf 'install.sh: %s missing (%s) and no supported package mapping; install manually\n' "$label" "$hint" >&2
        return 0
    fi
    printf 'install.sh: installing %s via %s (%s)\n' "$label" "$manager" "$package"
    if install_via_pm "$manager" "$package"; then
        printf 'install.sh: %s installed; open a new shell if commands are not on PATH yet\n' "$label"
    else
        printf 'install.sh: could not install %s automatically; install %s manually\n' "$label" "$package" >&2
    fi
}

ensure_cosign() {
    if have_any cosign; then
        printf 'install.sh: cosign: ok\n'
        return 0
    fi
    if [ "$INSTALL_DEPENDENCIES" -eq 0 ]; then
        printf 'install.sh: cosign missing; signature verification will be skipped\n' >&2
        return 0
    fi
    if [ "$(package_manager)" = brew ]; then
        if brew install sigstore/tap/cosign; then
            PATH="$(brew --prefix)/bin:$PATH"
            printf 'install.sh: cosign installed via Homebrew\n'
            return 0
        fi
    fi
    host_os=$(uname -s)
    host_arch=$(uname -m)
    case "$host_os:$host_arch" in
        Linux:x86_64|Linux:amd64) asset=cosign-linux-amd64 ;;
        Darwin:arm64|Darwin:aarch64) asset=cosign-darwin-arm64 ;;
        Darwin:x86_64) asset=cosign-darwin-amd64 ;;
        *) printf 'install.sh: cosign missing and no trusted binary mapping for %s/%s\n' "$host_os" "$host_arch" >&2; return 0 ;;
    esac
    cosign_base="https://github.com/sigstore/cosign/releases/download/v$COSIGN_VERSION"
    mkdir -p "$PREFIX/bin"
    printf 'install.sh: installing cosign v%s (%s)\n' "$COSIGN_VERSION" "$asset"
    download "$cosign_base/$asset" "$TMP/cosign"
    download "$cosign_base/cosign_checksums.txt" "$TMP/cosign_checksums.txt"
    expected=$(awk -v name="$asset" '$2 == name {print $1}' "$TMP/cosign_checksums.txt" | head -n 1)
    [ -n "$expected" ] || { printf 'install.sh: cosign checksum not published; skipping\n' >&2; return 0; }
    actual=$(sha256_of "$TMP/cosign")
    if [ "$expected" != "$actual" ]; then
        printf 'install.sh: cosign checksum mismatch (expected %s got %s); skipping\n' "$expected" "$actual" >&2
        return 0
    fi
    cp "$TMP/cosign" "$PREFIX/bin/cosign"
    chmod 755 "$PREFIX/bin/cosign"
    PATH="$PREFIX/bin:$PATH"
    printf 'install.sh: cosign installed to %s/bin/cosign (checksum verified)\n' "$PREFIX"
}

ensure_runtime 'Python 3' python3 python3
ensure_runtime 'Node.js' node node
ensure_runtime 'Lua' lua lua
ensure_cosign

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
