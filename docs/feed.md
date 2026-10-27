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
| `workspace` | The herdr workspace it runs in, on the machine the feed ran on. Without it, Kerdr looks in `home` for one workspace whose label starts with the id's last two digits and a dash (`41-...`). Add `"tab": "w1:t2"` when the work runs in one tab of a workspace other work shares, such as a helper in a tab of the workspace that started it: `Enter` lands on that tab (on the workspace alone once the tab has closed), the card shows only that tab's agents, and the card the workspace belongs to leaves that tab's agents out. |
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
| `resume` | For work under way or done: the agent conversation that worked it, `{"agent": "codex", "conversation": "<its id>"}`. Kerdr names the agent when `Enter` would reopen the card; the action backend does the resuming, so this is only what it would resume. |

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
Requests are limited to 64 KiB, beyond the images a `create` carries. The `action` field names the
operation; the backend defines which additional fields that operation needs:

```json
{"action":"dispatch","id":"0041","session":"web","override":false}
```

The action names are `create`, `dispatch`, `reopen`, `refresh`, `dismiss`, `restore` and `move`.
`create` carries the user's own words, the session they were looking at, and an id for this
submission, nothing else:

```json
{"action":"create","words":"The laptop's Wi-Fi drops after sleep.\nMake it reconnect on its own.","session":"default","submission":"kerdr-18f2c3a9d0e1-4d2"}
```

Images pasted into the box come as `images`, one entry for each `[Image #N]` tag still in the
words: `tag` (N, a positive integer), `type` (`image/png`, `image/jpeg`, `image/gif` or
`image/webp`, from the image's own bytes) and `data` (its bytes in standard base64). Each is at most
10 MB, and one request carries at most 20 of them and 20 MB in all, so a request with images may be
that much larger than 64 KiB. Keep them with the record, readable by exactly whoever can read it,
and say there which file each tag is. A request without images has no `images` field. A backend
that ignores the field drops the images, so update it before its users get a Kerdr that sends them.

```json
{"action":"create","words":"Moving it failed: [Image #1]","session":"default","submission":"kerdr-18f2c3a9d0e1-4d3","images":[{"tag":1,"type":"image/png","data":"iVBORw0KGgoAAAANSUhEUg…"}]}
```

Kerdr sends the same `submission` again only for the same words and images to the same session,
and only while an earlier send of them never got an answer (a timeout, say). When a record already
exists for it, answer with that record's `id` instead of making another. Changed words or images
are new work with a new `submission`.

Write the record up from the words however your system does (a person, a script, an agent),
and answer with its `id` as soon as it's saved, before any slow write-up. While it's being
written up, give the card `intake: "pending"` in the feed. A response that names an `id` means
the record exists even when `ok` is false (saved but not published, say): Kerdr won't send the
same words again.
`refresh` keeps wanted work fresh while preserving its creation date; `dismiss` drops it from
the active backlog and `restore` brings it back. `move` changes backlog order. `reopen` is
described below.

Optional request fields are `id`, `title`, `words`, `tenant`, `organization`, `project`, `session`, `submission` and `confirm` (strings),
`override`, `top` and `queue` (booleans), `delta` (a signed integer), and `images` (an array, on
`create`). Omit fields that do not apply.
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
again with `queue: true` and that `confirm` token, for the backend to queue the work in that session instead, as starting
new work would be. Without `[actions]`, `Enter` says the card has no workspace and sends nothing.

### Reopening done work

`Enter` on a done card (a record in Maintain that isn't dismissed or still being written up)
with no workspace open sends the same `reopen` request, so work that wrapped itself before you
were finished with it is one key away. The backend must ask first, every time: answer the first
request with a question whose `message` says the work is done and that a yes opens it again, and
put every other reason in the same question. Only the yes, with its `confirm` token, opens the
record again and reopens it; a no sends nothing, so nothing changes. Require the token for a
queue answer (`w`) to done work too, and ask again when it's missing or from a question that
didn't say the work was done (one asked while it was still under way). Done work found open
already (its workspace, or its conversation in another pane) is asked about the same way. Kerdr rechecks before
sending the yes that the card is still the same work with no new workspace.

Kerdr refuses a dismissed card itself ("u brings it back"), and a card homed on a machine it
doesn't know. Refuse in the backend, with a plain `message` and no question, whatever only it can
tell: a record another account owns, one homed on another machine, or one with no record of
running there. The card's hint line and the peek say `⏎ ask to reopen it`, naming the agent
from `resume` when the feed gives it. Without `[actions]`, `Enter` says the card is done and sends
nothing.

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


## Accounts

All account fields are optional additions to format 1. Older feeds need no changes and older
Kerdr ignores these fields. Each feed reports only that machine’s account state:

```json
{
  "accounts": [{"code":"KBZ","name":"Personal","provider":"claude","kind":"login",
    "runs":"claude","status":"out","until":"2026-10-22T09:00:00-05:00","pool":"week",
    "usage":{"five":86,"five_at":null,"week":100,"week_at":"2026-10-22T09:00:00-05:00","seen":"2026-10-21T17:46:00-05:00"}}],
  "owners": [{"id":"studio","kind":"organization","name":"Studio","parent":null,
    "models":["KBZ"],"own":true,"sessions":["default"]},
    {"id":"shop","kind":"tenant","name":"Shop","parent":"studio",
    "models":["KBZ"],"own":false,"sessions":["web","shop-bg"]}],
  "owners_source":"mock",
  "machine_owner":"studio",
  "unowned_sessions":[],
  "agents":[{"pane":"w1:p1","session":"web","conversation":"conversation-id",
    "account":"KBZ","how":"list","problem":null,"waiting":{"why":"usage","until":"2026-10-22T09:00:00-05:00"}}]
}
```

An account may also list `copies`: the separate stored sign-ins of it on this machine, in the
order to show them, such as your agents' copy and the one root holds for tenants:

```json
"copies": [{"id":"agents","label":"Your agents","stored":true,"works":true,
             "checked":"2026-10-26T17:40:00-05:00","note":null},
           {"id":"tenants","label":"Tenants","stored":false,"works":null,"checked":null,
             "note":"held by root"}]
```

`works` is `true`, `false`, or `null` (not tested); `checked` is when it was last tested; a
missing `label` shows the `id`. With two or more copies the accounts screen shows one line per
copy under the account; with none or one it is unchanged. The account's own `status` stays the
copy that launches use. Copies change nothing on the board, the usage line or card badges.

Account status is `ready`, `out`, `sign_in`, or `missing`. Usage values are percentages;
missing or null usage displays as unknown. Usage belongs to the account rather than the machine,
so wherever Kerdr shows an account's figures, on any machine tab or on All, it uses the reading
with the newest `seen` among every machine's feed for that account code; a reading without `seen`
counts as oldest, and a tie goes to the first machine in the board's order on every tab. A figure whose `five_at` or `week_at` has passed displays as unknown (`—`)
until a newer reading arrives, and the accounts screen says which window reset and when. Providers, kinds, and runner names are strings
so other programs and local endpoints can be represented. Creating accounts currently uses
`provider: "claude"` and `kind: "token"`; the engine validates what it supports.
An owner’s `models` contains its effective ordered list; `own: false` denotes inheritance.
Sessions are explicitly assigned to owners, with `default` falling back to `machine_owner`.
Unowned sessions are marked on the accounts screen. `owners_source: "mock"` is shown as such.

Cards join their herdr agents to the feed’s agents by **machine, session, and pane**, including
child cards that name a tab. `how` is `list`, `manual`, `next`, or `handed_over`. A usage wait
suppresses the ordinary “turn ended” attention for that card. Conversation IDs bind a move
and its confirmation; backend checks must also reject stale generations and enforce authorization.
An optional `problem` string reports a failed move. It draws attention to the card and appears
in its account details and move picker; explicit questions and blockers keep their priority.
No credential or token belongs in the feed.

### Account actions

These use the existing JSON-on-stdin adapter and `ok`/`message` response. Account actions run
on the selected account’s machine; a move runs on the agent’s machine, not its record’s home.
Model lists and account metadata have one canonical writer owned by the backend.

| Action | Request fields | Result |
|---|---|---|
| `account-move` | `pane`, `session`, `conversation` (empty for an agent the feed doesn't list), `to` (code or `list`), optional `confirm` | queued, stale, or the question below |
| `account-models` | `owner`, `models` array; or `inherit: true` | saves the list |
| `account-move-waiting` | `owner` | moves waiting sessions on this machine |
| `account-add` | `code`, `name`, `provider`, `kind` | adds, or renames the existing code |
| `account-test` | `code`, optional `copy` | status and message |
| `account-remove` | `code` | removes after Kerdr’s local confirmation |

The `S` tick runs `account-move-waiting` on configured feed machines only after the list was
saved successfully. Unreachable machines are reported rather than silently claimed as moved.

```json
{"ok":false,"confirmation":"This agent is mid-step. Move when it next stops? Restarting ends its background job.",
 "yes":"move when it stops","no":"leave it","confirm":"opaque-ticket","message":"Move needs a decision"}
```

For account moves, `confirmation` is the question text (ordinary card actions keep their
boolean field). Kerdr shows it only after selection, waits for a fresh `y`, then resends the
original machine, pane, session, conversation and target with `confirm`. It does **not** add `override`,
`when_idle`, or `end_jobs`: the backend’s opaque token covers exactly what was asked.

Moves send the feed's `agents[].conversation`. A changed conversation answers
`{"ok":true,"state":"stale","message":"the conversation changed"}`. An accepted move answers
`{"ok":true,"state":"queued","account":"ARK","message":"moving to ARK"}` promptly, then restarts
in a detached worker. Kerdr displays the message; the feed changes the account badge when the
move finishes. An unavailable target answers `ok: false` before stopping anything.

### Signing in

```json
{"action":"account-signin","code":"ARK","step":"start"}
{"ok":true,"message":"Authorize in your browser","status":"waiting","ticket":"opaque-ticket",
 "url":"https://example.com/authorize","needs_code":true}
```

On a copy's line, `start` names it: `{"action":"account-signin","code":"ARK","step":"start","copy":"tenants"}`.
Kerdr sends the line's `id` for every copy, the first included; a request without `copy` means
the account's default copy, as before. The ticket `start` returns belongs to that copy, so poll,
finish and cancel need nothing new. Signing in one copy must never change another, and
cancelling leaves the copy as it was. `account-test` takes `copy` the same way.

Continue on that same machine:

```json
{"action":"account-signin","ticket":"opaque-ticket","step":"poll"}
{"action":"account-signin","ticket":"opaque-ticket","step":"finish","page_code":"page-code"}
{"action":"account-signin","ticket":"opaque-ticket","step":"cancel"}
```

Responses have `status: "waiting"`, `"done"`, or `"failed"`; cancel answers `"cancelled"` with
`ok: true`, including an already finished or unknown valid ticket. A new start cancels an
unfinished sign-in for that code and copy on that machine only; another copy's sign-in carries on. A failed finish may report an account identity mismatch,
which Kerdr shows without advancing to another machine. Kerdr polls every two seconds
while the dialog remains open, pauses while a page code is being entered, and stops on unknown results.
A retry cancels any known previous ticket before starting another. Escape cancels a known ticket;
a ticket returned after Escape is also cancelled, without opening a browser. After `done`,
adding an account proceeds to the next selected machine with a **new** start and ticket.
A `paste` step instead sends `code` and a `token` string (and `copy`, when pasting for one),
and answers `done` or a refusal; a backend refuses a paste for a copy it can't take one for.
The token never enters the result queue or saved settings. The backend must not echo it and
must own its secure storage. Kerdr also removes submitted credential text from adapter output.
Adapter output is captured in temporary files before parsing, so backends must never echo credentials.
Only HTTPS URLs can be opened; URL data is passed directly to the OS opener, never a shell.

The start action must return promptly. On Unix, Kerdr cleans up the adapter's process group
after each call, so a persistent sign-in helper must start a new session (`setsid` or
`start_new_session=True`) and own its stdin, stdout and stderr. Return only the ticket and URL;
poll, finish and cancel reach that detached helper. Requests, including credentials, use an
in-memory pipe rather than a temporary input file.

The synthetic POSIX and PowerShell adapters in `tests/fixtures/accounts.sh` and
`tests/fixtures/accounts.ps1` exercise every account action and sign-in step in CI.
