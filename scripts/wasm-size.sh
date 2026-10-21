#!/bin/sh
# Measures the room core as a Worker would ship it. Needs:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129 --locked
# wasm-opt (binaryen) and brotli are used when present.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
OUT=$(mktemp -d "${TMPDIR:-/tmp}/pieceoff-room-wasm.XXXXXX")
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_PROFILE_RELEASE_STRIP=true \
  cargo build --release --locked --target wasm32-unknown-unknown \
    -p pieceoff-room-wasm --target-dir "$OUT/target"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" \
  "$OUT/target/wasm32-unknown-unknown/release/pieceoff_room_wasm.wasm"
W="$OUT/pieceoff_room_wasm_bg.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz -o "$W.opt" "$W" && mv "$W.opt" "$W"
else
  echo "note: wasm-opt not found, size is without it" >&2
fi
printf 'raw     %s bytes\n' "$(wc -c < "$W")"
printf 'gzip -9 %s bytes\n' "$(gzip -9 -c "$W" | wc -c)"
if command -v brotli >/dev/null 2>&1; then
  printf 'brotli  %s bytes\n' "$(brotli -q 11 -c "$W" | wc -c)"
fi
echo "output in $OUT"
