#!/bin/sh
# Gate for the browser play client in web/: core wasm build (the tests load
# it), typecheck, tests, build, and a size budget: what the lobby loads up
# front (play.js and the chunks it imports statically, the core wasm and
# play.css), gzipped, must stay at or under 400 KB; the lazy board chunk is
# reported (web/size.mjs).
#
#   scripts/web-check.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/web"

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
node size.mjs
echo "web checks passed."
