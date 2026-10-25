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
      "parent": "0038",
      "resume": {"agent": "claude", "conversation": "4f2b8c1e-…"}
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
| `intake` | `pending` while a record made from someone's words is being written up, `failed` when it couldn't be. Absent once it's written up. Kerdr marks it ✎ in the backlog. |
| `resume` | For work under way: the agent conversation that worked it, `{"agent": "codex", "conversation": "<its id>"}`. Kerdr names the agent when `Enter` would reopen the card; the action backend does the resuming, so this is only what it would resume. |

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

The action names are `create`, `dispatch`, `reopen`, `refresh`, `dismiss`, `restore` and `move`.
`create` carries the user's own words, the session they were looking at, and an id for this
submission, nothing else:

```json
{"action":"create","words":"The laptop's Wi-Fi drops after sleep.\nMake it reconnect on its own.","session":"default","submission":"kerdr-18f2c3a9d0e1-4d2"}
```

Kerdr sends the same `submission` again only for the same words to the same session, and only
while an earlier send of them never got an answer (a timeout, say). When a record already exists
for it, answer with that record's `id` instead of making another. Changed words are new work
with a new `submission`.

Write the record up from the words however your system does (a person, a script, an agent),
and answer with its `id` as soon as it's saved, before any slow write-up. While it's being
written up, give the card `intake: "pending"` in the feed. A response that names an `id` means
the record exists even when `ok` is false (saved but not published, say): Kerdr won't send the
same words again.
`refresh` keeps wanted work fresh while preserving its creation date; `dismiss` drops it from
the active backlog and `restore` brings it back. `move` changes backlog order. `reopen` is
described below.

Optional request fields are `id`, `title`, `words`, `tenant`, `organization`, `project`, `session`, `submission` and `confirm` (strings),
`override`, `top` and `queue` (booleans), and `delta` (a signed integer). Omit fields that do not apply.
The backend must validate the request, enforce its own permissions and slot limits, and print
exactly one JSON response to standard output. Send diagnostic output to standard error.

```json
{"ok":true,"message":"Work started","id":"0041","session":"web"}
```

`ok` (boolean) and `message` (string) are required. Optional fields are `confirmation` (boolean,
default false), `used` and `limit` (nonnegative integers or null), and `session`, `id`,
`workspace`, `label` and `confirm` (strings or null). Unknown response fields are allowed for future additions. A valid JSON response is
read even if the command exits nonzero, allowing the backend to explain a refused action or ask
for confirmation:

```json
{"ok":false,"message":"Session web is full","confirmation":true,"used":3,"limit":3,"session":"web"}
```

A confirmation is a separate user decision before sending a new request with `override: true`.
Kerdr asks it for `dispatch` and `reopen`.
Actions have a 120-second deadline. A timeout or an invalid response does not prove the mutation
failed: its result is unknown. Kerdr does not retry it automatically; check the canonical record
before trying again. After success, the next feed read reflects the saved changes.

## Reopening work whose workspace closed

`Enter` on a card for work under way (a record in Plan, Design, Build, Test or Deploy that isn't
done, dismissed or still being written up) whose herdr workspace is gone sends `reopen` to the
card's home machine instead of stopping at "nothing to jump into":

```json
{"action":"reopen","id":"0041","session":"web"}
```

`session` is the record's `home.session`, or the session tab you're on; leave it out when
neither is known and choose one in the backend. The backend decides everything else: where the
workspace goes, how it's named, which agent conversation to resume (or a fresh agent when there
is none), and whether to ask first. When it opened a workspace, or found one already open (the
conversation running in another pane, say), it answers with it, and Kerdr goes straight in:

```json
{"ok":true,"message":"Reopened #41 in web, resuming its Claude conversation.","id":"0041","session":"web","workspace":"w12","label":"41-checkout-expired-cards"}
```

`workspace` is a herdr workspace id in `session` on the machine the action ran on. Kerdr goes in
only while you're still on that card; if you've moved on, or pressed `Esc`, it says the work is
open again instead.

To ask first (the session is at its usual limit, the conversation may still be open somewhere
else), answer with `"confirmation": true`, a `message` that names every reason, and optionally
`confirm`, an opaque token of what you asked about. Kerdr shows the question only while you're
still on the card, as it was when you pressed `Enter`, and on a yes sends the request again with
`override: true`, the `session` the answer named and that `confirm` token, so the backend can go
ahead with exactly what was agreed. When something has changed since, answer the yes with a new
question and a new token: Kerdr asks again. Before sending a yes, Kerdr also rechecks that the
card has the same home and conversation and no new workspace.

When the question says the session is full (it carries `used`), it also offers `w`: the request
again with `queue: true`, for the backend to queue the work in that session instead, as starting
new work would be. Without `[actions]`, `Enter` says the card has no workspace and sends nothing.

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

Persistent action backends must give their workers their own log files or a discard stream.
The adapter's stdout carries its single JSON response. Kerdr bounds response capture to
32 MiB per stream (action JSON remains limited to 64 KiB); it preserves inherited handles
while the backend takes ownership, and persistent log storage belongs to the backend.
