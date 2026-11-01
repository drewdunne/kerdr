#!/bin/sh
# Synthetic account actions; never launches an agent or a browser.
request=$(cat)
case "$request" in
  *'"action":"account-move-waiting"'*) printf '%s\n' '{"ok":true,"message":"Waiting sessions moved","moved":2}' ;;
  *'"action":"account-move"'*)
    case "$request" in
      *'"conversation":"stale-conversation"'*) printf '%s\n' '{"ok":true,"state":"stale","message":"the conversation changed"}' ;;
      # An account another program runs: a hand-over, asked first.
      *'"to":"HAND_X"'*'"confirm":"fixture-handover"'*|*'"confirm":"fixture-handover"'*'"to":"HAND_X"'*) printf '%s\n' '{"ok":true,"state":"queued","account":"HAND_X","message":"handing over to HAND_X"}' ;;
      *'"to":"HAND_X"'*) printf '%s\n' '{"ok":false,"message":"move?","confirmation":"This agent runs one program and HAND_X another: a fresh agent picks up from the log. Hand it over?","yes":"hand it over","no":"leave it","confirm":"fixture-handover"}' ;;
      *'"confirm":"fixture-confirm"'*) printf '%s\n' '{"ok":true,"state":"queued","account":"ARK","message":"moving to ARK"}' ;;
      *) printf '%s\n' '{"ok":false,"message":"move?","confirmation":"Mid-step; move when idle and end background jobs?","yes":"move when it stops","no":"leave it","confirm":"fixture-confirm"}' ;;
    esac ;;
  *'"action":"account-signin"'*)
    # A copy other than the agents' one gets its own ticket, and refuses a pasted token.
    case "$request" in
      *'"step":"start"'*'"copy":"tenants"'*|*'"copy":"tenants"'*'"step":"start"'*) printf '%s\n' '{"ok":true,"message":"Authorize the tenants copy","status":"waiting","ticket":"fixture-tenants-ticket","url":"https://example.com/signin","needs_code":true}' ;;
      *'"step":"paste"'*'"copy":"tenants"'*|*'"copy":"tenants"'*'"step":"paste"'*) printf '%s\n' '{"ok":false,"message":"The tenants copy signs in on its machine, not by paste","status":"failed"}' ;;
      # A provider whose page takes a code: the code to type, and nothing to paste back.
      *'"step":"start"'*'"code":"DEVICE_X"'*|*'"code":"DEVICE_X"'*'"step":"start"'*) printf '%s\n' '{"ok":true,"message":"Type the code on the page","status":"waiting","ticket":"fixture-device-ticket","url":"https://example.com/device","needs_code":false,"user_code":"K7QD-3MPWX","expires":"2026-10-31T18:15:00Z"}' ;;
      *'"step":"start"'*) printf '%s\n' '{"ok":true,"message":"Authorize in your browser","status":"waiting","ticket":"fixture-ticket","url":"https://example.com/signin","needs_code":true}' ;;
      *'"step":"poll"'*) printf '%s\n' '{"ok":true,"message":"Waiting","status":"waiting"}' ;;
      *'"step":"cancel"'*) printf '%s\n' '{"ok":true,"message":"cancelled","status":"cancelled"}' ;;
      *) printf '%s\n' '{"ok":true,"message":"Done","status":"done"}' ;;
    esac ;;
  *'"action":"account-test"'*'"copy":"tenants"'*|*'"copy":"tenants"'*'"action":"account-test"'*) printf '%s\n' '{"ok":true,"message":"The tenants copy works","status":"ready"}' ;;
  *'"action":"account-models"'*|*'"action":"account-add"'*|*'"action":"account-test"'*|*'"action":"account-remove"'*) printf '%s\n' '{"ok":true,"message":"Account action completed","status":"ready"}' ;;
  *) printf '%s\n' '{"ok":false,"message":"Unknown fixture action"}'; exit 1 ;;
esac
