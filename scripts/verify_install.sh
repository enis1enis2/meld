#!/bin/sh
# Smoke-test an installed Meld distribution without installing anything.
# This is a layout/executable smoke test ONLY: it is not an integrity,
# signature, checksum or provenance verification. Use scripts/verify_artifacts.sh
# for release authenticity checks.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/verify_install.sh [--root PREFIX | --bin-dir DIR --share-dir DIR]

Smoke-checks binaries, bundled packs, licenses and docs. This does NOT verify
file integrity, signatures or provenance. MELD_PACKS is set for the smoke test
so an install layout that keeps packs under share/ works as shipped.
No language runtime is installed, downloaded or required by this script.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=
BIN_DIR=
SHARE_DIR=

while [ $# -gt 0 ]; do
    case "$1" in
        --root) PREFIX=${2:?--root needs a value}; shift 2 ;;
        --bin-dir) BIN_DIR=${2:?--bin-dir needs a value}; shift 2 ;;
        --share-dir) SHARE_DIR=${2:?--share-dir needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'verify_install.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
if [ -n "$PREFIX" ]; then
    [ -z "$BIN_DIR" ] && BIN_DIR="$PREFIX/bin"
    [ -z "$SHARE_DIR" ] && SHARE_DIR="$PREFIX/share/meld-lang"
fi
[ -n "$BIN_DIR" ] && [ -n "$SHARE_DIR" ] || { usage >&2; exit 2; }
[ -d "$BIN_DIR" ] || { printf 'verify_install.sh: missing bin directory %s\n' "$BIN_DIR" >&2; exit 1; }
[ -d "$SHARE_DIR" ] || { printf 'verify_install.sh: missing share directory %s\n' "$SHARE_DIR" >&2; exit 1; }

case "$(uname -s 2>/dev/null || printf unknown)" in
    MINGW*|MSYS*|CYGWIN*|Windows*) ext=.exe ;;
    *) if [ -f "$BIN_DIR/meld.exe" ]; then ext=.exe; else ext=; fi ;;
esac

for bin in meld meld-lsp meld-pack; do
    path="$BIN_DIR/$bin$ext"
    [ -f "$path" ] || { printf 'verify_install.sh: missing binary %s\n' "$path" >&2; exit 1; }
    [ -x "$path" ] || { printf 'verify_install.sh: not executable: %s\n' "$path" >&2; exit 1; }
done
printf 'verify_install.sh: PASS three binaries are present and executable\n'

# Installed prefixes keep packs under share/meld-lang/packs; raw release
# archives keep them next to bin/. Accept both layouts.
PACKS="$SHARE_DIR/packs"
if [ ! -d "$PACKS" ] && [ -d "$SHARE_DIR/../../packs" ]; then
    PACKS="$SHARE_DIR/../../packs"
fi
[ -f "$PACKS/python/pack.toml" ] || { printf 'verify_install.sh: missing Python pack data\n' >&2; exit 1; }
[ -f "$PACKS/bash/pack.toml" ] || { printf 'verify_install.sh: missing Bash pack data\n' >&2; exit 1; }
[ -f "$PACKS/python/queries/highlights.scm" ] || { printf 'verify_install.sh: missing Python queries\n' >&2; exit 1; }
printf 'verify_install.sh: PASS bundled pack data\n'

# Accept both an installed prefix and the raw release archive layout
# (share/doc/meld-lang next to share/meld-lang).
DOC_DIRS="$SHARE_DIR/doc $SHARE_DIR/docs $SHARE_DIR/../doc/meld-lang"
[ -n "$PREFIX" ] && DOC_DIRS="$PREFIX/share/doc/meld-lang $DOC_DIRS"
# Use the same payload manifest as scripts/package_release.sh (basenames).
PAYLOAD_MANIFEST="$ROOT/scripts/release_payload.txt"
[ -f "$PAYLOAD_MANIFEST" ] || {
    printf 'verify_install.sh: missing payload manifest %s\n' "$PAYLOAD_MANIFEST" >&2; exit 1
}
while IFS= read -r payload_path || [ -n "$payload_path" ]; do
    case "$payload_path" in ''|'#'*) continue ;; esac
    doc=${payload_path##*/}
    found=no
    for dir in $DOC_DIRS; do
        [ -f "$dir/$doc" ] && found=yes && break
    done
    [ "$found" = yes ] || {
        printf 'verify_install.sh: missing documentation or license %s\n' "$doc" >&2; exit 1;
    }
done < "$PAYLOAD_MANIFEST"
printf 'verify_install.sh: PASS licenses and documentation\n'

MELD_PACKS="$PACKS${MELD_PACKS:+:$MELD_PACKS}" "$BIN_DIR/meld$ext" doctor >/dev/null
printf 'verify_install.sh: PASS meld doctor smoke test\n'
"$BIN_DIR/meld-pack$ext" --help >/dev/null
printf 'verify_install.sh: PASS meld-pack help smoke test\n'
printf 'verify_install.sh: install SMOKE TEST complete (not an integrity verification; no runtime was installed)\n'
