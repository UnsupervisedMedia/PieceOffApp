#!/bin/sh
# Run the app on a virtual X display (:99) for screenshots. Silent by default:
# PIECEOFF_NO_AUDIO is forced on so test runs never make noise.
#   scripts/visual-dev.sh start [media]   start Xvfb + server + app
#   scripts/visual-dev.sh shot out.png    screenshot the virtual display
#   scripts/visual-dev.sh stop            stop everything it started
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN=${PIECEOFF_VISUAL_DIR:-${TMPDIR:-/tmp}/pieceoff-visual}
case "${1:-}" in
start)
  mkdir -p "$RUN"
  [ -S /tmp/.X11-unix/X99 ] || (setsid Xvfb :99 -screen 0 1600x1000x24 >/dev/null 2>&1 &)
  sleep 1
  (setsid env PIECEOFF_BIND=127.0.0.1:28878 PIECEOFF_DATA_DIR="$RUN/data" \
    "$ROOT/target/debug/pieceoff-server" >"$RUN/server.log" 2>&1 &)
  (setsid env -u WAYLAND_DISPLAY DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 PIECEOFF_NO_AUDIO=1 \
    PIECEOFF_CLIENT_DIR="$RUN/client" "$ROOT/target/debug/pieceoff-app" ${2:+"$2"} \
    >"$RUN/app.log" 2>&1 &)
  ;;
shot) DISPLAY=:99 import -window root "$2" ;;
stop) pkill -x pieceoff-app || true; pkill -x pieceoff-server || true; pkill -x Xvfb || true ;;
*) echo "usage: $0 start [media] | shot file.png | stop" >&2; exit 2 ;;
esac
