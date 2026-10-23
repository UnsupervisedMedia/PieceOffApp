#!/bin/sh
# Build the FFmpeg that PieceOff bundles, from the pinned upstream releases,
# and stage it with its corresponding source:
#
#   scripts/build-ffmpeg.sh <linux64|linuxarm64|macos|win64> <dest>
#
# Writes <dest>/ffmpeg/ (ffmpeg, ffprobe, licenses, FFMPEG-BUILD.txt) and
# <dest>/source/ (the pristine upstream tarballs, their signatures and
# checksums, this script, and the pin file). The build is LGPL only: no GPL or
# nonfree components. The one external library is zlib, built here from the
# pinned source and linked statically. See docs/FFMPEG.md.
#
# win64 builds natively under MSYS2 MINGW64, or cross-builds on Linux with
# mingw-w64. Downloads are cached in $PIECEOFF_DOWNLOAD_CACHE (default
# dist/.cache). PIECEOFF_FFMPEG_NO_ASM=1 skips the x86 assembly (and the
# nasm requirement) for a slower local build; official bundles never do.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=${1:?usage: build-ffmpeg.sh <linux64|linuxarm64|macos|win64> <dest>}
DEST=${2:?usage: build-ffmpeg.sh <linux64|linuxarm64|macos|win64> <dest>}
case "$DEST" in /*) ;; *) DEST=$PWD/$DEST ;; esac
CACHE=${PIECEOFF_DOWNLOAD_CACHE:-$ROOT/dist/.cache}
PIN=$ROOT/packaging/ffmpeg-source.txt

pin() { sed -n "s/^$1 //p" "$PIN" | head -n 1; }
VERSION=$(pin version)
SHA=$(pin sha256)
URL=$(pin url)
NAME=${URL##*/}
ZVERSION=$(pin zlib-version)
ZSHA=$(pin zlib-sha256)
ZURL=$(pin zlib-url)
ZNAME=${ZURL##*/}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# --- configure flags -------------------------------------------------------
# --disable-autodetect keeps every system library, hardware API and system
# codec out, so only what is listed here is linked. --disable-network:
# PieceOff only reads local files.
CONFIGURE="--disable-gpl --disable-nonfree --disable-autodetect --enable-zlib --disable-debug --disable-doc --disable-ffplay --disable-network --enable-static --disable-shared"
EXE=
CROSS=no
ZCONFIGURE_ENV=
case "$TARGET" in
  linux64|linuxarm64) ;;
  macos) CONFIGURE="$CONFIGURE --cc=clang" ;;
  win64)
    EXE=.exe
    CONFIGURE="$CONFIGURE --extra-ldflags=-static"
    case "$(uname -s)" in
      Linux)
        CROSS=yes
        ZCONFIGURE_ENV="CHOST=x86_64-w64-mingw32"
        CONFIGURE="$CONFIGURE --enable-cross-compile --target-os=mingw32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32-"
        ;;
    esac
    ;;
  *) echo "Unknown target '$TARGET' (linux64, linuxarm64, macos, win64)." >&2; exit 2 ;;
esac
if [ "${PIECEOFF_FFMPEG_NO_ASM:-0}" = 1 ]; then
  CONFIGURE="$CONFIGURE --disable-x86asm"
else
  case "$TARGET-$(uname -m)" in
    linux64-*|win64-*|macos-x86_64)
      command -v nasm >/dev/null 2>&1 || { echo "nasm is required for the x86 build (or set PIECEOFF_FFMPEG_NO_ASM=1)." >&2; exit 1; }
      ;;
  esac
fi

STAMP="$VERSION $SHA $ZVERSION $ZSHA $TARGET $(sha256_of "$0") $CONFIGURE"
if [ -f "$DEST/.stamp" ] && [ "$(cat "$DEST/.stamp")" = "$STAMP" ] && [ -f "$DEST/ffmpeg/ffmpeg$EXE" ]; then
  echo "FFmpeg $VERSION ($TARGET) is already staged in $DEST"
  exit 0
fi

# --- sources ---------------------------------------------------------------
mkdir -p "$CACHE"
# fetch <url> <sha256>: download into the cache, check the checksum, and
# fetch the detached signature beside it.
fetch() {
  archive=$CACHE/${1##*/}
  if [ ! -f "$archive" ] || [ "$(sha256_of "$archive")" != "$2" ]; then
    echo "Downloading ${1##*/}"
    curl --fail --location --silent --show-error --output "$archive.part" "$1"
    mv "$archive.part" "$archive"
  fi
  actual=$(sha256_of "$archive")
  if [ "$actual" != "$2" ]; then
    echo "SHA-256 mismatch for ${1##*/}" >&2
    echo "  expected $2" >&2
    echo "  actual   $actual" >&2
    rm -f "$archive"
    exit 1
  fi
  if [ ! -f "$archive.asc" ]; then
    curl --fail --location --silent --show-error --output "$archive.asc" "$1.asc"
  fi
}
fetch "$URL" "$SHA"
fetch "$ZURL" "$ZSHA"
ARCHIVE=$CACHE/$NAME
ZARCHIVE=$CACHE/$ZNAME

WORK=$(mktemp -d)
cleanup() { rm -r "$WORK"; }
trap cleanup EXIT
tar -xJf "$ARCHIVE" -C "$WORK"
tar -xzf "$ZARCHIVE" -C "$WORK"
SRC=$WORK/ffmpeg-$VERSION
ZSRC=$WORK/zlib-$ZVERSION
[ -d "$SRC" ] || { echo "$NAME did not unpack to ffmpeg-$VERSION/." >&2; exit 1; }
[ -d "$ZSRC" ] || { echo "$ZNAME did not unpack to zlib-$ZVERSION/." >&2; exit 1; }

# --- build -----------------------------------------------------------------
JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)

# zlib, static, into a private prefix that only this FFmpeg build sees.
ZPREFIX=$WORK/zlib-install
(
  cd "$ZSRC"
  # shellcheck disable=SC2086
  env $ZCONFIGURE_ENV ./configure --static --prefix="$ZPREFIX"
  make -j"$JOBS"
  make install
)

mkdir "$WORK/build"
(
  cd "$WORK/build"
  # shellcheck disable=SC2086
  "$SRC/configure" $CONFIGURE --extra-cflags="-I$ZPREFIX/include" --extra-ldflags="-L$ZPREFIX/lib"
  make -j"$JOBS" ffmpeg$EXE ffprobe$EXE
)

STRIP=strip
[ "$CROSS" = yes ] && STRIP=x86_64-w64-mingw32-strip
$STRIP "$WORK/build/ffmpeg$EXE" "$WORK/build/ffprobe$EXE"

# A binary that cannot decode what PieceOff feeds it is not worth shipping.
# Skipped when cross-compiling: the result cannot run here.
if [ "$CROSS" = no ]; then
  FF=$WORK/build/ffmpeg$EXE
  "$FF" -v error -f lavfi -i testsrc=duration=1:size=64x48:rate=10 \
    -f lavfi -t 1 -i anullsrc=r=44100:cl=mono -c:v mpeg4 -c:a aac -y "$WORK/selftest.mp4"
  "$WORK/build/ffprobe$EXE" -v error -show_entries stream=codec_name -of csv=p=0 "$WORK/selftest.mp4" | grep -q mpeg4 \
    || { echo "FFmpeg self-test failed (MP4)." >&2; exit 1; }
  # PNG needs zlib: thumbnails are PNG, and APNG is decoded with FFmpeg.
  "$FF" -v error -i "$WORK/selftest.mp4" -frames:v 1 -c:v png -f image2pipe - >"$WORK/selftest.png"
  [ "$(head -c 4 "$WORK/selftest.png" | tail -c 3)" = PNG ] || { echo "FFmpeg self-test failed (PNG)." >&2; exit 1; }
  "$FF" -v error -i "$WORK/selftest.png" -f null - || { echo "FFmpeg self-test failed (PNG decode)." >&2; exit 1; }
  # No GPL or nonfree component may have slipped in.
  if "$FF" -version | grep -qE -- '--enable-(gpl|nonfree|libx26)'; then
    echo "The build enabled a GPL or nonfree component." >&2
    exit 1
  fi
fi

# --- stage -----------------------------------------------------------------
rm -r "$DEST/ffmpeg" "$DEST/source" 2>/dev/null || true
mkdir -p "$DEST/ffmpeg" "$DEST/source"
cp "$WORK/build/ffmpeg$EXE" "$WORK/build/ffprobe$EXE" "$DEST/ffmpeg/"
chmod +x "$DEST/ffmpeg/ffmpeg$EXE" "$DEST/ffmpeg/ffprobe$EXE"
cp "$SRC/COPYING.LGPLv2.1" "$DEST/ffmpeg/LICENSE-FFmpeg.txt"
cp "$ZSRC/LICENSE" "$DEST/ffmpeg/LICENSE-zlib.txt"

cp "$ARCHIVE" "$ARCHIVE.asc" "$ZARCHIVE" "$ZARCHIVE.asc" "$DEST/source/"
cp "$0" "$DEST/source/build-ffmpeg.sh"
cp "$PIN" "$DEST/source/ffmpeg-source.txt"
{ echo "$SHA  $NAME"; echo "$ZSHA  $ZNAME"; } >"$DEST/source/SHA256SUMS.txt"
cat >"$DEST/source/README.txt" <<INFO
Corresponding source for the FFmpeg programs in ../ffmpeg/.

$NAME   the unmodified upstream FFmpeg $VERSION release
$ZNAME  the unmodified upstream zlib $ZVERSION release (linked statically)
*.asc                detached signatures of both (FFmpeg release signing key
                     FCF9 86EA 15E6 E293 A564 4F10 B432 2F04 D676 58D8; zlib:
                     Mark Adler, 5ED4 6A67 21D3 6558 7791 E2AA 783F CD8E 58BC AFBA)
SHA256SUMS.txt       their SHA-256 sums, which build-ffmpeg.sh checks first
build-ffmpeg.sh      the build script that produced the programs
ffmpeg-source.txt    the versions, URLs and checksums it builds from

No patches are applied. To rebuild the programs, put build-ffmpeg.sh in a
scripts/ folder and ffmpeg-source.txt in a packaging/ folder beside it (as in
the PieceOff repository), then run
    scripts/build-ffmpeg.sh <linux64|linuxarm64|macos|win64> <output folder>
The configure options are listed in ../ffmpeg/FFMPEG-BUILD.txt.
INFO

VERSION_LINE=$("$WORK/build/ffmpeg$EXE" -version 2>/dev/null | head -n 1 || true)
cat >"$DEST/ffmpeg/FFMPEG-BUILD.txt" <<INFO
This folder holds an unmodified FFmpeg $VERSION build for $TARGET, run by
PieceOff as a separate program (it is not linked into PieceOff).

License:   GNU LGPL version 2.1 or later (LICENSE-FFmpeg.txt). The build has
           no GPL or nonfree components. It statically links zlib $ZVERSION
           (zlib license, LICENSE-zlib.txt).
Source:    $URL
SHA-256:   $SHA
zlib:      $ZURL
SHA-256:   $ZSHA
Configure: $CONFIGURE --extra-cflags=-I<zlib>/include --extra-ldflags=-L<zlib>/lib
${VERSION_LINE:+Reported:  $VERSION_LINE
}
The exact corresponding source and the build script are in ../source/. The
programs are built by scripts/build-ffmpeg.sh in the PieceOff repository.
FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.
${EXE:+
This Windows build links the MinGW-w64 C runtime statically. Its runtime
libraries (libgcc, mingw-w64-crt) are under permissive licenses or the GCC
Runtime Library Exception.
}
You can replace these programs with any other FFmpeg build: put your own
ffmpeg$EXE and ffprobe$EXE in this folder, or choose a folder in PieceOff
under You > Media tools.
INFO

echo "$STAMP" >"$DEST/.stamp"
echo "Built FFmpeg $VERSION ($TARGET) into $DEST"
