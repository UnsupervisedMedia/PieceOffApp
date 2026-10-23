#!/bin/sh
# Builds the room core for the Worker into worker/wasm/. Needs:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
cd "$ROOT"
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_PROFILE_RELEASE_STRIP=true \
  cargo build --release --locked --target wasm32-unknown-unknown -p pieceoff-room-wasm
mkdir -p "$HERE/wasm"
wasm-bindgen --target web --out-dir "$HERE/wasm" \
  "$ROOT/target/wasm32-unknown-unknown/release/pieceoff_room_wasm.wasm"
