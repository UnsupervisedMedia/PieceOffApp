#!/bin/sh
# Cross-build the Windows (x86_64 GNU) bundle from Linux: app and server .exe,
# Windows FFmpeg (cross-built by scripts/build-ffmpeg.sh) with its source,
# license notices, docs, and a launcher, packed into
# dist/PieceOff-<version>-win64-gnu.zip.
#
#   scripts/bundle-windows-cross.sh [--no-ffmpeg] [--skip-build]
#
# This proves the tree compiles and links for Windows and gives testers a zip.
# It does not run anything: launch it on a real Windows machine before
# calling Windows verified (docs/TESTING.md). Needs the x86_64-pc-windows-gnu
# rustup target, mingw-w64 (x86_64-w64-mingw32-gcc), nasm, python3 and zip.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

TARGET=x86_64-pc-windows-gnu
WITH_FFMPEG=yes
BUILD=yes
while [ $# -gt 0 ]; do
  case "$1" in
    --no-ffmpeg) WITH_FFMPEG=no ;;
    --with-ffmpeg) WITH_FFMPEG=yes ;;  # the default; kept for old scripts
    --skip-build) BUILD=no ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

need() { command -v "$1" >/dev/null 2>&1 || { echo "Missing $1: $2" >&2; exit 1; }; }
need x86_64-w64-mingw32-gcc "install mingw-w64 (apt: gcc-mingw-w64-x86-64)"
need zip "install zip"
rustup target list --installed 2>/dev/null | grep -qx "$TARGET" \
  || { echo "Missing rustup target: rustup target add $TARGET" >&2; exit 1; }

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/pieceoff-app/Cargo.toml | head -n 1)
NAME=PieceOff-$VERSION-win64-gnu
OUT=$ROOT/dist/$NAME
# Its own target dir keeps cross artifacts apart from the Linux build.
WIN_TARGET_DIR=${PIECEOFF_WINDOWS_TARGET_DIR:-$ROOT/target/windows}
BIN=$WIN_TARGET_DIR/$TARGET/release

if [ "$BUILD" = yes ]; then
  CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    cargo build --release --locked --target "$TARGET" --target-dir "$WIN_TARGET_DIR" -p pieceoff-app -p pieceoff-server
fi
for binary in pieceoff-app.exe pieceoff-server.exe; do
  [ -f "$BIN/$binary" ] || { echo "$BIN/$binary is missing; build first." >&2; exit 1; }
  # Every Windows executable starts with the DOS "MZ" header.
  [ "$(head -c 2 "$BIN/$binary")" = MZ ] || { echo "$binary is not a Windows executable." >&2; exit 1; }
done

mkdir -p "$ROOT/dist"
rm -r "$OUT" 2>/dev/null || true
mkdir -p "$OUT/docs"
cp "$BIN/pieceoff-app.exe" "$BIN/pieceoff-server.exe" pieceoff.ps1 README.md LICENSE CONTRIBUTING.md "$OUT/"
cp docs/PACKAGING.md docs/FFMPEG.md docs/TESTING.md docs/DEPLOYMENT.md docs/LICENSING.md "$OUT/docs/" 2>/dev/null || true
sed "s/@VERSION@/$VERSION/" packaging/windows-START-HERE.txt >"$OUT/START-HERE.txt"

if [ "$WITH_FFMPEG" = yes ]; then
  [ "${PIECEOFF_FFMPEG_NO_ASM:-0}" = 1 ] || need nasm "install nasm (the x86 FFmpeg build needs it)"
  STAGE=${PIECEOFF_FFMPEG_STAGE:-$ROOT/dist/.ffmpeg-win64-cross}
  "$ROOT/scripts/build-ffmpeg.sh" win64 "$STAGE"
  cp -R "$STAGE/ffmpeg" "$STAGE/source" "$OUT/"
fi
python3 scripts/license-report.py "$OUT/THIRD_PARTY_LICENSES.txt"
python3 scripts/third-party-notices.py "$OUT/THIRD_PARTY_NOTICES.txt"

rm "dist/$NAME.zip" 2>/dev/null || true
( cd dist && zip -qr "$NAME.zip" "$NAME" && sha256sum "$NAME.zip" >"$NAME.zip.sha256" )
echo "Bundle:   dist/$NAME.zip ($(du -h "dist/$NAME.zip" | cut -f1))"
echo "Checksum: dist/$NAME.zip.sha256"
echo "Cross-built only: not launched on Windows."
