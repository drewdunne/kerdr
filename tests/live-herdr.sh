#!/bin/sh
# Runs Kerdr against a real herdr server in a throwaway session: the check CI runs every week
# against herdr's latest release, and before every release.
#   tests/live-herdr.sh [path to kerdr]
set -eu
KERDR=${1:-target/debug/kerdr}
S="kerdr-live-$$"
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
assert card["stage"] == "test", card
assert card["attention"] == "needs you", card
assert card["why"] == "live check", card
PY
herdr --session "$S" workspace list | grep -q '"kerdr_jump":"next:1"' || fail "workspace tokens aren't listed"

# herdr accepts Kerdr's keys in its config.
"$KERDR" herdr-keys >"$TMP/keys.toml"
grep -q 'prefix+space' "$TMP/keys.toml" || fail "herdr-keys printed no keys"
echo "live herdr check passed"
