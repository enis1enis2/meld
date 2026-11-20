#!/bin/sh
# Build the Phase 4 demo Rust guest and wrap it as a Component Model component.
#
# JUDGMENT CALL: the component binary is committed so `cargo test` needs no
# wasm target or wasm-tools install. This script is the reproducible regeneration
# path and is documented in spec/t3-component.md.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WASM_TOOLS=${WASM_TOOLS:-wasm-tools}
GUEST="$ROOT/crates/meld-t3/guest"
OUT="$ROOT/crates/meld-t3/component"
CORE="$GUEST/target/wasm32-unknown-unknown/release/meld_demo_guest.wasm"
EMBEDDED="$OUT/demo.embedded.wasm"
COMPONENT="$OUT/demo.component.wasm"

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
