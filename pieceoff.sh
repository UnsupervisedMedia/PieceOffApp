#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUN_DIR=${PIECEOFF_RUN_DIR:-"$ROOT/.pieceoff-runtime"}
DATA_DIR=${PIECEOFF_DATA_DIR:-"$ROOT/pieceoff-data"}
SERVER_PID_FILE="$RUN_DIR/server.pid"
APP_PID_FILE="$RUN_DIR/app.pid"

usage() {
    cat <<EOF
Usage: $0 {start|stop|restart|status}

Starts or stops the PieceOff room server and desktop app together.
Runtime logs and PID files are kept in $RUN_DIR.
EOF
}

resolve_binaries() {
    build_if_missing=${1:-no}
    BIN_DIR=${PIECEOFF_BIN_DIR:-}
    if [ -n "$BIN_DIR" ]; then
        SERVER_BIN="$BIN_DIR/pieceoff-server"
        APP_BIN="$BIN_DIR/pieceoff-app"
    elif [ -x "$ROOT/pieceoff-server" ] && [ -x "$ROOT/pieceoff-app" ]; then
        SERVER_BIN="$ROOT/pieceoff-server"
        APP_BIN="$ROOT/pieceoff-app"
    elif [ "$build_if_missing" = yes ] && command -v cargo >/dev/null 2>&1; then
        echo "Building PieceOff..."
        cargo build --workspace --manifest-path "$ROOT/Cargo.toml"
        SERVER_BIN="$ROOT/target/debug/pieceoff-server"
        APP_BIN="$ROOT/target/debug/pieceoff-app"
    elif [ -x "$ROOT/target/debug/pieceoff-server" ] && [ -x "$ROOT/target/debug/pieceoff-app" ]; then
        SERVER_BIN="$ROOT/target/debug/pieceoff-server"
        APP_BIN="$ROOT/target/debug/pieceoff-app"
    elif [ "$build_if_missing" = yes ]; then
        echo "PieceOff binaries were not found and Cargo is unavailable." >&2
        echo "Install Rust/Cargo or set PIECEOFF_BIN_DIR to a directory containing both binaries." >&2
        return 1
    else
        SERVER_BIN="$ROOT/target/debug/pieceoff-server"
        APP_BIN="$ROOT/target/debug/pieceoff-app"
    fi

    if [ "$build_if_missing" = yes ] && { [ ! -x "$SERVER_BIN" ] || [ ! -x "$APP_BIN" ]; }; then
        echo "Expected executable binaries at:" >&2
        echo "  $SERVER_BIN" >&2
        echo "  $APP_BIN" >&2
        return 1
    fi
}

process_matches() {
    pid_file=$1
    expected_bin=$2
    [ -f "$pid_file" ] || return 1
    pid=$(cat "$pid_file" 2>/dev/null || true)
    case "$pid" in ''|*[!0-9]*) return 1 ;; esac
    kill -0 "$pid" 2>/dev/null || return 1
    command_line=$(ps -p "$pid" -o command= 2>/dev/null || true)
    case "$command_line" in *"$expected_bin"*) return 0 ;; esac
    return 1
}

stop_one() {
    label=$1
    pid_file=$2
    expected_bin=$3
    if ! process_matches "$pid_file" "$expected_bin"; then
        rm -f "$pid_file"
        echo "$label is not running."
        return 0
    fi
    pid=$(cat "$pid_file")
    echo "Stopping $label (PID $pid)..."
    kill -TERM "$pid" 2>/dev/null || true
    tries=0
    while kill -0 "$pid" 2>/dev/null && [ "$tries" -lt 50 ]; do
        sleep 0.1
        tries=$((tries + 1))
    done
    if process_matches "$pid_file" "$expected_bin"; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$pid_file"
}

start_one() {
    label=$1
    binary=$2
    pid_file=$3
    log_file=$4
    if process_matches "$pid_file" "$binary"; then
        echo "$label is already running (PID $(cat "$pid_file"))."
        return 0
    fi
    rm -f "$pid_file"
    nohup "$binary" >>"$log_file" 2>&1 </dev/null &
    pid=$!
    printf '%s\n' "$pid" >"$pid_file"
    sleep 1
    if ! process_matches "$pid_file" "$binary"; then
        rm -f "$pid_file"
        echo "$label did not stay running. Recent log output:" >&2
        tail -n 30 "$log_file" >&2 || true
        return 1
    fi
    echo "Started $label (PID $pid). Log: $log_file"
}

start_all() {
    resolve_binaries yes
    umask 077
    mkdir -p "$RUN_DIR" "$DATA_DIR"
    PIECEOFF_BIND=${PIECEOFF_BIND:-0.0.0.0:28878}
    PIECEOFF_SERVER_URL=${PIECEOFF_SERVER_URL:-"ws://127.0.0.1:${PIECEOFF_BIND##*:}/ws"}
    export PIECEOFF_BIND PIECEOFF_SERVER_URL
    export PIECEOFF_DATA_DIR="$DATA_DIR"
    echo "PieceOff data: $DATA_DIR"
    start_one "room server" "$SERVER_BIN" "$SERVER_PID_FILE" "$RUN_DIR/server.log"
    start_one "desktop app" "$APP_BIN" "$APP_PID_FILE" "$RUN_DIR/app.log"
    echo "Open the app and create a room. Run '$0 stop' to close PieceOff."
}

case "${1:-}" in
    start)
        start_all
        ;;
    stop)
        resolve_binaries no
        stop_one "desktop app" "$APP_PID_FILE" "$APP_BIN"
        stop_one "room server" "$SERVER_PID_FILE" "$SERVER_BIN"
        ;;
    restart)
        resolve_binaries no
        stop_one "desktop app" "$APP_PID_FILE" "$APP_BIN"
        stop_one "room server" "$SERVER_PID_FILE" "$SERVER_BIN"
        start_all
        ;;
    status)
        resolve_binaries no
        if process_matches "$SERVER_PID_FILE" "$SERVER_BIN"; then
            echo "Room server is running (PID $(cat "$SERVER_PID_FILE"))."
        else
            echo "Room server is stopped."
        fi
        if process_matches "$APP_PID_FILE" "$APP_BIN"; then
            echo "Desktop app is running (PID $(cat "$APP_PID_FILE"))."
        else
            echo "Desktop app is stopped."
        fi
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
