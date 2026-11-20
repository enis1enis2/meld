#!/bin/sh
# Build the Phase 4 demo Rust guest and wrap it as a Component Model component.
#
# JUDGMENT CALL: the component binary is committed so `cargo test` needs no
# wasm target or wasm-tools install. This script is the reproducible regeneration
# path and is documented in spec/t3-component.md.
#
# `--check` is a non-mutating prerequisite probe for CI smoke jobs: it prints
# SKIP (exit 0) when the optional toolchain is unavailable, or READY when the
# full build path can run. Actual builds fail explicitly on missing tools.
set -eu

CHECK_ONLY=0
if [ "${1:-}" = "--check" ]; then
    CHECK_ONLY=1
    shift
fi
if [ "$#" -ne 0 ]; then
    echo "usage: $0 [--check]" >&2
    exit 2
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WASM_TOOLS=${WASM_TOOLS:-wasm-tools}
GUEST="$ROOT/crates/meld-t3/guest"
OUT="$ROOT/crates/meld-t3/component"
CORE="$GUEST/target/wasm32-unknown-unknown/release/meld_demo_guest.wasm"
EMBEDDED="$OUT/demo.embedded.wasm"
COMPONENT="$OUT/demo.component.wasm"

have_cmd() {
    command -v "$1" >/dev/null 2>&1
}

missing=""
if ! have_cmd cargo; then
    missing="$missing cargo"
fi
if ! have_cmd "$WASM_TOOLS"; then
    missing="$missing $WASM_TOOLS"
fi
if have_cmd rustup && ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
    missing="$missing rustup-target:wasm32-unknown-unknown"
fi

if [ -n "$missing" ]; then
    if [ "$CHECK_ONLY" -eq 1 ]; then
        echo "build_t3_guest.sh: SKIP (missing prerequisites:$missing)"
        exit 0
    fi
    echo "build_t3_guest.sh: missing prerequisites:$missing" >&2
    exit 1
fi
if [ "$CHECK_ONLY" -eq 1 ]; then
    echo "build_t3_guest.sh: READY (cargo, $WASM_TOOLS, wasm32-unknown-unknown)"
    exit 0
fi

mkdir -p "$OUT"
cd "$GUEST"
cargo build --release --target wasm32-unknown-unknown
cd "$ROOT"

# Embed the WIT world metadata into the core module, then componentize it.
"$WASM_TOOLS" component embed "$ROOT/crates/meld-t3/wit/meld.wit" "$CORE" -o "$EMBEDDED"
"$WASM_TOOLS" component new "$EMBEDDED" -o "$COMPONENT"
"$WASM_TOOLS" validate "$COMPONENT"
rm -f "$EMBEDDED"
echo "wrote $COMPONENT"
