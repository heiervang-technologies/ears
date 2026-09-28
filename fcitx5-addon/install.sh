#!/usr/bin/env bash
# Build and install the earsghost fcitx5 addon for the current user, then
# restart fcitx5 so it is loaded. No root needed: the addon config lives in
# ~/.local/share/fcitx5/addon and points at the library by absolute path.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
libdir="$HOME/.local/lib/fcitx5"
confdir="${XDG_DATA_HOME:-$HOME/.local/share}/fcitx5/addon"

cmake -S "$here" -B "$here/build" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$here/build"

mkdir -p "$libdir" "$confdir"
install -m 0755 "$here/build/libearsghost.so" "$libdir/libearsghost.so"
sed "s|@LIBRARY@|$libdir/libearsghost|" "$here/earsghost.conf.in" > "$confdir/earsghost.conf"

if pgrep -x fcitx5 >/dev/null; then
    # Stop, then start: avoids racing the old instance during a replace.
    pkill -x fcitx5 || true
    for _ in $(seq 50); do pgrep -x fcitx5 >/dev/null || break; sleep 0.1; done
    # Keep the flags Omarchy starts it with.
    setsid fcitx5 -d --disable notificationitem >/dev/null 2>&1 < /dev/null &
    sleep 2
fi
sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/ears/ghost.sock"
if [ -S "$sock" ]; then
    echo "earsghost loaded: $sock"
else
    echo "earsghost installed; start fcitx5 to load it" >&2
fi
