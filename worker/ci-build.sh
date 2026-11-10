#!/bin/sh
# Build step for Cloudflare Workers Builds (root directory `worker`, build
# command `./ci-build.sh`, deploy command `npx wrangler deploy`). The build
# image has Node but not Rust, so this installs what build.sh needs when it
# is missing, then builds the wasm room core. Safe to run locally: it skips
# anything already installed.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
WASM_BINDGEN_VERSION=0.2.129
# sha256 of wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz from the
# project's GitHub release, checked 2026-11-10.
WASM_BINDGEN_SHA256=82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e

export PATH="$HOME/.cargo/bin:$PATH"
fetch() {
  curl --proto '=https' --tlsv1.2 -sSfL --retry 3 -o "$2" "$1"
}

if ! command -v cargo >/dev/null 2>&1; then
  echo "== installing Rust (rustup-init, minimal profile)"
  tmp=$(mktemp -d)
  base="https://static.rust-lang.org/rustup/dist/$(uname -m)-unknown-linux-gnu"
  fetch "$base/rustup-init" "$tmp/rustup-init"
  fetch "$base/rustup-init.sha256" "$tmp/rustup-init.sha256"
  (cd "$tmp" && echo "$(cut -d' ' -f1 rustup-init.sha256)  rustup-init" | sha256sum -c -)
  chmod +x "$tmp/rustup-init"
  "$tmp/rustup-init" -y --profile minimal --default-toolchain stable --no-modify-path
  rm -rf "$tmp"
fi

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "== adding the wasm32-unknown-unknown target"
  rustup target add wasm32-unknown-unknown
fi

if ! wasm-bindgen --version 2>/dev/null | grep -qx "wasm-bindgen $WASM_BINDGEN_VERSION"; then
  if [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ]; then
    echo "== installing wasm-bindgen $WASM_BINDGEN_VERSION (prebuilt release, sha256 pinned)"
    name="wasm-bindgen-$WASM_BINDGEN_VERSION-x86_64-unknown-linux-musl"
    tmp=$(mktemp -d)
    fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$WASM_BINDGEN_VERSION/$name.tar.gz" \
      "$tmp/wb.tar.gz"
    echo "$WASM_BINDGEN_SHA256  $tmp/wb.tar.gz" | sha256sum -c -
    tar -xzf "$tmp/wb.tar.gz" -C "$tmp"
    mkdir -p "$HOME/.cargo/bin"
    install -m 0755 "$tmp/$name/wasm-bindgen" "$HOME/.cargo/bin/wasm-bindgen"
    rm -rf "$tmp"
  else
    echo "== installing wasm-bindgen $WASM_BINDGEN_VERSION from source (no pinned prebuilt here)"
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --locked
  fi
fi

exec "$HERE/build.sh"
