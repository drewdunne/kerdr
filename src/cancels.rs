//! Two stops the backend's `cancel` does for release 3 (intent 0591), each sent only to a backend
//! whose feed lists it in `supports`; any other keeps 0.33.0's words.
//!
//! - Esc on a start that had already begun (K1-44): Kerdr asks, then stops it once its workspace
//!   opens with `cancel` and `"to":"backlog"`, which puts the card back in the backlog rather than
//!   in Won't do. An older backend would read that as X and set it aside, so it's never sent there.
//! - X on a workspace with no intent (K1-27): Kerdr asks, naming what stops, then sends `cancel`
//!   with no `id`, only the workspace the yes was for; everything there is stopped and it's closed.

use super::controls::short;
use super::*;
use crate::actions::Response;
use crate::model::{CardSnapshot, Source};
use serde_json::{Value, json};

/// What a backend's feed lists in `supports` when its `cancel` takes `"to":"backlog"`.
pub const CANCEL_BACKLOG: &str = "cancel-backlog";

/// What a backend's feed lists in `supports` when its `cancel` with no `id` closes a workspace no
/// intent is behind.
pub const CANCEL_WORKSPACE: &str = "cancel-workspace";

/// The action of Kerdr's own question whether to stop a start Esc came too late for: never sent
/// to a backend.
pub(crate) const STOP_START: &str = "kerdr:stop-start";

/// What X said before, and still says to a backend without `cancel-workspace`.
pub(crate) const NO_INTENT: &str = "This workspace has no intent, so X has no work to cancel; close the workspace in herdr instead.";

/// Whether a request is X's on a workspace with no intent.
pub(crate) fn is_bare_cancel(request: &Value) -> bool {
    request["action"] == "cancel" && request.get("id").is_none()
}

/// Whether a request is Esc's stop of a start, back to the backlog.
pub(crate) fn is_stop_start(request: &Value) -> bool {
    request["action"] == "cancel" && request["to"] == "backlog"
}

/// A start Esc came too late for, which Kerdr asked whether to stop (intent 0591, K1-44). Bound to
/// its own change in the action resolution tool, whose number it keeps through a resend to its
/// record's real home and through a follow: never to its record's number, which another source's
/// card can share (Codex's fifth look).
#[derive(Clone, Debug)]
pub struct StopStart {
    /// Its start's change, by Kerdr's number for it.
    pub n: u64,
    pub id: String,
    /// Its card, by key.
    pub key: Option<String>,
    /// He said yes before its start answered: stopped as soon as it says it started.
    pub wanted: bool,
    /// How its start ended, when that came while he decided.
    pub ended: Option<StartEnd>,
}

/// How a start ended, by its own answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartEnd {
    /// It started, where its answer says.
    Started(Place),
    /// It started on the machine its answer names, which Kerdr isn't set up to reach: nothing of
    /// it can be stopped from here, and no other machine stands in for it (Codex's sixth look).
    Unreachable(String),
    /// It asked a question first, so nothing started.
    Asked,
    /// Refused, or done only in part: nothing of it runs.
    Refused,
    /// Its answer was lost: nobody can tell whether it started.
    Unknown,
}

/// Where a start runs: the machine its workspace runs on, and that workspace, as its answer named
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub machine: usize,
    pub session: Option<String>,
    pub workspace: Option<String>,
    pub tab: Option<String>,
}

impl Place {
    /// The stop of what runs there, back to the backlog: named by its workspace when the start
    /// named one, with as much of where as it said, so its backend stops that and nothing else.
    /// A workspace named without its session is still named, never dropped for a stop by number
    /// alone (Codex's sixth look, swept).
    fn stop(&self, id: &str) -> Value {
        let mut cancel = json!({"action":"cancel","id":id,"to":"backlog"});
        if let Some(workspace) = &self.workspace {
            if let Some(session) = &self.session {
                cancel["session"] = json!(session);
            }
            cancel["workspace"] = json!(workspace);
            if let Some(tab) = &self.tab {
                cancel["tab"] = json!(tab);
            }
        }
        cancel
    }

    /// Whether the board shows this card running exactly here.
    fn holds(&self, card: &Card) -> bool {
        card.is_open_workspace()
            && card.machine_index == self.machine
            && card.session == self.session
            && card.workspace_id == self.workspace
            && (self.tab.is_none() || card.tab_id == self.tab)
    }

    /// Where the board shows this card running now.
    fn of(card: &Card) -> Option<Place> {
        card.is_open_workspace().then(|| Place {
            machine: card.machine_index,
            session: card.session.clone(),
            workspace: card.workspace_id.clone(),
            tab: card.tab_id.clone(),
        })
    }
}

impl App {
    /// Whether this machine's backend takes `what`, by its feed's `supports`.
    pub(crate) fn cancels_on(&self, machine: usize, what: &str) -> bool {
        self.actions_on()
            && self
                .machines
                .get(machine)
                .and_then(|m| m.feed.as_ref())
                .is_some_and(|f| f.supports(what))
    }

    /// What X stops on this card, in a question's words: "its Claude Code agent (mid-step)".
    pub(super) fn stops_what(&self, card: &Card, machine: usize) -> String {
        match card.agents.as_slice() {
            [] => "whatever runs there".to_string(),
            [a] => format!(
                "its {} agent{}",
                self.providers(machine).program_label(&a.agent),
                match a.status.as_str() {
                    "working" => " (mid-step)",
                    "blocked" => " (at a question)",
                    "idle" | "done" => " (idle)",
                    _ => "",
                }
            ),
            agents => format!("its {} agents", agents.len()),
        }
    }

    /// X on a workspace with no intent (K1-27): asked first, naming what stops and where; the yes
    /// sends `cancel` for that workspace only. To a backend without it, the old words.
    pub(super) fn bare_cancel(&mut self, card: &Card) -> Action {
        let machine = card.machine_index;
        let place = card.session.clone().zip(card.workspace_id.clone());
        let Some((session, workspace)) = place.filter(|_| {
            card.source == Source::Workspace && self.cancels_on(machine, CANCEL_WORKSPACE)
        }) else {
            self.say(NO_INTENT, Tone::Info);
            return Action::None;
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        if self.resolver.unanswered_on(&card.key).is_some() {
            self.say(
                format!("{} is being closed already.", card.workspace_name()),
                Tone::Info,
            );
            return Action::None;
        }
        let name = card
            .workspace_label
            .clone()
            .unwrap_or_else(|| workspace.clone());
        let what = self.stops_what(card, machine);
        self.mode = Mode::Confirm {
            machine,
            request: json!({"action":"cancel","session":session,"workspace":workspace}),
            title: format!("Stop {what} and close workspace {name} ({session})?"),
            message: "Everything running in it stops, and it closes with every tab in it. No intent is behind it, so no record changes. Anything it hadn't committed stays as it left it.".to_string(),
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
            queue: false,
            ready_at: None,
            plain: false,
            choices: vec![],
            enter_queues: false,
        };
        Action::None
    }

    /// The workspace a bare cancel's yes was for, as the board shows it now: still there, on the
    /// same machine, and still with no intent behind it.
    fn bare_card_now(&self, machine: usize, request: &Value) -> Option<&Card> {
        self.board.cards.iter().find(|c| {
            c.machine_index == machine
                && c.session.as_deref() == request["session"].as_str()
                && c.workspace_id.as_deref() == request["workspace"].as_str()
                && c.tab_id.is_none()
        })
    }

    /// Whether X's yes on a workspace with no intent still fits it.
    pub(super) fn bare_cancel_applies(
        &self,
        machine: usize,
        request: &Value,
        target: Option<&CardSnapshot>,
    ) -> bool {
        self.bare_card_now(machine, request).is_some_and(|c| {
            c.detail.is_none() && target.is_some_and(|t| t.key == c.key && t.same_work(c))
        })
    }

    /// Its yes when it no longer fits: what changed is said, and nothing is sent.
    pub(super) fn bare_cancel_again(&mut self, machine: usize, request: &Value) -> Action {
        let said = match self.bare_card_now(machine, request) {
            Some(c) if c.detail.is_some() => format!(
                "{} holds {}'s work now, so nothing was stopped: X on its card stops it.",
                c.workspace_name(),
                c.reference
            ),
            Some(c) => format!(
                "{} changed while you decided, so nothing was stopped: X asks again.",
                c.workspace_name()
            ),
            None => "That workspace closed while you decided; nothing needed stopping.".to_string(),
        };
        self.say(said, Tone::Info);
        Action::None
    }

    /// Its answer. A question its backend asks is put as any is, with its token; anything else is
    /// said in its backend's words.
    pub(super) fn bare_cancel_answered(
        &mut self,
        machine: usize,
        request: Value,
        result: Result<Response, String>,
        from: Option<CardSnapshot>,
    ) {
        let name = from
            .as_ref()
            .and_then(|f| self.card(&f.key))
            .map(|c| c.workspace_name())
            .unwrap_or_else(|| "That workspace".to_string());
        match result {
            Ok(r) if r.confirmation && r.confirm.is_some() => {
                let mut request = request;
                request["confirm"] = json!(r.confirm);
                if !from.as_ref().is_some_and(|f| self.still_on(f, None)) {
                    self.say(
                        short(format!("{name} wasn't closed: {} X asks again.", r.message)),
                        Tone::Warn,
                    );
                    return;
                }
                self.mode = Mode::Confirm {
                    machine,
                    request,
                    title: r.message.clone(),
                    message: r.note.clone().unwrap_or_default(),
                    back: Box::new(self.mode.clone()),
                    target: from.map(Box::new),
                    queue: false,
                    ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
                    plain: false,
                    choices: r.choices.clone(),
                    enter_queues: false,
                };
            }
            Ok(r) if r.ok => {
                let mut said = if r.message.trim().is_empty() {
                    format!("{name} is closed.")
                } else {
                    r.message.clone()
                };
                if let Some(note) = r.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    said = format!("{said} {note}");
                }
                self.say(short(said), Tone::Info);
            }
            Ok(r) if r.partial => self.say(short(r.message), Tone::Warn),
            Ok(r) => self.fail(short(format!("{name} wasn't closed: {}", r.message))),
            Err(e) if crate::actions::result_unknown(&e) => {
                self.fail(short(format!("Closing {name}: {e}")))
            }
            Err(e) => self.fail(short(format!("{name} wasn't closed: {e}"))),
        }
    }

    /// The stop question about this start, by its change's number, if one was asked.
    fn stop_start_at(&self, n: u64) -> Option<usize> {
        self.stop_starts.iter().position(|s| s.n == n)
    }

    /// Whether this start is still on its way: unanswered, or answered and still followed.
    fn start_coming(&self, n: u64) -> bool {
        self.resolver.job(n).is_some_and(|j| j.settled.is_none())
            || self.follows.iter().any(|f| f.job == n)
    }

    /// The card a stop question is about, as the board shows it now: its own, by key, still that
    /// record.
    fn stop_card(&self, s: &StopStart, target: Option<&CardSnapshot>) -> Option<Card> {
        s.key
            .as_deref()
            .or(target.map(|t| t.key.as_str()))
            .and_then(|k| self.card(k))
            .filter(|c| c.detail.as_ref().is_some_and(|d| d.id == s.id))
            .cloned()
    }

    /// How a stop question names its card.
    fn stop_reference(&self, s: &StopStart, target: Option<&CardSnapshot>) -> String {
        self.stop_card(s, target)
            .map(|c| c.reference)
            .unwrap_or_else(|| self.reference_for(&s.id))
    }

    /// Puts up Kerdr's own question about stopping start `n`.
    fn put_stop_question(
        &mut self,
        machine: usize,
        s: &StopStart,
        title: String,
        message: String,
        target: Option<CardSnapshot>,
    ) {
        self.mode = Mode::Confirm {
            machine,
            request: json!({"action": STOP_START, "id": s.id, "n": s.n}),
            title,
            message,
            back: Box::new(self.mode.clone()),
            target: target.map(Box::new),
            queue: false,
            ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
    }

    /// Esc came a moment after a start began (K1-44): to a backend that can put it back in the
    /// backlog, Kerdr asks whether to stop it once its workspace opens. Whether it asked; when it
    /// didn't, the caller says it carries on, as before. What's kept for the question is bound to
    /// this start's own change (Codex's fifth look).
    pub(super) fn ask_stop_start(&mut self, n: u64) -> bool {
        let Some(job) = self.resolver.job(n) else {
            return false;
        };
        let Some(id) = job.id().map(str::to_string) else {
            return false;
        };
        let machine = job.machine;
        let key = job.key.clone();
        let target = job.from.clone().or_else(|| {
            key.as_deref()
                .and_then(|k| self.card(k))
                .map(CardSnapshot::of)
        });
        // Only where a question is safe to put up: on a list or a card, not over typing.
        if job.request["action"] != "dispatch"
            || !self.enter_can_land_here()
            || !self.cancels_on(machine, CANCEL_BACKLOG)
        {
            return false;
        }
        let s = StopStart {
            n,
            id,
            key,
            wanted: false,
            ended: None,
        };
        let reference = self.stop_reference(&s, target.as_ref());
        self.put_stop_question(
            machine,
            &s,
            format!(
                "{reference} had already begun starting. Stop it once its workspace opens, and put it back in the backlog?"
            ),
            format!(
                "Esc came a moment after its start began. Its agent is stopped as X stops one, and {reference} goes back to the backlog, not to Won't do. Anything it hadn't committed stays as it left it."
            ),
            target,
        );
        self.stop_starts.retain(|k| k.n != n);
        self.stop_starts.push(s);
        true
    }

    /// The answer to that question. No: it carries on, as it would have. Yes: stopped once its
    /// start says it started; or, when that answer came while he decided, only that start, where
    /// its answer said it runs, and asked again if it runs somewhere else now (Codex's fifth look).
    pub(super) fn stop_start_answered(
        &mut self,
        request: &Value,
        target: Option<CardSnapshot>,
        yes: bool,
    ) -> Action {
        let n = request["n"].as_u64().unwrap_or_default();
        let Some(i) = self.stop_start_at(n) else {
            let reference = self.reference_for(request["id"].as_str().unwrap_or_default());
            self.say(format!("{reference} carries on starting."), Tone::Info);
            return Action::None;
        };
        let reference = self.stop_reference(&self.stop_starts[i].clone(), target.as_ref());
        if !yes {
            let s = self.stop_starts.remove(i);
            let said = match s.ended {
                None => format!("{reference} carries on starting."),
                Some(StartEnd::Started(_)) => format!("{reference} started, and carries on."),
                Some(StartEnd::Unreachable(on)) => {
                    format!("{reference} started on {on}, and carries on.")
                }
                Some(StartEnd::Asked) => {
                    format!("{reference} asked a question before it started, so nothing started.")
                }
                Some(StartEnd::Refused) => format!("{reference} didn't start."),
                Some(StartEnd::Unknown) => {
                    format!("{reference}'s card shows how its start went.")
                }
            };
            self.say(said, Tone::Info);
            return Action::None;
        }
        match self.stop_starts[i].ended.clone() {
            None if self.start_coming(n) => {
                self.stop_starts[i].wanted = true;
                self.say(
                    format!(
                        "Kerdr stops {reference} once its workspace opens, and puts it back in the backlog."
                    ),
                    Tone::Info,
                );
                Action::None
            }
            // Its answer went by without Kerdr hearing how it ended.
            None | Some(StartEnd::Unknown) => self.late_stop_unknown(i, target),
            Some(StartEnd::Started(place)) => self.late_stop(i, place, target),
            Some(StartEnd::Unreachable(on)) => {
                self.stop_starts.remove(i);
                self.say(unreachable_said(&reference, &on), Tone::Warn);
                Action::None
            }
            Some(StartEnd::Asked) => {
                self.stop_starts.remove(i);
                self.say(
                    format!(
                        "{reference} asked a question before it started, so nothing started and there's nothing to stop."
                    ),
                    Tone::Info,
                );
                Action::None
            }
            Some(StartEnd::Refused) => {
                self.stop_starts.remove(i);
                self.say(
                    format!("{reference} didn't start, so there's nothing to stop."),
                    Tone::Info,
                );
                Action::None
            }
        }
    }

    /// A yes that came after its start answered that it started, at `place`: stopped there only
    /// while the board shows it still running there, or hasn't caught up with its start yet;
    /// asked again where it runs now if that moved; nothing at all if it's gone, closed, done, set
    /// aside or running outside herdr meanwhile. An answer that named no workspace is never taken
    /// as leave to stop it by its number once its start's change has left the board (Codex's
    /// sixth look).
    fn late_stop(&mut self, i: usize, place: Place, target: Option<CardSnapshot>) -> Action {
        let s = self.stop_starts[i].clone();
        let reference = self.stop_reference(&s, target.as_ref());
        let Some(card) = self.stop_card(&s, target.as_ref()) else {
            self.stop_starts.remove(i);
            self.say(
                format!(
                    "{reference} is gone from the board while you decided, so nothing was stopped."
                ),
                Tone::Warn,
            );
            return Action::None;
        };
        if card.dismissed() || card.done() {
            self.stop_starts.remove(i);
            let changed = match &target {
                Some(t) => crate::app::asking::what_changed(t, &card),
                None => format!("{reference} changed"),
            };
            self.say(format!("{changed}, so nothing was stopped."), Tone::Warn);
            return Action::None;
        }
        let Some(now) = Place::of(&card) else {
            self.stop_starts.remove(i);
            // Its conversation runs in a terminal outside herdr now: not what its start opened,
            // and not something a stop back to the backlog reaches.
            if let Some(outside) = card.outside_herdr() {
                self.say(
                    format!(
                        "{reference} now runs in {} outside herdr, not where its start opened it, so nothing was stopped: X on its card stops it there and sets it aside.",
                        outside.place()
                    ),
                    Tone::Warn,
                );
                return Action::None;
            }
            // Shown nowhere: as its answer said only while its start's change still shows, so the
            // board hasn't caught up with it yet. A missing workspace is no sign of that.
            if self.resolver.job(s.n).is_some() {
                return self.stop_at(&s, &place, Some(CardSnapshot::of(&card)), true);
            }
            let said = if place.workspace.is_some() {
                format!("{reference}'s workspace closed while you decided, so nothing was stopped.")
            } else {
                format!(
                    "{reference} shows nowhere running now, and its start didn't say where it opened, so nothing was stopped: X on its card stops it once it shows running."
                )
            };
            self.say(said, Tone::Warn);
            return Action::None;
        };
        // Shown running: only where its answer said. An answer that named no workspace has
        // nothing to tell another one from, so it's asked again.
        if place.workspace.is_some() && place.holds(&card) {
            self.stop_starts.remove(i);
            return self.stop_at(&s, &place, Some(CardSnapshot::of(&card)), true);
        }
        let at = describe(&now, &card);
        self.stop_starts[i].ended = Some(StartEnd::Started(now.clone()));
        self.put_stop_question(
            now.machine,
            &s,
            format!(
                "{reference} now runs in {at}, not where its start opened it. Stop it there and put it back in the backlog?"
            ),
            format!(
                "It moved while you decided, so nothing was stopped yet. Its agent there is stopped as X stops one, and {reference} goes back to the backlog, not to Won't do."
            ),
            Some(CardSnapshot::of(&card)),
        );
        Action::None
    }

    /// A yes whose start's answer was lost: asked again about where the board shows it running,
    /// if it does; otherwise nothing is stopped, since nobody can tell whether it started.
    fn late_stop_unknown(&mut self, i: usize, target: Option<CardSnapshot>) -> Action {
        let s = self.stop_starts[i].clone();
        let reference = self.stop_reference(&s, target.as_ref());
        let card = self
            .stop_card(&s, target.as_ref())
            .filter(|c| !c.dismissed() && !c.done());
        let now = card.as_ref().and_then(Place::of);
        let (Some(card), Some(now)) = (card, now) else {
            self.stop_starts.remove(i);
            self.say(
                format!(
                    "Kerdr can't tell whether {reference} started, so nothing was stopped: X on its card stops it once it shows running."
                ),
                Tone::Warn,
            );
            return Action::None;
        };
        let at = describe(&now, &card);
        self.stop_starts[i].ended = Some(StartEnd::Started(now.clone()));
        self.put_stop_question(
            now.machine,
            &s,
            format!(
                "{reference}'s start lost its answer, and it now shows running in {at}. Stop it there and put it back in the backlog?"
            ),
            format!(
                "Nothing was stopped yet. Its agent there is stopped as X stops one, and {reference} goes back to the backlog, not to Won't do."
            ),
            Some(CardSnapshot::of(&card)),
        );
        Action::None
    }

    /// Stops a start at `place`, back to the backlog: only to a backend there that can, by its
    /// feed; any other gets nothing, said (Codex's fifth look). `by_key` when his yes sends it, so
    /// it goes back as the key's action; otherwise it's queued now.
    fn stop_at(
        &mut self,
        s: &StopStart,
        place: &Place,
        from: Option<CardSnapshot>,
        by_key: bool,
    ) -> Action {
        let reference = self.stop_reference(s, from.as_ref());
        if !self.cancels_on(place.machine, CANCEL_BACKLOG) {
            self.say(
                format!(
                    "{reference} started on a machine whose backend can't put it back in the backlog, so it wasn't stopped: X on its card stops it and sets it aside."
                ),
                Tone::Warn,
            );
            return Action::None;
        }
        let cancel = place.stop(&s.id);
        if !by_key {
            self.say(
                format!("{reference} started; stopping it and putting it back in the backlog…"),
                Tone::Info,
            );
            self.action_from = from;
            self.queue_action(place.machine, cancel);
            return Action::None;
        }
        let action = self.emit_mutation(place.machine, cancel);
        if matches!(action, Action::Mutate { .. }) {
            self.action_from = from;
            self.say(
                format!("Stopping {reference}'s start and putting it back in the backlog…"),
                Tone::Info,
            );
        }
        action
    }

    /// A start's answer, for a start he was asked about stopping: matched by its own change, never
    /// by its record's number (Codex's fifth look). With his yes given, started means the stop goes
    /// now, to the workspace it names, and Kerdr doesn't take him in; a question means nothing
    /// started, and it isn't asked, since he asked for it not to start. Before his answer, how it
    /// ended is kept for it, and the answer goes on as usual. Whether this took the answer.
    pub(super) fn stop_if_started(
        &mut self,
        machine: usize,
        request: &Value,
        result: &Result<Response, String>,
        from: &Option<CardSnapshot>,
    ) -> bool {
        let Some(i) = self.answer_job.and_then(|n| self.stop_start_at(n)) else {
            return false;
        };
        if request["action"] != "dispatch" {
            return false;
        }
        let end = match result {
            // Still going: its end, when it comes, is this answer again.
            Ok(r) if r.ok && !r.confirmation && r.follow.is_some() => return false,
            Ok(r) if r.ok && !r.confirmation && r.questions.is_empty() => {
                // Where it runs: the machine its answer names, when it ran on another. One Kerdr
                // can't reach is kept as that, never stood in for by the machine that answered,
                // whose workspaces aren't the ones named (Codex's sixth look).
                let elsewhere = r
                    .machine
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .filter(|m| {
                        self.machines
                            .get(machine)
                            .is_some_and(|here| !here.answers_to(m))
                    });
                let ran = match elsewhere {
                    None => Ok(machine),
                    Some(on) => self.machine_index(on).ok_or(on),
                };
                match ran {
                    Ok(ran) => StartEnd::Started(Place {
                        machine: ran,
                        session: r.session.clone(),
                        workspace: r.workspace.clone(),
                        tab: r.tab.clone(),
                    }),
                    Err(on) => StartEnd::Unreachable(on.to_string()),
                }
            }
            Ok(r) if r.confirmation || !r.questions.is_empty() => StartEnd::Asked,
            Err(e) if crate::actions::result_unknown(e) => StartEnd::Unknown,
            // Refused, or done in part: nothing of it runs.
            _ => StartEnd::Refused,
        };
        if !self.stop_starts[i].wanted {
            self.stop_starts[i].ended = Some(end);
            return false;
        }
        let s = self.stop_starts.remove(i);
        let reference = self.stop_reference(&s, from.as_ref());
        match end {
            StartEnd::Started(place) => {
                self.stop_at(&s, &place, from.clone(), false);
                true
            }
            StartEnd::Unreachable(on) => {
                self.say(unreachable_said(&reference, &on), Tone::Warn);
                true
            }
            StartEnd::Asked => {
                self.say(
                    format!(
                        "{reference} asked a question before it started, so nothing started and it stays in the backlog; d asks it again."
                    ),
                    Tone::Info,
                );
                true
            }
            StartEnd::Unknown => {
                let e = result.as_ref().err().cloned().unwrap_or_default();
                self.fail(short(format!(
                    "Starting {reference}: {e} Kerdr can't tell whether it started, so it wasn't stopped."
                )));
                true
            }
            StartEnd::Refused => false,
        }
    }

    /// A followed start Kerdr lost track of: nobody can tell whether it started, so a stop he
    /// asked for isn't made, and that's said; one he hasn't answered yet keeps that for his answer.
    pub(super) fn start_lost(&mut self, n: u64) {
        let Some(i) = self.stop_start_at(n) else {
            return;
        };
        if !self.stop_starts[i].wanted {
            self.stop_starts[i].ended = Some(StartEnd::Unknown);
            return;
        }
        let s = self.stop_starts.remove(i);
        let reference = self.stop_reference(&s, None);
        self.fail(format!(
            "Kerdr can't tell whether {reference} started, so it wasn't stopped: X on its card stops it once it shows running."
        ));
    }
}

/// What a stop says for a start that ran on a machine Kerdr isn't set up to reach.
fn unreachable_said(reference: &str, on: &str) -> String {
    format!(
        "{reference} started on {on}, which Kerdr isn't set up to reach, so it wasn't stopped: it carries on there."
    )
}

/// Where a card runs, in a question's words: "workspace web-41 (web)".
fn describe(place: &Place, card: &Card) -> String {
    let name = card
        .workspace_label
        .clone()
        .or_else(|| place.workspace.clone())
        .unwrap_or_default();
    match &place.session {
        Some(session) => format!("workspace {name} ({session})"),
        None => format!("workspace {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{
        demo_app, press, press_code, room_in_every_session, same_number_elsewhere,
    };
    use crate::model::Stage;
    use ratatui::crossterm::event::KeyCode;

    fn reply(message: &str, extra: Value) -> Result<Response, String> {
        let mut reply = json!({"ok":true,"message":message});
        if let (Some(fields), Some(more)) = (reply.as_object_mut(), extra.as_object()) {
            fields.extend(more.clone());
        }
        Ok(crate::actions::parse_response(&reply.to_string()).unwrap())
    }

    fn supports(a: &mut App, machine: usize, what: &str, yes: bool) {
        let feed = a.machines[machine].feed.as_mut().unwrap();
        feed.supports.retain(|s| s != what);
        if yes {
            feed.supports.push(what.into());
        }
    }

    fn select(a: &mut App, key: &str) {
        let col = a.card(key).unwrap().stage.index();
        a.sel = Selection {
            col,
            key: Some(key.to_string()),
        };
    }

    /// A question that appeared on its own, ready for its answer.
    fn settle(a: &mut App) {
        if let Mode::Confirm { ready_at, .. } = &mut a.mode {
            *ready_at = None;
        }
    }

    fn asked(a: &App) -> (Value, String) {
        let Mode::Confirm { request, title, .. } = &a.mode else {
            panic!("nothing asked: {:?} {:?}", a.mode, a.message())
        };
        (request.clone(), title.clone())
    }

    fn said(a: &App) -> String {
        a.message().map(|(m, _)| m.to_string()).unwrap_or_default()
    }

    /// The changes handed to the runner, without their ops.
    fn sent(a: &mut App) -> Vec<(u64, usize, Value)> {
        a.resolver
            .ready()
            .into_iter()
            .map(|(n, m, r, _)| (n, m, crate::actions::without_op(&r)))
            .collect()
    }

    // X on a workspace with no intent (K1-27).

    /// A workspace with no intent, on a machine whose backend closes one when `closes`.
    fn bare(a: &mut App, closes: bool) -> Card {
        a.cfg.actions.command = Some("test-adapter".into());
        let card = a
            .board
            .cards
            .iter()
            .find(|c| c.source == Source::Workspace && c.is_open_workspace())
            .expect("the demo has a workspace with no intent")
            .clone();
        supports(a, card.machine_index, CANCEL_WORKSPACE, closes);
        select(a, &card.key);
        card
    }

    #[test]
    fn x_on_a_workspace_with_no_intent_asks_then_closes_it() {
        let mut a = demo_app();
        let card = bare(&mut a, true);
        let (session, workspace) = (
            card.session.clone().unwrap(),
            card.workspace_id.clone().unwrap(),
        );
        let name = card.workspace_label.clone().unwrap();
        assert_eq!(press(&mut a, 'X'), Action::None);
        let (request, title) = asked(&a);
        assert_eq!(
            request,
            json!({"action":"cancel","session":session,"workspace":workspace})
        );
        assert!(title.starts_with("Stop "), "{title}");
        assert!(
            title.ends_with(&format!(" and close workspace {name} ({session})?")),
            "{title}"
        );
        let Action::Mutate { machine, request } = press(&mut a, 'y') else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(machine, card.machine_index);
        a.queue_action(machine, request.clone());
        assert_eq!(said(&a), format!("Closing {}…", card.workspace_name()));
        // Nothing shown ahead of its answer: it's no record to set aside.
        let card_now = a.card(&card.key).unwrap();
        assert_eq!(card_now.stage, Stage::Misc);
        assert!(!card_now.dismissed());
        let [(n, _, out)] = &sent(&mut a)[..] else {
            panic!("one change")
        };
        assert_eq!(out, &request);
        a.finish_job(
            *n,
            request,
            reply(
                &format!("Stopped its Claude Code agent in workspace {name} and closed it."),
                json!({}),
            ),
        );
        assert_eq!(
            said(&a),
            format!("Stopped its Claude Code agent in workspace {name} and closed it.")
        );
        assert!(!a.failing());
    }

    #[test]
    fn x_on_a_workspace_with_no_intent_says_what_it_always_did_to_a_backend_that_cant_close_it() {
        let mut a = demo_app();
        bare(&mut a, false);
        assert_eq!(press(&mut a, 'X'), Action::None);
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        assert_eq!(said(&a), NO_INTENT);
        assert!(sent(&mut a).is_empty());
    }

    #[test]
    fn a_workspace_that_closed_or_was_claimed_while_he_decided_is_left_alone() {
        // Closed meanwhile: nothing sent.
        let mut a = demo_app();
        let card = bare(&mut a, true);
        press(&mut a, 'X');
        a.board.cards.retain(|c| c.key != card.key);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert_eq!(
            said(&a),
            "That workspace closed while you decided; nothing needed stopping."
        );
        // An intent took it meanwhile: X on that card is what stops it.
        let mut a = demo_app();
        let card = bare(&mut a, true);
        press(&mut a, 'X');
        let i = a
            .board
            .cards
            .iter()
            .position(|c| c.key == card.key)
            .unwrap();
        let claimed = a
            .board
            .cards
            .iter()
            .find(|c| c.detail.is_some())
            .unwrap()
            .clone();
        a.board.cards[i].key = format!("{}-claimed", card.key);
        a.board.cards[i].detail = claimed.detail.clone();
        a.board.cards[i].reference = claimed.reference.clone();
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(
            said(&a).contains(&format!(
                "holds {}'s work now, so nothing was stopped",
                claimed.reference
            )),
            "{}",
            said(&a)
        );
    }

    #[test]
    fn a_refused_close_says_why_and_a_backend_question_is_asked_with_its_token() {
        let mut a = demo_app();
        let card = bare(&mut a, true);
        press(&mut a, 'X');
        let Action::Mutate { machine, request } = press(&mut a, 'y') else {
            panic!("nothing sent")
        };
        a.action_from = Some(CardSnapshot::of(&card));
        a.finish_action(
            machine,
            request.clone(),
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"message":"In workspace w1, pid 4 (codex) runs as another account, so it can't be stopped from here; nothing was stopped."}"#,
            )
            .unwrap()),
        );
        assert!(a.failing());
        assert!(
            said(&a).starts_with(&format!(
                "{} wasn't closed: In workspace w1",
                card.workspace_name()
            )),
            "{}",
            said(&a)
        );
        // A question it asks goes back with its token, and the yes with override.
        let mut a = demo_app();
        let card = bare(&mut a, true);
        a.action_from = Some(CardSnapshot::of(&card));
        a.finish_action(
            machine,
            request.clone(),
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"confirmation":true,"confirm":"c-1","message":"Leave pid 4 to finish and close the rest?"}"#,
            )
            .unwrap()),
        );
        settle(&mut a);
        let (asked_for, title) = asked(&a);
        assert_eq!(title, "Leave pid 4 to finish and close the rest?");
        assert_eq!(asked_for["confirm"], "c-1");
        let Action::Mutate { request: yes, .. } = press(&mut a, 'y') else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(
            (yes["confirm"].as_str(), yes["override"].as_bool()),
            (Some("c-1"), Some(true))
        );
        assert!(yes.get("id").is_none());
    }

    // Esc on a start that had already begun (K1-44).

    /// d on backlog work still being written up, then Esc a moment after its start began: what
    /// the runner says once it has started it.
    fn too_late(a: &mut App, puts_back: bool) -> (String, String, u64, usize, Value) {
        room_in_every_session(a);
        a.cfg.actions.command = Some("test-adapter".into());
        let card = a
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && c.detail.is_some()
                    && !c.dismissed()
                    && c.intake().is_none()
            })
            .expect("backlog work in the demo")
            .clone();
        let (key, id, m) = (
            card.key.clone(),
            card.detail.unwrap().id,
            card.machine_index,
        );
        supports(a, m, CANCEL_BACKLOG, puts_back);
        for c in a.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.intake = Some("pending".into());
            }
        }
        a.rebuild();
        select(a, &key);
        press(a, 'd');
        let (n, machine, request, _) = a.resolver.ready()[0].clone();
        press_code(a, KeyCode::Esc);
        assert_eq!(a.resolver.take_dropped(), vec![n]);
        a.job_started(n);
        a.resolver.drop_answered(n, false);
        (key, id, n, machine, request)
    }

    fn started(a: &App) -> Result<Response, String> {
        let _ = a;
        reply(
            "Started #41 in workspace w9 (web).",
            json!({"session":"web","workspace":"w9","tab":"w9:t1","label":"41-dark-mode"}),
        )
    }

    #[test]
    fn esc_too_late_asks_and_its_yes_stops_the_start_once_its_workspace_opens() {
        let mut a = demo_app();
        let (_, id, n, machine, request) = too_late(&mut a, true);
        let reference = a.reference_for(&id);
        let (question, title) = asked(&a);
        assert_eq!(question["action"], STOP_START);
        assert!(
            title.starts_with(&format!("{reference} had already begun starting.")),
            "{title}"
        );
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(a.stop_starts.iter().any(|s| s.n == n && s.wanted));
        assert!(
            said(&a).contains("once its workspace opens"),
            "{}",
            said(&a)
        );
        // It starts: the stop goes at once, to the workspace it opened, and nobody goes in.
        let answer = started(&a);
        a.finish_job(n, request, answer);
        assert!(a.open_after.is_none());
        let [(c, to, cancel)] = &sent(&mut a)[..] else {
            panic!("one stop: {:?}", a.message())
        };
        assert_eq!(*to, machine);
        assert_eq!(
            cancel,
            &json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w9","tab":"w9:t1"})
        );
        assert!(a.stop_starts.is_empty());
        a.finish_job(
            *c,
            cancel.clone(),
            reply(
                &format!("Stopped {reference}'s start: its Claude Code agent, and it's back in the backlog."),
                json!({"id":id}),
            ),
        );
        assert_eq!(
            said(&a),
            format!(
                "Stopped {reference}'s start: its Claude Code agent, and it's back in the backlog."
            )
        );
        // A refusal says what didn't happen in those words.
        let mut a = demo_app();
        let (_, id, n, _, request) = too_late(&mut a, true);
        settle(&mut a);
        press(&mut a, 'y');
        let answer = started(&a);
        a.finish_job(n, request, answer);
        let [(c, _, cancel)] = &sent(&mut a)[..] else {
            panic!("one stop")
        };
        a.finish_job(
            *c,
            cancel.clone(),
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"message":"it's being started elsewhere"}"#,
            )
            .unwrap()),
        );
        assert_eq!(
            said(&a),
            format!(
                "{}'s start wasn't stopped: it's being started elsewhere",
                a.reference_for(&id)
            )
        );
    }

    #[test]
    fn esc_too_late_carries_on_as_before_to_a_backend_that_cant_put_it_back_or_after_a_no() {
        // A backend without it: said as before, nothing asked.
        let mut a = demo_app();
        let (_, id, n, _, request) = too_late(&mut a, false);
        assert!(!matches!(a.mode, Mode::Confirm { .. }), "{:?}", a.mode);
        assert_eq!(
            said(&a),
            format!(
                "{} had already begun starting when Esc came, so it carries on.",
                a.reference_for(&id)
            )
        );
        let answer = started(&a);
        a.finish_job(n, request, answer);
        assert!(sent(&mut a).is_empty());
        // No: it carries on, and nothing is stopped.
        let mut a = demo_app();
        let (_, id, n, _, request) = too_late(&mut a, true);
        settle(&mut a);
        assert_eq!(press(&mut a, 'n'), Action::None);
        assert_eq!(
            said(&a),
            format!("{} carries on starting.", a.reference_for(&id))
        );
        let answer = started(&a);
        a.finish_job(n, request, answer);
        assert!(sent(&mut a).is_empty());
    }

    #[test]
    fn a_start_that_asks_first_starts_nothing_so_nothing_is_stopped_or_asked() {
        let mut a = demo_app();
        let (_, _, n, _, request) = too_late(&mut a, true);
        settle(&mut a);
        press(&mut a, 'y');
        a.finish_job(
            n,
            request,
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"confirmation":true,"confirm":"t-1","message":"web is at its usual limit of 3. Start anyway?"}"#,
            )
            .unwrap()),
        );
        assert!(
            !matches!(a.mode, Mode::Confirm { .. }),
            "asked: {:?}",
            a.mode
        );
        assert!(sent(&mut a).is_empty());
        assert!(
            said(&a).contains("nothing started and it stays in the backlog"),
            "{}",
            said(&a)
        );
        assert!(a.stop_starts.is_empty());
    }

    #[test]
    fn a_yes_after_the_start_answered_stops_it_at_once() {
        let mut a = demo_app();
        let (key, id, n, machine, request) = too_late(&mut a, true);
        let answer = started(&a);
        a.finish_job(n, request, answer);
        settle(&mut a);
        let Action::Mutate {
            machine: to,
            request: cancel,
        } = press(&mut a, 'y')
        else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(to, machine);
        // Where its answer said it started, though the board hasn't caught up yet.
        assert_eq!(
            cancel,
            json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w9","tab":"w9:t1"})
        );
        assert_eq!(a.action_from.as_ref().map(|f| f.key.clone()), Some(key));
    }

    /// Where a card shows running now, as the board would after a read: in workspace `id` of the
    /// web session on `machine`.
    fn runs_in(a: &mut App, key: &str, machine: usize, id: &str, label: &str) {
        let card = a.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.machine_index = machine;
        card.session = Some("web".into());
        card.workspace_id = Some(id.into());
        card.workspace_label = Some(label.into());
        card.tab_id = None;
    }

    #[test]
    fn a_stop_is_bound_to_its_own_start_when_two_sources_share_its_number() {
        // Codex's fifth look: source A's #N and source B's #N are both starting; he asks to stop
        // A's. B's answer comes first: B goes on untouched. Then A's: A's is stopped, where it
        // started.
        let mut a = demo_app();
        room_in_every_session(&mut a);
        a.cfg.actions.command = Some("test-adapter".into());
        let card = a
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && c.detail.as_ref().is_some_and(|d| d.kickoff.is_none())
                    && !c.dismissed()
                    && c.intake().is_none()
            })
            .expect("backlog work in the demo")
            .clone();
        let (ka, id, here) = (
            card.key.clone(),
            card.detail.as_ref().unwrap().id.clone(),
            card.machine_index,
        );
        let (kb, there) = same_number_elsewhere(&mut a, &ka);
        for m in [here, there] {
            supports(&mut a, m, CANCEL_BACKLOG, true);
        }
        // A's is still being written up, so its d waits for that; B's goes at once.
        for c in a.machines[here].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.intake = Some("pending".into());
            }
        }
        a.rebuild();
        select(&mut a, &ka);
        press(&mut a, 'd');
        let (na, _, ra, _) = a.resolver.ready()[0].clone();
        select(&mut a, &kb);
        let Action::Mutate {
            machine: mb,
            request: rb,
        } = press(&mut a, 'd')
        else {
            panic!("B didn't start: {:?}", a.message())
        };
        assert_eq!(mb, there);
        a.queue_action(mb, rb.clone());
        let nb = a.resolver.last_added();
        assert_eq!(sent(&mut a).len(), 1);
        press_code(&mut a, KeyCode::Esc);
        assert_eq!(a.resolver.take_dropped(), vec![na]);
        a.job_started(na);
        a.resolver.drop_answered(na, false);
        settle(&mut a);
        assert_eq!(asked(&a).0["n"], na);
        assert_eq!(press(&mut a, 'y'), Action::None);
        a.finish_job(
            nb,
            rb,
            reply(
                "Started it in workspace w7 (api).",
                json!({"session":"api","workspace":"w7","tab":"w7:t1"}),
            ),
        );
        assert!(
            sent(&mut a).is_empty(),
            "B's start was stopped: {:?}",
            a.message()
        );
        let answer = started(&a);
        a.finish_job(na, ra, answer);
        let [(_, to, cancel)] = &sent(&mut a)[..] else {
            panic!("A's start wasn't stopped: {:?}", a.message())
        };
        assert_eq!(*to, here);
        assert_eq!(
            cancel,
            &json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w9","tab":"w9:t1"})
        );
        assert!(a.stop_starts.is_empty());
    }

    #[test]
    fn a_late_yes_asks_again_when_its_start_runs_somewhere_else_now() {
        // Codex's fifth look: its start opened W9 while he decided, then the board shows it in
        // W10. Nothing is stopped by the yes; it's asked again about W10, whose yes stops that.
        let mut a = demo_app();
        let (key, id, n, machine, request) = too_late(&mut a, true);
        let answer = started(&a);
        a.finish_job(n, request, answer);
        runs_in(&mut a, &key, machine, "w10", "41-again");
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(sent(&mut a).is_empty());
        let (question, title) = asked(&a);
        assert_eq!(question["action"], STOP_START);
        assert!(
            title.contains("now runs in workspace 41-again (web), not where its start opened it"),
            "{title}"
        );
        settle(&mut a);
        let Action::Mutate {
            machine: to,
            request: cancel,
        } = press(&mut a, 'y')
        else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(to, machine);
        assert_eq!(
            cancel,
            json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w10"})
        );
        assert!(a.stop_starts.is_empty());
    }

    #[test]
    fn a_late_yes_stops_nothing_after_a_refused_start_and_asks_where_a_lost_one_shows() {
        // Codex's fifth look: the yes acts on that start's own answer, kept while he decided.
        // Refused: nothing ran, so nothing is stopped.
        let mut a = demo_app();
        let (_, _, n, _, request) = too_late(&mut a, true);
        a.finish_job(
            n,
            request,
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"message":"only an open backlog intent can start"}"#,
            )
            .unwrap()),
        );
        assert!(said(&a).contains("didn't start: only an open backlog intent can start"));
        // Seen.
        a.message = None;
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(sent(&mut a).is_empty());
        assert!(
            said(&a).contains("didn't start, so there's nothing to stop"),
            "{}",
            said(&a)
        );
        assert!(a.stop_starts.is_empty());
        // Its answer lost, nothing shown running: nobody can tell, so nothing is stopped.
        let lost = || Err("The connection dropped. Kerdr can't tell whether it happened.".into());
        let mut a = demo_app();
        let (_, _, n, _, request) = too_late(&mut a, true);
        a.finish_job(n, request, lost());
        a.message = None;
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(sent(&mut a).is_empty());
        assert!(said(&a).contains("can't tell whether"), "{}", said(&a));
        // Lost, and the board shows it running in W9 now: asked again about W9.
        let mut a = demo_app();
        let (key, id, n, machine, request) = too_late(&mut a, true);
        a.finish_job(n, request, lost());
        a.message = None;
        runs_in(&mut a, &key, machine, "w9", "41-dark-mode");
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(sent(&mut a).is_empty());
        let (_, title) = asked(&a);
        assert!(
            title.contains(
                "lost its answer, and it now shows running in workspace 41-dark-mode (web)"
            ),
            "{title}"
        );
        settle(&mut a);
        let Action::Mutate {
            request: cancel, ..
        } = press(&mut a, 'y')
        else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(
            cancel,
            json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w9"})
        );
    }

    #[test]
    fn a_stop_sent_on_to_an_older_backend_or_answered_once_it_cant_is_never_sent() {
        // Codex's fifth look: the stop goes to a machine that takes it, which refuses it naming
        // another home. To an older backend there, which would read it as X and set the work
        // aside, it isn't sent on, and that's said; to a newer one, it goes on.
        for newer in [false, true] {
            let mut a = demo_app();
            let (_, id, n, machine, request) = too_late(&mut a, true);
            let other = if machine == 0 { 1 } else { 0 };
            supports(&mut a, other, CANCEL_BACKLOG, newer);
            settle(&mut a);
            press(&mut a, 'y');
            let answer = started(&a);
            a.finish_job(n, request, answer);
            let [(c, to, cancel)] = &sent(&mut a)[..] else {
                panic!("no stop: {:?}", a.message())
            };
            assert_eq!(*to, machine);
            let name = a.machines[other].name();
            a.finish_job(
                *c,
                cancel.clone(),
                Ok(Response {
                    ok: false,
                    message: format!("{id} lives on {name} now."),
                    home: Some(format!("{name}/web")),
                    ..Default::default()
                }),
            );
            let on = sent(&mut a);
            if newer {
                let [(_, to, again)] = &on[..] else {
                    panic!("not sent on: {:?}", a.message())
                };
                assert_eq!((*to, again), (other, cancel));
                continue;
            }
            assert!(on.is_empty(), "sent on to an older backend: {on:?}");
            assert!(
                said(&a).contains(&format!(
                    "lives on {name} now, whose backend can't put it back in the backlog, so it wasn't stopped"
                )),
                "{}",
                said(&a)
            );
        }
        // Its backend asks first; by the yes, its feed no longer says it takes the stop: the yes
        // isn't sent either.
        let mut a = demo_app();
        let (key, _, n, machine, request) = too_late(&mut a, true);
        settle(&mut a);
        press(&mut a, 'y');
        let answer = started(&a);
        a.finish_job(n, request, answer);
        let [(c, _, cancel)] = &sent(&mut a)[..] else {
            panic!("no stop")
        };
        a.finish_job(
            *c,
            cancel.clone(),
            Ok(crate::actions::parse_response(
                r#"{"ok":false,"confirmation":true,"confirm":"c-9","message":"Its agent is mid-step. Stop it anyway?"}"#,
            )
            .unwrap()),
        );
        settle(&mut a);
        let (asked_for, _) = asked(&a);
        assert_eq!(asked_for["confirm"], "c-9");
        supports(&mut a, machine, CANCEL_BACKLOG, false);
        let action = press(&mut a, 'y');
        if let Action::Mutate { machine, request } = action {
            a.queue_action(machine, request);
        }
        assert!(
            sent(&mut a).is_empty(),
            "its yes went to a backend that can't take it"
        );
        assert!(
            said(&a).contains("can't put it back in the backlog"),
            "{}",
            said(&a)
        );
        assert!(a.card(&key).is_some());
    }

    #[test]
    fn a_start_on_a_machine_kerdr_cant_reach_is_never_stopped_on_the_one_that_answered() {
        // Codex's sixth look: the answer says the start opened W9 on a machine Kerdr isn't set up
        // for. Nothing is sent, before his yes or after it, to the machine that answered, whose
        // W9 isn't that one; that's said.
        let far = || {
            reply(
                "Started #41 on far-away in workspace w9 (web).",
                json!({"machine":"far-away","session":"web","workspace":"w9","tab":"w9:t1"}),
            )
        };
        let unreachable = "started on far-away, which Kerdr isn't set up to reach, so it wasn't stopped: it carries on there.";
        // His yes came first.
        let mut a = demo_app();
        let (_, _, n, _, request) = too_late(&mut a, true);
        settle(&mut a);
        press(&mut a, 'y');
        a.finish_job(n, request, far());
        assert!(sent(&mut a).is_empty(), "stopped on the wrong machine");
        assert!(said(&a).contains(unreachable), "{}", said(&a));
        assert!(a.stop_starts.is_empty());
        // Its answer came first.
        let mut a = demo_app();
        let (_, _, n, _, request) = too_late(&mut a, true);
        a.finish_job(n, request, far());
        a.message = None;
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(sent(&mut a).is_empty(), "stopped on the wrong machine");
        assert!(said(&a).contains(unreachable), "{}", said(&a));
        assert!(a.stop_starts.is_empty());
        // A machine Kerdr does reach: stopped there, where its answer said.
        let mut a = demo_app();
        let (_, id, n, machine, request) = too_late(&mut a, true);
        let other = if machine == 0 { 1 } else { 0 };
        supports(&mut a, other, CANCEL_BACKLOG, true);
        let name = a.machines[other].name();
        a.finish_job(
            n,
            request,
            reply(
                &format!("Started #41 on {name} in workspace w9 (web)."),
                json!({"machine":name,"session":"web","workspace":"w9","tab":"w9:t1"}),
            ),
        );
        settle(&mut a);
        let Action::Mutate {
            machine: to,
            request: cancel,
        } = press(&mut a, 'y')
        else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(
            (to, cancel),
            (
                other,
                json!({"action":"cancel","id":id,"to":"backlog","session":"web","workspace":"w9","tab":"w9:t1"})
            )
        );
    }

    #[test]
    fn a_late_yes_after_an_answer_naming_no_workspace_stops_by_number_only_while_its_start_shows() {
        // Codex's sixth look: a start's answer may name no workspace. While its change still
        // shows, the board hasn't caught up with it, and the stop goes by its number, as its
        // answer said. Once that's gone, a card with nothing shown running isn't taken as that
        // start still running: its workspace may have closed, or it may have been resumed
        // outside herdr since. Nothing is stopped by number then; that's said.
        let bare = || reply("Started #41.", json!({}));
        let mut a = demo_app();
        let (_, id, n, machine, request) = too_late(&mut a, true);
        a.finish_job(n, request, bare());
        assert!(a.resolver.job(n).is_some());
        settle(&mut a);
        let Action::Mutate {
            machine: to,
            request: cancel,
        } = press(&mut a, 'y')
        else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(
            (to, cancel),
            (machine, json!({"action":"cancel","id":id,"to":"backlog"}))
        );
        for outside in [false, true] {
            let mut a = demo_app();
            let (_, id, n, machine, request) = too_late(&mut a, true);
            a.finish_job(n, request, bare());
            // Two reads go by: its change has left the board.
            for _ in 0..2 {
                a.resolver.read(machine, None);
            }
            assert!(a.resolver.job(n).is_none());
            if outside {
                for c in a.machines[machine].feed.as_mut().unwrap().cards.iter_mut() {
                    if c.id == id {
                        c.outside_herdr = Some(crate::feed::OutsideHerdr {
                            terminal: Some("foot".into()),
                            since: Some("2026-11-01T13:02:00-06:00".into()),
                        });
                    }
                }
            }
            a.rebuild();
            a.message = None;
            settle(&mut a);
            assert_eq!(press(&mut a, 'y'), Action::None, "{outside}");
            assert!(sent(&mut a).is_empty(), "stopped by number: {outside}");
            let expected = if outside {
                "now runs in a foot window outside herdr, not where its start opened it, so nothing was stopped: X on its card stops it there and sets it aside."
            } else {
                "shows nowhere running now, and its start didn't say where it opened, so nothing was stopped"
            };
            assert!(said(&a).contains(expected), "{}", said(&a));
            assert!(a.stop_starts.is_empty());
        }
    }

    #[test]
    fn a_stop_names_the_workspace_its_start_named_even_without_its_session() {
        // Codex's sixth look, swept: an answer naming its workspace but not its session. The stop
        // still names that workspace, never falling back to a stop by number alone.
        let mut a = demo_app();
        let (_, id, n, machine, request) = too_late(&mut a, true);
        settle(&mut a);
        press(&mut a, 'y');
        a.finish_job(
            n,
            request,
            reply("Started #41 in workspace w9.", json!({"workspace":"w9"})),
        );
        let [(_, to, cancel)] = &sent(&mut a)[..] else {
            panic!("no stop: {:?}", a.message())
        };
        assert_eq!(
            (*to, cancel),
            (
                machine,
                &json!({"action":"cancel","id":id,"to":"backlog","workspace":"w9"})
            )
        );
    }
}
