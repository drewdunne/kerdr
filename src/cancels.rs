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

    /// Esc came a moment after a start began (K1-44): to a backend that can put it back in the
    /// backlog, Kerdr asks whether to stop it once its workspace opens. Whether it asked; when it
    /// didn't, the caller says it carries on, as before.
    pub(super) fn ask_stop_start(&mut self, n: u64) -> bool {
        let Some(job) = self.resolver.job(n) else {
            return false;
        };
        let Some(id) = job.id().map(str::to_string) else {
            return false;
        };
        let machine = job.machine;
        let target = job.from.clone().or_else(|| {
            job.key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(CardSnapshot::of)
        });
        // Only where a question is safe to put up: on a list or a card, not over typing.
        let looking = matches!(
            self.mode,
            Mode::Board
                | Mode::Backlog { .. }
                | Mode::Misc { .. }
                | Mode::Done { .. }
                | Mode::Peek { .. }
                | Mode::Card { .. }
        ) && !self.typing_view_search();
        if job.request["action"] != "dispatch"
            || !looking
            || !self.cancels_on(machine, CANCEL_BACKLOG)
        {
            return false;
        }
        let reference = self.reference_for(&id);
        self.mode = Mode::Confirm {
            machine,
            request: json!({"action": STOP_START, "id": id, "n": n}),
            title: format!(
                "{reference} had already begun starting. Stop it once its workspace opens, and put it back in the backlog?"
            ),
            message: format!(
                "Esc came a moment after its start began. Its agent is stopped as X stops one, and {reference} goes back to the backlog, not to Won't do. Anything it hadn't committed stays as it left it."
            ),
            back: Box::new(self.mode.clone()),
            target: target.map(Box::new),
            queue: false,
            ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
        true
    }

    /// The answer to that question. No: it carries on, as it would have. Yes: stopped once its
    /// start says it started, or now when that answer came while he decided.
    pub(super) fn stop_start_answered(
        &mut self,
        machine: usize,
        request: &Value,
        target: Option<CardSnapshot>,
        yes: bool,
    ) -> Action {
        let id = request["id"].as_str().unwrap_or_default().to_string();
        let reference = self.reference_for(&id);
        if !yes {
            self.say(format!("{reference} carries on starting."), Tone::Info);
            return Action::None;
        }
        let n = request["n"].as_u64().unwrap_or_default();
        let coming = self.resolver.job(n).is_some_and(|j| j.settled.is_none())
            || self.following(&id).is_some();
        if coming {
            self.stop_after_start.insert(id);
            self.say(
                format!(
                    "Kerdr stops {reference} once its workspace opens, and puts it back in the backlog."
                ),
                Tone::Info,
            );
            return Action::None;
        }
        // Its answer came while he decided: stopped where the board shows it now.
        let card = target
            .as_ref()
            .and_then(|t| self.card(&t.key))
            .filter(|c| c.detail.as_ref().is_some_and(|d| d.id == id))
            .cloned();
        let mut cancel = json!({"action":"cancel","id":id,"to":"backlog"});
        if let Some(c) = &card
            && c.is_open_workspace()
            && let (Some(session), Some(workspace)) = (&c.session, &c.workspace_id)
        {
            cancel["session"] = json!(session);
            cancel["workspace"] = json!(workspace);
            if let Some(tab) = &c.tab_id {
                cancel["tab"] = json!(tab);
            }
        }
        let action = self.emit_mutation(machine, cancel);
        if matches!(action, Action::Mutate { .. }) {
            self.action_from = card.as_ref().map(CardSnapshot::of).or(target);
            self.say(
                format!("Stopping {reference}'s start and putting it back in the backlog…"),
                Tone::Info,
            );
        }
        action
    }

    /// A start he asked to stop once it started: its answer. Started, the stop goes now, to the
    /// workspace it names, and Kerdr doesn't take him in. A question means nothing started, and
    /// it isn't asked, since he asked for it not to start. Whether this took the answer; anything
    /// else (a refusal, a start still followed) goes on as usual.
    pub(super) fn stop_if_started(
        &mut self,
        machine: usize,
        request: &Value,
        result: &Result<Response, String>,
        from: &Option<CardSnapshot>,
    ) -> bool {
        let Some(id) = request["id"].as_str().map(str::to_string) else {
            return false;
        };
        if request["action"] != "dispatch" || !self.stop_after_start.contains(&id) {
            return false;
        }
        let reference = self.reference_for(&id);
        match result {
            // Still going: its end, when it comes, is this answer again.
            Ok(r) if r.ok && !r.confirmation && r.follow.is_some() => false,
            Ok(r) if r.ok && !r.confirmation && r.questions.is_empty() => {
                self.stop_after_start.remove(&id);
                if !self.cancels_on(machine, CANCEL_BACKLOG) {
                    self.say(
                        format!(
                            "{reference} started on a machine whose backend can't put it back in the backlog, so it wasn't stopped: X on its card stops it and sets it aside."
                        ),
                        Tone::Warn,
                    );
                    return true;
                }
                let mut cancel = json!({"action":"cancel","id":id,"to":"backlog"});
                if let (Some(session), Some(workspace)) = (&r.session, &r.workspace) {
                    cancel["session"] = json!(session);
                    cancel["workspace"] = json!(workspace);
                    if let Some(tab) = &r.tab {
                        cancel["tab"] = json!(tab);
                    }
                }
                self.say(
                    format!("{reference} started; stopping it and putting it back in the backlog…"),
                    Tone::Info,
                );
                self.action_from = from.clone();
                self.queue_action(machine, cancel);
                true
            }
            Ok(r) if r.confirmation || !r.questions.is_empty() => {
                self.stop_after_start.remove(&id);
                self.say(
                    format!(
                        "{reference} asked a question before it started, so nothing started and it stays in the backlog; d asks it again."
                    ),
                    Tone::Info,
                );
                true
            }
            Err(e) if crate::actions::result_unknown(e) => {
                self.stop_after_start.remove(&id);
                self.fail(short(format!(
                    "Starting {reference}: {e} Kerdr can't tell whether it started, so it wasn't stopped."
                )));
                true
            }
            // Refused, or done in part: nothing of it runs, and that's said as usual.
            _ => {
                self.stop_after_start.remove(&id);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{demo_app, press, press_code, room_in_every_session};
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
        assert!(a.stop_after_start.contains(&id));
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
        assert!(!a.stop_after_start.contains(&id));
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
        let (_, id, n, _, request) = too_late(&mut a, true);
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
        assert!(!a.stop_after_start.contains(&id));
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
        assert_eq!(cancel, json!({"action":"cancel","id":id,"to":"backlog"}));
        assert_eq!(a.action_from.as_ref().map(|f| f.key.clone()), Some(key));
    }
}
