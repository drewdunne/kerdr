#!/bin/sh
# Synthetic account actions; never launches an agent or a browser.
request=$(cat)
case "$request" in
  *'"action":"account-move-waiting"'*) printf '%s\n' '{"ok":true,"message":"Waiting sessions moved","moved":2}' ;;
  *'"action":"account-move"'*) printf '%s\n' '{"ok":false,"message":"move?","confirmation":"Mid-step; move when idle and end background jobs?","yes":"move when it stops","no":"leave it","confirm":"fixture-confirm"}' ;;
  *'"action":"account-signin"'*)
    case "$request" in
      *'"step":"start"'*) printf '%s\n' '{"ok":true,"message":"Authorize in your browser","status":"waiting","ticket":"fixture-ticket","url":"https://example.com/signin","needs_code":true}' ;;
      *'"step":"poll"'*) printf '%s\n' '{"ok":true,"message":"Waiting","status":"waiting"}' ;;
      *) printf '%s\n' '{"ok":true,"message":"Done","status":"done"}' ;;
    esac ;;
  *'"action":"account-models"'*|*'"action":"account-add"'*|*'"action":"account-test"'*|*'"action":"account-remove"'*) printf '%s\n' '{"ok":true,"message":"Account action completed","status":"ready"}' ;;
  *) printf '%s\n' '{"ok":false,"message":"Unknown fixture action"}'; exit 1 ;;
esac
