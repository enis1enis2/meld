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
  MELD_CARGO_TARGET_DIR
                       Cargo target root (default: ROOT/target); reproducibility
                       checks set a distinct root for each independent build.
                       Local checkout, target and registry paths are remapped to
                       stable placeholders in the compiled binaries.
  MELD_ALLOW_DIRTY_VERSION=1
                       opt in to --dirty/commit-hash suffixes in BUILD_INFO;
                       by default the version comes from the exact tag or the
                       latest tag, never from a dirty worktree or commit hash
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${MELD_VERSION:-}
TARGETS=${MELD_TARGETS:-}
OUT=${MELD_BUILD_DIR:-$ROOT/dist/build}
CARGO_TARGET_DIR=${MELD_CARGO_TARGET_DIR:-$ROOT/target}
export CARGO_TARGET_DIR

# Generated sources (notably cranelift-codegen's ISLE output) and dependency
# panic locations embed absolute OUT_DIR / registry paths in compiled binaries.
# Two independent release builds therefore differ in .rodata and in the ELF
# build ID even though the source is identical. Remap local roots to stable
# placeholders so artifacts do not depend on the checkout, target directory or
# runner home. More specific prefixes come first: remapping is applied in order.
CARGO_REGISTRY_SRC=${CARGO_HOME:-$(python3 -c 'import os; print(os.path.expanduser("~/.cargo"))')}/registry/src
if command -v cygpath >/dev/null 2>&1; then
    # Git Bash / MSYS paths must become native Windows paths for rustc to match.
    CARGO_TARGET_DIR=$(cygpath -w "$CARGO_TARGET_DIR")
    ROOT=$(cygpath -w "$ROOT")
    CARGO_REGISTRY_SRC=$(cygpath -w "$CARGO_REGISTRY_SRC")
else
    CARGO_TARGET_DIR=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$CARGO_TARGET_DIR")
    ROOT=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$ROOT")
    CARGO_REGISTRY_SRC=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$CARGO_REGISTRY_SRC")
fi
export CARGO_TARGET_DIR
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$CARGO_TARGET_DIR=/meld-target --remap-path-prefix=$CARGO_REGISTRY_SRC=/meld-registry --remap-path-prefix=$ROOT=/meld"


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
    if [ "${MELD_ALLOW_DIRTY_VERSION:-}" = "1" ]; then
        VERSION=$(git -C "$ROOT" describe --tags --always --dirty 2>/dev/null || printf 'unknown')
    else
        # JUDGMENT CALL: release BUILD_INFO must not leak a local worktree
        # state (`-dirty`) or an abbreviated commit hash. Prefer the exact tag,
        # otherwise the latest tag; opt in explicitly if a hash is wanted.
        VERSION=$(git -C "$ROOT" describe --tags --exact-match 2>/dev/null || \
            git -C "$ROOT" describe --tags --abbrev=0 2>/dev/null || printf '0.0.0')
    fi
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
BASE_CFLAGS=${CFLAGS:-}
BASE_CXXFLAGS=${CXXFLAGS:-}

for target in $TARGETS; do
    printf 'build_matrix.sh: building %s\n' "$target"

    # The bundled tree-sitter grammars compile C sources whose __FILE__ strings
    # survive into .rodata. rustc's remap flag does not reach the C compiler,
    # so give GCC/Clang the equivalent prefix map. MSVC does not accept these
    # flags; its generated Rust paths are handled by RUSTFLAGS above.
    case "$target" in
        *msvc*)
            CFLAGS=$BASE_CFLAGS
            CXXFLAGS=$BASE_CXXFLAGS
            ;;
        *)
            C_PREFIX_FLAGS=" -ffile-prefix-map=$CARGO_TARGET_DIR=/meld-target -ffile-prefix-map=$CARGO_REGISTRY_SRC=/meld-registry -ffile-prefix-map=$ROOT=/meld"
            CFLAGS="$BASE_CFLAGS$C_PREFIX_FLAGS"
            CXXFLAGS="$BASE_CXXFLAGS$C_PREFIX_FLAGS"
            ;;
    esac
    export CFLAGS CXXFLAGS

    cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked --target "$target" \
        -p meld-cli -p meld-lsp -p meld-installer

    case "$target" in
        *windows*) ext=.exe ;;
        *) ext= ;;
    esac
    srcdir="$CARGO_TARGET_DIR/$target/release"
    destdir="$OUT/$target/bin"
    mkdir -p "$destdir"
    for bin in meld meld-lsp meld-pack; do
        src="$srcdir/$bin$ext"
        [ -f "$src" ] || { printf 'build_matrix.sh: missing built binary %s\n' "$src" >&2; exit 1; }
        cp "$src" "$destdir/$bin$ext"
    done

    # link.exe/lld-link write the current time into the COFF header, all debug
    # directory entries and the CodeView GUID. Normalize them before anything
    # is hashed or signed so a rebuild of the same commit is byte-identical.
    case "$target" in
        *windows*)
            python3 "$ROOT/scripts/normalize_pe.py" \
                --epoch "$SOURCE_DATE_EPOCH" \
                --seed "$VERSION/$target" \
                "$destdir/"*.exe
            ;;
    esac

    # Fail the release build if any local path survived compilation.
    for bin in meld meld-lsp meld-pack; do
        python3 "$ROOT/tools/check_binary_paths.py" \
            --prefix "$ROOT" \
            --prefix "$CARGO_TARGET_DIR" \
            --prefix "$CARGO_REGISTRY_SRC" \
            "$destdir/$bin$ext"
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
