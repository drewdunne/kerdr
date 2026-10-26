#!/bin/sh
# Synthetic account actions; never launches an agent or a browser.
request=$(cat)
case "$request" in
  *'"action":"account-move-waiting"'*) printf '%s\n' '{"ok":true,"message":"Waiting sessions moved","moved":2}' ;;
  *'"action":"account-move"'*)
    case "$request" in
      *'"conversation":"stale-conversation"'*) printf '%s\n' '{"ok":true,"state":"stale","message":"the conversation changed"}' ;;
      *'"confirm":"fixture-confirm"'*) printf '%s\n' '{"ok":true,"state":"queued","account":"ARK","message":"moving to ARK"}' ;;
      *) printf '%s\n' '{"ok":false,"message":"move?","confirmation":"Mid-step; move when idle and end background jobs?","yes":"move when it stops","no":"leave it","confirm":"fixture-confirm"}' ;;
    esac ;;
  *'"action":"account-signin"'*)
    case "$request" in
      *'"step":"start"'*) printf '%s\n' '{"ok":true,"message":"Authorize in your browser","status":"waiting","ticket":"fixture-ticket","url":"https://example.com/signin","needs_code":true}' ;;
      *'"step":"poll"'*) printf '%s\n' '{"ok":true,"message":"Waiting","status":"waiting"}' ;;
      *'"step":"cancel"'*) printf '%s\n' '{"ok":true,"message":"cancelled","status":"cancelled"}' ;;
      *) printf '%s\n' '{"ok":true,"message":"Done","status":"done"}' ;;
    esac ;;
  *'"action":"account-models"'*|*'"action":"account-add"'*|*'"action":"account-test"'*|*'"action":"account-remove"'*) printf '%s\n' '{"ok":true,"message":"Account action completed","status":"ready"}' ;;
  *) printf '%s\n' '{"ok":false,"message":"Unknown fixture action"}'; exit 1 ;;
esac
