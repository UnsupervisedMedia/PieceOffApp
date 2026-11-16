#!/bin/sh
# Two-client browser e2e for the web play client: builds the room core for the
# Worker and the web release bundle, starts a local `wrangler dev` set up like
# PieceOff Online (worker/test/online-e2e-server.mjs: uploads off, Turnstile
# required through a loopback stand-in) with this run's page origin as the
# site, a static server for the built page (web/e2e/serve.mjs), and runs
# web/e2e/two-client.spec.ts in headless Chromium (Playwright, its own
# web/e2e/package.json so scripts/web-check.sh stays light). Everything it
# starts is stopped on exit. Local only: nothing is deployed.
#
#   scripts/web-e2e.sh [extra playwright test arguments]
#
# Needs what scripts/worker-test.sh and scripts/web-check.sh need, plus
# Playwright's Chromium 1200 in ~/.cache/ms-playwright (`npx playwright
# install chromium` inside web/e2e if missing). On failure, traces and
# screenshots are under web/e2e/test-results/.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)

echo "== web e2e: worker build"
cd "$ROOT/worker"
./build.sh
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then npm ci --no-audit --no-fund; fi

echo "== web e2e: web build"
cd "$ROOT/web"
if [ ! -d node_modules ]; then npm ci --no-audit --no-fund; fi
"$ROOT/web/build-wasm.sh"
npm run --silent build

cd "$ROOT/web/e2e"
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/pieceoff-web-e2e.XXXXXX")
PAGE_PID=
WORKER_PID=
cleanup() {
  for pid in $WORKER_PID $PAGE_PID; do
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$WORK"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# wait_for FILE PID WHAT: until FILE has content, or fail when PID dies / 120 s pass.
wait_for() {
  i=0
  while [ ! -s "$1" ]; do
    if ! kill -0 "$2" 2>/dev/null; then
      echo "The $3 did not start." >&2
      exit 1
    fi
    i=$((i + 1))
    if [ "$i" -gt 240 ]; then
      echo "Timed out waiting for the $3." >&2
      exit 1
    fi
    sleep 0.5
  done
}
field() {
  node -e 'process.stdout.write(String(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))[process.argv[2]]))' "$1" "$2"
}

echo "== web e2e: servers"
(cd "$ROOT/web" && exec node e2e/serve.mjs "$WORK/page.json") &
PAGE_PID=$!
wait_for "$WORK/page.json" "$PAGE_PID" "page server"
E2E_SITE=$(field "$WORK/page.json" url)

(cd "$ROOT/worker" && PIECEOFF_E2E_SITE_ORIGIN="$E2E_SITE" exec node test/online-e2e-server.mjs "$WORK/worker.json") &
WORKER_PID=$!
wait_for "$WORK/worker.json" "$WORKER_PID" "local rooms Worker"
E2E_API=$(field "$WORK/worker.json" api)
export E2E_SITE E2E_API
echo "page $E2E_SITE, rooms Worker $E2E_API"

echo "== web e2e: playwright"
start=$(date +%s)
status=0
npx playwright test "$@" || status=$?
took=$(( $(date +%s) - start ))
if [ "$status" -eq 0 ]; then
  echo "web e2e passed (${took}s)."
else
  echo "web e2e FAILED (exit $status, ${took}s); traces and screenshots: $ROOT/web/e2e/test-results/" >&2
fi
exit "$status"
