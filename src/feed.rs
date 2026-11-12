//! Card feeds: a command on a machine that prints its records of work as cards (docs/feed.md).
//! Kerdr runs it on that machine and merges its cards with the herdr workspaces it sees there.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A card's open questions as its feed read them, and a token of what was asked, for the Kickoff
/// Questions box (intent 0491).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Preloaded {
    pub asked: String,
    #[serde(default)]
    pub questions: Vec<crate::actions::Question>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Feed {
    pub kerdr_feed: u32,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub sessions: BTreeMap<String, FeedSlots>,
    #[serde(default)]
    pub cards: Vec<FeedCard>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Absent on older feeds. Account state is local to this feed's machine.
    #[serde(default)]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub accounts: Vec<crate::accounts::Account>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub owners: Vec<crate::accounts::Owner>,
    #[serde(default)]
    pub owners_source: Option<String>,
    /// Which shared account catalog this machine's backend writes (intent 0591, release 4): the
    /// same on every machine whose backend saves to the same central copy. A catalog change for a
    /// machine that can't take it goes through another with the same one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(default)]
    pub machine_owner: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub unowned_sessions: Vec<String>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub agents: Vec<crate::accounts::AccountAgent>,
    /// The providers its accounts can be (absent on older feeds): their names, kinds, letters,
    /// sign-in style and windows, so Kerdr names none itself.
    #[serde(default)]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub providers: Vec<crate::accounts::Provider>,
    /// The one account new sessions start on ahead of every owner's list, as this machine sees
    /// it: `Some(None)` for the feed's `null` (none set), `None` when the field is absent, an
    /// older backend that has no current account to show or set (intent 0455).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub current: Option<Option<crate::accounts::Current>>,
    /// The start requests (`dispatch`, `reopen`) this machine's backend starts on an `account`
    /// Kerdr names, so `s` can start a stopped card on one (intent 0580). Absent on older feeds,
    /// whose backends would start it on its owner's list instead: Kerdr never sends them one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub starts_on: Vec<String>,
    /// The stage lists its cards' stages belong to (intent 0595): each team's own columns. Absent
    /// on older feeds, whose cards are all on the standard list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub stage_lists: Vec<StageList>,
    /// The fields this machine's backend takes in `create` besides the words (`organization`,
    /// `tenant`, `project`, `team`). Absent on older feeds: Kerdr sends none of them there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub create_fields: Vec<String>,
    /// What this machine's backend does beyond feed version 1 (intent 0591): `op` (it keeps a
    /// record of each request by its op, so the same request sent again is never done twice),
    /// `retry` (a refusal that will likely clear by itself says how long to wait) and `follow`.
    /// Absent on older feeds: Kerdr never sends a request to them again by itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub supports: Vec<String>,
    /// What this feed couldn't read, in plain words (intent 0591): the board says it only once it
    /// has lasted. Anything here that isn't the shape below is passed over, not a failed feed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(deserialize_with = "read_failures")]
    pub read_failures: Vec<ReadFailure>,
    /// What its backend holds that has no card yet (intent 0591, release 4): a new intent's save
    /// finishing in the background, each naming its `submission`. Its card says so while it's
    /// listed here.
    #[serde(
        default,
        deserialize_with = "held_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub held: Vec<Held>,
}

/// One thing a feed couldn't read: `part` is "feed" (the whole feed failed), "herdr" (herdr on
/// that machine didn't answer) or "session" (one session, named in `session`, didn't).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ReadFailure {
    pub part: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub why: String,
}

impl ReadFailure {
    /// Which failure this is, whatever its words: a run of the same one counts as one.
    pub fn key(&self) -> String {
        match &self.session {
            Some(session) => format!("{} {session}", self.part),
            None => self.part.clone(),
        }
    }
}

impl ReadFailure {
    /// The shape docs/feed.md gives: `feed` or `herdr` naming no session, `session` naming one.
    /// Anything else is passed over: it must neither fail the whole feed nor warn of a session.
    fn well_formed(&self) -> bool {
        match self.part.as_str() {
            "feed" | "herdr" => self.session.is_none(),
            "session" => self
                .session
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            _ => false,
        }
    }

    /// Plain words of Kerdr's own when the feed gave none.
    fn said_why(mut self) -> ReadFailure {
        if self.why.trim().is_empty() {
            self.why = match (self.part.as_str(), &self.session) {
                ("feed", _) => "it failed without saying why".into(),
                ("session", Some(session)) => format!("herdr session {session} wasn't read"),
                _ => "herdr on this machine wasn't read".into(),
            };
        }
        self
    }
}

fn read_failures<'de, D>(d: D) -> Result<Vec<ReadFailure>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| serde_json::from_value::<ReadFailure>(e.clone()).ok())
                .filter(ReadFailure::well_formed)
                .map(ReadFailure::said_why)
                .collect()
        })
        .unwrap_or_default())
}

/// One stage list: the columns between Backlog and Done for the teams that use it (intent 0595).
/// Its id is whole, owner then name (`tinyhost/marketing`), or `standard`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct StageList {
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub id: String,
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub name: String,
    pub owner: Option<String>,
    /// Raised whenever its stages change: of two machines' copies, the higher wins.
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub rev: u64,
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub stages: Vec<StageEntry>,
}

/// One stage of a list. A retired one is kept while work still sits in it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct StageEntry {
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub id: String,
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub name: String,
    #[serde(deserialize_with = "crate::accounts::null_default")]
    pub retired: bool,
}

/// A field that's there, even as `null`, is `Some`; only a missing one is `None`.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(d)?))
}

impl Feed {
    /// Whether this machine's backend says it does `what` (Feed::supports).
    pub fn supports(&self, what: &str) -> bool {
        self.supports.iter().any(|s| s == what)
    }

    /// The record of the agent in a pane: of the conversation herdr sees there, and none when no
    /// record has it. Where herdr doesn't know the conversation yet, the newest record on the pane,
    /// which is the first, since a feed lists `agents` newest first. A pane can hold records left
    /// behind by agents that no longer run there, so the pane alone never decides.
    pub fn agent_on_pane(
        &self,
        session: &str,
        pane: &str,
        conversation: Option<&str>,
    ) -> Option<&crate::accounts::AccountAgent> {
        let mut on_pane = self
            .agents
            .iter()
            .filter(|a| a.pane == pane && a.session == session);
        match conversation.filter(|c| !c.is_empty()) {
            Some(conversation) => on_pane.find(|a| a.conversation == conversation),
            None => on_pane.next(),
        }
    }

    /// The current account, when one is set.
    pub fn current_account(&self) -> Option<&crate::accounts::Current> {
        self.current.as_ref().and_then(Option::as_ref)
    }

    /// Whether this feed's backend knows the current account, so Kerdr shows and sets it.
    pub fn knows_current(&self) -> bool {
        self.current.is_some()
    }

    /// Whether this feed's backend starts this request (`dispatch`, `reopen`) on a named account.
    pub fn starts_on(&self, action: &str) -> bool {
        self.starts_on.iter().any(|a| a == action)
    }

    /// Whether this feed's backend takes this field (`team`, `tenant`, …) in a new intent's `create`.
    pub fn creates_with(&self, field: &str) -> bool {
        self.create_fields.iter().any(|f| f == field)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FeedSlots {
    #[serde(default)]
    pub used: u32,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub max: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct FeedCard {
    pub id: String,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// The id of one of its owner's teams (intent 0595); one its owner doesn't list shows under No
    /// team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// The whole id of the stage list its `stage` belongs to; absent means `standard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_list: Option<String>,
    /// When the record was first created; keeping it fresh must not change this.
    #[serde(default)]
    pub created: Option<String>,
    /// When the user last confirmed that the work is still wanted.
    #[serde(default)]
    pub refreshed_at: Option<String>,
    #[serde(default)]
    pub backlog_rank: Option<i64>,
    #[serde(default)]
    pub disposition: Option<String>,
    /// `pending` while an agent writes up an intent from the words it was made with, `failed`
    /// when none could; absent once it's written up.
    #[serde(default)]
    pub intake: Option<String>,
    /// With `intake: failed`: why, in a few plain words ("no account was free"); absent on older
    /// feeds (intent 0591).
    #[serde(default)]
    pub intake_why: Option<String>,
    /// Every `<session>/<workspace id>` the intent is open in here, when that's more than one,
    /// since two agents may be at it (intent 0591).
    #[serde(default, deserialize_with = "crate::accounts::null_default")]
    pub open_in: Vec<String>,
    /// The submission Kerdr sent the words with, while the record is still being written up from
    /// them: the card shown while they were saved gives way to this one (intent 0445).
    #[serde(default)]
    pub submission: Option<String>,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub story: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub home: Option<Home>,
    #[serde(default)]
    pub workspace: Option<FeedWorkspace>,
    #[serde(default)]
    pub attention: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub needs: Option<String>,
    /// The questions `d` would ask it first, read ahead so the box opens at once (intent 0491).
    #[serde(default)]
    pub kickoff: Option<Preloaded>,
    /// A backlog item's whole text and its fingerprint, read ahead so `e` opens it at once and the
    /// save checks it (intent 0491). Both or neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_was: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub waits_on: Vec<WaitsOn>,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
    #[serde(default)]
    pub timeline: Vec<TimelineEntry>,
    #[serde(default)]
    pub log: Vec<LogEntry>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub stale_days: Option<i64>,
    #[serde(default)]
    pub words: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub closed: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    /// The record this one came out of, for a follow-up that isn't a child: its card holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// For an undertaker: the record whose agents it sees through, whose card holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undertaker_for: Option<String>,
    /// For work under way: the agent conversation that worked it, which a reopen resumes.
    #[serde(default)]
    pub resume: Option<Resume>,
    /// Open work in a working stage that no agent is at: no workspace on it, and nothing running
    /// its conversation. The card says "no agent"; `C` marks it done and `x` sets it aside, each
    /// starting nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_agent: bool,
    /// The conversation working it runs in a terminal outside herdr, so no workspace shows it:
    /// which terminal, and since when (intent 0411).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside_herdr: Option<OutsideHerdr>,
    /// A short hash of what a decision about it rests on, from a backend that holds requests
    /// (intent 0591, release 4): a request that waited before it went carries the one its card had
    /// when the key was pressed, and the backend does nothing when it's changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// What its backend holds for it, to do when it can: shown on the card and listed in its
    /// peek, where each can be dropped (intent 0591, release 4).
    #[serde(
        default,
        deserialize_with = "held_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub held: Vec<Held>,
}

/// One request a backend holds for a card (intent 0591, release 4): its `op`, the action it is,
/// what the card says meanwhile, when it was taken, and, for one that runs at a set time, when.
/// Both times are epoch seconds.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Held {
    pub op: String,
    #[serde(default)]
    pub action: String,
    pub pending: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    /// The new intent a save held at the feed's top level is for, by its submission id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<String>,
    /// With `submission`: the title its card shows meanwhile, from his words ("Your new intent"
    /// when there's none), so a Kerdr opened after it was sent can show it too (r4-a's shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A question it waits on, his to answer: its card shows it as asking him (r4-b's shape). One
    /// Kerdr can't use is passed over, and the entry shows its `pending` as any other does.
    #[serde(
        default,
        deserialize_with = "held_asks",
        skip_serializing_if = "Option::is_none"
    )]
    pub asks: Option<HeldAsks>,
}

/// A question a request its backend holds waits on (intent 0591, release 4, r4-b's shape): a
/// move for later that, when due, found a background job started since his yes, and won't end it
/// without asking. His yes, or a choice, sends `request` with `confirm` (and `choice`); his no
/// drops what's held.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct HeldAsks {
    pub message: String,
    pub confirm: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::actions::choices",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub choices: Vec<crate::actions::Choice>,
    /// The account request his answer sends.
    pub request: serde_json::Value,
}

/// `asks` as a backend gives it: only one with the whole question, its token, and an account
/// request to answer it with; anything else reads as none.
fn held_asks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<HeldAsks>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<HeldAsks>(value).ok().filter(|a| {
        !a.message.trim().is_empty()
            && !a.confirm.trim().is_empty()
            && a.request["action"]
                .as_str()
                .is_some_and(|action| action.starts_with("account-"))
    }))
}

/// A `held` list, its entries Kerdr can't read left out: one with no `op` or no words for the
/// card can't be shown or dropped. Anything that isn't a list reads as none.
pub fn held_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Held>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let mut out: Vec<Held> = vec![];
    for item in value.as_array().into_iter().flatten() {
        let Ok(held) = serde_json::from_value::<Held>(item.clone()) else {
            continue;
        };
        let pending: String = held
            .pending
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        if held.op.trim().is_empty()
            || pending.trim().is_empty()
            || out.iter().any(|h| h.op == held.op)
        {
            continue;
        }
        out.push(Held {
            pending: pending.trim().to_string(),
            ..held
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct OutsideHerdr {
    /// The terminal it runs in, as its process is named (`foot`, `kitty`, `sshd`).
    #[serde(default)]
    pub terminal: Option<String>,
    /// When its process started, as an ISO time.
    #[serde(default)]
    pub since: Option<String>,
}

impl OutsideHerdr {
    /// Where it runs, in a word or two for a card: "foot", "SSH", "terminal".
    pub fn short(&self) -> String {
        match self
            .terminal
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some("sshd") => "SSH".into(),
            Some(t) if t.starts_with("tmux") => "tmux".into(),
            Some("code") => "editor".into(),
            Some(t) => t.trim_end_matches('-').to_string(),
            None => "terminal".into(),
        }
    }

    /// Where it runs, for a sentence: "a foot window", "an SSH login", "a terminal window".
    pub fn place(&self) -> String {
        match self.short().as_str() {
            "SSH" => "an SSH login".into(),
            "editor" => "an editor window".into(),
            t => format!("a {t} window"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Resume {
    /// The agent that held it, such as `claude` or `codex`.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub conversation: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Home {
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct FeedWorkspace {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The herdr tab the work runs in, when it's one tab of a workspace other work shares (a
    /// helper in a tab of the workspace that started it). Enter lands on that tab.
    #[serde(default)]
    pub tab: Option<String>,
    /// The tab the card's own agent runs in, as its records say: Enter on the card lands there,
    /// never on a tab work inside the card runs in. `tab` keeps meaning work confined to one tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_tab: Option<String>,
    /// The conversations herdr shows running there, sorted (r4-a's point 13). Kerdr reads only
    /// whether a feed carries it: a machine whose feed does takes `conversations` on the cancel of
    /// a workspace with no intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversations: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct WaitsOn {
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub open: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Criterion {
    pub text: String,
    #[serde(default)]
    pub met: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct TimelineEntry {
    pub stage: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct LogEntry {
    #[serde(default)]
    pub date: Option<String>,
    pub text: String,
}

impl FeedCard {
    /// The handle drawn first on the card: the feed's `ref`, else `#` and the id without zeros.
    pub fn reference(&self) -> String {
        if let Some(r) = self.reference.as_deref().filter(|r| !r.trim().is_empty()) {
            return r.to_string();
        }
        let trimmed = self.id.trim_start_matches('0');
        if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
            self.id.clone()
        } else {
            format!("#{trimmed}")
        }
    }

    pub fn criteria_progress(&self) -> Option<(u32, u32)> {
        if self.criteria.is_empty() || self.criteria.iter().all(|c| c.met.is_none()) {
            return None;
        }
        let met = self.criteria.iter().filter(|c| c.met == Some(true)).count() as u32;
        Some((met, self.criteria.len() as u32))
    }
}

/// Why a feed's cards couldn't be read: plain words of Kerdr's own for the board, and what the feed
/// printed, as it printed it, for the error log alone (intent 0591).
#[derive(Clone, Debug, PartialEq)]
pub struct Unread {
    pub words: String,
    pub printed: Option<String>,
}

/// How much of what a feed printed instead of its cards goes to the error log.
const MOST_PRINTED: usize = 2048;

/// A feed's cards, or why they can't be read, in words for the board (intent 0591).
pub fn parse(text: &str) -> Result<Feed, String> {
    parse_keeping(text).map_err(|unread| unread.words)
}

/// Reads a feed, accepting only the version this Kerdr knows. What it printed in place of cards
/// never reaches the board: the words say what happened, and the error log gets what it printed.
pub fn parse_keeping(text: &str) -> Result<Feed, Unread> {
    let printed = || {
        let text = text.trim();
        let mut end = text.len().min(MOST_PRINTED);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(text[..end].to_string()).filter(|t| !t.is_empty())
    };
    let Some(start) = text.find('{') else {
        return Err(match printed() {
            None => Unread {
                words: "it printed no cards".into(),
                printed: None,
            },
            Some(said) => Unread {
                words: "it printed something other than its cards (what it printed is in Kerdr's error log)".into(),
                printed: Some(said),
            },
        });
    };
    let feed: Feed = serde_json::from_str(&text[start..]).map_err(|e| {
        if e.is_eof() {
            Unread {
                words: "its answer was cut off partway (most often the connection dropping)".into(),
                printed: None,
            }
        } else {
            Unread {
                words: "its cards couldn't be read (what it printed is in Kerdr's error log)"
                    .into(),
                printed: printed().map(|p| format!("{e}: {p}")),
            }
        }
    })?;
    if feed.kerdr_feed != 1 {
        return Err(Unread {
            words: format!(
                "its cards use feed version {}, and this Kerdr ({}) reads version 1: update Kerdr here, or restart it if it's already updated",
                feed.kerdr_feed,
                env!("CARGO_PKG_VERSION")
            ),
            printed: None,
        });
    }
    // It says itself that its cards are empty because it failed: the last good ones stand.
    if let Some(failed) = feed.read_failures.iter().find(|f| f.part == "feed") {
        return Err(Unread {
            words: failed.why.clone(),
            printed: None,
        });
    }
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_s_write_up_reason_is_optional() {
        let feed = parse(
            r#"{"kerdr_feed":1,"cards":[
                {"id":"1","title":"a","intake":"failed","intake_why":"no account was free"},
                {"id":"2","title":"b","intake":"failed"}]}"#,
        )
        .unwrap();
        assert_eq!(
            feed.cards[0].intake_why.as_deref(),
            Some("no account was free")
        );
        assert_eq!(feed.cards[1].intake_why, None);
    }

    #[test]
    fn a_card_says_what_its_backend_holds_for_it_and_its_rev() {
        // Intent 0591, release 4: what Kerdr can't read is left out, never a failed feed.
        let feed = parse(
            r#"{"kerdr_feed":1,"supports":["op","hold","forward"],"cards":[
                {"id":"41","title":"a","rev":"r-1a2b","held":[
                  {"op":"k-0000000000000001","action":"dispatch","pending":"queued in pick4me: starts when a workspace there frees up","since":1794400000},
                  {"op":"k-0000000000000002","action":"account-move","pending":"moves to\nARK_C at 02:00","since":1794400000,"until":1794430000},
                  {"op":"","action":"x","pending":"no op"},
                  {"op":"k-0000000000000003","action":"x"},
                  {"op":"k-0000000000000001","action":"dispatch","pending":"twice"},
                  "nonsense", 7]},
                {"id":"42","title":"b","held":"oops"},
                {"id":"43","title":"c"}],
              "agents":[{"pane":"w1:p1","session":"web","conversation":"c1","account":"KBZ_C",
                 "held":[{"op":"k-0000000000000004","action":"account-move","pending":"moves to ARK_C at 02:00","until":1794430000}]}]}"#,
        )
        .unwrap();
        assert!(feed.supports("hold") && feed.supports("forward"));
        let card = &feed.cards[0];
        assert_eq!(card.rev.as_deref(), Some("r-1a2b"));
        assert_eq!(card.held.len(), 2, "{:?}", card.held);
        assert_eq!(card.held[0].action, "dispatch");
        assert_eq!(card.held[1].pending, "moves to ARK_C at 02:00");
        assert_eq!(card.held[1].until, Some(1794430000));
        assert!(feed.cards[1].held.is_empty() && feed.cards[2].held.is_empty());
        assert_eq!(feed.cards[2].rev, None);
        assert_eq!(feed.agents[0].held[0].op, "k-0000000000000004");
        // An older feed says none of it.
        let old = parse(r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"a"}]}"#).unwrap();
        assert!(!old.supports("hold") && old.cards[0].held.is_empty());
        assert!(old.held.is_empty());
    }

    #[test]
    fn a_held_request_that_asks_carries_its_question_and_one_kerdr_cant_use_reads_as_none() {
        // r4-b's shape: the question, its token, and the account request that answers it.
        let feed = parse(
            r#"{"kerdr_feed":1,"cards":[],
              "agents":[{"pane":"w1:p1","session":"web","conversation":"c1","account":"KBZ_C",
                 "held":[
                  {"op":"k-0000000000000001","action":"account-move","pending":"waits for your answer",
                   "asks":{"message":"Move it now?","confirm":"t-1","yes":"move it now",
                           "choices":[{"id":"after-job","key":"j","label":"move it when the job ends"},"oops"],
                           "request":{"action":"account-move","pane":"w1:p1","to":"ARK_C"}}},
                  {"op":"k-0000000000000002","action":"account-move","pending":"no request",
                   "asks":{"message":"Move it now?","confirm":"t-2"}},
                  {"op":"k-0000000000000003","action":"account-move","pending":"not an account one",
                   "asks":{"message":"Move it now?","confirm":"t-3","request":{"action":"cancel"}}},
                  {"op":"k-0000000000000004","action":"account-move","pending":"no token",
                   "asks":{"message":"Move it now?","request":{"action":"account-move"}}},
                  {"op":"k-0000000000000005","action":"account-move","pending":"not a question",
                   "asks":"oops"}]}]}"#,
        )
        .unwrap();
        let held = &feed.agents[0].held;
        assert_eq!(
            held.len(),
            5,
            "an entry whose question can't be used still shows"
        );
        let asks = held[0].asks.as_ref().expect("its question");
        assert_eq!(
            (asks.message.as_str(), asks.confirm.as_str()),
            ("Move it now?", "t-1")
        );
        assert_eq!(asks.yes.as_deref(), Some("move it now"));
        assert_eq!(asks.no, None);
        assert_eq!(asks.choices.len(), 1);
        assert_eq!(asks.request["to"], "ARK_C");
        assert!(held[1..].iter().all(|h| h.asks.is_none()), "{held:?}");
    }

    #[test]
    fn a_feed_lists_the_saves_it_finishes_in_the_background_at_its_top() {
        let feed = parse(
            r#"{"kerdr_feed":1,"supports":["op","hold"],"cards":[],"held":[
                {"op":"k-0000000000000005","action":"create","pending":"saving: its number comes once the central copy answers","since":1794400000,"submission":"s-1"},
                {"op":"k-0000000000000006","action":"create"}, "nonsense"]}"#,
        )
        .unwrap();
        assert_eq!(feed.held.len(), 1, "{:?}", feed.held);
        assert_eq!(feed.held[0].submission.as_deref(), Some("s-1"));
        assert_eq!(feed.held[0].action, "create");
        // Anything else there is passed over, never a failed feed.
        let odd = parse(r#"{"kerdr_feed":1,"cards":[],"held":{"op":"x"}}"#).unwrap();
        assert!(odd.held.is_empty());
    }

    #[test]
    fn a_feed_says_what_it_takes_and_what_it_could_not_read() {
        let feed = parse(
            r#"{"kerdr_feed":1,"supports":["op","retry","follow"],"read_failures":[
                {"part":"herdr","why":"herdr on this machine didn't answer within a few seconds"},
                {"part":"session","session":"web","why":"herdr session web didn't list its tabs in time"},
                {"nonsense":true}, 7]}"#,
        )
        .unwrap();
        assert!(feed.supports("op") && !feed.supports("telepathy"));
        // What isn't that shape is passed over: never a failed feed.
        assert_eq!(feed.read_failures.len(), 2);
        assert_eq!(feed.read_failures[1].key(), "session web");
        assert!(
            parse(r#"{"kerdr_feed":1,"read_failures":"oops"}"#)
                .unwrap()
                .read_failures
                .is_empty()
        );
        // An older feed says nothing of either.
        let old = parse(r#"{"kerdr_feed":1}"#).unwrap();
        assert!(!old.supports("op") && old.read_failures.is_empty());
        // One that says it failed as a whole is a failed read: the last good cards stand.
        assert_eq!(
            parse(r#"{"kerdr_feed":1,"cards":[],"read_failures":[{"part":"feed","why":"ark-sdlc's card feed hit a bug"}]}"#)
                .unwrap_err(),
            "ark-sdlc's card feed hit a bug"
        );
    }

    #[test]
    fn a_read_failure_of_another_shape_is_passed_over_and_never_fails_the_feed() {
        // `session` goes only with `"part": "session"`, which needs one (docs/feed.md).
        let feed = parse(
            r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"a"}],"read_failures":[
                {"part":"feed","session":"web","why":"a feed failure naming a session"},
                {"part":"telepathy","session":"web","why":"a part nobody knows"},
                {"part":"session","why":"a session failure naming none"},
                {"part":"session","session":"  ","why":"a blank session"},
                {"part":"herdr","session":"web","why":"herdr naming a session"},
                {"part":"session","session":"api","why":""}]}"#,
        )
        .unwrap();
        assert_eq!(feed.cards.len(), 1, "the feed and its cards stand");
        assert_eq!(feed.read_failures.len(), 1, "{:?}", feed.read_failures);
        assert_eq!(feed.read_failures[0].key(), "session api");
        // An entry without words gets Kerdr's own, never an empty line on the board.
        assert_eq!(feed.read_failures[0].why, "herdr session api wasn't read");
        // A feed failure without words still fails the feed, in Kerdr's words.
        assert_eq!(
            parse(r#"{"kerdr_feed":1,"read_failures":[{"part":"feed","why":" "}]}"#).unwrap_err(),
            "it failed without saying why"
        );
        // A feed failure with a null session is the shape the docs give.
        assert!(
            parse(r#"{"kerdr_feed":1,"read_failures":[{"part":"feed","session":null,"why":"x"}]}"#)
                .is_err()
        );
    }

    #[test]
    fn what_a_feed_printed_instead_of_cards_never_reaches_the_board() {
        for printed in [
            "Traceback (most recent call last):\n  File \"/home/x/bin/ark-cards\", line 9\nKeyError: 'state'",
            "{\"kerdr_feed\":1,\"cards\":[{\"id\":7}]} ERR_SOMETHING at /srv/ark",
        ] {
            let unread = parse_keeping(printed).unwrap_err();
            for raw in [
                "Traceback",
                "KeyError",
                "/home/x",
                "ERR_SOMETHING",
                "/srv/ark",
                "line 1 column",
            ] {
                assert!(!unread.words.contains(raw), "{}", unread.words);
            }
            assert!(
                unread.words.contains("Kerdr's error log"),
                "{}",
                unread.words
            );
            let kept = unread.printed.unwrap();
            assert!(
                kept.contains("KeyError") || kept.contains("ERR_SOMETHING"),
                "{kept}"
            );
        }
        // One cut off partway says so, as before.
        assert!(
            parse(r#"{"kerdr_feed":1,"cards":["#)
                .unwrap_err()
                .contains("cut off partway")
        );
    }

    #[test]
    fn optional_account_fields_accept_future_providers_and_null_usage() {
        let f=parse(r#"{"kerdr_feed":1,"accounts":[{"code":"LOCAL","provider":"local","kind":"endpoint","status":"ready","usage":null}],"owners":[{"id":"studio","kind":"organization","parent":null,"models":["LOCAL"]}],"agents":[{"pane":"w1:p1","session":"web","account":"LOCAL","waiting":null}],"owners_source":"mock","machine_owner":"studio","unowned_sessions":["stray"]}"#).unwrap();
        // A provider the feed doesn't describe shows the windows a feed without providers has.
        assert_eq!(
            crate::accounts::numbers(
                &crate::accounts::shown_windows(f.accounts[0].usage.as_ref(), None),
                crate::accounts::Figures::Short,
                chrono::Local::now()
            ),
            "—│—"
        );
        assert_eq!(f.owners[0].models, ["LOCAL"]);
        assert!(!f.agents[0].waiting_for_usage());
        let old = parse(r#"{"kerdr_feed":1,"cards":[]}"#).unwrap();
        assert!(old.accounts.is_empty());
        assert!(old.agents.is_empty());
        let nullable = parse(r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"still visible"}],"accounts":[{"code":"KBZ","name":null}],"owners":[{"id":"shop","own":null,"models":null}],"agents":[{"conversation":null,"pane":null}],"unowned_sessions":null}"#).unwrap();
        assert_eq!(nullable.cards.len(), 1);
        assert!(nullable.accounts[0].name.is_empty());
        assert!(!nullable.owners[0].own);
        assert!(nullable.agents[0].conversation.is_empty());
        let empty =
            parse(r#"{"kerdr_feed":1,"accounts":null,"owners":null,"agents":null}"#).unwrap();
        assert!(empty.accounts.is_empty() && empty.owners.is_empty() && empty.agents.is_empty());
    }

    #[test]
    fn account_copies_read_with_their_states_and_nulls() {
        let f = parse(r#"{"kerdr_feed":1,"accounts":[{"code":"ARK","status":"ready","copies":[
            {"id":"agents","label":"Your agents","stored":true,"works":true,"checked":"2026-10-26T17:40:00Z","note":null},
            {"id":"tenants","label":"Tenants","stored":false,"works":null,"checked":null,"note":"held by root"},
            {"id":"spare","label":null,"stored":null,"works":false}]},
            {"code":"KBZ","copies":[{"id":"agents","stored":true,"works":true}]},
            {"code":"OLD","copies":null}]}"#)
        .unwrap();
        let ark = &f.accounts[0];
        assert_eq!(ark.shown_copies().len(), 3);
        let [agents, tenants, spare] = &ark.copies[..] else {
            panic!()
        };
        assert!(agents.works_label().starts_with("✓ works · "));
        assert_eq!(
            (tenants.stored_label(), tenants.works_label().as_str()),
            ("✕ not stored", "– not checked")
        );
        assert_eq!(tenants.note.as_deref(), Some("held by root"));
        assert_eq!(spare.name(), "spare");
        assert!(!spare.stored && spare.works == Some(false));
        assert_eq!(spare.works_label(), "✕ doesn't work");
        assert!(
            f.accounts[1].shown_copies().is_empty(),
            "one copy is today's display"
        );
        assert!(f.accounts[1].copy("agents").is_none());
        assert!(f.accounts[2].copies.is_empty());
    }

    #[test]
    fn reads_a_minimal_feed() {
        let feed =
            parse(r#"{"kerdr_feed":1,"cards":[{"id":"0143","title":"Find the next flywheel"}]}"#)
                .unwrap();
        assert_eq!(feed.cards.len(), 1);
        assert_eq!(feed.cards[0].reference(), "#143");
    }

    #[test]
    fn skips_noise_before_the_json() {
        let feed = parse("warning: something\n{\"kerdr_feed\":1}").unwrap();
        assert!(feed.cards.is_empty());
    }

    #[test]
    fn refuses_other_versions() {
        assert!(parse(r#"{"kerdr_feed":2}"#).is_err());
    }

    #[test]
    fn preserves_scope_order_and_distinct_creation_and_refresh_dates() {
        let feed = parse(r#"{"kerdr_feed":1,"cards":[{"id":"4","title":"t","tenant":"shop","organization":"org","project":"checkout","created":"2026-01-01","refreshed_at":"2026-10-22","backlog_rank":-2,"disposition":"kept"}]}"#).unwrap();
        let card = serde_json::to_value(&feed.cards[0]).unwrap();
        assert_eq!(card["tenant"], "shop");
        assert_eq!(card["organization"], "org");
        assert_eq!(card["project"], "checkout");
        assert_eq!(card["created"], "2026-01-01");
        assert_eq!(card["refreshed_at"], "2026-10-22");
        assert_eq!(card["backlog_rank"], -2);
        assert_eq!(card["disposition"], "kept");
    }

    #[test]
    fn reads_the_questions_a_start_would_ask() {
        let feed: Feed = serde_json::from_value(serde_json::json!({
            "kerdr_feed": 1,
            "cards": [{"id": "1", "title": "t", "kickoff": {"asked": "abc", "questions": [
                {"num": "1", "question": "Whose is it?", "pick": "Mine"}
            ]}}, {"id": "2", "title": "u"}],
        }))
        .unwrap();
        let k = feed.cards[0].kickoff.as_ref().unwrap();
        assert_eq!(
            (k.asked.as_str(), k.questions[0].pick.as_str()),
            ("abc", "Mine")
        );
        assert!(feed.cards[1].kickoff.is_none());
    }

    #[test]
    fn reads_the_conversation_a_reopen_resumes() {
        let feed = parse(r#"{"kerdr_feed":1,"cards":[{"id":"41","title":"t","stage":"deploy","resume":{"agent":"codex","conversation":"4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41","new":1}}]}"#).unwrap();
        let resume = feed.cards[0].resume.as_ref().unwrap();
        assert_eq!(resume.agent.as_deref(), Some("codex"));
        assert_eq!(
            resume.conversation.as_deref(),
            Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41")
        );
        assert!(
            parse(r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"t"}]}"#)
                .unwrap()
                .cards[0]
                .resume
                .is_none()
        );
    }

    #[test]
    fn reads_the_tab_a_cards_work_runs_in_when_it_names_one() {
        let feed = parse(r#"{"kerdr_feed":1,"cards":[
            {"id":"41","title":"t","workspace":{"session":"web","id":"w4","label":"41-checkout","tab":"w4:t2"}},
            {"id":"42","title":"t","workspace":{"session":"web","id":"w5","label":"42-search"}}]}"#)
        .unwrap();
        let tab = |i: usize| feed.cards[i].workspace.as_ref().unwrap().tab.clone();
        assert_eq!(tab(0).as_deref(), Some("w4:t2"));
        assert_eq!(tab(1), None);
    }

    #[test]
    fn progress_needs_at_least_one_known_criterion() {
        let mut card = FeedCard {
            id: "1".into(),
            title: "t".into(),
            criteria: vec![
                Criterion {
                    text: "a".into(),
                    met: None,
                },
                Criterion {
                    text: "b".into(),
                    met: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(card.criteria_progress(), None);
        card.criteria[0].met = Some(true);
        assert_eq!(card.criteria_progress(), Some((1, 2)));
    }

    #[test]
    fn teams_stage_lists_and_create_fields_are_optional_fields() {
        let f = parse(r#"{"kerdr_feed":1,
            "cards":[{"id":"5","title":"t","tenant":"tinyhost","team":"marketing","stage_list":"tinyhost/marketing","stage":"learn"}],
            "owners":[{"id":"tinyhost","kind":"tenant","parent":"arkship","agent_machine":"arknet-one",
                       "teams":[{"id":"dev","name":"Dev","retired":false,"stages":"standard"},{"id":"old","name":"Old","retired":true,"stages":null}]}],
            "stage_lists":[{"id":"tinyhost/marketing","name":"Marketing","owner":"tinyhost","rev":3,
                            "stages":[{"id":"plan","name":"Plan","retired":false},{"id":"learn","name":"Learn","retired":true}]}],
            "create_fields":["organization","tenant","project","team"]}"#).unwrap();
        assert_eq!(f.cards[0].team.as_deref(), Some("marketing"));
        assert_eq!(f.cards[0].stage_list.as_deref(), Some("tinyhost/marketing"));
        assert_eq!(f.owners[0].agent_machine.as_deref(), Some("arknet-one"));
        assert_eq!(f.owners[0].teams.len(), 2);
        assert!(f.owners[0].teams[1].retired && f.owners[0].teams[1].stages.is_none());
        assert_eq!(f.stage_lists[0].rev, 3);
        assert!(f.stage_lists[0].stages[1].retired);
        assert!(f.creates_with("team") && !f.creates_with("account"));
        // An older feed has none of them.
        let old = parse(r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"t"}],"owners":[{"id":"a","kind":"organization"}],"stage_lists":null,"create_fields":null}"#).unwrap();
        assert!(old.stage_lists.is_empty() && old.create_fields.is_empty());
        assert!(old.owners[0].teams.is_empty() && old.cards[0].team.is_none());
    }
}
