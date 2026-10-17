#!/bin/sh
# Build PieceOff if anything changed, then run it for a quick test.
#
#   ./play.sh [options] [media-or-package]
#
#   --two        open a second client too, for trying multiplayer alone
#   --release    build and run optimised binaries
#   --silent     run without audio
#   --no-build   skip the build and run what is already there
#   install      link this script into ~/.local/bin as `pieceoff-dev`
#
# A local room server is started on PIECEOFF_BIND (default 127.0.0.1:28878)
# unless one already answers there, and is stopped again when the app closes.
set -eu

SELF=$(readlink -f -- "$0" 2>/dev/null || printf '%s' "$0")
ROOT=$(CDPATH= cd -- "$(dirname -- "$SELF")" && pwd)

PROFILE=debug
BUILD=yes
TWO=no
MEDIA=
for arg in "$@"; do
    case "$arg" in
        install)
            mkdir -p "$HOME/.local/bin"
            ln -sf "$ROOT/play.sh" "$HOME/.local/bin/pieceoff-dev"
            echo "Linked $HOME/.local/bin/pieceoff-dev -> $ROOT/play.sh"
            case ":$PATH:" in
                *":$HOME/.local/bin:"*) ;;
                *) echo "Add $HOME/.local/bin to PATH to run it as 'pieceoff-dev'." ;;
            esac
            exit 0
            ;;
        --two) TWO=yes ;;
        --release) PROFILE=release ;;
        --silent) export PIECEOFF_NO_AUDIO=1 ;;
        --no-build) BUILD=no ;;
        -h|--help)
            sed -n '2,13p' "$SELF" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "Unknown option: $arg (try --help)" >&2
            exit 2
            ;;
        *) MEDIA=$arg ;;
    esac
done

if [ "$BUILD" = yes ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "Cargo was not found. Install Rust from https://rustup.rs, or pass --no-build." >&2
        exit 1
    fi
    # Cargo only rebuilds what changed, so this is quick when up to date.
    if [ "$PROFILE" = release ]; then
        cargo build --workspace --release --manifest-path "$ROOT/Cargo.toml"
    else
        cargo build --workspace --manifest-path "$ROOT/Cargo.toml"
    fi
fi
SERVER_BIN="$ROOT/target/$PROFILE/pieceoff-server"
APP_BIN="$ROOT/target/$PROFILE/pieceoff-app"
for binary in "$SERVER_BIN" "$APP_BIN"; do
    if [ ! -x "$binary" ]; then
        echo "Missing $binary. Run without --no-build first." >&2
        exit 1
    fi
done

PIECEOFF_BIND=${PIECEOFF_BIND:-127.0.0.1:28878}
PORT=${PIECEOFF_BIND##*:}
export PIECEOFF_BIND
export PIECEOFF_SERVER_URL=${PIECEOFF_SERVER_URL:-"ws://127.0.0.1:$PORT/ws"}
export PIECEOFF_DATA_DIR=${PIECEOFF_DATA_DIR:-"$ROOT/pieceoff-data"}
RUN_DIR=${PIECEOFF_RUN_DIR:-"$ROOT/.pieceoff-runtime"}
mkdir -p "$RUN_DIR" "$PIECEOFF_DATA_DIR"

server_answers() {
    curl -fsS --max-time 1 "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q PieceOff
}

SERVER_PID=
SECOND_PID=
cleanup() {
    [ -z "$SECOND_PID" ] || kill "$SECOND_PID" 2>/dev/null || true
    [ -z "$SERVER_PID" ] || kill "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

if server_answers; then
    echo "Using the room server already running on port $PORT."
else
    "$SERVER_BIN" >"$RUN_DIR/play-server.log" 2>&1 &
    SERVER_PID=$!
    tries=0
    until server_answers; do
        tries=$((tries + 1))
        if ! kill -0 "$SERVER_PID" 2>/dev/null || [ "$tries" -gt 50 ]; then
            echo "The room server did not start. Its log:" >&2
            tail -n 20 "$RUN_DIR/play-server.log" >&2 || true
            exit 1
        fi
        sleep 0.1
    done
    echo "Room server on port $PORT (log: $RUN_DIR/play-server.log)."
fi

if [ "$TWO" = yes ]; then
    # The second client keeps its own name and history so it acts as a guest.
    PIECEOFF_CLIENT_DIR="$RUN_DIR/second-client" PIECEOFF_NO_AUDIO=1 \
        "$APP_BIN" >"$RUN_DIR/play-second.log" 2>&1 &
    SECOND_PID=$!
    echo "Second client started (silent); join it with the first client's invite code."
fi

if [ -n "$MEDIA" ]; then
    "$APP_BIN" "$MEDIA"
else
    "$APP_BIN"
fi
