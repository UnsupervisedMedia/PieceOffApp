#!/bin/sh
# Gate for the browser play client in web/: core wasm build (the tests load
# it), typecheck, tests, build, and a size budget: what the lobby loads up
# front (play.js and the chunks it imports statically, the core wasm and
# play.css), gzipped, must stay at or under 400 KB; the lazy board chunk is
# reported (web/size.mjs). Then web/pack.sh builds the release tarball into
# dist/ and web/check-pack.mjs verifies it (contents, checksums, no leaks,
# budget again on the packed bytes, byte-identical second pack; ~1 s). With --e2e it then also runs the two-client
# browser e2e against a local rooms Worker (scripts/web-e2e.sh; opt-in: it
# needs wrangler, Playwright's Chromium and about a minute).
#
#   scripts/web-check.sh [--e2e]
set -eu
E2E=0
for arg in "$@"; do
  case "$arg" in
    --e2e) E2E=1 ;;
    *) echo "usage: scripts/web-check.sh [--e2e]" >&2; exit 2 ;;
  esac
done
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
echo "== web: pack (release tarball) and check it"
./pack.sh >/dev/null
node check-pack.mjs
node check-pack-selftest.mjs "$ROOT/dist/"pieceoff-web-*.tar.gz
if [ "$E2E" -eq 1 ]; then
  "$ROOT/scripts/web-e2e.sh"
fi
echo "web checks passed."
