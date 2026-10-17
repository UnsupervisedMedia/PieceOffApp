#!/bin/sh
# The local gate to run before pushing a slice (hosted CI is not relied on):
# formatting, clippy, the workspace tests, and a Windows cross-check so a
# change that only breaks the Windows build is caught on Linux.
#
#   scripts/check.sh [--no-windows] [--bundle]
#
# --bundle also assembles a no-FFmpeg test zip at
# dist/PieceOff-<version>-win64-gnu.zip. The Windows steps are skipped, loudly,
# when the target or mingw-w64 is not installed.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

WINDOWS=yes
BUNDLE=no
while [ $# -gt 0 ]; do
  case "$1" in
    --no-windows) WINDOWS=no ;;
    --bundle) BUNDLE=yes ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

TARGET=x86_64-pc-windows-gnu
if [ "$WINDOWS" = yes ]; then
  if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 \
    || ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    echo "WARNING: Windows cross-check skipped (needs mingw-w64 and: rustup target add $TARGET)" >&2
    WINDOWS=no
    BUNDLE=no
  fi
fi

echo "== fmt"
cargo fmt --all -- --check
echo "== clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings
echo "== test"
cargo test --workspace --locked
if [ "$WINDOWS" = yes ]; then
  echo "== windows cross-check ($TARGET)"
  CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    cargo check --workspace --all-targets --locked --target "$TARGET" \
      --target-dir "${PIECEOFF_WINDOWS_TARGET_DIR:-$ROOT/target/windows}"
fi
if [ "$BUNDLE" = yes ]; then
  echo "== windows bundle"
  "$ROOT/scripts/bundle-windows-cross.sh"
fi
echo "All checks passed."
