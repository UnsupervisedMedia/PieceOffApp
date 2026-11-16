#!/bin/sh
# Packs the browser play client into a release tarball a site can unpack into
# its public/play-app/ directory (the default PIECEOFF_WEB_CONFIG.assetBase):
#
#   web/pack.sh [--out DIR] [--no-build]    -> DIR/pieceoff-web-<version>.tar.gz
#
# <version> is `version` in crates/pieceoff-app/Cargo.toml (the workspace has no
# version of its own; release.yml tags v<that version>). DIR defaults to the
# repo's dist/. It rebuilds web/dist (esbuild) unless --no-build; the core wasm
# (web/wasm, from web/build-wasm.sh or scripts/web-check.sh) must already be built.
#
# Contents (tar root = play-app/): play.js, chunks/*.js, play.css,
# pieceoff_web_wasm_bg.wasm, builtin/* (mp4, jpg posters, manifest.json),
# play-app.json (entry/css/wasm paths, version, protocol, sha256 per file),
# SHA256SUMS, LICENSE, THIRD_PARTY_NOTICES.txt (the Rust crates in the wasm and
# the npm packages in the bundle). The wasm glue is inlined in play.js.
#
# Reproducible: files only, sorted by name (C locale), owner 0:0, mtime fixed,
# normalised modes, gzip -n. Packing twice gives the same bytes.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
OUT="$ROOT/dist"
BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    *) echo "usage: web/pack.sh [--out DIR] [--no-build]" >&2; exit 2 ;;
  esac
done
export LC_ALL=C TZ=UTC

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/pieceoff-app/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || { echo "no version in crates/pieceoff-app/Cargo.toml" >&2; exit 1; }
[ -f "$HERE/wasm/pieceoff_web_wasm_bg.wasm" ] || { echo "web/wasm is missing: run web/build-wasm.sh" >&2; exit 1; }

cd "$HERE"
WORK=
WORK_LOG=$(mktemp "${TMPDIR:-/tmp}/pieceoff-web-pack-build.XXXXXX")
trap 'rm -r -f "$WORK" "$WORK_LOG"' EXIT
if [ "$BUILD" -eq 1 ]; then
  npm run --silent build >"$WORK_LOG" 2>&1 || { cat "$WORK_LOG" >&2; exit 1; }
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/pieceoff-web-pack.XXXXXX")
STAGE="$WORK/stage"
mkdir -p "$STAGE/chunks" "$STAGE/builtin"
cp dist/play.js dist/play.css dist/pieceoff_web_wasm_bg.wasm "$STAGE/"
cp dist/chunks/*.js "$STAGE/chunks/"
cp public/builtin/* "$STAGE/builtin/"
cp "$ROOT/LICENSE" "$STAGE/LICENSE"
(cd "$ROOT" && python3 scripts/third-party-notices.py --web web/build/meta.json web/node_modules "$STAGE/THIRD_PARTY_NOTICES.txt" >/dev/null)
node pack-index.mjs "$STAGE" "$VERSION"

find "$STAGE" -type d -exec chmod 755 {} +
find "$STAGE" -type f -exec chmod 644 {} +
mkdir -p "$OUT"
NAME="pieceoff-web-$VERSION.tar.gz"
(cd "$STAGE" && find . -type f | sed 's|^\./||' | sort > "$WORK/files.list" \
  && tar --create --no-recursion --files-from="$WORK/files.list" --sort=name \
       --mtime='2020-01-01 00:00:00 UTC' --owner=0 --group=0 --numeric-owner \
       --format=ustar --file=- \
  | gzip -n -9 > "$OUT/$NAME")
echo "$OUT/$NAME"
