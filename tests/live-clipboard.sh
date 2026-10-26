#!/bin/sh
# Puts a real image on this machine's clipboard and checks the new-intent box reads it back
# (images.rs's live_clipboard test): under X11 and under Wayland on Linux, and on macOS. CI runs
# it before every release; Windows has its own step in .github/workflows/ci.yml.
#   tests/live-clipboard.sh [image]
# On Linux it needs xvfb-run and xclip for X11, and sway and wl-clipboard for Wayland; a display
# whose tools are missing is skipped, unless CI is set.
set -eu
image=${1:-docs/screenshots/new-intent.png}
KERDR_LIVE_CLIPBOARD_IMAGE=$(cd "$(dirname "$image")" && pwd)/$(basename "$image")
export KERDR_LIVE_CLIPBOARD_IMAGE
TEST="cargo test --locked --lib -- --ignored --exact images::tests::live_clipboard"
TMP=$(mktemp -d)
PID=""
cleanup() {
    if [ -n "$PID" ]; then kill "$PID" 2>/dev/null || true; fi
    rm -rf "$TMP"
}
trap cleanup EXIT
fail() { echo "live clipboard check failed: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
missing() {
    [ -n "${CI:-}" ] && fail "$1 isn't installed"
    echo "skipped: $1 isn't installed"
}

case "$(uname -s)" in
Darwin)
    echo "macOS:"
    osascript -e "set the clipboard to (read (POSIX file \"$KERDR_LIVE_CLIPBOARD_IMAGE\") as «class PNGf»)"
    $TEST
    ;;
Linux)
    echo "X11:"
    if have xvfb-run && have xclip; then
        # xclip stays behind to hand the image over, until the display it serves closes.
        env -u WAYLAND_DISPLAY xvfb-run -a sh -c "
            xclip -selection clipboard -t image/png -i \"\$KERDR_LIVE_CLIPBOARD_IMAGE\"
            i=0
            until xclip -selection clipboard -o -t TARGETS 2>/dev/null | grep -qx image/png; do
                i=\$((i + 1)); [ \$i -gt 100 ] && { echo 'xclip never offered the image' >&2; exit 1; }
                sleep 0.1
            done
            $TEST"
    else
        missing "xvfb-run or xclip"
    fi
    echo "Wayland:"
    if have sway && have wl-copy && have wl-paste; then
        XDG_RUNTIME_DIR=$TMP
        export XDG_RUNTIME_DIR
        chmod 700 "$TMP"
        : >"$TMP/sway.conf"
        env -u DISPLAY -u WAYLAND_DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
            WLR_RENDERER=pixman sway -c "$TMP/sway.conf" >"$TMP/sway.log" 2>&1 &
        PID=$!
        i=0
        until socket=$(cd "$TMP" && ls wayland-[0-9] 2>/dev/null | head -n 1) && [ -n "$socket" ]; do
            i=$((i + 1))
            [ "$i" -gt 100 ] && { cat "$TMP/sway.log" >&2; fail "sway didn't start"; }
            sleep 0.1
        done
        WAYLAND_DISPLAY=$socket
        export WAYLAND_DISPLAY
        wl-copy --type image/png <"$KERDR_LIVE_CLIPBOARD_IMAGE"
        i=0
        until wl-paste --list-types 2>/dev/null | grep -qx image/png; do
            i=$((i + 1))
            [ "$i" -gt 100 ] && fail "wl-copy never offered the image"
            sleep 0.1
        done
        env -u DISPLAY sh -c "$TEST"
    else
        missing "sway or wl-clipboard"
    fi
    ;;
*)
    fail "no live clipboard check for $(uname -s)"
    ;;
esac
