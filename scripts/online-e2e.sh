#!/bin/sh
# Plays the desktop app against the real rooms Worker: builds the room core
# for the Worker, starts a local `wrangler dev` set up like PieceOff Online
# (uploads off, device links approved through a stand-in Turnstile), runs the
# ignored-by-default e2e test `e2e::online::` with its address, and stops
# everything again. Needs what scripts/worker-test.sh needs plus cargo.
# Local only: nothing is deployed.
#
#   scripts/online-e2e.sh [extra cargo test arguments]
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/worker"
./build.sh
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then npm ci --no-audit --no-fund; fi

# Build the test binary first so the server is not left idle meanwhile.
cd "$ROOT"
cargo test -p pieceoff-app --locked --no-run

WORK=$(mktemp -d "${TMPDIR:-/tmp}/pieceoff-online-e2e.XXXXXX")
SERVER_PID=
cleanup() {
  if [ -n "$SERVER_PID" ]; then
    kill -TERM "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

(cd "$ROOT/worker" && exec node test/online-e2e-server.mjs "$WORK/server.json") &
SERVER_PID=$!
i=0
while [ ! -s "$WORK/server.json" ]; do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "The local rooms Worker did not start." >&2
    exit 1
  fi
  i=$((i + 1))
  if [ "$i" -gt 240 ]; then
    echo "Timed out waiting for the local rooms Worker." >&2
    exit 1
  fi
  sleep 0.5
done
field() {
  node -e 'process.stdout.write(String(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))[process.argv[2]]))' "$WORK/server.json" "$1"
}
PIECEOFF_ONLINE_E2E_URL=$(field url)
PIECEOFF_ONLINE_E2E_SITE_ORIGIN=$(field site_origin)
PIECEOFF_ONLINE_E2E_TURNSTILE_TOKEN=$(field turnstile_token)
export PIECEOFF_ONLINE_E2E_URL PIECEOFF_ONLINE_E2E_SITE_ORIGIN PIECEOFF_ONLINE_E2E_TURNSTILE_TOKEN

cargo test -p pieceoff-app --locked e2e::online:: -- --ignored --nocapture "$@"
