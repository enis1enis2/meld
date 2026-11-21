#!/bin/sh
# Verify that two independent release builds/package runs produce identical
# archives. With --compare, verify two existing package directories instead.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/check_reproducibility.sh --version VERSION --target TRIPLE [--out DIR]
       scripts/check_reproducibility.sh --compare DIR_A DIR_B

In build mode the script runs build_matrix.sh and package_release.sh twice with
one fixed SOURCE_DATE_EPOCH and compares the archive hashes. In compare mode it
only compares archives that already exist in DIR_A and DIR_B.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${MELD_VERSION:-}
TARGET=${MELD_TARGET:-}
OUT=$ROOT/dist/repro
COMPARE=

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --target) TARGET=${2:?--target needs a value}; shift 2 ;;
        --out) OUT=${2:?--out needs a value}; shift 2 ;;
        --compare) COMPARE=yes; A=${2:?--compare needs directory A}; B=${3:?--compare needs directory B}; shift 3 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'check_reproducibility.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

archive_hashes() {
    dir=$1
    [ -d "$dir" ] || { printf 'check_reproducibility.sh: missing directory %s\n' "$dir" >&2; exit 1; }
    find "$dir" -type f \( -name '*.tar.gz' -o -name '*.zip' \) -print | LC_ALL=C sort | while IFS= read -r file; do
        hash=$(sha256sum "$file" | awk '{print $1}')
        printf '%s  %s\n' "$hash" "${file##*/}"
    done
}

if [ -n "$COMPARE" ]; then
    A=$A B=$B archive_hashes "$A" > "$ROOT/.repro-a.$$"
    archive_hashes "$B" > "$ROOT/.repro-b.$$"
else
    [ -n "$VERSION" ] && [ -n "$TARGET" ] || { usage >&2; exit 2; }
    if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
        SOURCE_DATE_EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf '0')
    fi
    case "$SOURCE_DATE_EPOCH" in
        ''|*[!0-9]*) printf 'check_reproducibility.sh: SOURCE_DATE_EPOCH must be an integer\n' >&2; exit 2 ;;
    esac
    [ "$SOURCE_DATE_EPOCH" -gt 0 ] || { printf 'check_reproducibility.sh: SOURCE_DATE_EPOCH must be positive\n' >&2; exit 2; }
    export SOURCE_DATE_EPOCH

    mkdir -p "$OUT"
    for run in a b; do
        # A distinct Cargo target root forces a genuine second compile+link.
        # Reusing ROOT/target made the second "build" a no-op that copied the
        # first binary, which hid toolchain-introduced nondeterminism such as
        # PE link timestamps.
        MELD_CARGO_TARGET_DIR="$OUT/cargo-$run" \
            sh "$ROOT/scripts/build_matrix.sh" --version "$VERSION" --targets "$TARGET" --out "$OUT/build-$run"
        sh "$ROOT/scripts/package_release.sh" --version "$VERSION" --target "$TARGET" \
            --build-dir "$OUT/build-$run" --out "$OUT/package-$run"
    done
    archive_hashes "$OUT/package-a" > "$ROOT/.repro-a.$$"
    archive_hashes "$OUT/package-b" > "$ROOT/.repro-b.$$"
fi

trap 'rm -f "$ROOT/.repro-a.$$" "$ROOT/.repro-b.$$"' EXIT HUP INT TERM
if [ ! -s "$ROOT/.repro-a.$$" ]; then
    printf 'check_reproducibility.sh: no archives found to compare\n' >&2
    exit 1
fi
if cmp -s "$ROOT/.repro-a.$$" "$ROOT/.repro-b.$$"; then
    printf 'check_reproducibility.sh: PASS (identical archive hashes)\n'
    cat "$ROOT/.repro-a.$$"
    exit 0
fi
printf 'check_reproducibility.sh: FAIL (archives differ)\n' >&2
diff -u "$ROOT/.repro-a.$$" "$ROOT/.repro-b.$$" >&2 || true
exit 1
