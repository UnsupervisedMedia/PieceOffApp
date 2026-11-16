#!/usr/bin/env sh
# Regenerate web/public/builtin/ (mp4 + poster per bundled loop, manifest.json)
# from the animated WebPs, then verify the sha set equals the core's list.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
python3 "$root/scripts/generate-web-media.py"
cd "$root/web"
exec npx vitest run src/media/builtin.test.ts
