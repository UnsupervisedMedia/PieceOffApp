#!/bin/sh
# Download a pinned FFmpeg build, verify its SHA-256, and unpack ffmpeg,
# ffprobe, and the license text into <dest>/ffmpeg/.
#   scripts/fetch-ffmpeg.sh <linux64|linuxarm64|win64|winarm64> <dest>
# Downloads are cached in $PIECEOFF_DOWNLOAD_CACHE (default dist/.cache).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=${1:?usage: fetch-ffmpeg.sh <target> <dest>}
DEST=${2:?usage: fetch-ffmpeg.sh <target> <dest>}
CACHE=${PIECEOFF_DOWNLOAD_CACHE:-$ROOT/dist/.cache}

PIN=$(grep -E "^$TARGET " "$ROOT/packaging/ffmpeg-pins.txt" || true)
if [ -z "$PIN" ]; then
  echo "No pinned FFmpeg build for '$TARGET' (see docs/FFMPEG.md)." >&2
  exit 1
fi
SHA=$(echo "$PIN" | cut -d' ' -f2)
URL=$(echo "$PIN" | cut -d' ' -f3)
NAME=${URL##*/}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

mkdir -p "$CACHE" "$DEST/ffmpeg"
ARCHIVE=$CACHE/$NAME
if [ -f "$ARCHIVE" ] && [ "$(sha256_of "$ARCHIVE")" = "$SHA" ]; then
  echo "Using cached $NAME"
else
  echo "Downloading $NAME"
  curl --fail --location --silent --show-error --output "$ARCHIVE.part" "$URL"
  mv "$ARCHIVE.part" "$ARCHIVE"
fi
ACTUAL=$(sha256_of "$ARCHIVE")
if [ "$ACTUAL" != "$SHA" ]; then
  echo "SHA-256 mismatch for $NAME" >&2
  echo "  expected $SHA" >&2
  echo "  actual   $ACTUAL" >&2
  rm -f "$ARCHIVE"
  exit 1
fi

WORK=$(mktemp -d)
cleanup() { rm -r "$WORK"; }
trap cleanup EXIT
case "$NAME" in
  *.tar.xz) tar -xJf "$ARCHIVE" -C "$WORK" ;;
  *.zip) unzip -q "$ARCHIVE" -d "$WORK" ;;
  *) echo "Unknown archive type: $NAME" >&2; exit 1 ;;
esac
SRC=$(find "$WORK" -mindepth 1 -maxdepth 1 -type d | head -n 1)
EXE=
case "$TARGET" in win*) EXE=.exe ;; esac
cp "$SRC/bin/ffmpeg$EXE" "$SRC/bin/ffprobe$EXE" "$DEST/ffmpeg/"
cp "$SRC/LICENSE.txt" "$DEST/ffmpeg/LICENSE-FFmpeg.txt"
chmod +x "$DEST/ffmpeg/ffmpeg$EXE" "$DEST/ffmpeg/ffprobe$EXE"

VERSION=
if [ -z "$EXE" ] && "$DEST/ffmpeg/ffmpeg" -version >/dev/null 2>&1; then
  VERSION=$("$DEST/ffmpeg/ffmpeg" -version | head -n 1)
fi
cat >"$DEST/ffmpeg/FFMPEG-BUILD.txt" <<INFO
This folder holds an unmodified FFmpeg build, run by PieceOff as a separate
program (it is not linked into PieceOff).

Build:    $NAME
Archive:  $URL
SHA-256:  $SHA
${VERSION:+Version:  $VERSION
}License:  GNU LGPL version 3 (see LICENSE-FFmpeg.txt)
Built by: https://github.com/BtbN/FFmpeg-Builds (its build scripts and the
          exact configuration are public in that repository)
Source:   FFmpeg is free software; its source code is available from
          https://ffmpeg.org/download.html and https://git.ffmpeg.org/ffmpeg.git
          (this build is the commit named in the archive file name).
          The exact corresponding source and build materials are included in
          ../source/FFmpeg-corresponding-source.* beside this folder; see its
          SHA-256 file and docs/FFMPEG.md.

You can replace these programs with any other FFmpeg build: put your own
ffmpeg and ffprobe in this folder, or choose a folder in PieceOff under
Media tools.
INFO
echo "Unpacked FFmpeg ($TARGET) into $DEST/ffmpeg"
