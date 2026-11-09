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
    if secs < 60 || !secs.is_multiple_of(60) {
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
            format!(
                "{doing} ({secs} s so far), so nothing else changes {reference} until it's done."
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{demo_app, press};
    use serde_json::json;

    fn key(app: &mut App, code: KeyCode) -> Action {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// #39, under way on build-box, whose workspace has closed: set in that machine's feed and
    /// herdr's view, so the board keeps it so through every rebuild. Selected: ⏎ reopens it.
    pub(crate) fn closed_card(app: &mut App) -> (String, String) {
        app.cfg.actions.command = Some("test-adapter".into());
        let mi = app.machine_index("build-box").unwrap();
        if let Some(data) = app.machines[mi].data.as_mut() {
            for snap in data.snapshots.values_mut().flatten() {
                let gone: Vec<String> = snap
                    .workspaces
                    .iter()
                    .filter(|w| w.label.starts_with("39-"))
                    .map(|w| w.workspace_id.clone())
                    .collect();
                snap.workspaces.retain(|w| !gone.contains(&w.workspace_id));
                snap.agents.retain(|a| !gone.contains(&a.workspace_id));
            }
        }
        let feed = app.machines[mi].feed.as_mut().unwrap();
        let fc = feed.cards.iter_mut().find(|c| c.id == "39").unwrap();
        fc.workspace = None;
        fc.attention = Some("idle".into());
        fc.blockers.clear();
        fc.home = Some(crate::feed::Home {
            machine: Some("build-box".into()),
            session: Some("api".into()),
        });
        fc.resume = Some(crate::feed::Resume {
            agent: Some("codex".into()),
            conversation: Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41".into()),
        });
        app.rebuild();
        let key = "demo:39".to_string();
        let card = app.card(&key).expect("#39 on the board").clone();
        assert!(!card.is_open_workspace() && card.reopenable(), "{card:?}");
        app.sel = Selection {
            col: card.stage.index(),
            key: Some(key.clone()),
        };
        (key, "39".into())
    }

    /// What the event loop does with a key's change: into the action resolution tool, handed to
    /// the runner.
    fn sent(app: &mut App, action: Action) -> (usize, Value) {
        let Action::Mutate { machine, request } = action else {
            panic!("nothing was sent: {:?}", app.message())
        };
        app.queue_action(machine, request.clone());
        app.resolver.ready();
        (machine, request)
    }

    /// The change's answer, as the event loop takes it.
    fn answer(app: &mut App, machine: usize, request: &Value, reply: Value) {
        let n = app
            .resolver
            .unanswered_for(request["action"].as_str().unwrap(), request["id"].as_str())
            .expect("the change is on its way");
        let response = crate::actions::parse_response(&reply.to_string()).unwrap();
        app.finish_job(n, request.clone(), Ok(response));
        let _ = machine;
    }

    const TOKEN: &str = "f-0123456789abcdef0123";

    fn follow_reply(id: &str, what: &str, say: &str) -> Value {
        json!({"ok": true,
               "message": "Starting web; ⏎ again in a minute goes in.",
               "id": id, "session": "api",
               "follow": {"token": TOKEN, "what": what, "id": id, "session": "api",
                          "say": say, "limit": 300}})
    }

    /// The next progress request, as if it were due now.
    fn poll(app: &mut App) -> Value {
        for f in &mut app.follows {
            f.next = Instant::now();
        }
        let due = app.progress_due();
        assert_eq!(due.len(), 1, "one progress request due");
        let (token, _, request) = due.into_iter().next().unwrap();
        assert_eq!(token, TOKEN);
        request
    }

    fn progress(app: &mut App, reply: Value) -> bool {
        let response = crate::actions::parse_response(&{
            let mut r = reply;
            if r.get("message").is_none() {
                r["message"] = json!("");
            }
            r.to_string()
        })
        .unwrap();
        app.progress_answered(TOKEN, Ok(response))
    }

    /// What the board shows of this card's status: the feed's, with changes on their way laid over.
    fn status_shown(app: &App, key: &str) -> Option<String> {
        let source = key.rsplit_once(':').map(|(s, _)| s).unwrap();
        let id = key.rsplit_once(':').map(|(_, i)| i).unwrap();
        app.resolver
            .overlay(&app.machines)
            .iter()
            .filter_map(|m| m.feed.as_ref())
            .filter(|f| f.source.as_deref().unwrap_or("feed") == source)
            .flat_map(|f| f.cards.iter())
            .find(|c| c.id == id)
            .and_then(|c| c.status.clone())
    }

    #[test]
    fn a_followed_reopen_shows_its_words_and_goes_in_when_done() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        assert_eq!(request["action"], "reopen");
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(
                &id,
                "recover",
                "Bringing web back on its machine, then the work…",
            ),
        );
        // Its words, not the message meant for a Kerdr that doesn't follow.
        assert_eq!(
            app.message().unwrap().0,
            "Bringing web back on its machine, then the work…"
        );
        assert_eq!(app.follows.len(), 1);
        assert_eq!(
            status_shown(&app, &key).as_deref(),
            Some("Bringing web back on its machine, then the work…")
        );
        // ⏎ asked for it: in as soon as it's open, without another key.
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        // Nothing asked until its first wait is up; then the same machine, by token.
        assert!(app.progress_due().is_empty());
        let request = poll(&mut app);
        assert_eq!(request, json!({"action": "progress", "follow": TOKEN}));
        // One on its way at a time.
        assert!(app.progress_due().is_empty());
        assert!(!progress(
            &mut app,
            json!({"ok": true, "state": "running", "say": "Reopening it in web…"})
        ));
        assert_eq!(app.message().unwrap().0, "Reopening it in web…");
        assert_eq!(
            status_shown(&app, &key).as_deref(),
            Some("Reopening it in web…")
        );
        poll(&mut app);
        assert!(progress(
            &mut app,
            json!({"ok": true, "state": "done", "message": "Reopened, resuming its Codex conversation.",
                   "id": id, "session": "web", "workspace": "w7", "label": "41-checkout"})
        ));
        assert!(app.follows.is_empty());
        let open = app
            .open_after
            .clone()
            .expect("it goes in, as on a reopen's answer");
        assert_eq!(
            (
                open.session.as_str(),
                open.workspace.as_str(),
                open.from.as_str()
            ),
            ("web", "w7", key.as_str())
        );
        assert!(!app.failing());
    }

    fn key_enter(app: &mut App) -> Action {
        key(app, KeyCode::Enter)
    }

    #[test]
    fn a_start_on_an_account_joined_to_a_move_under_way_says_the_account_wasnt_applied() {
        let mut app = demo_app();
        let (_, id) = closed_card(&mut app);
        let machine = app.machine_index("build-box").unwrap();
        let request = json!({"action":"reopen","id":id,"session":"api","account":"ARK_C"});
        let (machine, request) = sent(
            &mut app,
            Action::Mutate {
                machine,
                request: request.clone(),
            },
        );
        let mut reply = follow_reply(&id, "move-in", "Moving #39's conversation into herdr…");
        reply["account_unchanged"] = json!(true);
        reply["message"] = json!(
            "#39's conversation is already moving into herdr, so it stays on its own account, not ARK_C."
        );
        answer(&mut app, machine, &request, reply);
        assert_eq!(app.follows.len(), 1, "the move is followed all the same");
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Info);
        assert!(said.contains("not ARK_C"), "{said}");
    }

    #[test]
    fn a_second_enter_while_it_is_followed_says_what_it_is_doing_and_sends_nothing() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "start", "Starting web on this machine, then the work…"),
        );
        app.forget_enter();
        // Its change has left the resolver (the feed showed it started) but the follow goes on.
        let n = app.follows[0].job;
        app.resolver.end_follow(n, false);
        assert_eq!(key_enter(&mut app), Action::None);
        let said = app.message().unwrap().0.to_string();
        assert!(
            said.starts_with("Starting web on this machine, then the work ("),
            "{said}"
        );
        assert!(
            said.ends_with("Kerdr goes in when it's open; Esc stops waiting."),
            "{said}"
        );
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        // d, x and the rest wait for it too.
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(app.message().unwrap().0.contains("nothing else changes"));
    }

    #[test]
    fn a_followed_start_that_asks_puts_the_question_and_the_yes_carries_its_token() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = {
            let c = app
                .board
                .cards
                .iter()
                .find(|c| crate::app::waiting_work(c))
                .unwrap();
            (c.key.clone(), c.detail.as_ref().unwrap().id.clone())
        };
        for c in app.board.cards.iter_mut().filter(|c| c.key == key) {
            c.detail.as_mut().unwrap().kickoff = None;
        }
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.sel = Selection {
            col: app.board.cards[i].stage.index(),
            key: Some(key.clone()),
        };
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        assert_eq!(request["action"], "dispatch");
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "start", "Starting web, then the work…"),
        );
        poll(&mut app);
        progress(
            &mut app,
            json!({"ok": false, "state": "asks", "confirmation": true, "confirm": "0123456789abcdef",
                   "message": "web has 3 workspaces, its usual limit. Start anyway?",
                   "session": "web", "used": 3, "limit": 3}),
        );
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("expected the question, got {:?}", app.mode)
        };
        assert!(title.starts_with("Start"), "{title}");
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("the yes sent nothing")
        };
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["override"], true);
        assert_eq!(request["confirm"], "0123456789abcdef");
        assert!(app.follows.is_empty());
    }

    #[test]
    fn a_followed_step_that_fails_says_what_didnt_happen_and_the_card_is_as_it_was() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "recover", "Bringing web back…"),
        );
        poll(&mut app);
        progress(
            &mut app,
            json!({"ok": false, "state": "failed",
                   "message": "herdr session web wouldn't start (its socket folder is full)."}),
        );
        assert!(app.failing());
        let said = app.message().unwrap().0.to_string();
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(
            said,
            format!(
                "{reference} didn't reopen: herdr session web wouldn't start (its socket folder is full)."
            )
        );
        assert_eq!(app.open_after, None);
        assert!(app.enter_when_open.is_none());
        assert_ne!(
            status_shown(&app, &key).as_deref(),
            Some("Bringing web back…")
        );
    }

    #[test]
    fn a_machine_with_no_record_of_it_is_said_plainly() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "move-in", "Moving it into herdr…"),
        );
        poll(&mut app);
        progress(
            &mut app,
            json!({"ok": false, "state": "unknown",
                   "message": "This machine has no record of that any more; the card shows how it stands."}),
        );
        let reference = app.card(&key).unwrap().reference.clone();
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(
            said.starts_with(&format!(
                "Kerdr lost track of {reference}'s move into herdr:"
            )),
            "{said}"
        );
        assert!(app.follows.is_empty());
    }

    #[test]
    fn past_its_limit_and_a_margin_it_says_the_step_goes_on_as_a_warning() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "start", "Starting web, then the work…"),
        );
        // Answers with no end, and its time is up.
        app.follows[0].began =
            Instant::now() - Duration::from_secs(300) - MARGIN - Duration::from_secs(1);
        poll(&mut app);
        assert!(progress(
            &mut app,
            json!({"ok": true, "state": "running", "say": "Starting web, then the work…"})
        ));
        let reference = app.card(&key).unwrap().reference.clone();
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn, "a warning, never a failure");
        assert!(!app.failing());
        assert!(said.contains("It may still be going on"), "{said}");
        assert!(
            said.contains(&format!("{reference}'s card shows how it ends")),
            "{said}"
        );
        assert!(app.follows.is_empty());
        assert!(app.enter_when_open.is_none());
    }

    #[test]
    fn unanswered_reads_are_asked_again_less_often_and_a_machine_away_past_the_deadline_gives_up() {
        let mut app = demo_app();
        let (_, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "start", "Starting web…"),
        );
        for n in 1..=POLL_SLOW_AFTER {
            poll(&mut app);
            assert!(!app.progress_answered(TOKEN, Err("ssh: connection reset".into())));
            assert_eq!(app.follows[0].misses, n);
        }
        let wait = app.follows[0]
            .next
            .saturating_duration_since(Instant::now());
        assert!(
            wait > POLL_EVERY,
            "asked less often after {POLL_SLOW_AFTER} misses: {wait:?}"
        );
        // Its machine stops answering altogether, and its time runs out: given up, as a warning.
        app.machines[machine].conn = Conn::Reconnecting {
            since: Local::now(),
            attempt: 1,
            retry_at: Local::now(),
            detail: "gone".into(),
        };
        app.follows[0].began = Instant::now() - Duration::from_secs(400);
        app.follows[0].next = Instant::now();
        assert!(app.progress_due().is_empty());
        assert!(app.follows.is_empty());
        assert_eq!(app.message().unwrap().1, Tone::Warn);
    }

    #[test]
    fn esc_lets_it_go_and_its_end_says_where_it_is_instead_of_going_in() {
        let mut app = demo_app();
        let (_, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "recover", "Bringing web back…"),
        );
        key(&mut app, KeyCode::Esc);
        assert!(app.message().unwrap().0.starts_with("Stopped waiting"));
        poll(&mut app);
        progress(
            &mut app,
            json!({"ok": true, "state": "done", "message": "Reopened, resuming its Codex conversation.",
                   "session": "web", "workspace": "w7"}),
        );
        assert_eq!(app.open_after, None);
        assert!(app.message().unwrap().0.ends_with("⏎ on its card goes in."));
    }

    #[test]
    fn going_in_yourself_means_its_end_takes_you_nowhere() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "move-in", "Moving it into herdr…"),
        );
        app.went_into(&key);
        poll(&mut app);
        progress(
            &mut app,
            json!({"ok": true, "state": "done", "message": "Moved into herdr.", "session": "web", "workspace": "w7"}),
        );
        assert_eq!(app.open_after, None);
    }

    #[test]
    fn its_workspace_showing_takes_a_waiting_enter_in_before_the_answer_that_it_is_done() {
        let mut app = demo_app();
        let (key, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "move-in", "Moving it into herdr…"),
        );
        assert_eq!(
            app.enter_when_open_due(),
            None,
            "nothing open yet: still waiting"
        );
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        // The feed shows its workspace.
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w7".into());
        app.board.cards[i].session = Some("web".into());
        assert_eq!(app.enter_when_open_due(), Some(Action::Jump(key.clone())));
    }

    #[test]
    fn a_start_d_asked_for_says_where_it_is_and_one_enter_asked_for_goes_in() {
        for by_enter in [false, true] {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            let (key, id) = {
                let c = app
                    .board
                    .cards
                    .iter()
                    .find(|c| crate::app::waiting_work(c))
                    .unwrap();
                (c.key.clone(), c.detail.as_ref().unwrap().id.clone())
            };
            for c in app.board.cards.iter_mut().filter(|c| c.key == key) {
                c.detail.as_mut().unwrap().kickoff = None;
            }
            let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
            app.sel = Selection {
                col: app.board.cards[i].stage.index(),
                key: Some(key.clone()),
            };
            let action = if by_enter {
                key_enter(&mut app)
            } else {
                press(&mut app, 'd')
            };
            let (machine, request) = sent(&mut app, action);
            answer(
                &mut app,
                machine,
                &request,
                follow_reply(&id, "start", "Starting web, then the work…"),
            );
            poll(&mut app);
            progress(
                &mut app,
                json!({"ok": true, "state": "done", "message": "Started in web.", "session": "web",
                       "workspace": "w9", "label": "9-new"}),
            );
            if by_enter {
                assert!(app.open_after.is_some(), "⏎ goes in");
            } else {
                assert_eq!(
                    app.open_after, None,
                    "d starts it and leaves you where you are"
                );
                assert_eq!(
                    app.message().unwrap().0,
                    "Started in web. ⏎ on its card goes in."
                );
            }
        }
    }

    #[test]
    fn q_says_followed_work_carries_on_without_kerdr() {
        let mut app = demo_app();
        let (_, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            follow_reply(&id, "start", "Starting web, then the work…"),
        );
        let said = app.still_running().unwrap();
        assert!(
            said.contains("carries on there after Kerdr quits"),
            "{said}"
        );
        assert!(said.contains("Starting web, then the work"), "{said}");
    }

    #[test]
    fn an_answer_without_follow_is_taken_as_ever() {
        let mut app = demo_app();
        let (_, id) = closed_card(&mut app);
        let enter = key_enter(&mut app);
        let (machine, request) = sent(&mut app, enter);
        answer(
            &mut app,
            machine,
            &request,
            json!({"ok": true, "message": "Reopened.", "id": id, "session": "web", "workspace": "w7"}),
        );
        assert!(app.follows.is_empty());
        assert!(app.open_after.is_some());
    }
}
