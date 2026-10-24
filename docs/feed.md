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
      "tenant": "shop",
      "organization": "my-org",
      "project": "checkout",
      "created": "2026-10-19",
      "refreshed_at": "2026-10-21",
      "backlog_rank": 10,
      "disposition": "kept",
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
| `tenant`, `organization`, `project` | Optional scope labels for filtering and showing where work belongs. Leave an unknown scope absent. |
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
| `created` | The record's original creation date. Keeping old work must preserve this date. |
| `refreshed_at` | The last date someone confirmed that the work is still wanted. This is separate from creation and ordinary record updates. |
| `backlog_rank` | An optional signed integer ordering the backlog, lower numbers first. The action backend writes the canonical order. |
| `disposition` | An optional record of a backlog decision, such as `kept` or `dropped`. |

## Actions

Actions are off by default. Opt in separately from the read-only feed:

```toml
[actions]
command = "~/bin/my-tracker-action"
```

Kerdr runs this trusted shell command on the card's home machine, locally or over the same
SSH connection used to read that machine. The backend must update the canonical records there;
Kerdr does not edit a cached feed or invent a replacement record. The configured command must
be available on each machine where you use actions.

The command receives one JSON object on standard input. User text is JSON data, including quotes,
newlines and shell punctuation. It is never appended to the configured command as shell source.
Requests are limited to 64 KiB. The `action` field names the operation; the backend defines which
additional fields that operation needs:

```json
{"action":"dispatch","id":"0041","session":"web","override":false}
```

The action names are `create`, `dispatch`, `refresh`, `dismiss`, `restore` and `move`.
`refresh` keeps wanted work fresh while preserving its creation date; `dismiss` drops it from
the active backlog and `restore` brings it back. `move` changes backlog order.

Optional request fields are `id`, `title`, `words`, `tenant`, `organization`, `project` and `session` (strings),
`override` and `top` (booleans), and `delta` (a signed integer). Omit fields that do not apply.
The backend must validate the request, enforce its own permissions and slot limits, and print
exactly one JSON response to standard output. Send diagnostic output to standard error.

```json
{"ok":true,"message":"Work started","id":"0041","session":"web"}
```

`ok` (boolean) and `message` (string) are required. Optional fields are `confirmation` (boolean,
default false), `used` and `limit` (nonnegative integers or null), and `session` and `id` (strings
or null). Unknown response fields are allowed for future additions. A valid JSON response is
read even if the command exits nonzero, allowing the backend to explain a refused action or ask
for confirmation:

```json
{"ok":false,"message":"Session web is full","confirmation":true,"used":3,"limit":3,"session":"web"}
```

A confirmation is a separate user decision before sending a new request with `override: true`.
Actions have a 120-second deadline. A timeout or an invalid response does not prove the mutation
failed: its result is unknown. Kerdr does not retry it automatically; check the canonical record
before trying again. After success, the next feed read reflects the saved changes.

## Agents reporting for themselves

Agents can report status from inside their herdr pane. Without a feed record claiming the
workspace, the conversation stays in Misc rather than being assigned a work phase:

```sh
kerdr report --stage test --status needs-you --note "your OK on the retry copy" --progress 2/3
kerdr report --clear
```

These are herdr workspace tokens (`kerdr_stage`, `kerdr_status`, `kerdr_note`,
`kerdr_progress`), so they need nothing but herdr. herdr keeps them until they change, the
workspace closes or its server restarts. A feed's card for the same workspace takes precedence,
except that a reported status still applies.

`kerdr cards` identifies unlinked conversations with `"stage": "misc"`. Misc is a separate
view (`i`, or `Ctrl+Space m`), not a lifecycle column. A matching feed record moves the
workspace into the work board; no duplicate Misc card remains.

## Native Windows adapters

On native Windows, local feed and action commands run in inbox Windows PowerShell 5.1 with
UTF-8 stdin/stdout. Use PowerShell syntax, for example `& 'C:\Tools\cards.exe' --json`.
Action JSON arrives on standard input; native adapters must read it as UTF-8. A PowerShell
adapter can use `[Console]::In.ReadToEnd()`, and should write only its response JSON to stdout.
Feed commands start in the user's home directory; local Windows action commands do too.
Feed descendants are closed when the feed finishes or reaches its deadline. Action deadlines
stop the invocation shell; backend work it already started can continue under its own lifetime.
This preserves dispatched sessions and is why actions are never retried automatically when
their result is unknown.

SSH targets continue to execute adapters in their POSIX shell, even when Kerdr runs on Windows.
Use `[[machine]] feed` to override the default feed for a Linux host or disable it locally.
The action command is shared across machines: use a command available with the appropriate
shell syntax on every action target, or a cross-platform executable on PATH. The bundled
ark-sdlc Bash/Python adapter examples require their existing Unix backend; this port does not
make those Bash tools native Windows programs. The JSON contracts are unchanged.
