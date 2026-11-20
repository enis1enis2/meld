#!/bin/sh
# Build the three release binaries for one or more Rust targets.
#
# This script performs real builds; it never fabricates artifacts. All builds
# are --locked and share one SOURCE_DATE_EPOCH so that packaging downstream can
# be byte-for-byte reproducible.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/build_matrix.sh [--version VERSION] [--targets "TRIPLE ..."] [--out DIR]

Options:
  --version VERSION    release version shown in BUILD_INFO (default: MELD_VERSION or git tag)
  --targets TRIPLES    space-separated target triples (default: host triple)
  --out DIR            output root (default: dist/build)

Environment:
  SOURCE_DATE_EPOCH    fixed timestamp (default: commit time of HEAD)
  MELD_VERSION         fallback release version
  MELD_TARGETS         fallback target list
  MELD_BUILD_DIR       fallback output root
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${MELD_VERSION:-}
TARGETS=${MELD_TARGETS:-}
OUT=${MELD_BUILD_DIR:-$ROOT/dist/build}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --targets) TARGETS=${2:?--targets needs a value}; shift 2 ;;
        --out) OUT=${2:?--out needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'build_matrix.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ -z "$VERSION" ]; then
    VERSION=$(git -C "$ROOT" describe --tags --always --dirty 2>/dev/null || printf 'unknown')
fi
if [ -z "$TARGETS" ]; then
    TARGETS=$(rustc -vV | sed -n 's/^host: //p')
fi
[ -n "$TARGETS" ] || { printf 'build_matrix.sh: could not determine host target\n' >&2; exit 1; }

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf '0')
fi
case "$SOURCE_DATE_EPOCH" in
    ''|*[!0-9]*) printf 'build_matrix.sh: SOURCE_DATE_EPOCH must be an integer\n' >&2; exit 2 ;;
esac
[ "$SOURCE_DATE_EPOCH" -gt 0 ] || { printf 'build_matrix.sh: SOURCE_DATE_EPOCH must be positive\n' >&2; exit 2; }
export SOURCE_DATE_EPOCH

COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')
mkdir -p "$OUT"

for target in $TARGETS; do
    printf 'build_matrix.sh: building %s\n' "$target"
    cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked --target "$target" \
        -p meld-cli -p meld-lsp -p meld-installer

    case "$target" in
        *windows*) ext=.exe ;;
        *) ext= ;;
    esac
    srcdir="$ROOT/target/$target/release"
    destdir="$OUT/$target/bin"
    mkdir -p "$destdir"
    for bin in meld meld-lsp meld-pack; do
        src="$srcdir/$bin$ext"
        [ -f "$src" ] || { printf 'build_matrix.sh: missing built binary %s\n' "$src" >&2; exit 1; }
        cp "$src" "$destdir/$bin$ext"
    done

    # Normalized metadata: no local paths, no wall-clock times, no host name.
    {
        printf 'product=meld-lang\n'
        printf 'version=%s\n' "$VERSION"
        printf 'target=%s\n' "$target"
        printf 'commit=%s\n' "$COMMIT"
        printf 'source_date_epoch=%s\n' "$SOURCE_DATE_EPOCH"
        printf 'cargo_flags=--release --locked\n'
    } > "$OUT/$target/BUILD_INFO"
    printf 'build_matrix.sh: wrote %s\n' "$OUT/$target"
done

printf 'build_matrix.sh: built %s target(s)\n' $(printf '%s\n' "$TARGETS" | wc -l | tr -d ' ')
