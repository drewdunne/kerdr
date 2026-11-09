//! Work that goes on after its answer, and how Kerdr follows it (intent 0591).
//!
//! Starting a herdr session and then the work in it, waiting for a session's restore, or moving a
//! conversation into herdr from a terminal outside it can take minutes, longer than Kerdr waits
//! for an answer. A backend that knows this answers at once with `follow` (docs/feed.md), and
//! finishes the work in a process of its own. Kerdr then:
//!
//! - shows the follow's words on the card and the message line ("Starting ARK on arknet-one, then
//!   #581…"), keeping the change shown on its card, so a second key can't start it twice;
//! - asks the same machine `{"action":"progress","follow":"<token>"}` every few seconds, on a
//!   thread of its own, so following never holds the board;
//! - on `running`, updates the words; on `done`, `asks` or `failed`, takes the answer exactly as it
//!   takes a direct answer to the request it followed: it goes into the workspace a reopen opened
//!   while you're still on its card, asks the question, or says what didn't happen; on `unknown`,
//!   says it lost track of it;
//! - past the step's own `limit` and a margin with no end, stops following and says the step is
//!   still going on that machine, as a warning: the card shows how it ends.
//!
//! ⏎ that asked for the work goes in as soon as the card's workspace shows in the feed, as a ⏎
//! remembered on a starting card does. Quitting Kerdr leaves the work going: only the reads stop.
//! A backend that never sends `follow` works exactly as before.
use super::*;
use crate::actions::{Follow, Response};
use crate::resolve::Job;
use serde_json::Value;

/// The first progress request goes this long after the answer, then every POLL_EVERY.
pub const POLL_FIRST: Duration = Duration::from_secs(2);
pub const POLL_EVERY: Duration = Duration::from_secs(3);
/// Progress requests that went unanswered in a row before Kerdr asks less often: its machine may be
/// away a moment.
const POLL_SLOW_AFTER: u32 = 3;
const POLL_SLOW: Duration = Duration::from_secs(10);
/// A step's limit when its answer names none (bin/lib/follow.py's DEFAULT_LIMIT).
pub const DEFAULT_LIMIT: u64 = 300;
/// How long past its limit Kerdr still follows a step: longer than the backend's own grace (30 s),
/// so the backend's answer about a step that overran normally comes first.
pub const MARGIN: Duration = Duration::from_secs(60);

/// One piece of work Kerdr is following.
#[derive(Clone, Debug)]
pub struct Following {
    /// Its token, sent back in each progress request.
    pub token: String,
    /// The machine that answered, which alone knows the token.
    pub machine: usize,
    pub follow: Follow,
    /// Its change in the action resolution tool, which shows it on its card meanwhile.
    pub job: u64,
    /// The request it answered, for its end to be taken as that request's answer: a question's yes
    /// goes back as this request with `override` and the question's token.
    pub request: Value,
    /// The record, by id, and its card, by key.
    pub id: String,
    pub key: Option<String>,
    /// The card it was asked from, as it was: its end asks you something or takes you in only while
    /// you're still on that work. None once Esc let it go.
    pub from: Option<CardSnapshot>,
    /// Whether its end takes you in: a reopen or a move into herdr, or a start ⏎ asked for.
    pub enter: bool,
    /// What it's doing now, as last said.
    pub say: String,
    pub began: Instant,
    /// When the next progress request is due, and whether one is on its way.
    pub next: Instant,
    pub asking: bool,
    /// Progress requests in a row that got no answer.
    pub misses: u32,
    /// The board's `away` when it began: once you've been into herdr since, its end takes you in
    /// only if ⏎ was pressed on its card again.
    pub away: u64,
    /// You went into its workspace yourself: its end takes you nowhere and says nothing more.
    pub entered: bool,
}

impl Following {
    /// The step's own limit, as its answer named it.
    pub fn limit(&self) -> Duration {
        Duration::from_secs(self.follow.limit.unwrap_or(DEFAULT_LIMIT).clamp(10, 3600))
    }

    /// When Kerdr stops following it, with no end heard.
    pub fn deadline(&self) -> Instant {
        self.began + self.limit() + MARGIN
    }

    /// Its words without their closing ellipsis, for the middle of a sentence.
    fn doing(&self) -> String {
        self.say
            .trim()
            .trim_end_matches(['…', '.', ' '])
            .to_string()
    }
}

/// How the end of followed work is being taken: as the answer to the request it followed, with
/// these differences.
#[derive(Clone, Debug, Default)]
pub struct FollowEnd {
    /// It goes into the workspace the answer names while you're still on its card, whatever the
    /// request was: a start ⏎ asked for goes in as a reopen does.
    pub enter: bool,
}

/// What a step is, in a word for a sentence: "start", "move into herdr".
fn step_word(what: &str, request: &Value) -> &'static str {
    match what {
        "start" | "dispatch" => "start",
        "recover" => "restore",
        "move-in" => "move into herdr",
        "reopen" => "reopen",
        _ => match request["action"].as_str() {
            Some("dispatch") => "start",
            Some("reopen") => "reopen",
            Some("close") => "close",
            Some("dismiss") | Some("cancel") => "stop",
            _ => "change",
        },
    }
}

/// Minutes in words, for a limit: "five minutes", "a minute", "90 seconds".
fn how_long(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 || secs % 60 != 0 {
        return format!("{secs} seconds");
    }
    match secs / 60 {
        1 => "a minute".into(),
        2 => "two minutes".into(),
        3 => "three minutes".into(),
        5 => "five minutes".into(),
        10 => "ten minutes".into(),
        n => format!("{n} minutes"),
    }
}

impl App {
    /// A change answered with `follow`: Kerdr follows it from now (see the module's notes). Its card
    /// keeps showing it, in the follow's words.
    pub(super) fn begin_follow(&mut self, mut job: Job, follow: Follow) {
        let id = job.id().unwrap_or_default().to_string();
        let reference = self.reference_for(&id);
        let say = if follow.say.trim().is_empty() {
            match follow.what.as_str() {
                "move-in" => format!("Moving {reference}'s conversation into herdr…"),
                "recover" => format!("Bringing {reference}'s session back…"),
                _ if job.request["action"] == "reopen" => format!("Reopening {reference}…"),
                _ => format!("Starting {reference}…"),
            }
        } else {
            follow.say.trim().to_string()
        };
        // A start or reopen keeps its look on its card while it's followed: one with none (it
        // could only have asked) is given the one it would have had.
        if job.effect.is_none() && job.starts() {
            job.effect = Some(if job.request["action"] == "reopen" {
                crate::resolve::Effect::Reopening
            } else {
                crate::resolve::Effect::Starting
            });
        }
        job.asking = false;
        let enter = job.request["action"] == "reopen" || job.by_enter;
        let from = job.from.clone();
        let key = job.key.clone();
        let following = Following {
            token: follow.token.clone(),
            machine: job.machine,
            job: job.n,
            request: job.request.clone(),
            id: id.clone(),
            key: key.clone(),
            from: from.clone(),
            enter,
            say: say.clone(),
            began: Instant::now(),
            next: Instant::now() + POLL_FIRST,
            asking: false,
            misses: 0,
            away: self.away,
            entered: job.entered,
            follow,
        };
        let life = following.limit() + MARGIN;
        // A newer follow of the same record takes the place of an older one.
        let replaced: Vec<u64> = self
            .follows
            .iter()
            .filter(|f| f.id == id && f.machine == following.machine)
            .map(|f| f.job)
            .collect();
        self.follows
            .retain(|f| !(f.id == id && f.machine == following.machine));
        for n in replaced {
            self.resolver.end_follow(n, false);
        }
        self.resolver.settle_following(job, say.clone(), life);
        self.follows.push(following);
        self.rebuild();
        self.say(say, Tone::Info);
        // ⏎ asked for it: in as soon as its workspace shows, as a ⏎ remembered on a starting card
        // goes (intent 0491), even before the answer that it's done.
        if enter
            && !self.answer_went_in
            && let (Some(from), Some(key)) = (from, key)
            && self.still_on(&from, None)
        {
            self.remember_enter(&key);
        }
    }

    /// The followed work for this record, if Kerdr is following any.
    pub fn following(&self, id: &str) -> Option<&Following> {
        self.follows.iter().find(|f| f.id == id)
    }

    /// The followed work shown on this card, if any.
    pub fn following_card(&self, card: &Card) -> Option<&Following> {
        let id = card.detail.as_ref()?.id.as_str();
        self.follows
            .iter()
            .find(|f| f.key.as_deref() == Some(card.key.as_str()) || f.id == id)
    }

    /// The progress requests due now, each with its token and machine, marked as on their way.
    /// Followed work whose machine isn't answering waits for it; past its deadline it's given up.
    pub fn progress_due(&mut self) -> Vec<(String, usize, Value)> {
        let now = Instant::now();
        let mut due = vec![];
        let mut late = vec![];
        for (i, f) in self.follows.iter_mut().enumerate() {
            if f.asking || now < f.next {
                continue;
            }
            let live = self
                .machines
                .get(f.machine)
                .is_some_and(|m| m.conn.is_live());
            if !live {
                if now >= f.deadline() {
                    late.push(i);
                } else {
                    f.next = now + POLL_EVERY;
                }
                continue;
            }
            f.asking = true;
            due.push((
                f.token.clone(),
                f.machine,
                crate::actions::progress_request(&f.token),
            ));
        }
        for i in late.into_iter().rev() {
            self.follow_late(i);
        }
        due
    }

    /// A progress request's answer. Whether the followed work ended, so its machine's feed is
    /// worth reading now.
    pub fn progress_answered(&mut self, token: &str, result: Result<Response, String>) -> bool {
        let Some(i) = self.follows.iter().position(|f| f.token == token) else {
            return false;
        };
        let now = Instant::now();
        let f = &mut self.follows[i];
        f.asking = false;
        let late = now >= f.deadline();
        let response = match result {
            Ok(response) => response,
            Err(_) => {
                // No answer this time (its machine away a moment, a slow read): asked again, less
                // often after a few, until its deadline.
                f.misses += 1;
                if late {
                    self.follow_late(i);
                    return true;
                }
                f.next = now
                    + if f.misses >= POLL_SLOW_AFTER {
                        POLL_SLOW
                    } else {
                        POLL_EVERY
                    };
                return false;
            }
        };
        f.misses = 0;
        match response.state.as_deref() {
            Some("running") => {
                let said = response
                    .say
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != f.say)
                    .map(str::to_string);
                if let Some(say) = said {
                    let old = std::mem::replace(&mut f.say, say.clone());
                    let n = f.job;
                    if self.resolver.follow_say(n, &say) {
                        self.rebuild();
                    }
                    // The words on screen follow it, but never over something said since.
                    if self.message.as_ref().is_some_and(|(t, _, _)| *t == old) {
                        self.say(say, Tone::Info);
                    }
                }
                if late {
                    self.follow_late(i);
                    return true;
                }
                self.follows[i].next = now + POLL_EVERY;
                false
            }
            Some("done") => {
                self.follow_ended(i, response, true);
                true
            }
            Some("asks" | "failed") => {
                self.follow_ended(i, response, false);
                true
            }
            // `unknown`, or an answer that isn't a progress answer at all.
            _ => {
                self.follow_lost(i);
                true
            }
        }
    }

    /// Followed work has finished, or needs a yes, or couldn't: its answer is taken as the answer to
    /// the request it followed would have been, had that waited.
    fn follow_ended(&mut self, i: usize, mut response: Response, done: bool) {
        let f = self.follows.remove(i);
        let done = done && response.ok;
        if self.resolver.end_follow(f.job, done) {
            self.rebuild();
        }
        self.unsay(&f.say);
        response.state = None;
        // Its end follows nothing more: a second follow here would be a second detached worker.
        response.follow = None;
        let went_in = f.entered;
        // In by its end only while you're still on its card and haven't been into herdr since,
        // unless ⏎ on it asked again; a question likewise only where you are.
        let entered_again = f.key.is_some() && self.enter_when_open == f.key;
        let from = f
            .from
            .clone()
            .filter(|_| !went_in && (f.away == self.away || entered_again));
        if entered_again {
            if done && (response.workspace.is_none() || response.session.is_none()) {
                // Done, with no workspace named yet: the ⏎ goes in when the feed shows one, as after
                // any start answered yes.
                self.started = Some((f.id.clone(), Instant::now()));
            } else {
                self.forget_enter();
            }
        }
        self.action_from = from;
        self.answer_went_in = went_in;
        self.follow_end = Some(FollowEnd { enter: f.enter });
        self.answer_stderr = String::new();
        self.finish_action(f.machine, f.request.clone(), Ok(response));
        self.follow_end = None;
        self.action_from = None;
        self.answer_went_in = false;
    }

    /// Past its limit and the margin with no end heard: Kerdr stops following, and says the step is
    /// still going on its machine and the card will show how it ends.
    fn follow_late(&mut self, i: usize) {
        let f = self.follows.remove(i);
        if self.resolver.end_follow(f.job, false) {
            self.rebuild();
        }
        self.unsay(&f.say);
        if f.key.is_some() && self.enter_when_open == f.key {
            self.forget_enter();
        }
        let machine = self
            .machines
            .get(f.machine)
            .map(|m| m.name())
            .unwrap_or_default();
        let reference = self.reference_for(&f.id);
        let step = step_word(&f.follow.what, &f.request);
        let how = how_long(f.limit() + MARGIN);
        self.say(
            format!(
                "Kerdr stopped following {reference}'s {step} after {how} ({}). It may still be going on {machine}; {reference}'s card shows how it ends.",
                f.doing()
            ),
            Tone::Warn,
        );
    }

    /// The machine has no record of the followed work (expired, or never its): Kerdr says so
    /// plainly, and the card shows how it stands.
    fn follow_lost(&mut self, i: usize) {
        let f = self.follows.remove(i);
        if self.resolver.end_follow(f.job, false) {
            self.rebuild();
        }
        self.unsay(&f.say);
        if f.key.is_some() && self.enter_when_open == f.key {
            self.forget_enter();
        }
        let machine = self
            .machines
            .get(f.machine)
            .map(|m| m.name())
            .unwrap_or_default();
        let reference = self.reference_for(&f.id);
        let step = step_word(&f.follow.what, &f.request);
        self.say(
            format!(
                "Kerdr lost track of {reference}'s {step}: {machine} has no record of it any more. {reference}'s card shows how it stands."
            ),
            Tone::Warn,
        );
    }

    /// Esc: followed work still finishes, but takes you nowhere and asks nothing where you are.
    /// Whether any would have.
    pub(super) fn let_go_follows(&mut self) -> bool {
        let mut any = false;
        for f in &mut self.follows {
            any |= f.from.take().is_some();
            f.enter = false;
        }
        any
    }

    /// You went into this card's workspace yourself: its followed work's end takes you nowhere.
    pub(super) fn entered_follow(&mut self, key: &str) {
        for f in self
            .follows
            .iter_mut()
            .filter(|f| f.key.as_deref() == Some(key))
        {
            f.entered = true;
        }
    }

    /// ⏎ or another key on a card whose work Kerdr is following, said: nothing else may change it
    /// until it ends, as its backend would refuse it. None when nothing is followed for it, or its
    /// workspace shows already (the usual rules speak).
    pub(super) fn follow_holds(&mut self, card: &Card, action: &str) -> Option<bool> {
        if card.is_open_workspace() {
            return None;
        }
        let f = self.following_card(card)?;
        let secs = f.began.elapsed().as_secs();
        let doing = f.doing();
        let reference = card.reference.clone();
        let text = if action == "enter" {
            if f.enter || f.request["action"] == "dispatch" {
                format!(
                    "{doing} ({secs} s so far). Kerdr goes in when it's open; Esc stops waiting."
                )
            } else {
                format!("{doing} ({secs} s so far).")
            }
        } else {
            format!("{doing} ({secs} s so far), so nothing else changes {reference} until it's done.")
        };
        let goes_in = action == "enter" && (f.enter || f.request["action"] == "dispatch");
        self.say(text, Tone::Info);
        if goes_in {
            let key = card.key.clone();
            if let Some(f) = self
                .follows
                .iter_mut()
                .find(|f| f.key.as_deref() == Some(key.as_str()))
            {
                f.enter = true;
            }
            self.remember_enter(&key);
        }
        Some(true)
    }

    /// What a card's own screen and its peek say while its work is followed: "◐ Starting ARK on
    /// arknet-one, then #581… · 40 s so far · Kerdr goes in when it's open".
    pub(super) fn follow_line(&self, card: &Card) -> Option<String> {
        let f = self.following_card(card)?;
        let secs = f.began.elapsed().as_secs();
        let then = if self.enter_when_open.as_deref() == Some(card.key.as_str()) {
            " · Kerdr goes in when it's open · Esc stops waiting"
        } else if f.request["action"] == "reopen" || f.request["action"] == "dispatch" {
            " · ⏎ goes in as soon as it's open"
        } else {
            ""
        };
        Some(format!("◐ {} · {secs} s so far{then}", f.say.trim()))
    }

    /// The machines whose feed is worth reading often for followed work: those whose ⏎ waits on it.
    pub(super) fn follows_awaited_on(&self) -> Vec<usize> {
        let Some(key) = self.enter_when_open.as_deref() else {
            return vec![];
        };
        self.follows
            .iter()
            .filter(|f| f.key.as_deref() == Some(key))
            .map(|f| f.machine)
            .filter(|&m| self.machines.get(m).is_some_and(|m| m.conn.is_live()))
            .collect()
    }

    /// What q says about followed work, which carries on without Kerdr: "#581 is still starting on
    /// arknet-one (Starting ARK, then #581)".
    pub(super) fn follows_running(&self) -> Option<String> {
        let f = self.follows.first()?;
        let reference = self.reference_for(&f.id);
        let machine = self
            .machines
            .get(f.machine)
            .map(|m| m.name())
            .unwrap_or_default();
        let more = match self.follows.len() {
            1 => String::new(),
            n => format!(" ({} more like it)", n - 1),
        };
        Some(format!(
            "{reference}'s {} is still going on {machine} ({}){more}; it carries on there after Kerdr quits.",
            step_word(&f.follow.what, &f.request),
            f.doing()
        ))
    }
}
