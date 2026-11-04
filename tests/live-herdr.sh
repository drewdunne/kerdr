#!/bin/sh
# Runs Kerdr against a real herdr server in a throwaway session: the check CI runs every week
# against herdr's latest release, and before every release.
#   tests/live-herdr.sh [path to kerdr]
set -eu
KERDR=${1:-target/debug/kerdr}
S="zz-kerdr-live-$$"
unset HERDR_SOCKET_PATH HERDR_SESSION HERDR_PANE_ID HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_ENV HERDR_BIN_PATH
TMP=$(mktemp -d)
PID=""
cleanup() {
    herdr session stop "$S" >/dev/null 2>&1 || true
    herdr session delete "$S" >/dev/null 2>&1 || true
    if [ -n "$PID" ]; then kill "$PID" 2>/dev/null || true; fi
    rm -rf "$TMP"
}
trap cleanup EXIT
fail() { echo "live herdr check failed: $*" >&2; exit 1; }

echo "herdr: $(herdr --version)"
herdr --session "$S" server >"$TMP/server.log" 2>&1 &
PID=$!
i=0
until herdr --session "$S" workspace list >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -gt 100 ] && { cat "$TMP/server.log" >&2; fail "the herdr server didn't start"; }
    sleep 0.2
done

W=$(herdr --session "$S" workspace create --label 01-alpha --no-focus |
    sed -n 's/.*"workspace":{[^}]*"workspace_id":"\([^"]*\)".*/\1/p')
[ -n "$W" ] || W=$(herdr --session "$S" workspace list | tr '{' '\n' | grep '"label":"01-alpha"' | sed -n 's/.*"workspace_id":"\([^"]*\)".*/\1/p')
[ -n "$W" ] || fail "couldn't create a workspace"

# What an agent reports with `kerdr report`, and what Kerdr's in-herdr keys leave behind.
herdr --session "$S" workspace report-metadata "$W" --source kerdr \
    --token kerdr_stage=test --token kerdr_status=needs_you --token "kerdr_note=live check" >/dev/null
herdr --session "$S" workspace report-metadata "$W" --source kerdr \
    --token kerdr_jump=next:1 --ttl-ms 15000 >/dev/null

printf '[machines]\npicker = false\n[board]\nonly = ["%s"]\n' "$S" >"$TMP/config.toml"
KERDR_CONFIG="$TMP/config.toml" "$KERDR" cards >"$TMP/cards.json" || fail "kerdr cards failed"
python3 - "$TMP/cards.json" <<'PY' || fail "the board didn't show the reported card"
import json, sys
board = json.load(open(sys.argv[1]))
assert board["machines"][0]["connection"] == "live", board["machines"]
card = next(c for c in board["cards"] if c["ref"] == "01-alpha")
assert card["stage"] == "misc", card
assert card["attention"] == "needs you", card
assert card["why"] == "live check", card
PY
herdr --session "$S" workspace list | grep -q '"kerdr_jump":"next:1"' || fail "workspace tokens aren't listed"

# What a held connection counts on (src/hold.rs), which a change in herdr would quietly turn back into
# the slow way in: the client socket's name beside the API socket's, and `herdr client` drawing on it.
A=$(herdr session list --json | tr '{' '\n' | grep "\"name\":\"$S\"" | sed -n 's/.*"socket_path":"\([^"]*\)".*/\1/p')
[ -n "$A" ] || fail "herdr's session list doesn't give the socket's path"
C="${A%.sock}-client.sock"
[ -S "$C" ] || fail "no client socket at $C"
HERDR_CLIENT_SOCKET_PATH="$C" HERDR_REMOTE_KEYBINDINGS=server python3 - <<'PY' || fail "herdr client didn't draw on the client socket"
import os, pty, select, signal, struct, fcntl, termios, time
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execvp("herdr", ["herdr", "client"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
seen, end = 0, time.time() + 8
while seen < 1500 and time.time() < end:
    if select.select([fd], [], [], 0.1)[0]:
        try:
            seen += len(os.read(fd, 65536))
        except OSError:
            break
os.kill(pid, signal.SIGTERM)
raise SystemExit(0 if seen >= 1500 else 1)
PY

# Enter on a card whose work runs in a tab focuses that tab after its workspace: herdr's answer
# names the tab's workspace, and a tab that has closed answers tab_not_found.
T=$(herdr --session "$S" tab create --workspace "$W" --label child --no-focus |
    sed -n 's/.*"tab":{[^}]*"tab_id":"\([^"]*\)".*/\1/p')
[ -n "$T" ] || fail "couldn't create a tab"
herdr --session "$S" workspace focus "$W" >/dev/null || fail "couldn't focus the workspace"
herdr --session "$S" tab focus "$T" | grep -q "\"workspace_id\":\"$W\"" ||
    fail "tab focus didn't name the tab's workspace"
herdr --session "$S" workspace list | grep -q "\"active_tab_id\":\"$T\"" || fail "tab focus didn't focus the tab"
herdr --session "$S" tab close "$T" >/dev/null || fail "couldn't close the tab"
if out=$(herdr --session "$S" tab focus "$T" 2>&1); then fail "a closed tab was focused"; fi
echo "$out" | grep -q tab_not_found || fail "a closed tab didn't answer tab_not_found: $out"

# herdr accepts Kerdr's keys in its config.
"$KERDR" herdr-keys >"$TMP/keys.toml"
grep -q 'prefix+space' "$TMP/keys.toml" || fail "herdr-keys printed no keys"
echo "live herdr check passed"
