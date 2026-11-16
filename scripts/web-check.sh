#!/bin/sh
# Gate for the browser play client in web/: core wasm build (the tests load
# it), typecheck, tests, build, and a size budget (gzipped web/dist/play.js
# must stay at or under 400 KB).
#
#   scripts/web-check.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/web"

BUDGET=$((400 * 1024))

if [ ! -d node_modules ]; then
  echo "== web: npm ci"
  npm ci --no-audit --no-fund
fi
echo "== web: wasm"
"$ROOT/web/build-wasm.sh"
echo "== web: typecheck"
npm run --silent typecheck
echo "== web: test"
npm test --silent
echo "== web: build"
npm run --silent build
echo "== web: size budget"
SIZE=$(gzip -9 -c dist/play.js | wc -c | tr -d ' ')
echo "play.js gzip: $SIZE bytes (budget $BUDGET)"
if [ "$SIZE" -gt "$BUDGET" ]; then
  echo "web/dist/play.js exceeds the 400 KB gzip budget" >&2
  exit 1
fi
echo "web checks passed."
