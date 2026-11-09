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

/// Reads a feed, accepting only the version this Kerdr knows.
pub fn parse(text: &str) -> Result<Feed, String> {
    let start = text.find('{').ok_or("the feed printed no JSON")?;
    let feed: Feed =
        serde_json::from_str(&text[start..]).map_err(|e| format!("the feed's JSON: {e}"))?;
    if feed.kerdr_feed != 1 {
        return Err(format!(
            "the feed is version {}; this Kerdr reads version 1",
            feed.kerdr_feed
        ));
    }
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
