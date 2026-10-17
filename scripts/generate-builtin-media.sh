#!/usr/bin/env sh
# Regenerate all original seamless procedural loop assets.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec python3 "$root/scripts/generate-builtin-media.py"
