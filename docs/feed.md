# Card feeds

A card feed is any command that prints your records of work as JSON. Kerdr runs it on each
machine (set `[feed] command` in `~/.config/kerdr/config.toml`, or `feed` on one `[[machine]]`),
every 15 seconds by default, and merges its cards with the herdr workspaces it sees there.

Keep it fast (well under a second) and make it print valid JSON even when something is missing:
Kerdr shows the last good feed, with herdr's live view, when a run fails.

## Format, version 1

```json
{
  "kerdr_feed": 1,
  "source": "my-tracker",
  "machine": "build-box",
  "generated_at": "2026-10-21T17:46:00-05:00",
  "sessions": {
    "api":        {"used": 3, "limit": 3, "max": 5},
    "background": {"used": 2, "limit": null}
  },
  "cards": [
    {
      "id": "0041",
      "ref": "#41",
      "title": "Checkout handles expired cards",
      "stage": "test",
      "state": "active",
      "priority": "normal",
      "lane": "interactive",
      "home": {"machine": "laptop", "session": "web"},
      "workspace": {"session": "web", "id": "w1", "label": "41-checkout-expired-cards"},
      "attention": "needs_you",
      "status": "Waiting on you",
      "needs": "Your OK on the retry message before it ships",
      "message": "Two of three checks pass; the copy is the last one.",
      "blockers": [],
      "waits_on": [{"ref": "#38", "title": "Nightly backups restore cleanly", "open": true}],
      "criteria": [{"text": "An expired card shows its own message.", "met": true}],
      "timeline": [{"stage": "plan", "date": "2026-10-19", "note": "triaged"}],
      "log": [{"date": "2026-10-21", "text": "End-to-end tests pass."}],
      "updated": "2026-10-21T17:11:00-05:00",
      "stale_days": 0,
      "story": "As a shopper whose card expired, I want ...",
      "words": "What started it, in the words of whoever asked",
      "result": "For finished work: what it delivered, in one line",
      "closed": "2026-10-21",
      "path": "intents/0041-checkout-expired-cards",
      "parent": "0038"
    }
  ],
  "warnings": []
}
```

Only `kerdr_feed`, and each card's `id` and `title`, are required. Leave out what you don't
know rather than guessing.

## Fields

| Field | What Kerdr does with it |
|---|---|
| `source` | Names the feed. Cards are unique by `source` and `id`, so two machines running the same feed show each card once (the copy from its `home.machine` wins). |
| `sessions` | Slot use per herdr session, when your records count slots their own way. Without it, Kerdr counts workspaces with an agent against `[board] slots`. `limit: null` means no limit. |
| `id`, `ref` | The card's identity, and the short handle drawn first on it (default: `#` and the id without leading zeros). |
| `stage` | `backlog`, `plan`, `design`, `build`, `test`, `deploy` or `maintain`: the column. Kerdr also accepts common synonyms (`review` is test, `done` is maintain). |
| `state` | Free text: `queued` and `shelved` group the backlog; `closed` is finished work. |
| `priority` | `high` cards go first, with a ▲. |
| `lane` | `background` adds a `bg` tag. |
| `home` | The machine and session the work belongs to. |
| `workspace` | The herdr workspace it runs in, on the machine the feed ran on. Without it, Kerdr looks in `home` for one workspace whose label starts with the id's last two digits and a dash (`41-...`). |
| `attention` | Your records' view: `needs_you`, `waiting`, `running`, `blocked`, `idle` or `done`. Kerdr combines it with herdr's agent state: an agent asking something always needs you, and an agent that stopped while the record says it's running needs you too, since nothing says what it's waiting on. |
| `status`, `needs`, `message`, `blockers` | Shown on the card and in its peek: what it's doing, what it needs from you, its last update, what's in its way. |
| `waits_on` | Work it can't start before; shown in the backlog and the peek. |
| `criteria` | Done-when criteria. With any `met` set, the peek shows progress, such as 2 of 3 met. |
| `timeline` | Notes under each stage in the peek's lifecycle rail. |
| `log` | The newest few log entries, newest first. |
| `updated`, `stale_days` | How long ago it last moved. Queued work untouched for 14 days or more asks whether you still want it. |

## Agents reporting for themselves

Without a feed, agents can place their own card from inside their herdr pane:

```sh
kerdr report --stage test --status needs-you --note "your OK on the retry copy" --progress 2/3
kerdr report --clear
```

These are herdr workspace tokens (`kerdr_stage`, `kerdr_status`, `kerdr_note`,
`kerdr_progress`), so they need nothing but herdr. herdr keeps them until they change, the
workspace closes or its server restarts. A feed's card for the same workspace takes precedence,
except that a reported status still applies.
