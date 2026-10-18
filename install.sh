#!/bin/sh
# Install PieceOff from the latest published GitHub release or build it locally.
set -eu

REPOSITORY=UnsupervisedMedia/PieceOffApp
INSTALL_ROOT=${PIECEOFF_INSTALL_DIR:-"${HOME:?HOME must be set}/.local/opt/PieceOff"}
BIN_DIR=${PIECEOFF_BIN_DIR:-"${HOME:?HOME must be set}/.local/bin"}
METHOD=
ASSUME_YES=no

usage() {
  cat <<'EOF'
Usage: ./install.sh [--release | --local] [--prefix DIR] [--bin-dir DIR] [-y|--yes]

Install the latest published GitHub release (default), or build from this checkout.

Options:
  --release       Download the latest published release for this OS and CPU
  --local         Build a release bundle from this checkout and install it
  --prefix DIR    Installation root (default: ~/.local/opt/PieceOff)
  --bin-dir DIR   Directory for the `pieceoff` command (default: ~/.local/bin)
  -y, --yes       Skip the method prompt; requires --release or --local
  -h, --help      Show this help

Linux and macOS are supported. Windows users should download the Windows zip
from https://github.com/UnsupervisedMedia/PieceOffApp/releases.
EOF
}

fail() { echo "install.sh: $*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --release) [ -z "$METHOD" ] || fail 'choose only one of --release and --local'; METHOD=release ;;
    --local) [ -z "$METHOD" ] || fail 'choose only one of --release and --local'; METHOD=local ;;
    --prefix) [ "$#" -ge 2 ] || fail '--prefix needs a directory'; INSTALL_ROOT=$2; shift ;;
    --bin-dir) [ "$#" -ge 2 ] || fail '--bin-dir needs a directory'; BIN_DIR=$2; shift ;;
    -y|--yes) ASSUME_YES=yes ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown option: $1 (use --help)" ;;
  esac
  shift
done

OS=$(uname -s)
ARCH=$(uname -m)
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) ASSET_SUFFIX=linux64 ;;
      aarch64|arm64) ASSET_SUFFIX=linuxarm64 ;;
      *) fail "unsupported Linux architecture: $ARCH" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) ASSET_SUFFIX=macos-x86_64 ;;
      arm64|aarch64) ASSET_SUFFIX=macos-arm64 ;;
      *) fail "unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*)
    fail 'install.sh runs in a POSIX shell; download the Windows zip from https://github.com/UnsupervisedMedia/PieceOffApp/releases and use pieceoff.ps1.'
    ;;
  *) fail "unsupported operating system: $OS" ;;
esac

if [ -z "$METHOD" ]; then
  if [ "$ASSUME_YES" = yes ]; then fail '--yes requires --release or --local'; fi
  printf 'Install PieceOff from the latest GitHub release or build this checkout? [release/local] '
  IFS= read -r answer || fail 'could not read the install choice'
  case "$answer" in
    ''|release|r|R) METHOD=release ;;
    local|l|L) METHOD=local ;;
    *) fail 'enter release or local' ;;
  esac
fi

case "$METHOD" in
  release)
    command -v curl >/dev/null 2>&1 || fail 'curl is required to download a release'
    command -v python3 >/dev/null 2>&1 || fail 'python3 is required to read GitHub release metadata and verify its checksum'
    command -v tar >/dev/null 2>&1 || fail 'tar is required to unpack the release'
    ;;
  local)
    command -v cargo >/dev/null 2>&1 || fail 'Cargo is required for a local build'
    ;;
esac

TMP=$(mktemp -d "${TMPDIR:-/tmp}/pieceoff-install.XXXXXX")
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT HUP INT TERM

if [ "$METHOD" = release ]; then
  API_URL="https://api.github.com/repos/$REPOSITORY/releases/latest"
  curl -fsSL -H 'Accept: application/vnd.github+json' "$API_URL" -o "$TMP/release.json" || fail 'could not fetch the latest published release'
  python3 - "$TMP/release.json" "$ASSET_SUFFIX" >"$TMP/asset-info" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    release = json.load(f)
suffix = sys.argv[2]
assets = [a for a in release.get("assets", []) if a["name"].startswith("PieceOff-") and a["name"].endswith("-" + suffix + ".tar.gz")]
if not assets:
    raise SystemExit("No published release bundle matches this OS and CPU.")
asset = assets[0]
print(asset["name"])
print(asset["browser_download_url"])
print(release["tag_name"])
PY
  ASSET_NAME=$(sed -n '1p' "$TMP/asset-info")
  ASSET_URL=$(sed -n '2p' "$TMP/asset-info")
  VERSION=$(sed -n '3p' "$TMP/asset-info")
  [ -n "$ASSET_NAME" ] && [ -n "$ASSET_URL" ] || fail 'could not select a release bundle'
  echo "Downloading $ASSET_NAME..."
  curl -fsSL "$ASSET_URL" -o "$TMP/$ASSET_NAME" || fail 'release bundle download failed'
  curl -fsSL "https://github.com/$REPOSITORY/releases/download/$VERSION/SHA256SUMS.txt" -o "$TMP/SHA256SUMS.txt" || fail 'could not download release checksums'
  python3 - "$TMP/SHA256SUMS.txt" "$TMP/$ASSET_NAME" "$ASSET_NAME" <<'PY'
import hashlib, sys
sums_path, asset_path, asset_name = sys.argv[1:]
expected = None
with open(sums_path, encoding="utf-8") as f:
    for line in f:
        fields = line.strip().split()
        if len(fields) == 2 and fields[1].lstrip("*") == asset_name:
            expected = fields[0].lower()
            break
if expected is None:
    raise SystemExit("The release checksum file does not include this bundle.")
h = hashlib.sha256()
with open(asset_path, "rb") as f:
    for chunk in iter(lambda: f.read(1024 * 1024), b""):
        h.update(chunk)
actual = h.hexdigest()
if actual != expected:
    raise SystemExit(f"SHA-256 mismatch for {asset_name}: expected {expected}, got {actual}")
PY
  mkdir "$TMP/unpacked"
  tar -xzf "$TMP/$ASSET_NAME" -C "$TMP/unpacked"
else
  ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  case "$ASSET_SUFFIX" in linux64|linuxarm64) TARGET=$ASSET_SUFFIX ;; macos-*) TARGET=macos ;; esac
  echo 'Building the local release bundle...'
  (cd "$ROOT" && scripts/bundle.sh --target "$TARGET" --skip-verify)
  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/pieceoff-app/Cargo.toml" | head -n 1)
  NAME="PieceOff-$VERSION-$TARGET"
  if [ "$TARGET" = macos ]; then NAME="$NAME-$ARCH"; fi
  BUNDLE="$ROOT/dist/$NAME.tar.gz"
  [ -f "$BUNDLE" ] || fail "local bundle was not created: $BUNDLE"
  mkdir "$TMP/unpacked"
  tar -xzf "$BUNDLE" -C "$TMP/unpacked"
fi

PACKAGE=$(find "$TMP/unpacked" -mindepth 1 -maxdepth 1 -type d -name 'PieceOff-*' -print | head -n 1)
[ -n "$PACKAGE" ] || fail 'archive does not contain a PieceOff install bundle'
VERSION=${VERSION#v}
[ -n "$VERSION" ] || VERSION=$(basename "$PACKAGE" | sed 's/^PieceOff-//')
DEST="$INSTALL_ROOT/versions/$VERSION"
mkdir -p "$INSTALL_ROOT/versions" "$BIN_DIR"
mkdir -p "$DEST"
cp -R "$PACKAGE/." "$DEST/"
ln -sfn "$DEST" "$INSTALL_ROOT/current.new"
mv -f "$INSTALL_ROOT/current.new" "$INSTALL_ROOT/current"
ln -sfn "$INSTALL_ROOT/current/pieceoff.sh" "$BIN_DIR/pieceoff"

echo "PieceOff $VERSION installed in $DEST"
echo "Run: $BIN_DIR/pieceoff start"
case ":${PATH:-}:" in *":$BIN_DIR:"*) : ;; *) echo "Add $BIN_DIR to PATH to run 'pieceoff' from any terminal." ;; esac
