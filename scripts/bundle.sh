#!/bin/sh
# Assemble a release bundle for this machine: the app and server, FFmpeg with
# its corresponding source, license notices, and docs, packed into
# dist/<name>.tar.gz.
#
#   scripts/bundle.sh [--target linux64|linuxarm64|macos] [--no-ffmpeg]
#                     [--skip-build] [--skip-verify]
#
# FFmpeg is built from source by scripts/build-ffmpeg.sh (see docs/FFMPEG.md);
# set PIECEOFF_FFMPEG_STAGE to a folder it already filled to reuse that build.
# Windows uses scripts/bundle.ps1.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

TARGET=
WITH_FFMPEG=yes
BUILD=yes
VERIFY=yes
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET=${2:?--target needs a value}; shift ;;
    --no-ffmpeg) WITH_FFMPEG=no ;;
    --with-ffmpeg) WITH_FFMPEG=yes ;;  # the default; kept for old scripts
    --skip-build) BUILD=no ;;
    --skip-verify) VERIFY=no ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$TARGET" ]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) TARGET=linux64 ;;
    Linux-aarch64|Linux-arm64) TARGET=linuxarm64 ;;
    Darwin-*) TARGET=macos ;;
    *) echo "Cannot tell the target for $(uname -s)-$(uname -m); use --target." >&2; exit 2 ;;
  esac
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/pieceoff-app/Cargo.toml | head -n 1)
NAME=PieceOff-$VERSION-$TARGET
case "$TARGET" in macos) NAME=$NAME-$(uname -m) ;; esac
OUT=$ROOT/dist/$NAME

if [ "$BUILD" = yes ]; then
  cargo build --release --locked -p pieceoff-app -p pieceoff-server
fi
for binary in pieceoff-app pieceoff-server; do
  [ -x "target/release/$binary" ] || { echo "target/release/$binary is missing; build first." >&2; exit 1; }
done

rm -r "$OUT" 2>/dev/null || true
mkdir -p "$OUT/docs"
cp target/release/pieceoff-app target/release/pieceoff-server pieceoff.sh "$OUT/"
cp README.md LICENSE CONTRIBUTING.md "$OUT/"
cp docs/PACKAGING.md docs/FFMPEG.md docs/TESTING.md docs/DEPLOYMENT.md docs/LICENSING.md "$OUT/docs/" 2>/dev/null || true

if [ "$WITH_FFMPEG" = yes ]; then
  STAGE=${PIECEOFF_FFMPEG_STAGE:-$ROOT/dist/.ffmpeg-$TARGET-$(uname -m)}
  "$ROOT/scripts/build-ffmpeg.sh" "$TARGET" "$STAGE"
  cp -R "$STAGE/ffmpeg" "$STAGE/source" "$OUT/"
fi

python3 scripts/license-report.py "$OUT/THIRD_PARTY_LICENSES.txt"
python3 scripts/third-party-notices.py "$OUT/THIRD_PARTY_NOTICES.txt"

if [ "$VERIFY" = yes ]; then
  echo "Verifying the bundle..."
  if [ "$WITH_FFMPEG" = yes ]; then
    "$OUT/ffmpeg/ffmpeg" -version | head -n 1
    "$OUT/ffmpeg/ffprobe" -version | head -n 1
  fi
  PORT=${PIECEOFF_VERIFY_PORT:-28979}
  DATA=$(mktemp -d)
  PIECEOFF_BIND=127.0.0.1:$PORT PIECEOFF_DATA_DIR=$DATA "$OUT/pieceoff-server" >"$DATA/server.log" 2>&1 &
  SERVER=$!
  HEALTHY=no
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl --silent --fail "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then HEALTHY=yes; break; fi
    sleep 0.5
  done
  kill "$SERVER" 2>/dev/null || true
  wait "$SERVER" 2>/dev/null || true
  if [ "$HEALTHY" = yes ]; then rm -r "$DATA"; fi
  if [ "$HEALTHY" != yes ]; then
    echo "The bundled server did not answer /health:" >&2
    cat "$DATA/server.log" >&2
    exit 1
  fi
  echo "Bundled server answers /health."
fi

tar -czf "dist/$NAME.tar.gz" -C dist "$NAME"
( cd dist && { sha256sum "$NAME.tar.gz" 2>/dev/null || shasum -a 256 "$NAME.tar.gz"; } >"$NAME.tar.gz.sha256" )
echo "Bundle:   dist/$NAME.tar.gz ($(du -h "dist/$NAME.tar.gz" | cut -f1))"
echo "Checksum: dist/$NAME.tar.gz.sha256"
