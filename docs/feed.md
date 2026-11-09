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
      "team": "dev",
      "stage_list": "standard",
      "created": "2026-10-19",
      "refreshed_at": "2026-10-21",
      "backlog_rank": 10,
      "disposition": "kept",
      "stage": "test",
      "state": "active",
      "priority": "normal",
      "lane": "interactive",
      "home": {"machine": "laptop", "session": "web"},
      "workspace": {"session": "web", "id": "w1", "label": "41-checkout-expired-cards", "own_tab": "w1:t1"},
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
      "from": "0036",
      "undertaker_for": "0035",
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
| `tenant`, `organization`, `project` | Optional scope labels for filtering and showing where work belongs. Leave an unknown scope absent. When the feed lists `owners` (see [Owners, teams and stage lists](#owners-teams-and-stage-lists)), a card's owner is its `tenant`, else its `organization`, matched by id against that list: use the list's ids. |
| `team` | Optional: the id of one of its owner's teams. A team its owner doesn't list shows under No team. |
| `stage` | The column: `backlog` for work not started and `maintain` for finished work, on every list; otherwise a stage of the card's `stage_list`. On the standard list (no `stage_list`) that's `plan`, `design`, `build`, `test` or `deploy`, and Kerdr also accepts common synonyms (`review` is test, `done` is maintain); on any other list, only its stage ids. A stage its list doesn't have shows in a column of its own, marked unknown, never in another stage. With no `stage`, open work goes in Build on the standard list and in the first stage of any other. |
| `stage_list` | Optional: the whole id of the stage list its `stage` belongs to (`tinyhost/marketing`); absent means `standard`. |
| `state` | Free text: `queued` and `shelved` group the backlog; `closed` is finished work. |
| `priority` | `high` cards go first, with a ▲. |
| `lane` | `background` adds a `bg` tag. |
| `home` | The machine and session the work belongs to. A record with no home can still name the machine alone, with no `session`, where its work is (the one holding its agent's conversation, say): that copy wins over copies that name none, and `Enter` sends its reopen there. When copies name different machines, one that gives `resume` wins over one that doesn't. |
| `workspace` | The herdr workspace it runs in, on the machine the feed ran on. Without it, Kerdr looks in `home` for one workspace whose label starts with the id's last two digits and a dash (`41-...`). Add `"tab": "w1:t2"` when the work runs in one tab of a workspace other work shares, such as a helper in a tab of the workspace that started it: `Enter` lands on that tab (on the workspace alone once the tab has closed), the card shows only that tab's agents, and the card the workspace belongs to leaves that tab's agents out. |
| `attention` | Your records' view: `needs_you`, `waiting`, `running`, `blocked`, `idle` or `done`. Kerdr combines it with herdr's agent state: an agent asking something always needs you, and an agent that stopped while the record says it's running needs you too, since nothing says what it's waiting on. A card's own agent back at work after the feed was made no longer needs you, even while the record still says `needs_you`: Kerdr saw it stopped in a read after the feed was made, and now it's working, so you've answered it, and the card turns at the next herdr read rather than the next feed. One working all along (a workflow or a background job still running in its pane after its turn ended on a question) still needs you until the record says otherwise; coming back from herdr, Kerdr reads that machine's feed at once, so an answer there shows within a read. Kerdr times the feed by `generated_at`, else by when it read it. An agent in a pane with herdr's `role=helper` token, a reviewer's, isn't the card's own unless the card names that tab. |
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
| `item_text`, `item_was` | Optional, together. A backlog item's whole text as an `edit` read answers it, and the `was` that read would give, so `e` opens your editor at once with no read. Kerdr then sends no read at all, and the save's `was` is what checks it. Leave them out for a record whose text you wouldn't hand over, and for a copy from a machine that isn't the record's home, since a save goes there. Without them `e` reads first, as below. |
| `submission` | The `submission` a `create` request carried, for a record it made. Kerdr's card for new words, shown while they're saved, gives way to the card carrying it as soon as any machine's feed shows one, even before the request answers. Optional. |
| `resume` | For work under way or done: the agent conversation that worked it, `{"agent": "codex", "conversation": "<its id>"}`. Kerdr names the agent when `Enter` would reopen the card; the action backend does the resuming, so this is only what it would resume. |
| `parent`, `undertaker_for`, `from` | The card this one sits inside (see [Work inside a card](#work-inside-a-card)): its `parent`, for a child; else the card whose agents it sees through, for an undertaker; else the card whose work it came out of, for a follow-up that isn't a child. Each is an `id` in the same feed. Give at most what's true; Kerdr uses the first of the three a card names. |
| `workspace.own_tab` | The tab the card's own agent runs in, from your record of the pane that agent was started or reopened in, never guessed. `Enter` on the card lands there, never on a tab work inside the card runs in. `tab` keeps its meaning (work confined to one tab, which `X` closes alone); `own_tab` is only for `Enter`. |
| `no_agent` | `true` for open work in a working column that no agent is at: no workspace on it, and nothing running the conversation that worked it. Kerdr says "no agent" on the card (unless a workspace or agent turns up for it), and its bottom line offers `C` and `x`. Leave it out when you can't tell. With no `state` either, `d` on it sends `reopen` as `Enter` does, so the backend starts it when it never ran and resumes it when it did. |
| `outside_herdr` | For a card whose conversation runs in a process outside herdr (a plain terminal window, an SSH login): `{"terminal": "foot", "since": "<ISO time>"}`, the terminal's process name and when the process started. Decide it from the process itself (no herdr pane in its environment), never from herdr failing to answer. Kerdr shows "outside herdr · foot" on the card, and `Enter` on it says where it runs and sends nothing; a second `Enter` within five seconds goes on to the reopen, whose backend asks before starting a second copy. |

## Owners, teams and stage lists

All optional additions to format 1, which an older Kerdr ignores. When any machine's feed lists
`owners` (see [Accounts](#accounts) for the rest of an owner's fields), Kerdr's top bar splits the
board by owner and then by team instead of by machine and herdr session; with none, it keeps the
machine split.

```json
{
  "owners": [
    {"id": "studio", "kind": "organization", "name": "Example Studio", "code": "ARK", "parent": null,
     "sessions": ["default"], "agent_machine": "build-box"},
    {"id": "shop", "kind": "tenant", "name": "Shop", "code": "SHP", "parent": "studio",
     "sessions": ["web", "shop-bg"], "agent_machine": "laptop",
     "teams": [{"id": "dev", "name": "Dev", "retired": false, "stages": "standard"},
               {"id": "marketing", "name": "Marketing", "retired": false, "stages": "shop/marketing"}]}
  ],
  "stage_lists": [
    {"id": "standard", "name": "Standard", "owner": null, "rev": 1,
     "stages": [{"id": "plan", "name": "Plan", "retired": false}, {"id": "design", "name": "Design", "retired": false},
                {"id": "build", "name": "Build", "retired": false}, {"id": "test", "name": "Test", "retired": false},
                {"id": "deploy", "name": "Deploy", "retired": false}]},
    {"id": "shop/marketing", "name": "Marketing", "owner": "shop", "rev": 3,
     "stages": [{"id": "plan", "name": "Plan", "retired": false}, {"id": "test", "name": "Test", "retired": false},
                {"id": "execute", "name": "Execute", "retired": false}, {"id": "learn", "name": "Learn", "retired": false}]}
  ],
  "create_fields": ["organization", "tenant", "project", "team"]
}
```

| Field | What Kerdr does with it |
|---|---|
| `owners[].teams` | The owner's teams, its tabs on the top bar after `All`: `id` (never changes, so renaming a team keeps its cards), `name`, `retired` (kept for its old cards and shown only while it holds one) and `stages`, the whole id of the stage list it uses (absent: `standard`). |
| `owners[].agent_machine` | The machine the owner's new work runs on, by the name Kerdr knows it by: `A` opens there when the owner is chosen, and the bar shows how full the owner's main session (its `code`) is there. |
| `owners[].sessions` | Its herdr sessions: a workspace no card claims is the owner's whose sessions name its session, else whose `code` its session's name starts with (`SHP`, `SHP-Background`). |
| `stage_lists` | Every list a team uses or an open card names, `standard` among them. A list's `id` is whole, owner then name (`shop/marketing`), so two owners' lists of one name never mix. `rev` goes up whenever its stages change: of two machines' copies of a list, Kerdr uses the one with the higher `rev`. A stage taken out of a list stays, `retired`, while work still sits in it; its column shows only while it holds a card. Without `standard`, Kerdr uses its own: Plan, Design, Build, Test and Deploy. |
| `create_fields` | The fields this machine's action backend takes in `create` besides the words. Kerdr sends `organization`, `tenant` and `team` only when they're listed here; see [Creating a new intent](#actions). |

The bar lists the owners with work in the order the feeds give them, each organization followed by
the tenants inside it. Owners are merged across machines by `id`, the first machine's copy (this
one's) leading. On a view whose cards all share one list, the columns are Backlog, that list's
stages and Done. On a view that mixes lists, each list gets a band of its own between Backlog and
Done; see the README.

## Work inside a card

What an agent made for a card sits inside that card rather than in a column of its own: its
children (`parent`), the undertaker seeing its agents through (`undertaker_for`) and the
follow-ups that came out of its work (`from`). Kerdr matches each by `id` within the same `source`,
across every machine's feed, so a follow-up running on another machine still sits in its card. A
piece of a piece sits in the topmost card of its family that's on the board, and a loop in these
links is broken where it closes, leaving every card on the board.

On the board, a card holding work has one more line: how many pieces, a few marks (▲ needs you, ○
no agent at it, ● under way, ■ waiting, ◇ waits on another intent, · not started, ✓ done), `+N` for
the rest, and the piece that needs you, named. Its border turns yellow only when a piece needs you
(asking, waiting on you, blocked, or open with no agent at it). Column counts count the card once. A
done card with work still open inside goes to the top of Maintain with a plain count, "3 open",
yellow only when some of it needs you, so **keep a closed card in the feed while anything inside
it is open**. When the card a piece sits in is out of view (another machine's tab, a filter, a
record no feed has), the piece shows as a card of its own marked "in #256", and `o` on it opens
that card when it can.

`o` opens a card's own screen: the card itself, then what's inside grouped as needs you, under way,
not started and done, with the selected row's details beside the list (below it on a narrow
screen). `Enter` on a row goes wherever that record runs, reopening one with no agent at it as
`Enter` on the board does. `Enter` on a card goes to `workspace.own_tab`; when that tab has closed,
or no tab is recorded while other cards' work runs in tabs of the workspace, Kerdr says so and
offers to send `reopen` instead of landing on another tab. A Kerdr older than 0.15 ignores all of this: it shows
every card in its own column and opens the workspace on `Enter`.

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

The action names are `create`, `dispatch`, `reopen`, `refresh`, `dismiss`, `restore`, `move`,
`close`, `cancel` and `edit`.
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

On the owner split, `create` carries the view it was made in: on all of Kerdr, no owner, and your
backend works out whose it is from the words; on an owner, or its No team tab, the owner's
`organization` (the organization itself, or a tenant's) and, for a tenant, `tenant`; on a team,
those and `team`. Kerdr sends each only to a backend whose feed lists it in `create_fields`, and
says in the box before you send that the intent will go under No team (or under no owner) when the
backend can't take it. File the record under that owner and team, home it where that owner's work
runs, and refuse a `team` that isn't one of the request's owner's teams (or is retired), so a
board that's out of date can't file work under another owner's team. Kerdr sends `create` to the
machine it runs on.

```json
{"action":"create","words":"A spring sale email","session":"SHP","submission":"kerdr-18f2c3a9d0e1-4d4","organization":"studio","tenant":"shop","team":"marketing"}
```

Kerdr sends the same `submission` again only for the same words, images, owner and team to the
same session, and only while an earlier send of them never got an answer (a timeout, say). When a record already
exists for it, answer with that record's `id` instead of making another. Changed words or images
are new work with a new `submission`.

Write the record up from the words however your system does (a person, a script, an agent),
and answer with its `id` as soon as it's saved, before any slow write-up. While it's being
written up, give the card `intake: "pending"` in the feed. A response that names an `id` means
the record exists even when `ok` is false (saved but not published, say): Kerdr won't send the
same words again.
`refresh` keeps wanted work fresh while preserving its creation date; `dismiss` drops it from
the active backlog and `restore` brings it back. `move` changes backlog order. `reopen`,
`close`, `cancel` and `edit` are described below, and so is `dismiss` for work already started.

Optional request fields are `id`, `title`, `words`, `tenant`, `organization`, `project`, `session`, `workspace`, `tab`, `submission`, `confirm`, `text`, `was` and `from` (strings),
`override`, `top` and `queue` (booleans), `delta` (a signed integer), and `images` (an array, on
`create`). Omit fields that do not apply.
The backend must validate the request, enforce its own permissions and slot limits, and print
exactly one JSON response to standard output. Send diagnostic output to standard error: Kerdr keeps it
with any failure it shows, in the log the README's "Errors are kept" describes.

```json
{"ok":true,"message":"Work started","id":"0041","session":"web"}
```

`ok` (boolean) and `message` (string) are required. Optional fields are `confirmation` (boolean,
default false), `used` and `limit` (nonnegative integers or null), and `status`, `session`, `id`,
`workspace`, `label`, `tab`, `home`, `confirm`, `text` and `was` (strings or null). Unknown response fields are allowed for future additions. A valid JSON response is
read even if the command exits nonzero, allowing the backend to explain a refused action or ask
for confirmation:

```json
{"ok":false,"message":"Session web is full","confirmation":true,"used":3,"limit":3,"session":"web"}
```

A confirmation is a separate user decision before sending a new request with `override: true`.

Kickoff questions: background work can have questions only you can answer before it can run on its
own. To put them to you when `d` starts it, answer the start with `questions` and an `asked` token,
and Kerdr opens its Kickoff Questions box, labelled a background task, instead of a yes or no:

```json
{"ok":false,"message":"#41 can't start on its own: 1 question is yours first.","confirmation":true,"confirm":"opaque",
 "asked":"opaque","session":"web","questions":[{"num":"1","question":"Whose work is this?",
 "background":"It decides which machine runs it.","pick":"Kibbelz","why":"it sounds personal"}]}
```

`background`, `pick` and `why` are optional, and `note` says one more thing in the box. Each question
shows whole, one at a time. ⏎ takes its recommendation (`pick` and `why`) or sends what you typed.
After the last, the same `dispatch` goes again with `asked` and `answers`, as
`[{"num":"1","take":true},{"num":"2","text":"your words"}]`: save them, then start it. Esc saves what's
answered with `{"action":"answer","id":"0041","asked":"opaque","answers":[…]}` and starts nothing, and
is the only request that isn't a start. Tab, offered when the answer has `confirmation`, starts it with
you along instead: `override: true` and its `confirm`, with `asked` and `answers` when some are given.
When the questions have changed since `asked`, save nothing, and answer with them as they are now. A
start with answers that fails keeps what you typed: `d` opens the box again with your words in it.
Kerdr asks it for `dispatch` and `reopen`.

Preloading them: a card can carry those questions in the feed, so `d` opens the box at once rather than
after a start is refused, `"kickoff":{"asked":"opaque","questions":[…]}`. Behind the open box Kerdr
sends `{"action":"kickoff","id":"0041"}`, which must start nothing: answer it as you'd answer the start,
with the questions as they are now, or with `ok` and no questions when none are left, and the box closes
saying your `message`. A question that changed shows as it is now, and the box says so; one that didn't
keeps its place and what's typed. Tab is offered only once that answer has come. With no answer, the box
stays up and says it couldn't check, and your check of `asked` still decides. Leave `kickoff` out where
`d` wouldn't ask them.

Actions have a 120-second deadline, and `dispatch` and `reopen` five minutes, since a start on
another machine can wait minutes for it there; keep your own limits under those, so your answer
comes first. A timeout or an invalid response does not prove the mutation failed: its result is
unknown. Kerdr does not retry it automatically; for a start or a reopen it says the work may still
open and to look at its card before trying again. After success, the next feed read reflects the
saved changes.

Kerdr doesn't wait for an action before going on (since 0.21). It shows the change on the board at
once, laid over the feed as Kerdr expects your backend to save it: `dispatch` marks the card
starting where it is, and moves it out of the backlog once it's answered yes, `close` shows it done, `dismiss` and `cancel` put it in Won't do,
`restore` takes it out, `refresh` dates it today, `move` (with `delta`, `top` or `bottom`) reorders its group (open backlog work
of the same owner from the same feed) as `0, 1, 2…` by `backlog_rank` and then id, and an `edit`
with a `text` shows that text as the card's `words` with `intake: "pending"`. The command runs
in the background: requests for one record go one at a time, in the order they were made, and so
do `move`s to one machine, whatever their record, since each reorders what the one before left;
other requests for different records may run at the same time, so a backend must keep concurrent
requests for different records safe. The one exception is an `edit` without a `text`, a read that
changes nothing: it waits for the requests made before it for its record, but nothing waits for
it, so a later change to that record may run while it does, and `q` neither waits for it nor
counts it as still running.
A `dispatch` to a session your feed says is at its `limit` (unless its card's `lane` is `background`),
or a `reopen` of done work, shows nothing on the card until your backend's answer: it can only come back
as a question, so Kerdr says it's asking instead; if it comes back started after all, it shows from then.
A success shows from the feed itself once a read has it (or
after two reads, or a minute). Anything else (`ok: false`, a confirmation, a timeout, an invalid
response) takes the change off the board at once, so the card is as the feed last said, and Kerdr
says what didn't happen in a red box that stays until a key is pressed, or asks the question.
While a `dispatch` or `reopen` is unanswered, or answered but not yet in the feed, Kerdr sends
nothing else for that record. A later change the feed shows retires the earlier ones to the same
record, and a failed `move` takes the later moves' shown orders with it. Account
actions still wait for their answer, one at a time.

When a request is refused because the record lives on another machine than the one asked (a copy
there that was behind named the wrong home), answer `ok: false` with `home`, the record's home as
`<Machine>/<Session>`:

```json
{"ok":false,"message":"#41 lives on build-box now (ARK); ⏎ again opens it there.","home":"build-box/ARK"}
```

When that machine is one of Kerdr's, other than the one asked, and is answering, Kerdr sends the
same request there once (a `reopen` with `session` set to that session), and sends that record's
later actions there too until the feed catches up, so a question that machine asks can be answered
yes. It never sends it on a second time: when that machine's answer names yet another home, Kerdr
says the two copies disagree. When the home is a machine Kerdr doesn't list, or one that isn't
answering, Kerdr says that in its own words instead of the message, since the message's "⏎ again"
holds only when Kerdr sends it on. A home naming the machine asked is shown as the message.

## Starting work nobody started

`Enter` on a card for backlog work nobody has started (no workspace open, its state not
`active`, `background` or `closed`, not in won't do) sends the `dispatch` request `d` sends, to
the card's home machine, and a `confirmation` answer asks the same question before an
`override: true`. On a search hit or a row inside a card it starts that card, not the selected
one; a search hit starts from a peek on it, and a start from a peek stays in the peek, so the
question still finds its card. A second `Enter` before the feed shows the work under way sends
nothing. The backlog screen's `Enter` peeks, and in that peek `Enter` leaves starting to `d`.

## Editing a backlog item's text

`e` on a backlog card, on the board's Backlog column, in the backlog and in a peek, opens the
item's text, the words that started it, full screen in your own editor. Kerdr refuses a card
that isn't waiting in the backlog (started, done or in won't do) itself, and one still being written
up (`intake: "pending"`) with "#416 is still being written up; edit it once that's done.", without
asking the backend; so too a new intent's card while its words are still being saved, a card whose
start or reopen is on its way, and one `d` asked to start once it's written up. A card whose feed
carries `item_text` and `item_was`, read within the last five minutes, opens from those with no read,
and only its save asks the backend anything; so whatever a read would refuse that the feed can't show,
the save must refuse. Otherwise Kerdr says it's reading, and reads the text:

```json
{"action":"edit","id":"0414","from":"amd-lzm4"}
```

`from` is the hostname of the machine Kerdr runs on, for the record's log. Answer with the text and
`was`, a fingerprint of it that Kerdr sends back with a save (ark-sdlc's is `sha256:` and the
text's SHA-256). Which text that is belongs to the backend (ark-sdlc's is the body of intent.md's
first `## ` section); it never ends in a newline:

```json
{"ok":true,"message":"","text":"Need the ability to edit items in my backlog.","was":"sha256:9f2c…"}
```

Kerdr writes it, with one final newline, to `NNNN-text.md` (so the editor treats it as markdown) in
a folder of its own in the system temp folder, readable only by you (0700, and the file 0600, on
Unix), opens it in the editor Kerdr's `editor` setting names, else `$VISUAL`, else `$EDITOR`, else
`nvim`, else `vim`, and removes both when the editor closes. The setting's words are split as a
shell would split them, but no shell runs it; the file's path is its own last argument. An editor
that opens a window of its own (`code`, `codium`, `subl`, `gvim`, `mvim`) named without its
waiting flag (`--wait`, `-w`, `-f`) isn't started, since Kerdr would be back before you'd written
anything: Kerdr says which flag to add to its `editor` setting, and keeps the text for `e`. When
the editor exits with an error (`:cq`), closes without writing the file (`:q`), or leaves the text
as it was read, nothing is sent and nothing is said. When it dies instead (killed by a signal, or
crashed, so it has no exit code), what the file held is kept as your version, sent nowhere, and the
red box says so. When the file can't be read back, nothing is sent and the file is kept where it
is, in a folder renamed `kerdr-kept-…` that no Kerdr removes, and the red box names it. Otherwise Kerdr normalises it (line endings to `\n`, whitespace and newlines at
the very end removed) and saves it, with the `was` it was read with:

```json
{"action":"edit","id":"0414","text":"Need the ability to edit a backlog item's text.","was":"sha256:9f2c…","from":"amd-lzm4"}
```

A request with `text` is a save; without it, a read. The save is a change like any other (see
above): the moment the editor closes, the card shows the new text under "How it started", with ✎
for being written up, and the save runs in the background in its turn, so the board is yours at
once. Normalise what you receive the same way, refuse an empty text, and answer:

- Saved: `{"ok":true,"status":"saved","message":"Saved #414's text. Its title and done-when catch up in about a minute."}`.
  Answer as soon as the text is saved: redo any slow write-up on its own, with the card's
  `intake: "pending"` meanwhile, so the feed goes on showing it as being written up. Kerdr says
  nothing (the card already shows it, so `message` isn't shown) and reads the feed again, as after
  any action that succeeds; the card shows the new text until a read's `words` have it, whatever
  their spacing, or two reads or a minute have gone by. Words cut short with `…` prove nothing,
  since the old text may start the same way. Until then `e` on it
  says it's still being written up, as it does for `intake: "pending"`, and `d` on it starts it
  once it's written up, as it does for any card being written up, trusting only a read begun after
  the save was answered: one from before says nothing of the write-up the save began.
- Saved, but something after it didn't happen (the write-up couldn't start, so the title and
  done-when won't catch up until it's rerun, or the push to the other machines failed):
  `{"ok":true,"status":"partial","message":"<what didn't happen, in plain words>"}`. Kerdr counts
  it as saved (your version is let go, the card keeps the new text) and shows `message` in the red
  box, since that part is a failure.
- No change: `{"ok":true,"status":"unchanged","message":"Nothing changed; #414 is as it was."}`,
  writing nothing. A success too, so Kerdr says nothing.
- Changed underneath, when the current text's fingerprint isn't `was`:
  `{"ok":false,"status":"stale","message":"#414's text changed while you had it open.","text":"<current text>","was":"sha256:<current>"}`.
  The next `e` opens your version with the newer text below it, in a block marked as left out
  when you save, so you keep what you want of each; the save then carries the newer `was`. The
  block's first and closing lines carry a marker Kerdr makes for that one edit, and only lines with
  it are left out, so a line of your text that looks like one stays as you wrote it.
- Refused, nothing written: `ok: false` and a plain `message`, such as an agent being at it, a
  line that looks like a token or password (never echo it), or a line that would start a new
  section. The next `e` opens your version against the same `was`.
- On another machine: the same `home` refusal as every action. Kerdr sends the save there with
  your text, still showing it on the card.

Anything but a success takes the new text off the card at once and says, in the red box that stays
until it's read, that the text wasn't saved, why, and that your version is kept. Kerdr keeps your
unsaved version, in memory, until a save of it succeeds, and `q` says first, naming it, when
quitting would lose it; a second `q` quits. `e` opens your version without asking its machine, so it's yours to get at even while
that machine isn't answering; what you write is kept, and sent only once it can be. When a save
gets no answer (a timeout, a dropped connection, an answer it can't read), it may or may not have
happened: the next `e` reads the text first, says it was saved when it now reads as your version,
and otherwise opens your version against the fresh `was`, with the newer text below it when it's
neither what you started from nor yours. When that check can't be made (its machine isn't
answering, or the read is refused or gets no answer), your version opens all the same, to copy
from: nothing is sent when it closes, what you write there is kept, and the next `e` checks first
again. Since a save that works says nothing, what you wrote that Kerdr can't send goes in the red
box too: an empty text, one past the 64 KiB a request may be, a machine that stopped answering
while the editor was open, an editor that died, a file that couldn't be read back, and words written
into your kept version of an item that can no longer be edited or whose last save couldn't be
checked.

The read isn't a change: Kerdr shows nothing for it, it waits for the requests made to that record
before it, and nothing waits for it; `e` again while it's on its way sends nothing more. Its answer
opens the editor only while you're still on the card you pressed `e` on, within 15 seconds of that
`e`, with no trip into herdr since and no `Esc` on the board to let it go; otherwise Kerdr says the
text is ready, and `e` opens it. While a red box is up the editor waits, since it would hide the
box. All of that is checked again at the moment the editor would open: once the box is read, it
opens only if it's still within those 15 seconds of `e` and you're still on that card, with no trip
into herdr since; otherwise the text is said to be ready instead. A
refused read is said in the message line, since nothing changed. An `edit`'s answer may be up to
192 KiB, room for a text of 48 KiB however much of it JSON escapes. A backend without `edit` should
refuse it with a plain `message`, which Kerdr shows.

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

`workspace` is a herdr workspace id in `session` on the machine the action ran on. When the reopen
opened a new tab in a workspace already open (beside its parent's agent, or beside a tab still
open there), add `tab`, that tab's id: herdr leaves a workspace's active tab as it was, so Kerdr
goes onto that tab, and only that tab. Create the tab without focusing it, so nobody else's screen
moves. Kerdr goes in only while you're still on that card; if you've moved on, or pressed `Esc`,
it says the work is open again instead.

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
new work would be. At the most a session holds (`used` reaching the feed's `max` for it), a yes
can't open it there, so ask with a question all the same, saying `w` queues it: Kerdr titles it
"Queue #41 in web?". Without `[actions]`, `Enter` says the card has no workspace and sends nothing.

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

## Closing work with no agent

`C` on a card asks, then sends `close`, for the backend to mark the record done without starting
anything:

```json
{"action":"close","id":"0117"}
```

Kerdr offers it for an open record that isn't dismissed or still being written up, in the backlog
or in a working column, with no workspace open. `x` (`dismiss`) works on the same started work,
not only on the backlog: the record leaves the board for the Won't do view, keeping its state, and
`restore` brings it back where it was. Kerdr refuses a card with a workspace open itself, since its
agent may be there; refuse in the backend, with a plain `message`, whatever only it can tell, such
as the work's agent conversation still running outside any workspace. Done work is refused too.
Before sending the yes, Kerdr rechecks that the card is still the same work, still open, with no
workspace, and, for `x` on started work, not back in the backlog.

## Cancelling work an agent is at

`X` is for the other case, work an agent is still at: its card has a workspace open, or, for a
child running in a tab of its parent's workspace, a tab. Kerdr asks first, saying what it will
stop (the card's agent, and whether it's mid-step, idle or at a question), what it will close
(that workspace with every tab in it, or only the child's tab), that the work goes to Won't do,
and which children in tabs of that workspace stop with it. A yes sends `cancel` to the card's own
machine, naming where the yes was given:

```json
{"action":"cancel","id":"0300","session":"default","workspace":"w2M"}
{"action":"cancel","id":"0322","session":"default","workspace":"w2M","tab":"w2M:t3"}
```

The backend stops that agent, mid-step or idle, and everything else running in that workspace or
tab, then closes it, and then sets the record aside as `dismiss` does: it leaves the board for
Won't do, keeping its state, so `restore` brings it back and `Enter` then reopens it, resuming its
conversation. ark-sdlc's backend stops every process under the panes, and every one still in a
pane's terminal session, where it stands first, holding each by a pidfd, walks them again until
nothing new appears, and only then ends them, so nothing they start meanwhile gets away and no
signal reaches a process that took a pid since. Anything going wrong on the way lets go of what it
stopped. Refuse, with
a plain `message` and nothing stopped, when the work runs anywhere other than the workspace and
tab named, or in more than one place, when it's done, in Won't do or still being written up, or
when the action itself runs inside that workspace. When the agent stopped but the work is still
open somewhere, or its conversation still running, say so and leave the record as it was. Before
sending the yes, Kerdr rechecks that the card is the same work, still open in that workspace and
tab.

`X` on a card with no workspace open says `x` and `C` are for it instead. Children in tabs of a
workspace that closes stop too and stay open with no agent; `x` or `C` ends them.

## Moving one card's agent to another account

`s` on a card moves that card's agent, and only that agent, to another account, resuming its
conversation there; the session's other agents and its owner's list stay as they are. It's
`account-move` (see [Account actions](#account-actions)), keyed by the card's pane. A card an
agent is at says so on the board's bottom line (`s move its agent off KBZ_C`), in its peek, and in
the `Ctrl+Space` menu. An agent is movable when the feed records it or herdr reports it running a
program the machine's accounts run (a provider's `programs`, or an account's `runs`). A move to
an account another program runs is a hand-over; the backend asks first, and Kerdr titles its
question so ("Hand #45 over to Codex?"). `S` edits the list new agents start on, and moves no agent that's running:
saved unchanged while an agent there could move, it says that `s` on its card is the key and names
those cards, those off the list's first account first. It edits the viewed session's owner's list,
or with no session chosen, the selected card's owner's: its session's, its home session's, else the
tenant or organization its record names.

## Starting a card on an account

`s` on a card with no agent running starts it on the account you pick. Kerdr sees each card in one
of three states: an agent the machine's accounts run is at it (`s` moves it, as above); nothing runs
it and Kerdr can start it (`s` asks which account); or neither, and `s` says why. A card that can
start does what `Enter` or `d` would do for it, with the account added to the same request:

```json
{"action":"dispatch","id":"0041","account":"ARK_C"}
{"action":"reopen","id":"0041","session":"web","account":"ARK_C"}
```

`dispatch` is for work nobody started; `reopen` is for work that ran and has no agent now (its
workspace closed, or open with its agent gone) and for done work, and it resumes the conversation, so
the picker offers only accounts of the program that conversation ran (the card's `resume.agent`).
"follow its owner's list" sends `"account":"list"`, which wins over an account a queued start
kept from an earlier pick; a start with no `account` (⏎, `d`) uses one kept that way. A question its backend answers with
(a full session, kickoff questions, done work) keeps the account for the yes. A start that restarted nothing (its agent was
running already) answers `"account_unchanged": true`, with a message saying the account wasn't
applied; Kerdr shows it and doesn't go in. A backend that can't start it on the account (a stopped
session it would have to restore as it was) should refuse, saying so, rather than start it on another.

Kerdr sends `account` only to a machine whose feed lists the request in `starts_on`; with no
`starts_on`, an older backend would start the work on its owner's list without a word, so `s` says
to update it instead:

```json
"starts_on": ["dispatch", "reopen"]
```

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
except that a reported status still applies. A reported `needs-you` gives way while the
workspace's own agent is working, since nothing takes the token down when you answer it.

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
  "providers": [{"id":"claude","label":"Claude","letter":"C","kinds":["token","login"],
    "kind_labels":{"token":"a token per machine","login":"the machine's saved sign-in"},
    "programs":["claude"],"program_labels":{"claude":"Claude Code"},
    "signin":"page_code","windows":["five","week"],"fields":[]},
    {"id":"codex","label":"Codex","letter":"X","kinds":["login","token"],
    "kind_labels":{"login":"this machine's own Codex sign-in","token":"a sign-in of its own, per machine"},
    "programs":["codex"],"program_labels":{"codex":"Codex"},
    "signin":"device_code","windows":["five","week"],"fields":[]}],
  "accounts": [{"code":"KBZ_C","name":"Personal","provider":"claude","kind":"login",
    "runs":"claude","provider_label":"Claude","program_label":"Claude Code","paste":false,
    "status":"out","until":"2026-10-22T09:00:00-05:00","pool":"week",
    "usage":{"five":86,"five_at":null,"week":100,"week_at":"2026-10-22T09:00:00-05:00","seen":"2026-10-21T17:46:00-05:00"}},
    {"code":"KBZ_X","name":"Personal Codex","provider":"codex","kind":"login","runs":"codex",
    "status":"ready","usage":{"week":7,"week_at":"2026-10-28T01:48:00-05:00","seen":"2026-10-21T17:44:00-05:00"}}],
  "owners": [{"id":"studio","kind":"organization","name":"Studio","code":"KBZ","parent":null,
    "models":["KBZ_C","KBZ_X"],"own":true,"sessions":["default"]},
    {"id":"shop","kind":"tenant","name":"Shop","code":"SHP","parent":"studio",
    "models":["KBZ_C","KBZ_X"],"own":false,"sessions":["web","shop-bg"]}],
  "owners_source":"mock",
  "machine_owner":"studio",
  "unowned_sessions":[],
  "agents":[{"pane":"w1:p1","session":"web","conversation":"conversation-id","program":"claude",
    "account":"KBZ_C","how":"list","problem":null,"waiting":{"why":"usage","until":"2026-10-22T09:00:00-05:00"}},
    {"pane":"w2:p1","session":"web","conversation":"another-id","program":"codex","account":"KBZ_X",
    "how":"next","handed_from":{"account":"KBZ_C","program":"claude","conversation":"conversation-id",
    "why":"usage_limit","at":"2026-10-21T17:44:00-05:00"}}],
  "current": {"account":"KBZ_X","name":"Personal Codex","since":"2026-10-21T16:00:00-05:00",
    "status":"ready","until":null,"machine":"build-box"}
}
```

`agents` lists the agents this machine started, newest first. A card shows the account of the
record whose `conversation` is the one herdr sees in the card's pane, and shows none when no record
has it; until herdr knows the pane's conversation, it takes the first record on that pane. A
backend should leave out records of agents that no longer run, but one left in can't change a card
whose conversation herdr knows.

`current` is the **current account**: the one new sessions start on ahead of every owner's
list, set for every machine at once. It's `null` when none is set. A backend that has no current
account leaves the field out: Kerdr then shows nothing about it and never sends `account-current`
there, so a backend that has one always sends the field, `null` included. `status` and `until` are the account's as this feed's machine
sees it (`ready`, `out`, `sign_in`, `missing`), so each machine's feed says its own: where it
isn't `ready`, new sessions on that machine start on their lists, and Kerdr shows that machine
in a warning colour with why. A session started on it has `how: "current"` in `agents`.

### Providers

`providers` describes every provider the backend knows, so Kerdr names none in its code and a
provider added later (a local model, say) is listed, added, signed in and shown with no Kerdr
release:

- `label` names it; `letter` is the last letter of its accounts' codes, which are the owner's
  code, `_`, then that letter (`KBZ_C`, `KBZ_X`; up to six characters).
- `kinds` are its kinds of account, the one to offer first first, with `kind_labels` saying each
  in its own words, for the accounts screen and the add form.
- `programs` are the programs that run its accounts, the usual one first, named for people by
  `program_labels`. A program's id is the agent name herdr reports for it (`claude`, `codex`),
  which is how Kerdr knows what a card's agent runs. `s` moves agents of any program the
  machine's own providers (or its accounts' `runs`) name, and the agents filter (`a`) cycles
  through every program the feeds name, then any other agent herdr reports.
- `signin` is how an account signs in: `page_code` (a link; the page then shows a code to paste
  back), `device_code` (a link and a code to type on the provider's page; nothing comes back),
  or `null` (no sign-in: adding one starts no sign-in, and Enter on its line tests it).
- `windows` are the usage windows its accounts can report. An account shows the windows its
  reading has; with no reading, it shows these as unknown.
- `fields` are anything else its accounts need when added, `[{"id","label","required"}]`, which
  the add form asks after the name and sends with `account-add`.

A feed without `providers` (an older backend) still works as before: each account's `provider`
is named by its `provider_label`, else the id with a capital; kinds read "saved sign-in" and
"token per machine"; usage always shows the five-hour and weekly windows; and `+` adds a token
account of a provider its accounts name, with a code of 2–4 capitals. Names come from any
machine's feed (the viewed one's first), so a machine whose feed predates `providers` still shows
the words another's gives. What a machine can do comes from its own feed alone: the providers
`+` offers there, their code rules, how its accounts sign in and take a paste, and the programs
`s` moves, since another machine's newer backend may know more than this one's. The add form ticks only
machines whose own feed offers that provider, and each machine's sign-in takes a pasted token by
its own feed's answer.

An owner's optional `code` (`KBZ`) is what `+` suggests a new account's code from: the viewed
session's owner's, else the machine owner's, else the first organization's whose code with the
provider's letter isn't taken. An account's optional `paste` says whether its sign-in takes a
pasted token; missing, a `token` account takes one unless its provider signs in by `device_code`.

An account may also list `copies`: the separate stored sign-ins of it on this machine, in the
order to show them, such as your agents' copy and the one root holds for tenants:

```json
"copies": [{"id":"agents","label":"Your agents","stored":true,"works":true,
             "checked":"2026-10-26T17:40:00-05:00","note":null},
           {"id":"tenants","label":"Tenants","stored":false,"works":null,"checked":null,
             "note":"held by root"}]
```

`works` is `true`, `false`, or `null` (not tested); `checked` is when it was last tested; a
missing `label` shows the `id`. The first copy is the one launches use, so the account's own
`status` is that copy's. An optional `paste` says whether the copy takes a pasted token (`false`
for one that signs in only on its own machine, as root's does); missing, the first copy of a
`token` account takes one and any other copy doesn't. A copy that is `stored` and `works: true`,
or the first copy while the account is `ready` and no test says it doesn't work, is asked about
before Enter signs it in again. With two or more copies the accounts screen shows one line per
copy under the account; with none or one it is unchanged. The account's own `status` stays the
copy that launches use. Copies change nothing on the board, the usage line or card badges.

Account status is `ready`, `out`, `sign_in`, or `missing`. Usage values are percentages, one
per window: `five` and `week`, and any other length as `w<minutes>` with `w<minutes>_at`. Kerdr
shows the windows a reading has, shortest first: `86│100` for both, `7` for a plan with only a
weekly limit (`7 this week` where there's room). Missing or null usage displays as unknown. Usage belongs to the account rather than the machine,
so wherever Kerdr shows an account's figures, on any machine tab or on All, it uses the reading
with the newest `seen` among every machine's feed for that account code; a reading without `seen`
counts as oldest, and a tie goes to the first machine in the board's order on every tab. A figure whose window's reset time (`five_at`, `week_at`, `w<minutes>_at`) has passed displays as unknown (`—`)
until a newer reading arrives, and the accounts screen says which window reset and when. Creating
an account sends the provider and kind chosen in the add form; the backend validates them.
An owner’s `models` contains its effective ordered list; `own: false` denotes inheritance.
Sessions are explicitly assigned to owners, with `default` falling back to `machine_owner`.
Unowned sessions are marked on the accounts screen. `owners_source: "mock"` is shown as such.

Cards join their herdr agents to the feed’s agents by **machine, session, and pane**, including
child cards that name a tab. `how` is `list`, `manual`, `next`, `found`, `current` (started on the current account), or `handed_over`. An agent's
`program` is what it runs (one of a provider's `programs`). `handed_from` marks an agent that
took over from another program: the account and program before, the conversation left there,
`why` (`usage_limit`, `sign_in`, `manual`, or anything else, shown as words) and `at`. Its card
shows `⇄` beside its code, and its peek says from which account and program, why and when. A usage wait
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
| `account-add` | `code`, `name`, `provider`, `kind` (one of that provider's `kinds`), optional `fields` object | adds, or renames the existing code |
| `account-test` | `code`, optional `copy` | status and message |
| `account-remove` | `code` | removes after Kerdr’s local confirmation |
| `account-current` | `to` (a code), or `clear: true` | makes it the current account for every machine, or clears it; answers `message` and `current` (the field above, as this machine sees it now, or null) |

`c` on the accounts screen sends `account-current` with `to`, and `C` sends it with `clear`, to
the machine being viewed, whatever its feed last said (the backend answers if it was already
so); Kerdr shows the answer's `message`, and its `current` at once on that
machine, while the other machines' feeds follow as their backends do.

The `S` tick runs `account-move-waiting` on configured feed machines only after the list was
saved successfully. Unreachable machines are reported rather than silently claimed as moved.

```json
{"ok":false,"confirmation":"This agent is mid-step. Move when it next stops? Restarting ends its background job.",
 "yes":"move when it stops","no":"leave it","confirm":"opaque-ticket","message":"Move needs a decision"}
```

A move to an account another program runs answers with a question too, whose `yes` says it's a
hand-over ("hand it over", or "hand over when it stops" for an agent mid-step); Kerdr titles it
"Hand #45 over to Codex?" when the agent's `program` and the target's `runs` differ.

For account moves, `confirmation` is the question text (ordinary card actions keep their
boolean field). Kerdr shows it only after selection, waits for a fresh `y`, then resends the
original machine, pane, session, conversation and target with `confirm`. It does **not** add `override`,
`when_idle`, or `end_jobs`: the backend’s opaque token covers exactly what was asked.

Moves send the feed's `agents[].conversation`. A changed conversation answers
`{"ok":true,"state":"stale","message":"the conversation changed"}`. An accepted move answers
`{"ok":true,"state":"queued","account":"ARK_C","message":"moving to ARK_C"}` promptly, then restarts
in a detached worker. Kerdr displays the message; the feed changes the account badge when the
move finishes. An unavailable target answers `ok: false` before stopping anything.

### Signing in

```json
{"action":"account-signin","code":"ARK_C","step":"start"}
{"ok":true,"message":"Authorize in your browser","status":"waiting","ticket":"opaque-ticket",
 "url":"https://example.com/authorize","needs_code":true}
```

For a provider whose `signin` is `device_code`, `start` answers with the code to type on its
page, when it expires, and `needs_code: false`:

```json
{"ok":true,"message":"Type the code on OpenAI's page","status":"waiting","ticket":"opaque-ticket",
 "url":"https://auth.example.com/codex/device","needs_code":false,"user_code":"K7QD-3MPWX",
 "expires":"2026-10-31T18:15:00Z"}
```

Kerdr shows the code in a box with how long it lasts and no code field; `c` copies the code,
`l` the link, and `o` opens the page again. Polling finishes it as usual; once `expires` has
passed, `r` cancels that ticket and starts again. A `user_code` that isn't 4–32 letters, digits
and dashes is never shown or copied: Kerdr stops that sign-in, cancelling its ticket, and says
so. `user_code` and `expires` go through the same credential scrub as the rest of an answer; a
`status` the protocol knows (`waiting`, `done`, `failed`, …) is kept as it is.

Enter starts this on the machine the accounts screen is viewing, which opens on the board's
machine tab or else on the machine Kerdr runs on, then, one after another as below, on every
other answering machine whose feed says `sign_in` or `missing` for that code (a copy's line
signs in that copy alone). When the account's `status` on the viewed machine is already
`ready`, Kerdr asks first instead, offering those machines (all in turn, then each) and signing
in again there; each `start` goes to its own machine. By each later machine's turn, one whose
feed no longer says `sign_in` or `missing` is skipped (machines ticked when adding an account
aren't). A `cancel` for a machine not answering is kept and sent once it answers.

On a copy's line, `start` names it: `{"action":"account-signin","code":"ARK_C","step":"start","copy":"tenants"}`.
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
Kerdr offers it (`p`) only for an account or copy whose `paste` is `true`, or, without `paste`, a `token` account whose provider doesn't sign in by `device_code`. The
`page_code` a `finish` sends is the code the page shows once you allow it. A backend that
describes its providers checks it: refusing one, it answers `ok: false` with no `status` and its
sign-in waits on, and Kerdr keeps the code box open with the backend's message under it (a
refusal that ends the sign-in says `status: "failed"`). For a provider no feed describes, whose
older backend ends a sign-in on a wrong code, Kerdr keeps back an entry of 4–10 digits alone, a
sign-in page's emailed code, and says where it goes.
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
