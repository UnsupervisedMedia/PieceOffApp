#!/bin/sh
# Builds the room core for the Worker and runs the two-client integration test
# against a local `wrangler dev`. Needs node 22+, npm, and the Rust wasm target
# (see worker/build.sh). Local only: nothing is deployed.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/worker"
./build.sh
if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then npm ci --no-audit --no-fund; fi
npx tsc --noEmit
exec node --test test/schedule.test.mjs test/two-clients.test.mjs
