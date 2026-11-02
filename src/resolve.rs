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
use crate::feed::{Feed, FeedCard};
use crate::model::CardSnapshot;

/// How long a settled effect may stay without the feed showing it.
const SETTLED_MOST: Duration = Duration::from_secs(60);
/// How many of its machine's reads a settled effect waits for: the first may have started before
/// the change was saved.
const SETTLED_READS: u8 = 2;

/// What the board shows for a change while its command runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `dispatch`: out of the backlog and starting.
    Starting,
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
}

impl Effect {
    /// Lays this effect over one feed card, its record's copy on one machine.
    fn apply(&self, fc: &mut FeedCard, key: &str) {
        match self {
            Effect::Starting => {
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
            Effect::Reopening => {
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
                fc.stale_days = None;
            }
            Effect::Order(order) => {
                if let Some((_, rank)) = order.iter().find(|(k, _)| k == key) {
                    fc.backlog_rank = Some(*rank);
                }
            }
        }
    }

    /// Whether a fresh read of the record already says what this effect showed.
    fn shown(&self, fc: &FeedCard, feed: &Feed) -> bool {
        let state = fc.state.as_deref().unwrap_or("active");
        match self {
            Effect::Starting => !matches!(state, "queued" | "shelved" | "none"),
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
    /// When its command started; None while it waits behind another change to the same record.
    pub running: Option<Instant>,
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
            running: None,
            settled: None,
        });
        self.next
    }

    /// The changes that can start now, marked running: each record's oldest unanswered one, once
    /// nothing ahead of it for that record is still running. A request with no record (a new
    /// intent) runs straight away.
    pub fn ready(&mut self) -> Vec<(u64, usize, Value)> {
        let mut out = vec![];
        for i in 0..self.jobs.len() {
            let job = &self.jobs[i];
            if job.running.is_some() || job.settled.is_some() {
                continue;
            }
            let ahead = job.record().is_some_and(|id| {
                self.jobs[..i].iter().any(|j| {
                    j.settled.is_none() && j.record() == Some(id) && j.machine == job.machine
                })
            });
            if ahead {
                continue;
            }
            let job = &mut self.jobs[i];
            job.running = Some(Instant::now());
            out.push((job.n, job.machine, job.request.clone()));
        }
        out
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
        self.jobs.push(job);
    }

    /// A read of `machine` came in: settled effects it shows, or that have waited long enough, go.
    /// Whether any went.
    pub fn read(&mut self, machine: usize, feed: Option<&Feed>) -> bool {
        let before = self.jobs.len();
        self.jobs.retain_mut(|j| {
            let Some((at, reads)) = j.settled.as_mut() else {
                return true;
            };
            if j.machine == machine {
                *reads = reads.saturating_add(1);
                let shown = feed.is_some_and(|feed| {
                    let (Some(key), Some(effect)) = (&j.key, &j.effect) else {
                        return true;
                    };
                    let source = feed.source.as_deref().unwrap_or("feed");
                    feed.cards
                        .iter()
                        .find(|c| &format!("{source}:{}", c.id) == key)
                        .is_some_and(|c| effect.shown(c, feed))
                });
                if shown || *reads >= SETTLED_READS {
                    return false;
                }
            }
            at.elapsed() < SETTLED_MOST
        });
        self.jobs.len() != before
    }

    /// Settled effects past their time, gone. Whether any went.
    pub fn expire(&mut self) -> bool {
        let before = self.jobs.len();
        self.jobs
            .retain(|j| j.settled.is_none_or(|(at, _)| at.elapsed() < SETTLED_MOST));
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
        for j in self.jobs.iter_mut().filter(|j| j.settled.is_none()) {
            any |= j.from.take().is_some();
        }
        any
    }

    /// The unanswered change to this record that starts work, if one is on its way.
    pub fn starting(&self, id: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| j.settled.is_none() && j.starts() && j.record() == Some(id))
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
            .any(|j| j.settled.is_none() && j.from.is_some())
    }

    /// Whether a new intent is on its way.
    pub fn creating(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| j.settled.is_none() && j.request["action"] == "create")
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
        let mut r = Resolver::default();
        let a = r.add(
            0,
            json!({"action":"move","id":"1","delta":1}),
            None,
            None,
            None,
        );
        let b = r.add(
            0,
            json!({"action":"move","id":"1","delta":1}),
            None,
            None,
            None,
        );
        let c = r.add(0, json!({"action":"close","id":"2"}), None, None, None);
        let d = r.add(0, json!({"action":"create","words":"x"}), None, None, None);
        let first: Vec<u64> = r.ready().into_iter().map(|(n, ..)| n).collect();
        assert_eq!(first, vec![a, c, d]);
        assert!(r.ready().is_empty(), "nothing starts twice");
        r.take(a);
        assert_eq!(
            r.ready().into_iter().map(|(n, ..)| n).collect::<Vec<_>>(),
            vec![b]
        );
        // The same id on another machine is another record.
        let e = r.add(
            1,
            json!({"action":"move","id":"2","delta":1}),
            None,
            None,
            None,
        );
        assert_eq!(
            r.ready().into_iter().map(|(n, ..)| n).collect::<Vec<_>>(),
            vec![e]
        );
        assert_eq!(r.unanswered(), 4, "b, c, d and e");
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
            r.starting("7").is_none(),
            "answered: the feed and the start guard take over"
        );
        let mut fc: FeedCard = serde_json::from_value(
            json!({"id":"7","title":"t","state":"queued","stage":"backlog"}),
        )
        .unwrap();
        Effect::Starting.apply(&mut fc, "s:7");
        assert_eq!(
            (fc.state.as_deref(), fc.stage.as_deref()),
            (Some("active"), Some("build"))
        );
        let mut fc: FeedCard =
            serde_json::from_value(json!({"id":"7","title":"t","stage":"design"})).unwrap();
        Effect::Starting.apply(&mut fc, "s:7");
        assert_eq!(
            fc.stage.as_deref(),
            Some("design"),
            "a working column it names stays"
        );
    }
}
