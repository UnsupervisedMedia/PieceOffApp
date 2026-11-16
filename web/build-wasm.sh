#!/bin/sh
# Builds the web client's core exports into web/wasm/. Needs:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
cd "$ROOT"
# Path-independent output: panic/location strings must not carry the builder's
# home directory, cargo registry path or checkout path (the wasm is published).
CARGO_DIR=${CARGO_HOME:-$HOME/.cargo}
SYSROOT=$(rustc --print sysroot)
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$CARGO_DIR=/cargo --remap-path-prefix=$ROOT=/src --remap-path-prefix=$SYSROOT=/rustc-sysroot"
export RUSTFLAGS
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_PROFILE_RELEASE_STRIP=true \
  cargo build --release --locked --target wasm32-unknown-unknown -p pieceoff-web-wasm
mkdir -p "$HERE/wasm"
wasm-bindgen --target web --out-dir "$HERE/wasm" \
  "$ROOT/target/wasm32-unknown-unknown/release/pieceoff_web_wasm.wasm"
