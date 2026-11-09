//! The action resolution tool (intent 0416): every change to a card (start, reopen, close, won't
//! do, restore, refresh, cancel, move, a backlog item's new text, and a new intent) goes through
//! here instead of holding the board until its command answers. So does the read `e` sends for an
//! item's text (intent 0414), which isn't a change: nothing waits for it.
//!
//! A change shows on the board the moment it's asked for: its expected effect is laid over the
//! feeds the board is built from. Its command runs in the background, one at a time for each record
//! (so J J K reach the backend in order) and side by side for different records. When it succeeds
//! nothing more is said: the effect stays until the feed shows it (or two reads go by, or a minute),
//! then the feed alone speaks. When it fails, or the backend asks a question first, the effect goes
//! at once, so the card is back as it really is, and the app says so where it can't be missed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::board::MachineState;
use crate::feed::{Feed, FeedCard, Home};
use crate::model::CardSnapshot;

/// The intake a new intent's card has while its words are still being saved (intent 0445): no
/// record exists yet, so nothing may be done to it.
pub const SAVING: &str = "saving";

/// What a start that waited for its intent's write-up says when the write-up failed (intent 0445).
pub const NOT_WRITTEN_UP: &str = "it couldn't be written up from your words; d starts it as it is.";

/// What a card says while its change is sent again by itself (intent 0591).
pub const RETRYING: &str = "Trying again…";
/// What a new intent's card says while its words are sent again by itself.
pub const RETRYING_SAVE: &str = "Saving, trying again…";

/// How long a settled effect may stay without the feed showing it.
const SETTLED_MOST: Duration = Duration::from_secs(60);
/// How long a start asked for on another machine shows as starting there: a start over SSH may
/// take four minutes, and the requests pass there two (intent 0445).
const ELSEWHERE_MOST: Duration = Duration::from_secs(300);
/// How many of its machine's reads a settled effect waits for: the first may have started before
/// the change was saved.
const SETTLED_READS: u8 = 2;

/// What the board shows for a change while its command runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `dispatch`, unanswered: where it is, saying it's starting. It stays put, so a question the
    /// backend asks first still finds it where it was.
    Starting,
    /// `dispatch`, answered yes: out of the backlog, under way.
    Started,
    /// `dispatch`, answered yes by a start asked for on another machine (named), which takes
    /// minutes there: where it is, saying it's starting there, until it reaches this copy.
    StartingOn(String),
    /// `reopen`: where it is, saying so.
    Reopening,
    /// `close`: done.
    Close,
    /// `dismiss` and `cancel`: in Won't do.
    SetAside,
    /// `restore`: out of Won't do.
    Restore,
    /// `refresh`: confirmed wanted today (the date, as the backend writes it).
    Refresh(String),
    /// `move`: the backlog's new order for the records it reorders, by card key, as the backend
    /// writes it (0, 1, 2… within the moved card's group).
    Order(Vec<(String, i64)>),
    /// `create`: a card for the new intent in its machine's backlog, saying it's being saved, until
    /// the feed has the record the answer names (intent 0445).
    Saving(Placeholder),
    /// `edit` with a new text (intent 0414): the new words under "How it started", and being
    /// written up again (✎), as the backend marks it while it redoes the write-up.
    Text(String),
}

/// The card a new intent shows as while it's saved: Andrew's first words, in the backlog of the
/// machine and session they went to.
#[derive(Clone, Debug, PartialEq)]
pub struct Placeholder {
    /// Its id in that machine's feed, `saving-<submission>`: never a record's, which are numbers.
    pub id: String,
    pub title: String,
    pub machine: String,
    pub session: String,
    /// The source of the store it's saved in (its machine's feed's), where `made` is a record.
    pub source: String,
    /// The record the save made, once its answer names it: the card goes once the feed has it.
    pub made: Option<String>,
    /// Who it was filed under (intent 0595), so it shows under that owner and team while it saves.
    pub organization: Option<String>,
    pub tenant: Option<String>,
    pub team: Option<String>,
}

impl Placeholder {
    /// The id its card has for a submission.
    pub fn id_for(submission: &str) -> String {
        format!("saving-{submission}")
    }

    /// Its title: the start of his words, on one line.
    pub fn title_for(words: &str) -> String {
        let line = words.split_whitespace().collect::<Vec<_>>().join(" ");
        match line.char_indices().nth(72) {
            Some((at, _)) => format!("{}…", line[..at].trim_end()),
            None => line,
        }
    }

    /// The submission it was sent with.
    pub fn submission(&self) -> &str {
        self.id.strip_prefix("saving-").unwrap_or(&self.id)
    }

    /// Whether this feed, any machine's, has the record the save made: one that carries its
    /// submission, which the record's header has from the moment it's written, before the answer
    /// comes; or the record the answer named, in a copy of the store it went to (intent 0445).
    pub fn made_in(&self, feed: &Feed) -> bool {
        let source = feed.source.as_deref().unwrap_or("feed");
        let submission = self.submission();
        feed.cards.iter().any(|c| {
            c.submission.as_deref() == Some(submission)
                || (source == self.source && self.made.as_ref().is_some_and(|id| &c.id == id))
        })
    }

    /// Its card. Saved a while ago and still not read (`late`), it says so rather than going.
    fn card(&self, late: bool) -> FeedCard {
        let status = match (&self.made, late) {
            (Some(id), true) => format!(
                "Saved as #{}; not on the board yet",
                id.trim_start_matches('0')
            ),
            _ => "Saving…".into(),
        };
        FeedCard {
            id: self.id.clone(),
            reference: Some("new".into()),
            title: self.title.clone(),
            stage: Some("backlog".into()),
            state: Some("queued".into()),
            intake: Some(SAVING.into()),
            status: Some(status),
            home: Some(Home {
                machine: Some(self.machine.clone()),
                session: Some(self.session.clone()),
            }),
            created: Some(chrono::Local::now().date_naive().to_string()),
            stale_days: Some(0),
            organization: self.organization.clone(),
            tenant: self.tenant.clone(),
            team: self.team.clone(),
            ..Default::default()
        }
    }
}

impl Effect {
    /// Lays this effect over one feed card, its record's copy on one machine.
    fn apply(&self, fc: &mut FeedCard, key: &str) {
        match self {
            Effect::Starting => {
                fc.status = Some("Starting…".into());
                fc.no_agent = false;
            }
            Effect::StartingOn(machine) => {
                // A copy that already shows it under way speaks for itself (intent 0445).
                let state = fc.state.as_deref().unwrap_or("active");
                let open = fc.workspace.as_ref().is_some_and(|w| w.id.is_some());
                if matches!(state, "queued" | "shelved" | "none") && !open {
                    fc.status = Some(format!("Starting on {machine}…"));
                    fc.no_agent = false;
                }
            }
            Effect::Started => {
                fc.state = Some("active".into());
                let own_list = fc
                    .stage_list
                    .as_deref()
                    .is_some_and(|l| !l.is_empty() && l != crate::model::STANDARD);
                let said = fc.stage.as_deref().map(crate::model::Stage::parse);
                if own_list {
                    // On a team's own list the backend's column comes with the next read; until
                    // then the list's first stage, where a card with none goes (intent 0595).
                    if matches!(
                        said,
                        None | Some(Some(
                            crate::model::Stage::Backlog | crate::model::Stage::Maintain
                        ))
                    ) {
                        fc.stage = None;
                    }
                } else if !said.flatten().is_some_and(crate::model::Stage::is_working) {
                    // The backend's column for it comes with the next read; build is where most
                    // started work goes (a guess the feed corrects in seconds).
                    fc.stage = Some("build".into());
                }
                fc.status = Some("Starting…".into());
                fc.no_agent = false;
            }
            // Where it is, saying so: done work stays in Maintain, but no longer reads done, and
            // work under way no longer reads what it waited on before (intent 0445).
            Effect::Reopening => {
                if fc.state.as_deref() == Some("closed") {
                    fc.state = Some("active".into());
                }
                fc.attention = Some("running".into());
                fc.needs = None;
                fc.blockers.clear();
                fc.status = Some("Reopening…".into());
                fc.no_agent = false;
            }
            Effect::Close => {
                fc.state = Some("closed".into());
                fc.stage = Some("maintain".into());
                fc.attention = Some("done".into());
                fc.status = Some("Done".into());
                fc.no_agent = false;
                if fc.closed.is_none() {
                    fc.closed = Some(chrono::Local::now().date_naive().to_string());
                }
            }
            Effect::SetAside => fc.disposition = Some("wont_do".into()),
            Effect::Restore => fc.disposition = None,
            Effect::Refresh(date) => {
                fc.refreshed_at = Some(date.clone());
                fc.stale_days = Some(0);
            }
            Effect::Order(order) => {
                if let Some((_, rank)) = order.iter().find(|(k, _)| k == key) {
                    fc.backlog_rank = Some(*rank);
                }
            }
            // Its card is added, not changed: Resolver::overlay.
            Effect::Saving(_) => {}
            Effect::Text(text) => {
                fc.words = Some(text.clone());
                fc.intake = Some("pending".into());
                // What the feed read ahead is the text this replaces.
                fc.item_text = None;
                fc.item_was = None;
            }
        }
    }

    /// Whether a fresh read of the record already says what this effect showed.
    fn shown(&self, fc: &FeedCard, feed: &Feed) -> bool {
        let state = fc.state.as_deref().unwrap_or("active");
        match self {
            // Only a record that says so: a feed with no state, or a read begun before the start,
            // mustn't let a second start through.
            Effect::Starting | Effect::Started | Effect::StartingOn(_) => {
                fc.state
                    .as_deref()
                    .is_some_and(|s| !matches!(s, "queued" | "shelved" | "none"))
                    || fc.workspace.as_ref().is_some_and(|w| w.id.is_some())
            }
            Effect::Reopening => fc.workspace.as_ref().is_some_and(|w| w.id.is_some()),
            Effect::Close => state == "closed",
            Effect::SetAside => fc.disposition.as_deref() == Some("wont_do"),
            Effect::Restore => fc.disposition.as_deref() != Some("wont_do"),
            Effect::Refresh(date) => fc.refreshed_at.as_deref() == Some(date.as_str()),
            Effect::Order(order) => {
                let source = feed.source.as_deref().unwrap_or("feed");
                let rank = |k: &str| {
                    feed.cards
                        .iter()
                        .find(|c| format!("{source}:{}", c.id) == k)
                        .map(|c| c.backlog_rank)
                };
                order
                    .iter()
                    .all(|(k, r)| rank(k).is_none_or(|got| got == Some(*r)))
            }
            Effect::Saving(p) => p.made_in(feed),
            Effect::Text(text) => fc.words.as_deref().is_some_and(|w| same_words(w, text)),
        }
    }
}

/// Whether a feed's words for a record are this text: the same words, whatever the spacing. Words
/// the feed cut short with "…" (ark-sdlc's feed keeps 200 characters) prove nothing, since the old
/// text may start the same way: the effect then goes as any other does, after two reads or a
/// minute.
fn same_words(feed: &str, text: &str) -> bool {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let feed = words(feed);
    !feed.ends_with('…') && feed == words(text)
}

/// What the runner starting a change means for the board (Resolver::started).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Started {
    /// A start that waited for its write-up: it shows from now.
    pub shows_now: bool,
    /// Esc dropped it a moment after its write-up was read and it began: it goes on.
    pub too_late: bool,
}

/// One change on its way to the backend.
#[derive(Clone, Debug)]
pub struct Job {
    pub n: u64,
    pub machine: usize,
    pub request: Value,
    /// The card it was asked on, as it was: a question it brings back is asked only while you're
    /// still on that card.
    pub from: Option<CardSnapshot>,
    /// The card it changes, by key, and what the board shows meanwhile.
    pub key: Option<String>,
    pub effect: Option<Effect>,
    pub asked: Instant,
    /// Handed to the runner, which starts it in its turn (intent 0445).
    pub sent: bool,
    /// When its command started; None while it waits behind another change to the same record.
    pub running: Option<Instant>,
    /// For a start or reopen: the workspaces its machine had when it was asked, so one that opens
    /// for it meanwhile can be told apart (intent 0445).
    pub before: Vec<(String, String)>,
    /// Succeeded: when, and how many of its machine's reads have come since.
    pub settled: Option<(Instant, u8)>,
    /// A start d asked for while its intent was still being written up: the runner holds it until
    /// its machine's feed shows the write-up done, wherever Andrew is meanwhile (intent 0445).
    pub gated: bool,
    /// Andrew has gone into the workspace it opened himself, so its answer takes him nowhere.
    pub entered: bool,
    /// Its whole request is in its command's hands (written to the adapter's input), so it
    /// carries on if Kerdr quits (review of intent 0445).
    pub handed: bool,
    /// A start waiting for its write-up that Esc dropped: gone once the runner says it never
    /// started, or shown again if it already had (review of intent 0445).
    pub dropping: bool,
    /// A start or reopen that can only come back as a question, or wait in a queue: nothing has
    /// started, so it shows nothing on its card and says so in words (intent 0491).
    pub asking: bool,
    /// What a second key last said about it while it asks, which its answer takes down.
    pub said: Option<String>,
    /// Answered with `follow` (intent 0591): the work goes on in a process of its own on that
    /// machine, and Kerdr follows it. What it's doing now, in words for its card, and how long it
    /// may stay shown without the feed showing it: the follow's own limit and a margin.
    pub follow: Option<(String, Duration)>,
    /// Asked for by ⏎ on its card, not by a key that only starts it (d): a start ⏎ asked for goes
    /// in when the followed work is done, as a reopen does (intent 0591).
    pub by_enter: bool,
    /// Being sent again by itself, with the same op: refused for something passing, or its answer
    /// lost (intent 0591). Its card says so quietly meanwhile, not in red.
    pub retrying: bool,
}

impl Job {
    fn record(&self) -> Option<&str> {
        self.request["id"].as_str()
    }

    /// Whether it starts work: a second change to the record waits for the start's answer.
    pub fn starts(&self) -> bool {
        matches!(self.request["action"].as_str(), Some("dispatch" | "reopen"))
    }

    /// Whether it reorders a backlog: moves on one machine reach it one at a time, since each
    /// is shown, and saved, as a change to the order the one before left.
    fn moves(&self) -> bool {
        self.request["action"] == "move"
    }

    /// Whether it only reads: `edit` without a text, for the editor `e` opens (intent 0414), or
    /// `kickoff`, checking a box already open (intent 0491). It waits for the changes to its record made before it, but it isn't a change: nothing waits
    /// for it, and q neither waits for it nor counts it as running.
    pub fn reads(&self) -> bool {
        reads(&self.request)
    }

    /// How long its effect may stay, settled, without the feed showing it. A new intent's card
    /// stays until its record is read, however long that takes: its card says so once it's late
    /// (review of intent 0445).
    fn life(&self) -> Duration {
        if let Some((_, life)) = &self.follow {
            return *life;
        }
        match self.effect {
            Some(Effect::StartingOn(_)) => ELSEWHERE_MOST,
            Some(Effect::Saving(_)) => Duration::MAX,
            _ => SETTLED_MOST,
        }
    }

    /// Whether only the feed showing it retires it, not a count of reads.
    fn waits_for_feed(&self) -> bool {
        self.follow.is_some()
            || matches!(self.effect, Some(Effect::StartingOn(_) | Effect::Saving(_)))
    }

    /// The record it changes, by id.
    pub fn id(&self) -> Option<&str> {
        self.record()
    }

    /// Whether it's a start still waiting for its intent's write-up: not started, nor shown.
    pub fn waits_for_write_up(&self) -> bool {
        self.gated && self.running.is_none() && self.settled.is_none()
    }

    /// Whether any machine's read may show it: a start that ran on another machine reaches
    /// copies there first, and a new intent may reach any copy of its store first.
    fn shows_anywhere(&self) -> bool {
        matches!(self.effect, Some(Effect::StartingOn(_) | Effect::Saving(_)))
    }
}

/// The order changes run in, kept by the runner away from the board's thread, so a change whose
/// turn comes while Andrew is inside a workspace starts then (intent 0445): each record's changes
/// one at a time, a machine's moves one at a time, everything else side by side. A record is its
/// card, wherever a refusal sent its change, else its id on one machine (intent 0416).
#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<Queued>,
    /// Saves of an item's text just answered, each with how many more reads of its machine to
    /// pass over: its write-up starts again, and a read begun before the save landed would say
    /// it's done (intent 0414).
    rewritten: Vec<(Queued, u8)>,
}

/// Whether a request only reads: `edit` without a text (intent 0414), or the check behind a Kickoff
/// Questions box already open (intent 0491).
fn reads(request: &Value) -> bool {
    request["action"] == "kickoff" || (request["action"] == "edit" && request.get("text").is_none())
}

#[derive(Debug)]
struct Queued {
    n: u64,
    machine: usize,
    record: Option<String>,
    key: Option<String>,
    moves: bool,
    /// A read of an item's text (intent 0414): it waits its turn, but nothing waits for it.
    reads: bool,
    /// A save of an item's text, after which the backend writes it up again (intent 0414).
    writes: bool,
    started: bool,
    /// Waiting for its record's write-up (a start d asked for while it was written up).
    gated: bool,
}

impl Queued {
    /// Whether `other` changes the same record.
    fn same(&self, other: &Queued) -> bool {
        (self.key.is_some() && other.key == self.key)
            || (self.record.is_some()
                && other.record == self.record
                && other.machine == self.machine)
    }
}

impl Queue {
    /// Takes a change in its turn. One already here, refused and sent on to another machine, goes
    /// back where it was, to start again there before anything behind it for its record.
    pub fn push(&mut self, n: u64, machine: usize, key: Option<String>, request: &Value) {
        let item = Queued {
            n,
            machine,
            record: request["id"].as_str().map(str::to_string),
            key,
            moves: request["action"] == "move",
            reads: reads(request),
            writes: request["action"] == "edit" && request.get("text").is_some(),
            started: false,
            gated: false,
        };
        match self.items.iter_mut().find(|j| j.n == n) {
            Some(held) => *held = item,
            None => self.items.push(item),
        }
    }

    /// A start that waits for its record's write-up before it may start, as `written` finds it.
    pub fn push_gated(&mut self, n: u64, machine: usize, key: Option<String>, request: &Value) {
        self.push(n, machine, key, request);
        if let Some(item) = self.items.iter_mut().find(|j| j.n == n) {
            item.gated = item.record.is_some();
        }
    }

    /// A read of `machine`'s feed: each record there by id, with its intake. Starts waiting for a
    /// record it shows written up may start; ones whose write-up failed are taken out, and
    /// returned, since starting an intent nobody wrote up isn't what d was pressed for. A start
    /// behind a save of its record's text, or just after one, isn't judged by a read from before
    /// that save landed: its write-up starts again then (intent 0414).
    pub fn written(
        &mut self,
        machine: usize,
        intakes: &HashMap<String, Option<String>>,
    ) -> Vec<u64> {
        let mut failed = vec![];
        for i in 0..self.items.len() {
            let q = &self.items[i];
            if !q.gated || q.machine != machine {
                continue;
            }
            let rewriting = self.items[..i].iter().any(|j| j.writes && q.same(j))
                || self.rewritten.iter().any(|(j, _)| q.same(j));
            if rewriting {
                continue;
            }
            match q.record.as_ref().and_then(|r| intakes.get(r)) {
                Some(None) => self.items[i].gated = false,
                Some(Some(intake)) if intake == "failed" => failed.push(q.n),
                _ => {}
            }
        }
        self.items.retain(|q| !failed.contains(&q.n));
        failed
    }

    /// Takes out of a read what may be from before a text was saved: kept, it would judge a start
    /// pressed before the next read comes (intent 0414).
    pub fn forget_rewriting(&self, machine: usize, intakes: &mut HashMap<String, Option<String>>) {
        let saves = self.items.iter().filter(|j| j.writes);
        for j in saves.chain(self.rewritten.iter().map(|(j, _)| j)) {
            if let (true, Some(record)) = (j.machine == machine, &j.record) {
                intakes.remove(record);
            }
        }
    }

    /// A read of `machine`'s feed has been taken in (Queue::written): saves of a text answered
    /// before it began speak for themselves from the next read on (intent 0414).
    pub fn read_passed(&mut self, machine: usize) {
        for (j, left) in &mut self.rewritten {
            if j.machine == machine {
                *left = left.saturating_sub(1);
            }
        }
        self.rewritten.retain(|(_, left)| *left > 0);
    }

    /// Takes out a change that hasn't started (a start Esc dropped). Whether it was still there
    /// unstarted.
    pub fn drop_unstarted(&mut self, n: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|q| q.n != n || q.started);
        self.items.len() != before
    }

    /// The changes that can start now, marked started: each record's oldest one, once nothing
    /// ahead of it for that record (or, for a move, that machine's backlog) is still running or
    /// held, and nothing behind it that it would clash with is running: a change refused and sent
    /// on goes back in its place, but a later move may already be running where it now goes.
    /// A request with no record (a new intent) starts straight away, and a read holds nothing up
    /// (intent 0414).
    pub fn startable(&mut self) -> Vec<u64> {
        let mut out = vec![];
        for i in 0..self.items.len() {
            let job = &self.items[i];
            if job.started || job.gated {
                continue;
            }
            let clashes = |j: &Queued| {
                !j.reads && (job.same(j) || (job.moves && j.moves && j.machine == job.machine))
            };
            let ahead = self.items[..i].iter().any(clashes);
            let running_behind = self.items[i + 1..].iter().any(|j| j.started && clashes(j));
            if ahead || running_behind {
                continue;
            }
            self.items[i].started = true;
            out.push(self.items[i].n);
        }
        out
    }

    /// A change's command has answered and it's finished with: the next for its record may start.
    /// One whose refusal may send it on stays held until the board says (Resolver::sent_on).
    pub fn done(&mut self, n: u64) {
        if let Some(i) = self.items.iter().position(|j| j.n == n) {
            let item = self.items.remove(i);
            // The read that comes next may have begun before it landed (intent 0414).
            if item.writes {
                self.rewritten.push((item, 1));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Resolver {
    jobs: Vec<Job>,
    next: u64,
    /// Changes the runner holds that were dropped before they started: it's told on its next pass.
    dropped: Vec<u64>,
}

impl Resolver {
    /// Takes a change: it shows at once, and runs once nothing else for its record is running.
    /// One that can change something gets an op of its own (intent 0591): every key Andrew
    /// presses is a new request, and only Kerdr's own resends of it keep its op.
    pub fn add(
        &mut self,
        machine: usize,
        request: Value,
        from: Option<CardSnapshot>,
        key: Option<String>,
        effect: Option<Effect>,
    ) -> u64 {
        let request = crate::actions::with_op(request);
        self.next += 1;
        self.jobs.push(Job {
            n: self.next,
            machine,
            request,
            from,
            key,
            effect,
            asked: Instant::now(),
            sent: false,
            running: None,
            before: vec![],
            settled: None,
            gated: false,
            entered: false,
            handed: false,
            dropping: false,
            asking: false,
            said: None,
            follow: None,
            by_enter: false,
            retrying: false,
        });
        self.next
    }

    /// This change is being sent again by itself (Job::retrying), or, false, has been answered.
    pub fn note_retrying(&mut self, n: u64, retrying: bool) -> bool {
        match self.jobs.iter_mut().find(|j| j.n == n) {
            Some(job) if job.retrying != retrying => {
                job.retrying = retrying;
                true
            }
            _ => false,
        }
    }

    /// This change shows nothing on its card because nothing has started (Job::asking).
    pub fn mark_asking(&mut self, n: u64) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.n == n) {
            job.asking = true;
        }
    }

    /// Notes what a key has just said about this job, for its answer to take down.
    pub fn note_said(&mut self, n: u64, line: String) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.n == n) {
            job.said = Some(line);
        }
    }

    /// Takes a start d asked for while its intent is still being written up: it shows nothing,
    /// and the runner starts it once its machine's feed shows the write-up done (intent 0445).
    pub fn add_gated(
        &mut self,
        machine: usize,
        request: Value,
        from: Option<CardSnapshot>,
        key: Option<String>,
        effect: Option<Effect>,
    ) -> u64 {
        let n = self.add(machine, request, from, key, effect);
        if let Some(j) = self.jobs.last_mut() {
            j.gated = true;
        }
        n
    }

    /// The changes not yet handed to the runner, marked handed, each with whether it waits for a
    /// write-up: it starts each in its turn (Queue), so this needs nothing from the board's thread
    /// later (intent 0445).
    pub fn ready(&mut self) -> Vec<(u64, usize, Value, bool)> {
        self.jobs
            .iter_mut()
            .filter(|j| !j.sent && j.settled.is_none())
            .map(|j| {
                j.sent = true;
                (j.n, j.machine, j.request.clone(), j.gated)
            })
            .collect()
    }

    /// The card a change is to, by key: the runner keeps a record's changes in order by it, wherever
    /// a refusal sends one (intent 0416).
    pub fn key_of(&self, n: u64) -> Option<String> {
        self.jobs
            .iter()
            .find(|j| j.n == n)
            .and_then(|j| j.key.clone())
    }

    /// Whether this change, answered, was sent on to another machine and waits to be handed to the
    /// runner again: its place in the runner's queue stays held until then.
    pub fn sent_on(&self, n: u64) -> bool {
        self.jobs
            .iter()
            .any(|j| j.n == n && !j.sent && j.settled.is_none())
    }

    /// The runner has started this change's command. Whether it's a start that waited for its
    /// write-up, which shows only from now, and whether Esc had dropped it a moment too late.
    pub fn started(&mut self, n: u64) -> Started {
        match self.jobs.iter_mut().find(|j| j.n == n) {
            Some(j) => {
                let shows_now = j.waits_for_write_up() && j.effect.is_some();
                let too_late = j.dropping;
                j.dropping = false;
                j.running.get_or_insert_with(Instant::now);
                Started {
                    shows_now,
                    too_late,
                }
            }
            None => Started::default(),
        }
    }

    /// A change by its number.
    pub fn job(&self, n: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.n == n)
    }

    /// Its whole request has been written to its command, which carries on without Kerdr.
    pub fn handed(&mut self, n: u64) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.n == n) {
            j.handed = true;
        }
    }

    /// The start waiting for this card's write-up, if one is.
    pub fn waiting_for_write_up(&self, key: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| j.waits_for_write_up() && !j.dropping && j.key.as_deref() == Some(key))
    }

    /// Every start waiting for a write-up.
    pub fn write_ups_awaited(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.waits_for_write_up() && !j.dropping)
            .collect()
    }

    /// Esc on the board: starts waiting for their write-up won't start after all. The cards they
    /// were for, by key. Each stays, out of sight, until the runner says whether it took it out
    /// before it started (Resolver::drop_answered): the write-up may have been read, and the start
    /// begun, a moment before Esc.
    pub fn drop_awaited(&mut self) -> Vec<String> {
        let mut keys = vec![];
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| j.waits_for_write_up() && !j.dropping)
        {
            j.dropping = true;
            self.dropped.push(j.n);
            keys.extend(j.key.clone());
        }
        keys
    }

    /// The runner's answer to a drop: taken out before it started, it goes; otherwise it had
    /// started already (Resolver::started showed it again) and its answer still comes.
    pub fn drop_answered(&mut self, n: u64, dropped: bool) {
        if dropped {
            self.jobs.retain(|j| j.n != n);
        } else if let Some(j) = self.jobs.iter_mut().find(|j| j.n == n) {
            j.dropping = false;
        }
    }

    /// The changes dropped since the runner was last told.
    pub fn take_dropped(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dropped)
    }

    /// Andrew has gone into this record's workspace himself while its start or reopen is on its
    /// way: its answer neither asks him anything nor takes him in again (intent 0445).
    pub fn went_in(&mut self, id: &str) {
        for j in self.jobs.iter_mut().filter(|j| {
            j.settled.is_none() && j.starts() && !j.waits_for_write_up() && j.record() == Some(id)
        }) {
            j.from = None;
            j.entered = true;
        }
    }

    /// Notes the workspaces a start's machine had when it was asked.
    pub fn note_workspaces(&mut self, n: u64, before: Vec<(String, String)>) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.n == n) {
            j.before = before;
        }
    }

    /// The unanswered new intent with this submission, so its answer finds its card.
    pub fn saving(&self, submission: &str) -> Option<&Job> {
        let id = Placeholder::id_for(submission);
        self.jobs.iter().find(|j| {
            j.settled.is_none() && matches!(&j.effect, Some(Effect::Saving(p)) if p.id == id)
        })
    }

    /// Changes Kerdr mustn't quit before: ones whose whole request isn't yet in their command's
    /// hands (quitting would stop them unsent, or half sent), and new intents until they're
    /// answered (a failed save gives their words back, which only Kerdr keeps). A start or another
    /// change its command has whole carries on without Kerdr.
    pub fn holding_quit(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| {
                j.settled.is_none()
                    && !j.waits_for_write_up()
                    && !j.reads()
                    && (!j.handed || j.request["action"] == "create")
            })
            .collect()
    }

    /// Unanswered changes whose commands have their whole request and carry on without Kerdr. A
    /// read isn't one: quitting loses nothing but its answer.
    pub fn running(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.settled.is_none() && j.handed && !j.reads())
            .collect()
    }

    /// Takes a change out as its answer arrives: a failure's effect goes with it.
    /// The number of the change added last.
    pub fn last_added(&self) -> u64 {
        self.next
    }

    pub fn take(&mut self, n: u64) -> Option<Job> {
        let i = self.jobs.iter().position(|j| j.n == n)?;
        Some(self.jobs.remove(i))
    }

    /// Keeps a succeeded change's effect until the feed shows it.
    pub fn settle(&mut self, mut job: Job) {
        if job.effect.is_none() {
            return;
        }
        job.settled = Some((Instant::now(), 0));
        if job.effect == Some(Effect::Starting) {
            job.effect = Some(Effect::Started);
        }
        // Back in its place: effects are laid over the board in the order they were made.
        let at = self.jobs.partition_point(|j| j.n < job.n);
        self.jobs.insert(at, job);
    }

    /// Keeps a change answered with `follow` showing while Kerdr follows it, saying `say` on its
    /// card, for at most `life` unless the feed shows it first (intent 0591). A start stays where
    /// it is, saying so, until the follow says it has started.
    pub fn settle_following(&mut self, mut job: Job, say: String, life: Duration) {
        job.settled = Some((Instant::now(), 0));
        job.follow = Some((say, life));
        let at = self.jobs.partition_point(|j| j.n < job.n);
        self.jobs.insert(at, job);
    }

    /// Marks a change as asked for by ⏎ (Job::by_enter).
    pub fn mark_by_enter(&mut self, n: u64) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.n == n) {
            job.by_enter = true;
        }
    }

    /// What a followed change is doing now, for its card. Whether it's still shown.
    pub fn follow_say(&mut self, n: u64, say: &str) -> bool {
        match self.jobs.iter_mut().find(|j| j.n == n) {
            Some(Job {
                follow: Some((words, _)),
                ..
            }) => {
                *words = say.to_string();
                true
            }
            _ => false,
        }
    }

    /// A followed change has ended. Done, it shows as any change answered yes does from now, until
    /// the feed has it; otherwise it goes, and its card is as the feed says. Whether it was shown.
    pub fn end_follow(&mut self, n: u64, done: bool) -> bool {
        let Some(i) = self
            .jobs
            .iter()
            .position(|j| j.n == n && j.follow.is_some())
        else {
            return false;
        };
        if !done {
            self.jobs.remove(i);
            return true;
        }
        let job = &mut self.jobs[i];
        job.follow = None;
        job.settled = Some((Instant::now(), 0));
        if job.effect == Some(Effect::Starting) {
            job.effect = Some(Effect::Started);
        }
        true
    }

    /// A change refused where it was asked, sent on to the machine that refusal named: it goes
    /// back in its place, unanswered, so nothing made after it for its record overtakes it.
    pub fn retarget(
        &mut self,
        mut job: Job,
        machine: usize,
        request: Value,
        from: Option<CardSnapshot>,
    ) {
        job.machine = machine;
        job.request = request;
        job.from = from;
        // Handed to the runner again, which still holds its place (intent 0445); a start that
        // waited for its write-up has had it.
        job.sent = false;
        job.gated = false;
        job.running = None;
        job.handed = false;
        job.settled = None;
        let at = self.jobs.partition_point(|j| j.n < job.n);
        self.jobs.insert(at, job);
    }

    /// A move on `machine` failed: the moves made after it showed orders that counted on it, so
    /// they show nothing more; the feed shows what they did once it has it.
    pub fn forget_moves_after(&mut self, machine: usize, n: u64) {
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| j.n > n && j.machine == machine && j.moves())
        {
            j.effect = None;
        }
        self.jobs
            .retain(|j| j.settled.is_none() || j.effect.is_some());
    }

    /// A read of `machine` came in: settled effects it shows, or that have waited long enough, go.
    /// Whether any went.
    pub fn read(&mut self, machine: usize, feed: Option<&Feed>) -> bool {
        let before = self.jobs.len();
        let shown = |j: &Job| {
            feed.is_some_and(|feed| {
                let (Some(key), Some(effect)) = (&j.key, &j.effect) else {
                    return true;
                };
                if matches!(effect, Effect::Saving(_)) {
                    return effect.shown(&FeedCard::default(), feed);
                }
                let source = feed.source.as_deref().unwrap_or("feed");
                feed.cards
                    .iter()
                    .find(|c| &format!("{source}:{}", c.id) == key)
                    .is_some_and(|c| effect.shown(c, feed))
            })
        };
        // A later change to a record the feed shows speaks for the ones before it: a dismiss then
        // a restore, both saved, mustn't leave the dismiss up once the restore is read.
        // A move reorders its whole group: the feed showing a later move's order speaks for
        // every move before it on that machine, whatever card it moved.
        let latest_order = self
            .jobs
            .iter()
            .filter(|j| j.settled.is_some() && j.machine == machine && j.moves() && shown(j))
            .map(|j| j.n)
            .max();
        let latest: Vec<(String, u64)> = self
            .jobs
            .iter()
            .filter(|j| j.settled.is_some() && j.machine == machine && shown(j))
            .filter_map(|j| j.key.clone().map(|k| (k, j.n)))
            .collect();
        // A move the feed shows carries its whole group's order, so it speaks for every earlier
        // move that reordered any of the same cards: J on one card then J on its neighbour,
        // both read, mustn't put the first order back (intent 0445).
        let orders: Vec<(u64, Vec<String>)> = self
            .jobs
            .iter()
            .filter(|j| j.settled.is_some() && j.machine == machine && shown(j))
            .filter_map(|j| match &j.effect {
                Some(Effect::Order(o)) => Some((j.n, o.iter().map(|(k, _)| k.clone()).collect())),
                _ => None,
            })
            .collect();
        self.jobs.retain_mut(|j| {
            let life = j.life();
            let elsewhere = j.waits_for_feed();
            let reordered = match &j.effect {
                Some(Effect::Order(o)) => orders
                    .iter()
                    .any(|(n, keys)| *n > j.n && o.iter().any(|(k, _)| keys.contains(k))),
                _ => false,
            };
            let superseded = reordered
                || latest
                    .iter()
                    .any(|(k, n)| Some(k) == j.key.as_ref() && *n > j.n)
                || (j.moves() && latest_order.is_some_and(|n| n > j.n));
            let here = j.machine == machine;
            let Some((at, reads)) = j.settled.as_mut() else {
                return true;
            };
            if here {
                *reads = reads.saturating_add(1);
                if superseded || (*reads >= SETTLED_READS && !elsewhere) {
                    return false;
                }
            }
            at.elapsed() < life
        });
        // What the feed shows now speaks for itself: for a start that ran elsewhere, or a new
        // intent, any machine's copy may show it first (intent 0445).
        self.jobs.retain(|j| {
            !(j.settled.is_some() && (j.machine == machine || j.shows_anywhere()) && shown(j))
        });
        self.jobs.len() != before
    }

    /// Settled effects past their time, gone. Whether any went.
    pub fn expire(&mut self) -> bool {
        let before = self.jobs.len();
        self.jobs
            .retain(|j| j.settled.is_none_or(|(at, _)| at.elapsed() < j.life()));
        self.jobs.len() != before
    }

    /// Whether any effect is showing.
    pub fn shows(&self) -> bool {
        self.jobs.iter().any(|j| j.effect.is_some())
    }

    /// The machines as the board should show them: each effect laid over every copy of its record,
    /// oldest first, so a later change to the same record wins.
    pub fn overlay(&self, machines: &[MachineState]) -> Vec<MachineState> {
        let mut out = machines.to_vec();
        for job in &self.jobs {
            let (Some(key), Some(effect)) = (&job.key, &job.effect) else {
                continue;
            };
            if job.waits_for_write_up() {
                continue;
            }
            if let Effect::Saving(p) = effect {
                // A new card in the backlog it went to, until any machine's feed has the record it
                // made: never two cards for one intent (intent 0445).
                if machines
                    .iter()
                    .filter_map(|m| m.feed.as_ref())
                    .any(|f| p.made_in(f))
                {
                    continue;
                }
                let late = job
                    .settled
                    .is_some_and(|(at, _)| at.elapsed() >= SETTLED_MOST);
                if let Some(feed) = out.get_mut(job.machine).and_then(|m| m.feed.as_mut()) {
                    let mut card = p.card(late);
                    if job.retrying {
                        card.status = Some(RETRYING_SAVE.into());
                    }
                    feed.cards.push(card);
                }
                continue;
            }
            for m in &mut out {
                let Some(feed) = m.feed.as_mut() else {
                    continue;
                };
                let source = feed.source.clone().unwrap_or_else(|| "feed".into());
                for fc in &mut feed.cards {
                    let this = format!("{source}:{}", fc.id);
                    if &this == key {
                        effect.apply(fc, key);
                        // Followed: what it's doing now, in the backend's words (intent 0591).
                        if let Some((say, _)) = job.follow.as_ref().filter(|(s, _)| !s.is_empty()) {
                            fc.status = Some(say.clone());
                            fc.no_agent = false;
                        }
                        // Sent again by itself meanwhile: said quietly, on the card (intent 0591).
                        if job.retrying {
                            fc.status = Some(RETRYING.into());
                        }
                    } else if let Effect::Order(order) = effect
                        && order.iter().any(|(k, _)| *k == this)
                    {
                        effect.apply(fc, &this);
                    }
                }
            }
        }
        out
    }

    /// Esc on the board: changes still on their way neither ask nor take you anywhere when they
    /// answer. Whether any would have.
    pub fn let_go(&mut self) -> bool {
        let mut any = false;
        // Only a start or a reopen ever asks or takes you in, and a read of an item's text opens
        // your editor (intent 0414); other changes keep where they were asked, for a resend to the
        // record's real home (intent 0445).
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| j.settled.is_none() && (j.starts() || j.reads()) && !j.waits_for_write_up())
        {
            any |= j.from.take().is_some();
        }
        any
    }

    /// The change to this record that starts work, while it's on its way or answered but not yet
    /// in the feed: another start or reopen meanwhile would be a second copy.
    pub fn starting(&self, id: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| j.starts() && !j.waits_for_write_up() && j.record() == Some(id))
    }

    /// The newest unanswered change shown on this card, by its key.
    pub fn unanswered_on(&self, key: &str) -> Option<&Job> {
        self.jobs.iter().rev().find(|j| {
            j.settled.is_none()
                && !j.waits_for_write_up()
                && j.effect.is_some()
                && j.key.as_deref() == Some(key)
        })
    }

    /// The unanswered change sending this action for this record, oldest first.
    pub fn unanswered_for(&self, action: &str, id: Option<&str>) -> Option<u64> {
        self.jobs
            .iter()
            .find(|j| j.settled.is_none() && j.request["action"] == action && j.record() == id)
            .map(|j| j.n)
    }

    /// Whether an unanswered change may still ask you something or take you somewhere: Esc on the
    /// board lets that go.
    pub fn holds_you(&self) -> bool {
        self.jobs.iter().any(|j| {
            j.settled.is_none()
                && (j.starts() || j.reads())
                && !j.waits_for_write_up()
                && j.from.is_some()
        })
    }

    /// Whether a change whose request `matches` is still held: on its way, or answered yes and not
    /// yet in the feed (a failure lets go of it at once).
    pub fn holds_where(&self, matches: impl Fn(&Value) -> bool) -> bool {
        self.jobs.iter().any(|j| matches(&j.request))
    }

    /// The oldest unanswered change whose request `matches`.
    pub fn unanswered_where(&self, matches: impl Fn(&Value) -> bool) -> Option<u64> {
        self.jobs
            .iter()
            .find(|j| j.settled.is_none() && matches(&j.request))
            .map(|j| j.n)
    }

    /// What's on its way, unanswered, oldest first: changes and reads alike.
    pub fn on_its_way(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter().filter(|j| j.settled.is_none())
    }

    /// How many changes haven't been answered yet; a read isn't one.
    pub fn unanswered(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.settled.is_none() && !j.reads())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn here() -> crate::config::Machine {
        crate::config::Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        }
    }

    fn feed(cards: Value) -> Feed {
        serde_json::from_value(json!({"kerdr_feed":1,"source":"s","cards":cards})).unwrap()
    }

    #[test]
    fn a_record_s_changes_run_in_order_and_others_alongside() {
        let mut q = Queue::default();
        q.push(1, 0, None, &json!({"action":"move","id":"1","delta":1}));
        q.push(2, 0, None, &json!({"action":"move","id":"1","delta":1}));
        q.push(3, 0, None, &json!({"action":"close","id":"2"}));
        q.push(4, 0, None, &json!({"action":"create","words":"x"}));
        assert_eq!(q.startable(), vec![1, 3, 4]);
        assert!(q.startable().is_empty(), "nothing starts twice");
        q.done(1);
        assert_eq!(q.startable(), vec![2]);
        // The same id on another machine is another record.
        q.push(5, 1, None, &json!({"action":"move","id":"2","delta":1}));
        assert_eq!(q.startable(), vec![5]);
        for n in [2, 3, 4, 5] {
            q.done(n);
        }
        assert!(q.is_empty());
    }

    #[test]
    fn every_change_goes_to_the_runner_once_and_is_held_for_quitting_until_it_runs() {
        let mut r = Resolver::default();
        let a = r.add(0, json!({"action":"move","id":"1"}), None, None, None);
        let b = r.add(0, json!({"action":"move","id":"1"}), None, None, None);
        let c = r.add(0, json!({"action":"create","words":"x"}), None, None, None);
        let sent: Vec<u64> = r.ready().into_iter().map(|(n, ..)| n).collect();
        assert_eq!(sent, vec![a, b, c], "the runner orders them, not the board");
        assert!(r.ready().is_empty());
        assert_eq!(r.holding_quit().len(), 3);
        r.started(a);
        r.started(c);
        // Started, but its command may not have all of the request yet: quitting could cut it
        // off (review of intent 0445).
        assert_eq!(r.holding_quit().len(), 3);
        assert!(r.running().is_empty());
        r.handed(a);
        r.handed(c);
        let held: Vec<u64> = r.holding_quit().iter().map(|j| j.n).collect();
        assert_eq!(held, vec![b, c], "not yet started, and a new intent");
        assert_eq!(r.running().len(), 2);
    }

    #[test]
    fn a_settled_effect_stays_until_the_feed_shows_it_or_two_reads_pass() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"close","id":"1"}),
            None,
            Some("s:1".into()),
            Some(Effect::Close),
        );
        r.ready();
        let job = r.take(n).unwrap();
        r.settle(job);
        assert!(r.shows());
        // Another machine's read changes nothing.
        assert!(!r.read(
            1,
            Some(&feed(json!([{"id":"1","title":"t","state":"active"}])))
        ));
        // The first read still says it's open: it may have started before the save.
        assert!(!r.read(
            0,
            Some(&feed(json!([{"id":"1","title":"t","state":"active"}])))
        ));
        assert!(r.shows());
        assert!(r.read(
            0,
            Some(&feed(json!([{"id":"1","title":"t","state":"active"}])))
        ));
        assert!(!r.shows(), "the second read speaks for itself");

        let n = r.add(
            0,
            json!({"action":"close","id":"1"}),
            None,
            Some("s:1".into()),
            Some(Effect::Close),
        );
        r.ready();
        let job = r.take(n).unwrap();
        r.settle(job);
        assert!(r.read(
            0,
            Some(&feed(json!([{"id":"1","title":"t","state":"closed"}])))
        ));
        assert!(!r.shows(), "shown in the feed: the feed speaks");
    }

    #[test]
    fn effects_lay_over_every_copy_and_a_failed_one_leaves_nothing() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"dismiss","id":"1"}),
            None,
            Some("s:1".into()),
            Some(Effect::SetAside),
        );
        let mut m = MachineState::new(here());
        m.feed = Some(feed(json!([{"id":"1","title":"t","state":"queued"}])));
        let shown = r.overlay(std::slice::from_ref(&m));
        assert_eq!(
            shown[0].feed.as_ref().unwrap().cards[0]
                .disposition
                .as_deref(),
            Some("wont_do")
        );
        assert_eq!(
            m.feed.as_ref().unwrap().cards[0].disposition,
            None,
            "the real feed is untouched"
        );
        r.ready();
        r.take(n);
        let shown = r.overlay(std::slice::from_ref(&m));
        assert_eq!(shown[0].feed.as_ref().unwrap().cards[0].disposition, None);
    }

    #[test]
    fn a_move_reorders_its_group_and_shows_once_the_feed_has_that_order() {
        let order = Effect::Order(vec![("s:2".into(), 0), ("s:1".into(), 1)]);
        let mut r = Resolver::default();
        r.add(
            0,
            json!({"action":"move","id":"2","top":true}),
            None,
            Some("s:2".into()),
            Some(order.clone()),
        );
        let mut m = MachineState::new(here());
        m.feed = Some(feed(
            json!([{"id":"1","title":"a","state":"queued"},{"id":"2","title":"b","state":"queued"},{"id":"3","title":"c","state":"queued"}]),
        ));
        let shown = r.overlay(std::slice::from_ref(&m));
        let ranks: Vec<_> = shown[0]
            .feed
            .as_ref()
            .unwrap()
            .cards
            .iter()
            .map(|c| c.backlog_rank)
            .collect();
        assert_eq!(ranks, vec![Some(1), Some(0), None]);
        let f = feed(
            json!([{"id":"1","title":"a","backlog_rank":1},{"id":"2","title":"b","backlog_rank":0}]),
        );
        assert!(order.shown(&f.cards[1], &f));
        let f = feed(
            json!([{"id":"1","title":"a","backlog_rank":0},{"id":"2","title":"b","backlog_rank":1}]),
        );
        assert!(!order.shown(&f.cards[1], &f));
    }

    #[test]
    fn a_start_marks_its_record_starting_until_answered() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"dispatch","id":"7"}),
            None,
            Some("s:7".into()),
            Some(Effect::Starting),
        );
        assert!(r.starting("7").is_some());
        assert!(r.starting("8").is_none());
        r.ready();
        let job = r.take(n).unwrap();
        r.settle(job);
        assert!(
            r.starting("7").is_some(),
            "answered but not in the feed yet: still no second start"
        );
        r.read(
            0,
            Some(&feed(json!([{"id":"7","title":"t","state":"active"}]))),
        );
        assert!(r.starting("7").is_none(), "the feed has it");
        let queued = json!({"id":"7","title":"t","state":"queued","stage":"backlog"});
        let mut fc: FeedCard = serde_json::from_value(queued.clone()).unwrap();
        Effect::Starting.apply(&mut fc, "s:7");
        assert_eq!(
            (
                fc.state.as_deref(),
                fc.stage.as_deref(),
                fc.status.as_deref()
            ),
            (Some("queued"), Some("backlog"), Some("Starting…")),
            "unanswered, it stays where a question would find it"
        );
        let mut fc: FeedCard = serde_json::from_value(queued).unwrap();
        Effect::Started.apply(&mut fc, "s:7");
        assert_eq!(
            (fc.state.as_deref(), fc.stage.as_deref()),
            (Some("active"), Some("build"))
        );
        let mut fc: FeedCard =
            serde_json::from_value(json!({"id":"7","title":"t","stage":"design"})).unwrap();
        Effect::Started.apply(&mut fc, "s:7");
        assert_eq!(
            fc.stage.as_deref(),
            Some("design"),
            "a working column it names stays"
        );
    }

    #[test]
    fn moves_on_one_machine_go_one_at_a_time_whatever_the_card() {
        let mut q = Queue::default();
        q.push(1, 0, None, &json!({"action":"move","id":"1","delta":1}));
        q.push(2, 0, None, &json!({"action":"move","id":"2","delta":1}));
        q.push(3, 1, None, &json!({"action":"move","id":"3","delta":1}));
        assert_eq!(
            q.startable(),
            vec![1, 3],
            "another machine's backlog is its own"
        );
        q.done(1);
        assert_eq!(q.startable(), vec![2]);
    }

    #[test]
    fn an_answered_change_keeps_its_place_among_the_effects() {
        // J then K on one card: J's answer mustn't put its order over K's.
        let mut r = Resolver::default();
        let down = Effect::Order(vec![("s:2".into(), 0), ("s:1".into(), 1)]);
        let up = Effect::Order(vec![("s:1".into(), 0), ("s:2".into(), 1)]);
        let j = r.add(
            0,
            json!({"action":"move","id":"1","delta":1}),
            None,
            Some("s:1".into()),
            Some(down),
        );
        r.add(
            0,
            json!({"action":"move","id":"1","delta":-1}),
            None,
            Some("s:1".into()),
            Some(up),
        );
        r.ready();
        let job = r.take(j).unwrap();
        r.settle(job);
        let mut m = MachineState::new(here());
        m.feed = Some(feed(json!([{"id":"1","title":"a"},{"id":"2","title":"b"}])));
        let shown = r.overlay(std::slice::from_ref(&m));
        let ranks: Vec<_> = shown[0]
            .feed
            .as_ref()
            .unwrap()
            .cards
            .iter()
            .map(|c| c.backlog_rank)
            .collect();
        assert_eq!(ranks, vec![Some(0), Some(1)], "K, made last, still shows");
    }

    #[test]
    fn a_failed_move_takes_the_later_moves_effects_with_it() {
        let mut r = Resolver::default();
        let order = || Some(Effect::Order(vec![("s:1".into(), 0)]));
        let a = r.add(
            0,
            json!({"action":"move","id":"1","delta":1}),
            None,
            Some("s:1".into()),
            order(),
        );
        r.add(
            0,
            json!({"action":"move","id":"2","delta":1}),
            None,
            Some("s:2".into()),
            order(),
        );
        r.add(
            1,
            json!({"action":"move","id":"3","delta":1}),
            None,
            Some("s:3".into()),
            order(),
        );
        r.ready();
        r.take(a);
        r.forget_moves_after(0, a);
        assert_eq!(
            r.jobs.iter().filter(|j| j.effect.is_some()).count(),
            1,
            "only the other machine's"
        );
        assert_eq!(r.unanswered(), 2, "the later move still runs");
    }

    #[test]
    fn the_feed_showing_a_later_change_retires_the_earlier_ones() {
        let mut r = Resolver::default();
        for (action, effect) in [("dismiss", Effect::SetAside), ("restore", Effect::Restore)] {
            let n = r.add(
                0,
                json!({"action":action,"id":"1"}),
                None,
                Some("s:1".into()),
                Some(effect),
            );
            r.ready();
            let job = r.take(n).unwrap();
            r.settle(job);
        }
        r.read(
            0,
            Some(&feed(json!([{"id":"1","title":"t","state":"queued"}]))),
        );
        assert!(
            !r.shows(),
            "the restore is read, so the dismiss before it goes too"
        );
    }

    #[test]
    fn a_start_is_guarded_until_the_feed_proves_it_started() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"dispatch","id":"7"}),
            None,
            Some("s:7".into()),
            Some(Effect::Starting),
        );
        r.ready();
        let job = r.take(n).unwrap();
        r.settle(job);
        // A feed that names no state proves nothing.
        r.read(0, Some(&feed(json!([{"id":"7","title":"t"}]))));
        assert!(r.starting("7").is_some());
        r.read(
            0,
            Some(&feed(
                json!([{"id":"7","title":"t","workspace":{"session":"web","id":"w3"}}]),
            )),
        );
        assert!(r.starting("7").is_none(), "a workspace proves it");
    }

    #[test]
    fn a_change_sent_on_to_another_machine_keeps_its_place() {
        let mut r = Resolver::default();
        let mut q = Queue::default();
        let card = || Some("s:1".to_string());
        let j = r.add(
            0,
            json!({"action":"move","id":"1","delta":1}),
            None,
            card(),
            None,
        );
        let k = r.add(
            0,
            json!({"action":"move","id":"1","delta":-1}),
            None,
            card(),
            None,
        );
        // The runner takes both, by card.
        let hand = |r: &mut Resolver, q: &mut Queue| {
            for (n, machine, request, _) in r.ready() {
                q.push(n, machine, r.key_of(n), &request);
            }
            q.startable()
        };
        assert_eq!(hand(&mut r, &mut q), vec![j]);
        // J is refused, naming another machine: its place stays held while the board sends it on.
        let job = r.take(j).unwrap();
        r.retarget(job, 1, json!({"action":"move","id":"1","delta":1}), None);
        assert!(r.sent_on(j));
        assert!(q.startable().is_empty(), "K waits while J is sent on");
        assert_eq!(
            hand(&mut r, &mut q),
            vec![j],
            "K waits for J, now on its new machine"
        );
        assert!(!r.sent_on(j));
        r.take(j);
        q.done(j);
        assert_eq!(hand(&mut r, &mut q), vec![k], "then K, after it");
    }

    #[test]
    fn a_record_s_changes_keep_their_order_by_card_across_machines() {
        let mut q = Queue::default();
        let card = || Some("s:1".to_string());
        q.push(1, 1, card(), &json!({"action":"move","id":"1","delta":1}));
        q.push(2, 0, card(), &json!({"action":"close","id":"1"}));
        assert_eq!(q.startable(), vec![1], "the same card, on another machine");
        q.done(1);
        assert_eq!(q.startable(), vec![2]);
    }

    #[test]
    fn the_feed_showing_a_later_move_retires_earlier_moves_of_other_cards() {
        // A, B, C: A down, then B down, both saved: A, B, C again.
        let mut r = Resolver::default();
        let a_down = Effect::Order(vec![
            ("s:B".into(), 0),
            ("s:A".into(), 1),
            ("s:C".into(), 2),
        ]);
        let b_down = Effect::Order(vec![
            ("s:A".into(), 0),
            ("s:B".into(), 1),
            ("s:C".into(), 2),
        ]);
        for (id, effect) in [("A", a_down), ("B", b_down)] {
            let n = r.add(
                0,
                json!({"action":"move","id":id,"delta":1}),
                None,
                Some(format!("s:{id}")),
                Some(effect),
            );
            r.ready();
            let job = r.take(n).unwrap();
            r.settle(job);
        }
        r.read(
            0,
            Some(&feed(json!([
                {"id":"A","title":"a","backlog_rank":0},
                {"id":"B","title":"b","backlog_rank":1},
                {"id":"C","title":"c","backlog_rank":2}
            ]))),
        );
        assert!(!r.shows(), "A's older order mustn't come back");
    }

    #[test]
    fn a_new_intent_shows_as_a_card_until_the_feed_has_the_record_it_made() {
        let mut r = Resolver::default();
        let p = Placeholder {
            id: Placeholder::id_for("kerdr-1"),
            title: Placeholder::title_for("Back up\nthe NAS"),
            machine: "here".into(),
            session: "web".into(),
            source: "s".into(),
            made: None,
            organization: None,
            tenant: None,
            team: None,
        };
        assert_eq!(p.title, "Back up the NAS");
        let n = r.add(
            0,
            json!({"action":"create","words":"Back up the NAS","submission":"kerdr-1"}),
            None,
            Some("s:saving-kerdr-1".into()),
            Some(Effect::Saving(p)),
        );
        assert!(r.saving("kerdr-1").is_some());
        let mut m = MachineState::new(here());
        m.feed = Some(feed(json!([{"id":"1","title":"a","state":"queued"}])));
        let shown = r.overlay(std::slice::from_ref(&m));
        let cards = &shown[0].feed.as_ref().unwrap().cards;
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].intake.as_deref(), Some(SAVING));
        assert_eq!(cards[1].title, "Back up the NAS");
        // Unanswered, reads leave it.
        r.read(0, m.feed.as_ref());
        r.read(0, m.feed.as_ref());
        assert!(r.shows());
        // Answered with the record it made: it stays until the feed has that record.
        let mut job = r.take(n).unwrap();
        if let Some(Effect::Saving(p)) = job.effect.as_mut() {
            p.made = Some("9001".into());
        }
        r.settle(job);
        let with = feed(json!([{"id":"1","title":"a"},{"id":"9001","title":"Back up the NAS"}]));
        m.feed = Some(with.clone());
        assert_eq!(
            r.overlay(std::slice::from_ref(&m))[0]
                .feed
                .as_ref()
                .unwrap()
                .cards
                .len(),
            2,
            "never two cards for one intent"
        );
        r.read(0, Some(&with));
        assert!(!r.shows());
    }

    fn placeholder() -> Placeholder {
        Placeholder {
            id: Placeholder::id_for("kerdr-1"),
            title: "Back up the NAS".into(),
            machine: "here".into(),
            session: "web".into(),
            source: "s".into(),
            made: None,
            organization: None,
            tenant: None,
            team: None,
        }
    }

    fn cards_in(shown: &[MachineState], i: usize) -> Vec<String> {
        shown[i]
            .feed
            .as_ref()
            .unwrap()
            .cards
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    #[test]
    fn a_new_intent_s_record_read_before_its_answer_takes_the_card_s_place() {
        // create writes the record, submission and all, seconds before it answers: a read
        // meanwhile shows it, and the card shown while it saved gives way (intent 0445).
        let mut r = Resolver::default();
        r.add(
            0,
            json!({"action":"create","words":"Back up the NAS","submission":"kerdr-1"}),
            None,
            Some("s:saving-kerdr-1".into()),
            Some(Effect::Saving(placeholder())),
        );
        let mut m = MachineState::new(here());
        m.feed = Some(feed(json!([{"id":"1","title":"a"}])));
        assert_eq!(
            cards_in(&r.overlay(std::slice::from_ref(&m)), 0),
            ["1", "saving-kerdr-1"]
        );
        m.feed = Some(feed(
            json!([{"id":"1","title":"a"},{"id":"9001","title":"back up nas","intake":"pending","submission":"kerdr-1"}]),
        ));
        assert_eq!(
            cards_in(&r.overlay(std::slice::from_ref(&m)), 0),
            ["1", "9001"],
            "never two cards for one intent"
        );
        r.read(0, m.feed.as_ref());
        assert!(r.shows(), "unanswered, its answer still finds it");
    }

    #[test]
    fn a_new_intent_s_record_on_another_machine_first_takes_the_card_s_place() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"create","words":"Back up the NAS","submission":"kerdr-1"}),
            None,
            Some("s:saving-kerdr-1".into()),
            Some(Effect::Saving(placeholder())),
        );
        let mut here_m = MachineState::new(here());
        here_m.feed = Some(feed(json!([{"id":"1","title":"a"}])));
        let mut there = MachineState::new(here());
        there.feed = Some(feed(
            json!([{"id":"1","title":"a"},{"id":"9001","title":"t"}]),
        ));
        let both = [here_m.clone(), there.clone()];
        // Answered: the record it made, by id, on another machine's copy of the same store.
        let mut job = r.take(n).unwrap();
        if let Some(Effect::Saving(p)) = job.effect.as_mut() {
            p.made = Some("9001".into());
        }
        r.settle(job);
        let shown = r.overlay(&both);
        assert_eq!(cards_in(&shown, 0), ["1"], "no card shown while it saved");
        assert_eq!(cards_in(&shown, 1), ["1", "9001"]);
        // Another store's record with that number isn't it.
        let mut other = there.clone();
        other.feed.as_mut().unwrap().source = Some("elsewhere".into());
        let shown = r.overlay(&[here_m.clone(), other]);
        assert_eq!(cards_in(&shown, 0), ["1", "saving-kerdr-1"]);
        // The other machine's read retires it.
        assert!(r.read(1, there.feed.as_ref()));
        assert!(!r.shows());
    }

    #[test]
    fn a_start_that_ran_elsewhere_goes_once_any_copy_shows_it_under_way() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"dispatch","id":"7"}),
            None,
            Some("s:7".into()),
            Some(Effect::Starting),
        );
        let mut job = r.take(n).unwrap();
        job.effect = Some(Effect::StartingOn("far-box".into()));
        r.settle(job);
        let mut here_m = MachineState::new(here());
        here_m.feed = Some(feed(json!([{"id":"7","title":"t","state":"queued"}])));
        let mut there = MachineState::new(here());
        there.feed = Some(feed(
            json!([{"id":"7","title":"t","state":"active","status":"Working"}]),
        ));
        let shown = r.overlay(&[here_m, there.clone()]);
        let status = |i: usize| shown[i].feed.as_ref().unwrap().cards[0].status.clone();
        assert_eq!(status(0).as_deref(), Some("Starting on far-box…"));
        assert_eq!(
            status(1).as_deref(),
            Some("Working"),
            "a copy under way speaks for itself"
        );
        assert!(r.starting("7").is_some());
        assert!(r.read(1, there.feed.as_ref()));
        assert!(r.starting("7").is_none(), "the far machine's read shows it");
    }

    #[test]
    fn a_start_waiting_for_its_write_up_shows_nothing_and_counts_for_nothing_yet() {
        let mut r = Resolver::default();
        let n = r.add_gated(
            0,
            json!({"action":"dispatch","id":"7"}),
            Some(CardSnapshot::default()),
            Some("s:7".into()),
            Some(Effect::Starting),
        );
        let mut m = MachineState::new(here());
        m.feed = Some(feed(json!([{"id":"7","title":"t","state":"queued"}])));
        let status = |r: &Resolver| {
            r.overlay(std::slice::from_ref(&m))[0]
                .feed
                .as_ref()
                .unwrap()
                .cards[0]
                .status
                .clone()
        };
        assert_eq!(status(&r), None);
        assert!(r.starting("7").is_none());
        assert!(r.holding_quit().is_empty());
        assert!(!r.holds_you());
        assert!(r.waiting_for_write_up("s:7").is_some());
        assert!(r.ready()[0].3);
        assert!(r.started(n).shows_now, "it shows from now");
        assert_eq!(status(&r).as_deref(), Some("Starting…"));
        assert!(r.starting("7").is_some());
        assert!(
            r.drop_awaited().is_empty(),
            "started: Esc no longer drops it"
        );
    }

    #[test]
    fn the_runner_holds_a_start_until_its_write_up_and_drops_one_that_failed() {
        let mut q = Queue::default();
        let start = json!({"action":"dispatch","id":"7"});
        q.push_gated(1, 0, None, &start);
        q.push(2, 0, None, &json!({"action":"move","id":"7","delta":1}));
        q.push(3, 0, None, &json!({"action":"close","id":"8"}));
        assert_eq!(
            q.startable(),
            vec![3],
            "the record's later change waits behind it"
        );
        let feed =
            |intake: Option<&str>| HashMap::from([("7".to_string(), intake.map(str::to_string))]);
        assert!(
            q.written(1, &feed(None)).is_empty(),
            "another machine's feed isn't its home's"
        );
        assert!(q.startable().is_empty());
        assert!(q.written(0, &feed(Some("pending"))).is_empty());
        assert!(q.written(0, &feed(None)).is_empty());
        assert_eq!(q.startable(), vec![1]);
        let mut q = Queue::default();
        q.push_gated(1, 0, None, &start);
        assert_eq!(q.written(0, &feed(Some("failed"))), vec![1]);
        assert!(q.is_empty());
        let mut q = Queue::default();
        q.push_gated(1, 0, None, &start);
        assert!(q.drop_unstarted(1));
        assert!(q.is_empty());
    }

    #[test]
    fn going_in_yourself_lets_a_start_s_answer_go() {
        let mut r = Resolver::default();
        r.add(
            0,
            json!({"action":"reopen","id":"7"}),
            Some(CardSnapshot::default()),
            Some("s:7".into()),
            Some(Effect::Reopening),
        );
        assert!(r.holds_you());
        r.went_in("8");
        assert!(r.holds_you(), "another record's");
        r.went_in("7");
        assert!(!r.holds_you());
        assert!(r.starting("7").unwrap().entered);
    }

    #[test]
    fn a_later_move_of_a_neighbour_speaks_for_the_earlier_one() {
        // J on card 1, then J on card 2 of the same group: both saved, and the read shows the
        // second's order. The first's mustn't come back over it.
        let mut r = Resolver::default();
        let first = Effect::Order(vec![
            ("s:2".into(), 0),
            ("s:1".into(), 1),
            ("s:3".into(), 2),
        ]);
        let second = Effect::Order(vec![
            ("s:2".into(), 0),
            ("s:3".into(), 1),
            ("s:1".into(), 2),
        ]);
        for (id, e) in [("1", first), ("2", second)] {
            let n = r.add(
                0,
                json!({"action":"move","id":id,"delta":1}),
                None,
                Some(format!("s:{id}")),
                Some(e),
            );
            r.ready();
            let job = r.take(n).unwrap();
            r.settle(job);
        }
        r.read(
            0,
            Some(&feed(
                json!([{"id":"1","title":"a","backlog_rank":2},{"id":"2","title":"b","backlog_rank":0},{"id":"3","title":"c","backlog_rank":1}]),
            )),
        );
        assert!(!r.shows(), "nothing laid over a board already right");
    }

    #[test]
    fn a_reopen_reads_as_reopening_and_a_refresh_as_today() {
        let mut fc: FeedCard = serde_json::from_value(
            json!({"id":"7","title":"t","state":"closed","stage":"maintain","attention":"done"}),
        )
        .unwrap();
        Effect::Reopening.apply(&mut fc, "s:7");
        assert_eq!(fc.state.as_deref(), Some("active"));
        assert_eq!(
            fc.stage.as_deref(),
            Some("maintain"),
            "it stays where it is"
        );
        assert_eq!(fc.status.as_deref(), Some("Reopening…"));
        let mut fc: FeedCard = serde_json::from_value(
            json!({"id":"7","title":"t","attention":"needs_you","needs":"a question","blockers":["x"]}),
        )
        .unwrap();
        Effect::Reopening.apply(&mut fc, "s:7");
        assert!(fc.needs.is_none() && fc.blockers.is_empty());
        let mut fc: FeedCard =
            serde_json::from_value(json!({"id":"7","title":"t","stale_days":20})).unwrap();
        Effect::Refresh("2026-11-02".into()).apply(&mut fc, "s:7");
        assert_eq!(fc.stale_days, Some(0), "today, not unknown");
    }

    #[test]
    fn a_change_sent_on_never_runs_beside_a_later_move_already_running_there() {
        // Review of intent 0445: J on machine 0 and K on machine 1 both run; J is refused and sent
        // on to machine 1, where K is still running: J waits for K.
        let mut q = Queue::default();
        q.push(1, 0, None, &json!({"action":"move","id":"1","delta":1}));
        q.push(2, 1, None, &json!({"action":"move","id":"2","delta":1}));
        assert_eq!(q.startable(), vec![1, 2]);
        q.push(1, 1, None, &json!({"action":"move","id":"1","delta":1}));
        assert!(
            q.startable().is_empty(),
            "K is still reordering that backlog"
        );
        q.done(2);
        assert_eq!(q.startable(), vec![1]);
    }

    #[test]
    fn esc_racing_a_start_that_already_began_keeps_it_until_the_runner_answers() {
        // Review of intent 0445: the write-up is read and the start begins a moment before Esc.
        let mut r = Resolver::default();
        let start = |r: &mut Resolver| {
            r.add_gated(
                0,
                json!({"action":"dispatch","id":"7"}),
                None,
                Some("s:7".into()),
                Some(Effect::Starting),
            )
        };
        let n = start(&mut r);
        r.ready();
        assert_eq!(r.drop_awaited(), vec!["s:7".to_string()]);
        assert_eq!(r.take_dropped(), vec![n]);
        assert!(r.write_ups_awaited().is_empty(), "no longer said to wait");
        assert!(r.drop_awaited().is_empty(), "dropped once");
        // The runner had already started it: it shows, and its answer still finds it.
        assert_eq!(
            r.started(n),
            Started {
                shows_now: true,
                too_late: true
            }
        );
        r.drop_answered(n, false);
        assert!(r.starting("7").is_some());
        assert!(r.take(n).is_some());
        // Taken out before it started: it goes.
        let n = start(&mut r);
        r.ready();
        r.drop_awaited();
        r.drop_answered(n, true);
        assert!(r.job(n).is_none());
    }

    #[test]
    fn a_saved_new_intent_s_card_stays_until_its_record_is_read_and_says_when_it_s_late() {
        // Review of intent 0445: the answer names the record, but the feed is slow to show it.
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"create","words":"Back up the NAS","submission":"kerdr-1"}),
            None,
            Some("s:saving-kerdr-1".into()),
            Some(Effect::Saving(placeholder())),
        );
        let mut job = r.take(n).unwrap();
        if let Some(Effect::Saving(p)) = job.effect.as_mut() {
            p.made = Some("9001".into());
        }
        r.settle(job);
        let without = feed(json!([{"id":"1","title":"a"}]));
        for _ in 0..4 {
            r.read(0, Some(&without));
        }
        assert!(!r.expire());
        let mut m = MachineState::new(here());
        m.feed = Some(without.clone());
        let status = |r: &Resolver| {
            r.overlay(std::slice::from_ref(&m))[0]
                .feed
                .as_ref()
                .unwrap()
                .cards
                .iter()
                .find(|c| c.id == "saving-kerdr-1")
                .and_then(|c| c.status.clone())
        };
        assert_eq!(status(&r).as_deref(), Some("Saving…"));
        // A minute on, still not read: it says so rather than going.
        for j in &mut r.jobs {
            j.settled = Some((Instant::now() - Duration::from_secs(61), 2));
        }
        assert!(!r.expire());
        r.read(0, Some(&without));
        assert_eq!(
            status(&r).as_deref(),
            Some("Saved as #9001; not on the board yet")
        );
        r.read(
            0,
            Some(&feed(
                json!([{"id":"1","title":"a"},{"id":"9001","title":"t"}]),
            )),
        );
        assert!(!r.shows());
    }

    #[test]
    fn only_a_start_or_reopen_is_let_go_by_esc() {
        let mut r = Resolver::default();
        let snap = || Some(CardSnapshot::default());
        r.add(0, json!({"action":"move","id":"1"}), snap(), None, None);
        assert!(!r.holds_you());
        assert!(!r.let_go(), "a move never takes you anywhere");
        r.add(0, json!({"action":"dispatch","id":"2"}), snap(), None, None);
        assert!(r.holds_you());
        assert!(r.let_go());
        assert!(!r.holds_you());
        // A read of an item's text would open your editor: Esc lets that go too (intent 0414).
        r.add(0, json!({"action":"edit","id":"3"}), snap(), None, None);
        assert!(r.holds_you());
        assert!(r.let_go());
        assert!(!r.holds_you());
        // A save of one asks nothing and takes you nowhere.
        r.add(
            0,
            json!({"action":"edit","id":"3","text":"New.","was":"sha256:aa"}),
            snap(),
            None,
            None,
        );
        assert!(!r.holds_you());
    }

    #[test]
    fn a_read_waits_for_the_changes_before_it_and_holds_up_nothing() {
        // Intent 0414: e's read of an item's text isn't a change. The runner's queue keeps the
        // order: it waits for the move made before it, and nothing waits for it.
        let card = || Some("s:1".to_string());
        let mut q = Queue::default();
        let move_first = json!({"action":"move","id":"1","delta":1});
        let read = json!({"action":"edit","id":"1"});
        let after = json!({"action":"refresh","id":"1"});
        let save = json!({"action":"edit","id":"1","text":"New.","was":"sha256:aa"});
        q.push(1, 0, card(), &move_first);
        q.push(2, 0, card(), &read);
        q.push(3, 0, card(), &after);
        assert_eq!(q.startable(), vec![1], "the read waits for the move");
        q.done(1);
        assert_eq!(q.startable(), vec![2, 3], "nothing waits for the read");
        // A save is a change like any other: it waits for the refresh, never for the read.
        q.push(4, 0, card(), &save);
        assert!(q.startable().is_empty(), "the save waits for the refresh");
        q.done(3);
        assert_eq!(q.startable(), vec![4]);
        for n in [2, 4] {
            q.done(n);
        }
        assert!(q.is_empty());
        // q neither waits for a read nor counts it as running; it does for the changes.
        let mut r = Resolver::default();
        let changes = [
            r.add(0, move_first, None, card(), None),
            r.add(0, read, None, card(), None),
            r.add(0, after, None, card(), None),
        ];
        r.ready();
        assert_eq!(r.holding_quit().len(), 2, "the move and the refresh");
        for n in changes {
            r.started(n);
            r.handed(n);
        }
        assert!(r.holding_quit().is_empty());
        assert_eq!(r.running().len(), 2, "the read isn't running work");
        assert_eq!(r.unanswered(), 2);
        assert_eq!(r.on_its_way().count(), 3);
    }

    #[test]
    fn a_start_waiting_for_its_write_up_trusts_no_read_from_before_its_new_text_was_saved() {
        // d on a card showing ✎ for a text just saved (intent 0414): its write-up starts again
        // once the save lands, so a read from before then mustn't start it (intent 0445's gate).
        let key = || Some("s:7".to_string());
        let save = json!({"action":"edit","id":"7","text":"New.","was":"sha256:aa"});
        let start = json!({"action":"dispatch","id":"7"});
        let intakes = |v: Option<&str>| HashMap::from([("7".to_string(), v.map(str::to_string))]);
        let mut q = Queue::default();
        q.push(1, 0, key(), &save);
        q.push_gated(2, 0, key(), &start);
        assert_eq!(q.startable(), vec![1]);
        let read = |q: &mut Queue, intake: Option<&str>| {
            assert!(q.written(0, &intakes(intake)).is_empty());
            q.read_passed(0);
            q.startable()
        };
        assert!(
            read(&mut q, None).is_empty(),
            "the save is still on its way"
        );
        q.done(1);
        assert!(
            read(&mut q, None).is_empty(),
            "the first read after it may have begun before it landed"
        );
        assert!(read(&mut q, Some("pending")).is_empty(), "written up again");
        assert_eq!(read(&mut q, None), vec![2], "then it starts");
        // Another machine's reads count for nothing here.
        let mut q = Queue::default();
        q.push(1, 0, key(), &save);
        q.push_gated(2, 0, key(), &start);
        q.startable();
        q.done(1);
        assert!(q.written(1, &intakes(None)).is_empty());
        q.read_passed(1);
        assert!(read(&mut q, None).is_empty());
        assert_eq!(read(&mut q, None), vec![2]);
        // Any other change ahead: the gate is 0.23.0's, a read saying it's written up starts it
        // once that change has gone.
        let mut q = Queue::default();
        q.push(3, 0, key(), &json!({"action":"refresh","id":"7"}));
        q.push_gated(4, 0, key(), &start);
        assert_eq!(read(&mut q, None), vec![3]);
        q.done(3);
        assert_eq!(q.startable(), vec![4]);
    }

    #[test]
    fn a_read_from_before_a_new_text_is_never_kept_to_judge_a_later_start() {
        // d pressed after the first read since the save: the runner judges it by the read it kept.
        let key = || Some("s:7".to_string());
        let save = json!({"action":"edit","id":"7","text":"New.","was":"sha256:aa"});
        let start = json!({"action":"dispatch","id":"7"});
        let intakes = |v: Option<&str>| HashMap::from([("7".to_string(), v.map(str::to_string))]);
        let mut q = Queue::default();
        q.push(1, 0, key(), &save);
        assert_eq!(q.startable(), vec![1]);
        // Both a read while the save runs and the first after it say nothing that's kept.
        let mut during = intakes(None);
        q.forget_rewriting(0, &mut during);
        assert!(during.is_empty(), "the save is still on its way");
        q.done(1);
        let mut first = intakes(None);
        assert!(q.written(0, &first).is_empty());
        q.forget_rewriting(0, &mut first);
        q.read_passed(0);
        assert!(first.is_empty(), "it may have begun before the save landed");
        // So the start waits, on what was kept, until a read after the save says it's done.
        q.push_gated(2, 0, key(), &start);
        assert!(q.written(0, &first).is_empty());
        assert!(q.startable().is_empty());
        let mut next = intakes(Some("pending"));
        assert!(q.written(0, &next).is_empty());
        q.forget_rewriting(0, &mut next);
        assert_eq!(next, intakes(Some("pending")), "a later read is kept");
        assert!(q.startable().is_empty());
        assert!(q.written(0, &intakes(None)).is_empty());
        assert_eq!(q.startable(), vec![2]);
        // Another machine's save leaves this machine's read alone.
        let mut q = Queue::default();
        q.push(3, 1, key(), &save);
        let mut here = intakes(None);
        q.forget_rewriting(0, &mut here);
        assert_eq!(here, intakes(None));
    }

    #[test]
    fn a_new_text_shows_at_once_and_goes_once_the_feed_has_its_words() {
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"edit","id":"1","text":"New words,\nas written.","was":"sha256:aa"}),
            None,
            Some("s:1".into()),
            Some(Effect::Text("New words,\nas written.".into())),
        );
        let mut m = MachineState::new(here());
        m.feed = Some(feed(
            json!([{"id":"1","title":"t","state":"queued","words":"Old words."}]),
        ));
        let shown = r.overlay(std::slice::from_ref(&m));
        let fc = &shown[0].feed.as_ref().unwrap().cards[0];
        assert_eq!(
            (fc.words.as_deref(), fc.intake.as_deref()),
            (Some("New words,\nas written."), Some("pending"))
        );
        r.ready();
        r.started(n);
        let job = r.take(n).unwrap();
        r.settle(job);
        // A read with the old words: it may have begun before the save.
        assert!(!r.read(0, m.feed.as_ref()));
        assert!(r.shows());
        // The feed's own spacing counts as having it; its words cut short don't.
        let text = Effect::Text("New words,\nas written.".into());
        let card = |words: &str| {
            feed(json!([{"id":"1","title":"t","words":words}]))
                .cards
                .remove(0)
        };
        let f = feed(json!([]));
        assert!(text.shown(&card("New words, as written."), &f));
        assert!(
            !text.shown(&card("New words…"), &f),
            "cut short proves nothing"
        );
        assert!(!text.shown(&card("Old words."), &f));
        assert!(!text.shown(&card("New words, as written. More…"), &f));
        assert!(!text.shown(&card("…"), &f));
        assert!(r.read(
            0,
            Some(&feed(
                json!([{"id":"1","title":"t","words":"New words, as written."}])
            ))
        ));
        assert!(!r.shows());
    }

    #[test]
    fn a_feed_cut_short_never_proves_a_new_text_and_it_goes_after_two_reads() {
        // Old and new texts the same for the feed's first 200 characters: a read of the old text,
        // begun before the save, looks just like one of the new.
        let start = "a".repeat(200);
        let (old, new) = (
            format!("{start} old ending."),
            format!("{start} new ending."),
        );
        let mut r = Resolver::default();
        let n = r.add(
            0,
            json!({"action":"edit","id":"1","text":new,"was":"sha256:aa"}),
            None,
            Some("s:1".into()),
            Some(Effect::Text(new.clone())),
        );
        r.ready();
        r.started(n);
        let job = r.take(n).unwrap();
        r.settle(job);
        let cut = |text: &str| {
            let shown: String = text.chars().take(200).collect();
            feed(json!([{"id":"1","title":"t","words":format!("{shown}…")}]))
        };
        assert_eq!(cut(&old).cards[0].words, cut(&new).cards[0].words);
        assert!(
            !r.read(0, Some(&cut(&old))),
            "the first read proves nothing"
        );
        assert!(r.shows());
        // The second read retires it, as any settled effect: the bounded rule, not the words.
        assert!(r.read(0, Some(&cut(&new))));
        assert!(!r.shows());
    }
}
