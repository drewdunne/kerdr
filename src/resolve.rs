//! The action resolution tool (intent 0416): every change to a card (start, reopen, close, won't
//! do, restore, refresh, cancel, move, and a new intent) goes through here instead of holding the
//! board until its command answers.
//!
//! A change shows on the board the moment it's asked for: its expected effect is laid over the
//! feeds the board is built from. Its command runs in the background, one at a time for each record
//! (so J J K reach the backend in order) and side by side for different records. When it succeeds
//! nothing more is said: the effect stays until the feed shows it (or two reads go by, or a minute),
//! then the feed alone speaks. When it fails, or the backend asks a question first, the effect goes
//! at once, so the card is back as it really is, and the app says so where it can't be missed.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::board::MachineState;
use crate::feed::{Feed, FeedCard, Home};
use crate::model::CardSnapshot;

/// The intake a new intent's card has while its words are still being saved (intent 0445): no
/// record exists yet, so nothing may be done to it.
pub const SAVING: &str = "saving";

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
    /// The record the save made, once its answer names it: the card goes once the feed has it.
    pub made: Option<String>,
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

    fn card(&self) -> FeedCard {
        FeedCard {
            id: self.id.clone(),
            reference: Some("new".into()),
            title: self.title.clone(),
            stage: Some("backlog".into()),
            state: Some("queued".into()),
            intake: Some(SAVING.into()),
            status: Some("Saving…".into()),
            home: Some(Home {
                machine: Some(self.machine.clone()),
                session: Some(self.session.clone()),
            }),
            created: Some(chrono::Local::now().date_naive().to_string()),
            stale_days: Some(0),
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
                fc.status = Some(format!("Starting on {machine}…"));
                fc.no_agent = false;
            }
            Effect::Started => {
                fc.state = Some("active".into());
                if !fc
                    .stage
                    .as_deref()
                    .and_then(crate::model::Stage::parse)
                    .is_some_and(|s| {
                        matches!(
                            s,
                            crate::model::Stage::Plan
                                | crate::model::Stage::Design
                                | crate::model::Stage::Build
                                | crate::model::Stage::Test
                                | crate::model::Stage::Deploy
                        )
                    })
                {
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
            Effect::Saving(p) => p
                .made
                .as_ref()
                .is_some_and(|id| feed.cards.iter().any(|c| &c.id == id)),
        }
    }
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

    /// How long its effect may stay, settled, without the feed showing it.
    fn life(&self) -> Duration {
        match self.effect {
            Some(Effect::StartingOn(_)) => ELSEWHERE_MOST,
            _ => SETTLED_MOST,
        }
    }

    /// The record it changes, by id.
    pub fn id(&self) -> Option<&str> {
        self.record()
    }
}

/// The order changes run in, kept by the runner away from the board's thread, so a change whose
/// turn comes while Andrew is inside a workspace starts then (intent 0445): each record's changes
/// one at a time, a machine's moves one at a time, everything else side by side. A record is its
/// card, wherever a refusal sent its change, else its id on one machine (intent 0416).
#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<Queued>,
}

#[derive(Debug)]
struct Queued {
    n: u64,
    machine: usize,
    record: Option<String>,
    key: Option<String>,
    moves: bool,
    started: bool,
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
            started: false,
        };
        match self.items.iter_mut().find(|j| j.n == n) {
            Some(held) => *held = item,
            None => self.items.push(item),
        }
    }

    /// The changes that can start now, marked started: each record's oldest one, once nothing
    /// ahead of it for that record (or, for a move, that machine's backlog) is still running or
    /// held. A request with no record (a new intent) starts straight away.
    pub fn startable(&mut self) -> Vec<u64> {
        let mut out = vec![];
        for i in 0..self.items.len() {
            let job = &self.items[i];
            if job.started {
                continue;
            }
            let ahead = self.items[..i]
                .iter()
                .any(|j| job.same(j) || (job.moves && j.moves && j.machine == job.machine));
            if ahead {
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
        self.items.retain(|j| j.n != n);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Resolver {
    jobs: Vec<Job>,
    next: u64,
}

impl Resolver {
    /// Takes a change: it shows at once, and runs once nothing else for its record is running.
    pub fn add(
        &mut self,
        machine: usize,
        request: Value,
        from: Option<CardSnapshot>,
        key: Option<String>,
        effect: Option<Effect>,
    ) -> u64 {
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
        });
        self.next
    }

    /// The changes not yet handed to the runner, marked handed: it starts each in its turn
    /// (Queue), so this needs nothing from the board's thread later (intent 0445).
    pub fn ready(&mut self) -> Vec<(u64, usize, Value)> {
        self.jobs
            .iter_mut()
            .filter(|j| !j.sent && j.settled.is_none())
            .map(|j| {
                j.sent = true;
                (j.n, j.machine, j.request.clone())
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

    /// The runner has started this change's command.
    pub fn started(&mut self, n: u64) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.n == n) {
            j.running.get_or_insert_with(Instant::now);
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

    /// Changes Kerdr mustn't quit before: ones not yet started (they'd never be sent) and new
    /// intents (his words may still be on their way to the command). A start or another change
    /// already running finishes without Kerdr.
    pub fn holding_quit(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| {
                j.settled.is_none() && (j.running.is_none() || j.request["action"] == "create")
            })
            .collect()
    }

    /// Unanswered changes whose commands are running and finish without Kerdr.
    pub fn running(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.settled.is_none() && j.running.is_some())
            .collect()
    }

    /// Takes a change out as its answer arrives: a failure's effect goes with it.
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
        // Handed to the runner again, which still holds its place (intent 0445).
        job.sent = false;
        job.running = None;
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
            let elsewhere = matches!(j.effect, Some(Effect::StartingOn(_)));
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
        // What the feed shows now speaks for itself.
        self.jobs
            .retain(|j| !(j.settled.is_some() && j.machine == machine && shown(j)));
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
            if let Effect::Saving(p) = effect {
                // A new card in the backlog it went to, until the feed has the record it made.
                if let Some(feed) = out.get_mut(job.machine).and_then(|m| m.feed.as_mut())
                    && !p
                        .made
                        .as_ref()
                        .is_some_and(|id| feed.cards.iter().any(|c| &c.id == id))
                {
                    feed.cards.push(p.card());
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
        // Only a start or a reopen ever asks or takes you in; other changes keep where they were
        // asked, for a resend to the record's real home (intent 0445).
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| j.settled.is_none() && j.starts())
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
            .find(|j| j.starts() && j.record() == Some(id))
    }

    /// The newest unanswered change shown on this card, by its key.
    pub fn unanswered_on(&self, key: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .rev()
            .find(|j| j.settled.is_none() && j.effect.is_some() && j.key.as_deref() == Some(key))
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
        self.jobs
            .iter()
            .any(|j| j.settled.is_none() && j.starts() && j.from.is_some())
    }

    /// The oldest unanswered change whose request `matches`.
    pub fn unanswered_where(&self, matches: impl Fn(&Value) -> bool) -> Option<u64> {
        self.jobs
            .iter()
            .find(|j| j.settled.is_none() && matches(&j.request))
            .map(|j| j.n)
    }

    /// How many changes haven't been answered yet.
    pub fn unanswered(&self) -> usize {
        self.jobs.iter().filter(|j| j.settled.is_none()).count()
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
            for (n, machine, request) in r.ready() {
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
            made: None,
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
    }
}
