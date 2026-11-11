//! Questions that stay questions (intent 0591, release 3): asked with the answers that move things
//! on (`choices`), shown whole, waiting on their card when you've moved on rather than failing, and
//! asked again at once, in the card's new state, when it changed while you decided.

use super::*;
use crate::actions::{Choice, Response};
use serde_json::{Value, json};

/// A question a backend asked after you'd moved off its card: it waits there, the card shown as
/// asking you (a yellow box, as any card that asks you something), until you open it (n, or the key
/// that asked it), or the card changes so it no longer applies. It's never a red "didn't start".
#[derive(Clone, Debug)]
pub struct Waiting {
    pub machine: usize,
    /// The request as sent, its answer, and the card as it was when the key was pressed: opening
    /// it asks exactly as the answer would have if you'd still been there.
    pub request: Value,
    pub response: Response,
    pub from: CardSnapshot,
    pub by_enter: bool,
    /// What its card says meanwhile: "asks you: past ARK's usual limit".
    pub line: String,
    /// What its card showed before it was marked as asking, put back when the question opens.
    pub was: Option<(Attention, String)>,
}

/// What a question asks, in a few words for its card: "past ARK's usual limit", "reopen it?".
/// None for kickoff questions, which the card names its own way.
fn asked_about(request: &Value, response: &Response) -> Option<String> {
    if !response.questions.is_empty() {
        return None;
    }
    let session = response
        .session
        .as_deref()
        .or(request["session"].as_str())
        .unwrap_or("its session");
    if response.used.is_some() {
        return Some(format!("past {session}'s usual limit"));
    }
    Some(
        match request["action"].as_str() {
            Some("close") => "mark it done?",
            Some("dismiss") => "set it aside?",
            Some("cancel") => "stop its agent?",
            Some("reopen") => "reopen it?",
            Some("edit") => "where your new words go",
            _ => "start it?",
        }
        .into(),
    )
}

/// The request a waiting question's yes would send, as ask_now prepares it: with its backend's
/// token, so it's checked as that backend's question about the card, the way its yes is (a stop of
/// running work, an edit of done work), and not as the bare request it answered (Codex's review).
fn asked(w: &Waiting) -> Value {
    let mut request = w.request.clone();
    if let Some(token) = &w.response.confirm {
        request["confirm"] = json!(token);
    }
    request
}

/// What a card says while a question waits on it, from what was asked.
fn waiting_line(request: &Value, response: &Response) -> String {
    match asked_about(request, response) {
        Some(what) => format!("asks you: {what}"),
        None => "questions for you".into(),
    }
}

/// The key that asks a request's question again, for the words that say so.
fn asking_key(request: &Value) -> &'static str {
    match request["action"].as_str() {
        Some("close") => "C",
        Some("dismiss") => "x",
        Some("cancel") => "X",
        Some("edit") => "e",
        Some("reopen") => "⏎",
        _ => "d",
    }
}

/// Whether a start or reopen answers its question another way than yes: a queue, or one of its
/// choices. Nothing starts on its card from it here, so the card shows nothing until it's answered.
pub(crate) fn answers_otherwise(request: &Value) -> bool {
    request["queue"] == true || request.get("choice").is_some()
}

impl App {
    /// The card an intent's request was about: the one it was asked from, under its key, which is
    /// its feed's source and number, so a move home or another machine's copy keeps it; with none,
    /// the one the machine asked would give that key. Never another source's card with the same
    /// number, which is other work (Codex's review).
    pub(super) fn intent_card(
        &self,
        machine: usize,
        id: &str,
        from: Option<&CardSnapshot>,
    ) -> Option<&Card> {
        self.card(&self.intent_key(machine, id, from))
    }

    /// The stable key of the card a request for record `id` on `machine` is about: the card it was
    /// asked from, else its record's source and number as that machine's feed names them. Never
    /// the number alone, which another source's card can share (Codex's fifth look).
    pub(crate) fn intent_key(
        &self,
        machine: usize,
        id: &str,
        from: Option<&CardSnapshot>,
    ) -> String {
        match from {
            Some(f) => f.key.clone(),
            None => {
                let source = self
                    .machines
                    .get(machine)
                    .and_then(|m| m.feed.as_ref())
                    .and_then(|f| f.source.clone())
                    .unwrap_or_else(|| "feed".into());
                format!("{source}:{id}")
            }
        }
    }

    /// The card an answer is for: the one it was asked from, else the card its change was
    /// queued for, by key, which it keeps even once that card has gone from the board; by its
    /// machine's source and number (`intent_key`) only for a change that never had a card
    /// (Codex's seventh look).
    pub(crate) fn answer_card_key(
        &self,
        machine: usize,
        id: &str,
        from: Option<&CardSnapshot>,
    ) -> String {
        match (from, &self.answer_key) {
            (Some(f), _) => f.key.clone(),
            (None, Some(key)) => key.clone(),
            (None, None) => self.intent_key(machine, id, None),
        }
    }

    /// A question that came while you were somewhere else: it waits on its card rather than fail,
    /// said quietly once, and n takes you to it (intent 0591).
    pub(super) fn wait_on_card(
        &mut self,
        machine: usize,
        request: Value,
        response: Response,
        from: Option<CardSnapshot>,
        by_enter: bool,
    ) {
        let id = request["id"].as_str().unwrap_or_default().to_string();
        let reference = from
            .as_ref()
            .and_then(|f| self.card(&f.key))
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| self.reference_for(&id));
        let key = asking_key(&request);
        let line = waiting_line(&request, &response);
        let said = match asked_about(&request, &response) {
            Some(what) => {
                format!("{reference} asks you first ({what}): n goes to it, and {key} on it asks.")
            }
            None => format!(
                "{reference} has kickoff questions for you: n goes to it, and {key} on it shows them."
            ),
        };
        let Some(from) = from else {
            // No card it was asked from: only said, as a question to come back to.
            self.say(
                format!("{reference} asks you something first; {key} on it asks again."),
                Tone::Info,
            );
            return;
        };
        let card_key = from.key.clone();
        let then = from.clone();
        // What it asked, with its token: a change its own first step made isn't one to name.
        let mut asked_with = request.clone();
        if let Some(token) = &response.confirm {
            asked_with["confirm"] = json!(token);
        }
        self.waiting.retain(|w| w.from.key != card_key);
        self.waiting.push(Waiting {
            machine,
            request,
            response,
            from,
            by_enter,
            line,
            was: None,
        });
        self.show_waiting();
        if self.waiting_on(&card_key).is_some() {
            self.say(said, Tone::Info);
            return;
        }
        // The card changed before the question came, so it no longer fits: said, and the key on
        // the card as it is now asks afresh.
        let text = match self.card(&card_key) {
            Some(now) => format!(
                "{}, so its question no longer fits; {key} on it asks again.",
                what_changed(
                    &self.as_its_steps_left_it(machine, &asked_with, &then, now),
                    now
                )
            ),
            None => format!("{reference} is gone from the board, so its question was dropped."),
        };
        self.say(text, Tone::Warn);
    }

    /// The question waiting on this card, if any.
    pub fn waiting_on(&self, key: &str) -> Option<&Waiting> {
        self.waiting.iter().find(|w| w.from.key == key)
    }

    /// What a card says while a question waits on it.
    pub fn waiting_line_for(&self, key: &str) -> Option<&str> {
        self.waiting_on(key).map(|w| w.line.as_str())
    }

    /// Questions waiting on cards, after each read: one whose card has gone, or changed so the
    /// question no longer fits it, is dropped (its key on the card as it is now asks afresh); every
    /// other shows its card as asking you, so n finds it.
    pub(super) fn show_waiting(&mut self) {
        if self.waiting.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.waiting);
        let kept: Vec<Waiting> = waiting
            .into_iter()
            .filter(|w| {
                self.card(&w.from.key).is_some_and(|c| w.from.same_work(c))
                    && self.confirm_still_applies(w.machine, &asked(w), Some(&w.from))
            })
            .collect();
        let mut kept = kept;
        for w in &mut kept {
            if let Some(card) = self.board.cards.iter_mut().find(|c| c.key == w.from.key) {
                // Read once per build: a card already marked keeps what it showed before.
                if card.why != w.line || card.attention != Attention::NeedsYou {
                    w.was = Some((card.attention, card.why.clone()));
                }
                card.attention = Attention::NeedsYou;
                card.why = w.line.clone();
            }
        }
        self.waiting = kept;
    }

    /// Opens the question waiting on this card, when you're on it: asked exactly as its answer
    /// would have been. Whether one opened.
    pub(super) fn open_waiting(&mut self, key: &str) -> bool {
        let Some(at) = self.waiting.iter().position(|w| w.from.key == key) else {
            return false;
        };
        if !self.still_on(&self.waiting[at].from.clone(), None) {
            return false;
        }
        let w = self.waiting.remove(at);
        if let (Some((attention, why)), Some(card)) = (
            w.was.clone(),
            self.board.cards.iter_mut().find(|c| c.key == w.from.key),
        ) {
            card.attention = attention;
            card.why = why;
        }
        self.ask_now(w.machine, w.request, w.response, Some(w.from), w.by_enter);
        true
    }

    /// Whether the key just pressed on this card is the one whose question waits on it: it opens
    /// that question rather than send its request again.
    pub(super) fn key_opens_waiting(&mut self, key: &str, action: &str) -> bool {
        let fits = self.waiting_on(key).is_some_and(|w| {
            let asked = w.request["action"].as_str().unwrap_or_default();
            asked == action || (matches!(asked, "dispatch" | "reopen") && action == "enter")
        });
        fits && self.open_waiting(key)
    }

    /// Asks the question an answer brought, while you're on its card: the Kickoff Questions box for
    /// open questions, else a yes or no with the answers its backend offers, shown whole.
    pub(super) fn ask_now(
        &mut self,
        machine: usize,
        request: Value,
        response: Response,
        from: Option<CardSnapshot>,
        by_enter: bool,
    ) {
        let id = request["id"].as_str().unwrap_or("this intent").to_string();
        let card = from.as_ref().and_then(|f| self.card(&f.key)).cloned();
        let reference = card
            .as_ref()
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| self.reference_for(&id));
        let title = card.as_ref().map(|c| c.title.clone()).unwrap_or_default();
        // What's kept for this card, by its key: never another source's with the same number
        // (Codex's fifth look).
        let held = self.intent_key(machine, &id, from.as_ref());
        let kept = self.kickoff_kept.get(&held).cloned().unwrap_or_default();
        // His answers given back earlier, when their questions changed while he answered, unless
        // this answer gives its own (intent 0591, B1-108).
        let mut response = response;
        if response.prefill.is_empty()
            && let Some(prefill) = self.kickoff_prefill.get(&held)
        {
            response.prefill = prefill.clone();
        }
        if let Some(form) = Kickoff::from_answer(
            machine,
            &request,
            &response,
            reference.clone(),
            title,
            &kept,
        ) {
            self.kickoff_prefill.remove(&held);
            self.mode = Mode::Kickoff {
                form,
                back: Box::new(self.mode.clone()),
                target: from.map(Box::new),
            };
            return;
        }
        let mut request = request;
        if let Some(fields) = request.as_object_mut() {
            for answer in ["override", "choice", "queue", "op"] {
                fields.remove(answer);
            }
        }
        // The yes goes back for what was asked: the session it named and its token.
        if let Some(session) = &response.session {
            request["session"] = json!(session);
        }
        if let Some(token) = &response.confirm {
            request["confirm"] = json!(token);
        }
        let session = response.session.as_deref().unwrap_or("its home session");
        let action = request["action"].as_str().unwrap_or_default().to_string();
        let starts = matches!(action.as_str(), "dispatch" | "reopen");
        let choices = response.choices.clone();
        // A full session's queue, from a backend that offers no choices: w, as before.
        let queue = action == "reopen" && response.used.is_some() && choices.is_empty();
        // At the most a session holds, a yes can't open it there: its queue is the answer, so the
        // title says so and ⏎ queues it (intent 0591, K2-54). That holds whether or not the backend
        // offers its queue as a choice: at the most, its own yes queues it (the end-to-end check).
        let at_most = starts
            && response.used.is_some_and(|used| {
                self.machines
                    .get(machine)
                    .and_then(|m| m.feed.as_ref())
                    .and_then(|f| f.sessions.get(session))
                    .and_then(|s| s.max)
                    .is_some_and(|most| used >= most)
            });
        let (title, message) = if at_most {
            (format!("Queue {reference} in {session}?"), response.message)
        } else {
            match action.as_str() {
                "close" => (format!("Mark {reference} done?"), response.message),
                "dismiss" => (format!("Won't do {reference}?"), response.message),
                "cancel" => (format!("Cancel {reference}?"), response.message),
                "edit" => (format!("Save {reference}'s new words?"), response.message),
                "reopen" => (
                    format!("Reopen {reference} in {session}?"),
                    response.message,
                ),
                _ => {
                    // The count, unless the backend's own words already give it ("web has 2
                    // workspaces. Start anyway?"): never said twice.
                    let message = match (response.used, response.limit) {
                        (Some(used), Some(limit))
                            if !controls::says_the_count(&response.message, used, limit) =>
                        {
                            format!(
                                "{used} active workspaces; the usual limit is {limit}. {}",
                                response.message
                            )
                        }
                        _ => response.message,
                    };
                    (format!("Start {reference} in {session}?"), message)
                }
            }
        };
        // A start ⏎ asked for stays ⏎'s through its yes: if that is followed too, its end goes in
        // (Codex's review).
        self.confirm_by_enter = by_enter
            .then(|| from.as_ref().map(|f| f.key.clone()))
            .flatten();
        self.mode = Mode::Confirm {
            machine,
            request,
            title,
            message,
            back: Box::new(self.mode.clone()),
            target: from.map(Box::new),
            queue,
            ready_at: Some(Instant::now() + Duration::from_millis(500)),
            plain: false,
            choices,
            enter_queues: at_most,
        };
    }

    /// A choice picked in a question: the request again with `choice` and the question's token, and
    /// no `override` (intent 0591). Sent only while the card is still as it was asked about; else the
    /// question is asked again for the card as it is now.
    pub(super) fn answer_choice(
        &mut self,
        machine: usize,
        mut request: Value,
        target: Option<Box<CardSnapshot>>,
        choice: &Choice,
        by_enter: Option<String>,
    ) -> Action {
        if !self.confirm_still_applies(machine, &request, target.as_deref()) {
            return self.ask_again(machine, request, target.map(|t| *t));
        }
        if let Some(fields) = request.as_object_mut() {
            fields.remove("override");
            fields.remove("queue");
        }
        request["choice"] = json!(choice.id);
        let action = self.emit_mutation(machine, request);
        if matches!(action, Action::Mutate { .. }) {
            self.enter_sent = by_enter;
            self.action_from = target.map(|t| *t);
        }
        action
    }

    /// w on a full session's question from a backend that offers no choices: the request again
    /// with `queue: true` and its token, for it to wait in that session's queue.
    pub(super) fn answer_queue(
        &mut self,
        machine: usize,
        mut request: Value,
        target: Option<Box<CardSnapshot>>,
        by_enter: Option<String>,
    ) -> Action {
        if !self.confirm_still_applies(machine, &request, target.as_deref()) {
            return self.ask_again(machine, request, target.map(|t| *t));
        }
        request["queue"] = json!(true);
        let action = self.emit_mutation(machine, request);
        if matches!(action, Action::Mutate { .. }) {
            self.enter_sent = by_enter;
            self.action_from = target.map(|t| *t);
        }
        action
    }

    /// A yes, or a choice, to a question about a card that changed while you decided (intent 0591,
    /// K1-38): never "select it again". The key that asked is tried on the card as it is now; when
    /// that asks a question (Kerdr's or its backend's), it's asked at once, led by what changed.
    /// What no longer applies is said, and nothing is sent.
    pub(super) fn ask_again(
        &mut self,
        machine: usize,
        request: Value,
        target: Option<CardSnapshot>,
    ) -> Action {
        if crate::app::cancels::is_bare_cancel(&request) {
            return self.bare_cancel_again(machine, &request);
        }
        let id = request["id"].as_str().unwrap_or_default().to_string();
        let reference = target
            .as_ref()
            .and_then(|t| self.card(&t.key))
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| self.reference_for(&id));
        let now = self.intent_card(machine, &id, target.as_ref()).cloned();
        let Some(card) = now else {
            self.say(
                format!(
                    "{reference} is gone from the board while you decided, so nothing was sent."
                ),
                Tone::Warn,
            );
            return Action::None;
        };
        let changed = target
            .as_ref()
            .map(|t| {
                what_changed(
                    &self.as_its_steps_left_it(machine, &request, t, &card),
                    &card,
                )
            })
            .unwrap_or_else(|| format!("{} changed", card.reference));
        // What he asked for goes with it, never dropped for a plainer key's request (Codex's
        // review). A save of his words goes again as it was, without its answer: its backend
        // checks them against the card as it is now (`was`) and asks again if it needs to.
        if request["action"] == "edit" && request.get("text").is_some() {
            let mut again = request;
            if let Some(fields) = again.as_object_mut() {
                for answer in ["override", "confirm", "choice", "queue", "op"] {
                    fields.remove(answer);
                }
            }
            // Where the card's record lives now, not where the first save went: it may have
            // moved home meanwhile (Codex's review).
            let Some(home) = self.home_machine(&card) else {
                return Action::None;
            };
            if let Some(held) = self.edits.get_mut(&card.key) {
                held.machine = home;
            }
            self.say(format!("{changed}; asking again."), Tone::Info);
            let action = self.emit_mutation(home, again);
            if matches!(action, Action::Mutate { .. }) {
                self.action_from = Some(CardSnapshot::of(&card));
            }
            return action;
        }
        // A start on the account he picked: that account goes to the card as it is now, as s
        // applies one picked while the card changed (intent 0591): started differently, moved to
        // it if an agent runs it now, or the picker again when it can't take it.
        if let Some(to) = request["account"].as_str()
            && matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
        {
            let to = to.to_string();
            self.sel.key = Some(card.key.clone());
            let start = match self.run_state(&card) {
                crate::app::run_state::RunState::Stopped(start) => start,
                _ => Default::default(),
            };
            let said_before = self.message.clone();
            let action = self.start_on_account(&card.key, &start, &to);
            if matches!(action, Action::Mutate { .. })
                && self
                    .message
                    .as_ref()
                    .is_none_or(|m| said_before.as_ref().is_some_and(|b| b.0 == m.0))
            {
                self.say(format!("{changed}; asking again."), Tone::Info);
            }
            return action;
        }
        let key = match request["action"].as_str() {
            Some("close") => 'C',
            Some("dismiss") => 'x',
            Some("cancel") => 'X',
            Some("reopen") => '\n',
            _ => 'd',
        };
        // The key on the card as it is now, chosen as if pressed there.
        self.sel.key = Some(card.key.clone());
        if matches!(self.mode, Mode::Backlog { .. }) {
            // The backlog's own selection follows its focus at the next build.
            self.backlog_focus = Some(card.key.clone());
            self.rebuild();
            // It left this list (it started, say): no key here acts on whatever took its place.
            if self.selected_for_control().map(|c| &c.key) != Some(&card.key) {
                self.say(
                    format!("{changed}, so nothing was sent; it's no longer in this list."),
                    Tone::Warn,
                );
                return Action::None;
            }
        }
        let said_before = self.message.clone();
        let action = if key == '\n' {
            self.jump_action(card.key.clone())
        } else {
            self.card_action(key, 1)
        };
        match (&mut self.mode, &action) {
            // A question of Kerdr's own: led by what changed.
            (Mode::Confirm { message, .. }, Action::None) => {
                *message = format!("{changed}. {message}");
                action
            }
            (Mode::Kickoff { .. }, Action::None) => action,
            // Sent to its backend afresh: it decides for the card as it is now, and asks again if it
            // needs to; its new question comes back at once.
            (_, Action::Mutate { request: sent, .. })
                if sent["action"] == request["action"] && sent.get("override").is_none() =>
            {
                self.say(format!("{changed}; asking again."), Tone::Info);
                action
            }
            // A yes to reopen it, now that it has a workspace again: Enter goes in, as it would
            // on the card now. Going in changes nothing.
            (_, Action::Jump(_)) if key == '\n' => {
                self.say(format!("{changed}; going in."), Tone::Info);
                action
            }
            _ => {
                // It means something else now (going in, say): said, not done, since the yes was
                // for something else.
                let now_said = self
                    .message
                    .as_ref()
                    .filter(|m| said_before.as_ref().is_none_or(|b| b.0 != m.0))
                    .map(|m| m.0.clone());
                let text = match now_said {
                    Some(said) => format!("{changed}, so nothing was sent. {said}"),
                    None => format!("{changed}, so nothing was sent."),
                };
                self.say(text, Tone::Warn);
                Action::None
            }
        }
    }
}

/// What changed about a card since a question was asked about it, in a few words: "#41 started
/// meanwhile", "#41 now runs in W2".
pub(crate) fn what_changed(from: &CardSnapshot, now: &Card) -> String {
    let reference = &now.reference;
    let then = from;
    let was_open = then.workspace.is_some();
    let now_snap = CardSnapshot::of(now);
    if now_snap.dismissed && !then.dismissed {
        return format!("{reference} was set aside meanwhile");
    }
    if now_snap.done && !then.done {
        return format!("{reference} was marked done meanwhile");
    }
    if then.dismissed && !now_snap.dismissed {
        return format!("{reference} was brought back meanwhile");
    }
    if then.done && !now_snap.done {
        return format!("{reference} was reopened meanwhile");
    }
    if now_snap.home_machine != then.home_machine || now_snap.home_session != then.home_session {
        let place = match (&now_snap.home_machine, &now_snap.home_session) {
            (Some(m), Some(s)) => format!("{m}/{s}"),
            (Some(m), None) => m.clone(),
            (None, Some(s)) => s.clone(),
            (None, None) => "another place".into(),
        };
        return format!("{reference} moved to {place} meanwhile");
    }
    match (&then.workspace, &now.workspace_id) {
        (None, Some(_)) => {
            let place = now
                .workspace_label
                .clone()
                .or_else(|| now.workspace_id.clone())
                .unwrap_or_default();
            return format!("{reference} now runs in {place}");
        }
        (Some(a), Some(b)) if a != b => {
            let place = now.workspace_label.clone().unwrap_or_else(|| b.clone());
            return format!("{reference} now runs in {place}");
        }
        (Some(_), None) if was_open => {
            return format!("{reference}'s workspace closed meanwhile");
        }
        _ => {}
    }
    if then.waiting && !crate::app::waiting_work(now) {
        return format!("{reference} started meanwhile");
    }
    if !then.waiting && crate::app::waiting_work(now) {
        return format!("{reference} went back to the backlog meanwhile");
    }
    if now_snap.resume_conversation != then.resume_conversation
        || now_snap.resume_agent != then.resume_agent
    {
        return format!("{reference}'s conversation changed meanwhile");
    }
    format!("{reference} changed meanwhile")
}

#[cfg(test)]
mod tests {
    use crate::actions::{Choice, choice_keys, parse_response};

    fn choice(id: &str, key: &str) -> Choice {
        Choice {
            id: id.into(),
            key: key.into(),
            label: format!("{id} label"),
        }
    }

    #[test]
    fn a_choice_takes_its_suggested_key_unless_taken_then_the_next_free_digit() {
        let keys = choice_keys(&[
            choice("queue", "w"),
            choice("shelve-oldest", "y"),
            choice("other", "w"),
            choice("third", "N"),
            choice("fourth", "ab"),
            choice("fifth", "o"),
        ]);
        assert_eq!(
            keys,
            vec![
                Some('w'),
                Some('1'),
                Some('2'),
                Some('3'),
                Some('4'),
                Some('o')
            ]
        );
    }

    #[test]
    fn n_q_and_y_are_never_a_choice_s_key() {
        let keys = choice_keys(&[choice("a", "n"), choice("b", "q"), choice("c", "y")]);
        assert_eq!(keys, vec![Some('1'), Some('2'), Some('3')]);
    }

    #[test]
    fn choices_are_read_leniently_and_never_fail_the_answer() {
        let r = parse_response(
            r#"{"ok":false,"message":"Start it anyway?","confirmation":true,"confirm":"t",
                "choices":[{"id":"queue","key":"w","label":"queue it"},
                           {"id":"Bad Id","label":"x"},
                           {"id":"no-label","label":"  "},
                           {"id":"queue","key":"q","label":"twice"},
                           "not an object",
                           {"id":"shelve-oldest","label":"shelve\nthe oldest"}]}"#,
        )
        .unwrap();
        let ids: Vec<&str> = r.choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["queue", "shelve-oldest"]);
        assert_eq!(r.choices[1].label, "shelve the oldest");
        let none = parse_response(r#"{"ok":false,"message":"m","choices":"queue"}"#).unwrap();
        assert!(none.choices.is_empty());
        let absent = parse_response(r#"{"ok":true,"message":"m"}"#).unwrap();
        assert!(absent.choices.is_empty());
    }
}
