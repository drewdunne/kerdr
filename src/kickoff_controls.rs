//! The Kickoff Questions box (intent 0409): `d` on background work whose questions for you are still
//! open. The backend answers the start with those questions instead of starting it, and this box
//! takes them one at a time, whole: ⏎ takes the recommendation or sends what you typed. The answers
//! go back with the start itself, so nothing is saved until the last one, and then it runs on its
//! own. Esc keeps the answers already given, and leaves the rest for the next `d`.
//!
//! When the card's feed read its questions ahead (intent 0491), `d` opens the box at once from those,
//! and a `kickoff` request, which starts nothing, checks them behind it: the box takes what's true
//! now from its answer without losing what's typed, and says so if the questions changed. Answers
//! carry the token of what they answered, so the backend still refuses any it wasn't asking.

use super::*;
use crate::actions::{Prefill, Question, Response};
use crate::model::CardSnapshot;
use serde_json::{Value, json};
use std::cell::Cell;

/// A new question never takes a key held down, or one pressed before it appeared.
const READY_AFTER: Duration = Duration::from_millis(500);
/// As long as the backend takes an answer.
const ANSWER_MOST: usize = 2000;
/// When the questions read ahead turned out not to be the ones it asks now.
const CHANGED: &str =
    "These questions changed since the board last read them. These are the current ones.";
/// What it adds when it has words typed for a question that has gone or been reworded.
const CHANGED_KEPT: &str = "What you typed for the old wording:";
/// When the check behind a box opened from the feed gets no answer.
const UNCHECKED: &str =
    "Kerdr couldn't check these are still current. Your answers are checked when they're sent.";

/// Words typed for a question, kept until they're saved: its number, its text, and the words. They
/// fill the box again only for that same question (a question reworded since isn't the same one).
pub type KeptAnswer = (String, String, String);

/// Text from a record never reaches the terminal as anything but text.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn cleaned(questions: &[Question]) -> Vec<Question> {
    questions
        .iter()
        .map(|q| Question {
            num: q.num.clone(),
            question: clean(&q.question),
            background: clean(&q.background),
            pick: clean(&q.pick),
            why: clean(&q.why),
        })
        .collect()
}

/// What was typed for each of these questions: only for that same question, not one reworded since.
fn drafts_for(questions: &[Question], kept: &[KeptAnswer]) -> Vec<String> {
    questions
        .iter()
        .map(|q| {
            kept.iter()
                .find(|(num, question, _)| *num == q.num && *question == q.question)
                .map_or_else(String::new, |(.., words)| words.clone())
        })
        .collect()
}

/// His answers that couldn't be saved, because their questions changed while he answered, filled
/// in again on the question with the same number where nothing's typed (intent 0591, B1-108): his
/// words, for him to check. One he took the recommendation for stays as it is, the recommendation.
/// The numbers it filled.
fn fill_in(questions: &[Question], drafts: &mut [String], prefill: &[Prefill]) -> Vec<String> {
    let mut filled = vec![];
    for (q, draft) in questions.iter().zip(drafts.iter_mut()) {
        if !draft.trim().is_empty() {
            continue;
        }
        if let Some(text) = prefill
            .iter()
            .find(|p| p.num == q.num)
            .and_then(|p| p.text.as_deref())
        {
            *draft = as_filled(text);
            filled.push(q.num.clone());
        }
    }
    filled
}

/// Words given back, as the box shows them.
fn as_filled(text: &str) -> String {
    clean(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(ANSWER_MOST)
        .collect()
}

/// A note with any words typed against an old wording after it.
fn with_orphaned(note: Option<String>, orphaned: &[String]) -> Option<String> {
    if orphaned.is_empty() {
        return note;
    }
    let words = format!("{CHANGED_KEPT} {}", orphaned.join(" / "));
    Some(note.map_or_else(|| words.clone(), |note| format!("{note} {words}")))
}

/// What's on screen in the box.
#[derive(Clone, Debug)]
pub struct Kickoff {
    pub machine: usize,
    /// The start it answers, as it was asked for.
    pub request: Value,
    pub reference: String,
    pub title: String,
    pub questions: Vec<Question>,
    /// The question showing.
    pub at: usize,
    /// How many are answered: ↑ goes back to one without taking its answer away.
    pub reached: usize,
    /// What's typed for each question; nothing means its recommendation.
    pub drafts: Vec<String>,
    /// The token of what was asked, sent back with the answers.
    pub asked: String,
    /// The token for starting it with you along instead, when that's offered.
    pub along: Option<String>,
    /// Something the backend wants said in the box.
    pub note: Option<String>,
    /// Which check of the questions this box waits on; 0 when none was sent behind it.
    pub check: u64,
    /// Words typed for a question the check found changed: they're no answer to the new one, so
    /// they stay in the note, and go with the rest of what's kept if he leaves (intent 0491).
    pub orphaned: Vec<String>,
    /// His answers its backend gave back because their questions changed while he answered:
    /// filled in by number, again after a check finds the questions changed (intent 0591).
    pub prefill: Vec<Prefill>,
    pub ready_at: Instant,
    /// How far the question's text is scrolled, on a screen too small for it, and how far it can
    /// be, which only drawing it can tell.
    pub scroll: Cell<u16>,
    pub scroll_most: Cell<u16>,
}

impl Kickoff {
    /// The box a start's answer asks for, or None when it asks nothing it can take answers to. It
    /// can come from a reopen too, of work that turned out never to have started; its answers go
    /// back with a start either way. `kept` is what was typed before and not saved.
    pub fn from_answer(
        machine: usize,
        request: &Value,
        response: &Response,
        reference: String,
        title: String,
        kept: &[KeptAnswer],
    ) -> Option<Kickoff> {
        let asked = response
            .asked
            .clone()
            .filter(|_| !response.questions.is_empty())?;
        let mut request = request.clone();
        if let Some(fields) = request.as_object_mut() {
            for key in ["override", "answers", "asked", "confirm"] {
                fields.remove(key);
            }
        }
        request["action"] = json!("dispatch");
        if let Some(session) = &response.session {
            request["session"] = json!(session);
        }
        let questions = cleaned(&response.questions);
        let mut drafts = drafts_for(&questions, kept);
        fill_in(&questions, &mut drafts, &response.prefill);
        let orphaned: Vec<String> = kept
            .iter()
            .filter(|(num, ..)| num.is_empty())
            .map(|(.., words)| words.clone())
            .collect();
        Some(Kickoff {
            machine,
            request,
            reference: clean(&reference),
            title: clean(&title),
            drafts,
            questions,
            at: 0,
            reached: 0,
            asked,
            along: if response.confirmation {
                response.confirm.clone()
            } else {
                None
            },
            note: with_orphaned(response.note.as_deref().map(clean), &orphaned),
            check: 0,
            orphaned,
            prefill: response.prefill.clone(),
            ready_at: Instant::now() + READY_AFTER,
            scroll: Cell::new(0),
            scroll_most: Cell::new(0),
        })
    }

    /// What the check behind a box opened from the feed answered: the questions as they are now,
    /// and what only the backend can say (the session, Tab's token, a note). Questions that didn't
    /// change stay exactly where they are, with what's typed in them.
    fn checked(&mut self, machine: usize, response: &Response) {
        self.machine = machine;
        if let Some(session) = &response.session {
            self.request["session"] = json!(session);
        }
        self.along = if response.confirmation {
            response.confirm.clone()
        } else {
            None
        };
        self.asked = response.asked.clone().unwrap_or_default();
        self.note = with_orphaned(response.note.as_deref().map(clean), &self.orphaned);
        let now = cleaned(&response.questions);
        if now != self.questions {
            let kept = self.kept();
            self.drafts = drafts_for(&now, &kept);
            // Answers given back to fill in go on the question with their number, as reworded now.
            let filled = fill_in(&now, &mut self.drafts, &self.prefill);
            // Words typed against the old wording can't be answers to the new one, but they were
            // typed while this box was up, so they stay on screen rather than vanish (intent 0491).
            // Words given back that are filled in again aren't lost, so they aren't said twice.
            let refilled = |num: &String, words: &String| {
                filled.contains(num)
                    && self.prefill.iter().any(|p| {
                        p.num == *num
                            && p.text.as_deref().map(as_filled).as_deref() == Some(words.as_str())
                    })
            };
            let lost: Vec<String> = kept
                .iter()
                .filter(|(num, question, words)| {
                    !now.iter().any(|q| q.num == *num && q.question == *question)
                        && !refilled(num, words)
                })
                .map(|(.., words)| clean(words))
                .collect();
            self.orphaned.extend(lost);
            self.questions = now;
            self.at = 0;
            self.reached = 0;
            self.note = with_orphaned(Some(CHANGED.into()), &self.orphaned);
            self.ready_at = Instant::now() + READY_AFTER;
            self.scroll.set(0);
        }
    }

    /// What's typed so far, to keep until it's saved.
    fn kept(&self) -> Vec<KeptAnswer> {
        self.questions
            .iter()
            .zip(&self.drafts)
            .filter(|(_, words)| !words.trim().is_empty())
            .map(|(q, words)| (q.num.clone(), q.question.clone(), words.clone()))
            .collect()
    }

    pub fn question(&self) -> &Question {
        &self.questions[self.at]
    }

    pub fn typed(&self) -> &str {
        &self.drafts[self.at]
    }

    pub fn is_last(&self) -> bool {
        self.at + 1 == self.questions.len()
    }

    pub fn recommends(&self) -> bool {
        let q = self.question();
        !q.pick.trim().is_empty() || !q.why.trim().is_empty()
    }

    /// The first `count` questions' answers, as the backend takes them.
    fn answers(&self, count: usize) -> Value {
        let answers: Vec<Value> = self
            .questions
            .iter()
            .zip(&self.drafts)
            .take(count)
            .map(
                |(q, draft)| match draft.split_whitespace().collect::<Vec<_>>().join(" ") {
                    said if said.is_empty() => json!({"num": q.num, "take": true}),
                    said => json!({"num": q.num, "text": said}),
                },
            )
            .collect();
        Value::Array(answers)
    }
}

/// How the box was left.
enum Leaving {
    /// The last answer: it starts.
    Start,
    /// Tab: it starts with you along, its other questions left for its agent.
    Along,
    /// Esc: what's answered is kept, and nothing starts.
    Keep,
}

impl App {
    /// `d` on work whose questions its feed read ahead: the box opens now, and this is the check
    /// to send behind it. None when there's nothing to open from, so the start goes as it would.
    pub(super) fn open_preloaded_kickoff(
        &mut self,
        card: &Card,
        machine: usize,
        request: &Value,
    ) -> Option<Action> {
        if request["action"] != "dispatch" || request.get("override").is_some() {
            return None;
        }
        let detail = card.detail.as_ref()?;
        let preloaded = detail.kickoff.as_ref()?;
        if self.kickoff_sent.contains(&preloaded.asked) {
            return None;
        }
        // His answers given back when their questions changed, filled in again (intent 0591).
        let response = Response {
            asked: Some(preloaded.asked.clone()),
            questions: preloaded.questions.clone(),
            prefill: self
                .kickoff_prefill
                .get(&card.key)
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        };
        let kept = self
            .kickoff_kept
            .get(&card.key)
            .cloned()
            .unwrap_or_default();
        let mut form = Kickoff::from_answer(
            machine,
            request,
            &response,
            card.reference.clone(),
            card.title.clone(),
            &kept,
        )?;
        self.kickoff_checks += 1;
        form.check = self.kickoff_checks;
        let check = form.check;
        let id = detail.id.clone();
        // In the box now: kept there, and with what's typed if he leaves it.
        self.kickoff_prefill.remove(&card.key);
        // Its check is this card's, by key (Codex's seventh look).
        self.action_key = Some(card.key.clone());
        self.mode = Mode::Kickoff {
            form,
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
        };
        Some(Action::Mutate {
            machine,
            request: json!({"action": "kickoff", "id": id, "check": check}),
        })
    }

    /// The check's answer, while the box it was sent behind is still up; once that's gone, nothing
    /// it says matters: his answers went with their token, and the backend checks that itself. A
    /// token the check disproves is done with, so a feed still holding it can't open the box again.
    pub(super) fn finish_kickoff_check(
        &mut self,
        machine: usize,
        request: &Value,
        result: Result<Response, String>,
    ) {
        let Mode::Kickoff { form, back, target } = &mut self.mode else {
            return;
        };
        if form.request["id"] != request["id"] || request["check"] != json!(form.check) {
            return;
        }
        match result {
            Ok(response) if !response.questions.is_empty() && response.asked.is_some() => {
                if response.asked.as_ref() != Some(&form.asked) {
                    self.kickoff_sent.insert(form.asked.clone());
                }
                form.checked(machine, &response);
            }
            Err(_) => form.note = Some(UNCHECKED.into()),
            // A backend older than this request: as good as no answer.
            Ok(response) if !response.ok && response.message == "unknown action" => {
                form.note = Some(UNCHECKED.into());
            }
            // None left to ask, or it can't be answered now: the box goes, keeping what's typed.
            Ok(response) => {
                let (form, back, target) = (form.clone(), back.clone(), target.clone());
                self.kickoff_sent.insert(form.asked.clone());
                self.keep_kickoff(&form, target.as_deref());
                self.mode = *back;
                if response.ok {
                    self.say(response.message, Tone::Info);
                } else {
                    self.fail(response.message);
                }
            }
        }
    }

    pub(super) fn kickoff_key(
        &mut self,
        key: KeyEvent,
        mut form: Kickoff,
        back: Box<Mode>,
        target: Option<Box<CardSnapshot>>,
    ) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.mode = *back;
                return if form.reached > 0 {
                    self.leave_kickoff(form, target, Leaving::Keep)
                } else {
                    self.keep_kickoff(&form, target.as_deref());
                    Action::None
                };
            }
            KeyCode::Tab if form.along.is_some() => {
                self.mode = *back;
                return self.leave_kickoff(form, target, Leaving::Along);
            }
            KeyCode::Up if form.at > 0 => {
                // No guard to wait out: you've read this one already (intent 0491).
                form.at -= 1;
                form.scroll.set(0);
            }
            KeyCode::PageDown => {
                form.scroll
                    .set((form.scroll.get() + 3).min(form.scroll_most.get()));
            }
            KeyCode::PageUp => form.scroll.set(form.scroll.get().saturating_sub(3)),
            // In a box of one paragraph, a line break is never a send.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {}
            KeyCode::Enter => {
                if key.kind == KeyEventKind::Repeat || Instant::now() < form.ready_at {
                    // A key held down, or pressed before this question showed, never answers it.
                } else if form.typed().trim().is_empty() && !form.recommends() {
                    self.say(
                        "This one has no recommendation, so it needs your own answer.",
                        Tone::Info,
                    );
                } else if form.typed().trim().is_empty()
                    && form.scroll.get() < form.scroll_most.get()
                {
                    // Its recommendation isn't all on screen, and ⏎ never takes what you can't see.
                    form.scroll.set(form.scroll_most.get());
                    form.ready_at = Instant::now() + READY_AFTER;
                } else if form.is_last() {
                    self.mode = *back;
                    return self.leave_kickoff(form, target, Leaving::Start);
                } else {
                    form.at += 1;
                    form.reached = form.reached.max(form.at);
                    form.scroll.set(0);
                    form.scroll_most.set(0);
                    form.ready_at = Instant::now() + READY_AFTER;
                }
            }
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let draft = &mut form.drafts[form.at];
                let kept = draft
                    .trim_end()
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i + 1);
                draft.truncate(kept);
            }
            KeyCode::Backspace => {
                form.drafts[form.at].pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.drafts[form.at].clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && form.typed().chars().count() < ANSWER_MOST =>
            {
                form.drafts[form.at].push(c);
            }
            _ => {}
        }
        self.mode = Mode::Kickoff { form, back, target };
        Action::None
    }

    /// Keeps what's typed in the box until it's saved, for when `d` opens it again: for its own
    /// card, by key, never another source's with the same number (Codex's fifth look).
    fn keep_kickoff(&mut self, form: &Kickoff, target: Option<&CardSnapshot>) {
        let Some(id) = form.request["id"].as_str() else {
            return;
        };
        let held = self.intent_key(form.machine, id, target);
        let mut kept = form.kept();
        // No question of their own: an empty number marks them for the next box's note.
        kept.extend(
            form.orphaned
                .iter()
                .map(|words| (String::new(), String::new(), words.clone())),
        );
        if kept.is_empty() {
            self.kickoff_kept.remove(&held);
        } else {
            self.kickoff_kept.insert(held, kept);
        }
    }

    /// What a failure adds when it has words of yours in hand: where they went. `held` is its
    /// card's key.
    pub(super) fn kickoff_kept_note(&self, request: &Value, held: Option<&str>) -> &'static str {
        let holds = matches!(request["action"].as_str(), Some("dispatch" | "answer"))
            && request.get("answers").is_some()
            && held.is_some_and(|k| self.kickoff_kept.contains_key(k));
        if holds {
            " Your answers are kept: d opens them again."
        } else {
            ""
        }
    }

    /// Text pasted into the box goes in as one line.
    pub(super) fn kickoff_paste(form: &mut Kickoff, text: &str) {
        let room = ANSWER_MOST.saturating_sub(form.typed().chars().count());
        let line: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(room)
            .collect();
        form.drafts[form.at].push_str(&line);
    }

    /// Whether it's still work nobody started, on that machine. Not the card as it was: asking the
    /// questions can itself change its record, and the backend checks they're the same ones.
    fn kickoff_still_applies(&self, form: &Kickoff) -> bool {
        let Some(id) = form.request["id"].as_str() else {
            return false;
        };
        let name = self.machines.get(form.machine).map(|m| m.name());
        self.board.cards.iter().any(|c| {
            c.detail.as_ref().is_some_and(|d| {
                d.id == id
                    && (self.moved_target(c) == Some(form.machine)
                        || c.machine_index == form.machine
                        || d.home.as_ref().and_then(|h| h.machine.as_deref()) == name.as_deref())
            }) && !c.dismissed()
                && !c.is_open_workspace()
                && !matches!(c.state.as_deref(), Some("closed" | "active" | "background"))
        })
    }

    fn leave_kickoff(
        &mut self,
        form: Kickoff,
        target: Option<Box<CardSnapshot>>,
        how: Leaving,
    ) -> Action {
        self.keep_kickoff(&form, target.as_deref());
        let answered = if matches!(how, Leaving::Start) {
            form.questions.len()
        } else {
            form.reached
        };
        if !self.kickoff_still_applies(&form) {
            return self.answers_for_changed_work(form, target.map(|t| *t), answered);
        }
        // The feed still has these until its next read: d goes by the backend's answer till then.
        if answered > 0 {
            self.kickoff_sent.insert(form.asked.clone());
        }
        let answers = form.answers(answered);
        let request = match how {
            Leaving::Keep => json!({
                "action": "answer",
                "id": form.request["id"],
                "asked": form.asked,
                "answers": answers,
            }),
            Leaving::Start | Leaving::Along => {
                let mut request = form.request.clone();
                if answered > 0 {
                    request["asked"] = json!(form.asked);
                    request["answers"] = answers;
                }
                if let (Leaving::Along, Some(token)) = (&how, &form.along) {
                    request["override"] = json!(true);
                    request["confirm"] = json!(token);
                }
                request
            }
        };
        let action = self.emit_mutation(form.machine, request);
        if matches!(action, Action::Mutate { .. }) {
            self.action_from = target.map(|t| *t);
        }
        action
    }

    /// The card stopped being unstarted work there while he answered (it started, was set aside or
    /// moved home): what he answered is saved on it anyway, where it lives now, and nothing starts.
    /// The questions' token goes with them, so its backend saves none for questions that changed
    /// (intent 0591, K1-76). A backend from before that refuses in its own words, and the box keeps
    /// his words for the next d either way.
    fn answers_for_changed_work(
        &mut self,
        form: Kickoff,
        from: Option<CardSnapshot>,
        answered: usize,
    ) -> Action {
        let id = form.request["id"].as_str().unwrap_or_default().to_string();
        // Its own card, never another source's with the same number (Codex's review).
        let card = self.intent_card(form.machine, &id, from.as_ref()).cloned();
        let Some(card) = card else {
            self.say(
                format!(
                    "{} is gone from the board while you answered, so your answers weren't sent; they're kept for its next d.",
                    form.reference
                ),
                Tone::Warn,
            );
            return Action::None;
        };
        // As it was when the box opened: unstarted work, not set aside, with no workspace.
        let then = from.unwrap_or_else(|| CardSnapshot {
            workspace: None,
            waiting: true,
            done: false,
            dismissed: false,
            ..CardSnapshot::of(&card)
        });
        let changed = crate::app::asking::what_changed(&then, &card);
        if answered == 0 {
            self.say(format!("{changed}, so nothing was sent."), Tone::Warn);
            return Action::None;
        }
        let Some(machine) = self.home_machine(&card) else {
            // Its home isn't one of Kerdr's machines: home_machine said so.
            return Action::None;
        };
        self.kickoff_sent.insert(form.asked.clone());
        let request = json!({
            "action": "answer",
            "id": id,
            "asked": form.asked,
            "answers": form.answers(answered),
        });
        let action = self.emit_mutation(machine, request);
        if matches!(action, Action::Mutate { .. }) {
            self.action_from = Some(CardSnapshot::of(&card));
            self.say(
                format!("{changed}; saving your answers on it, and nothing starts."),
                Tone::Info,
            );
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn question(num: &str, question: &str, pick: &str) -> Question {
        Question {
            num: num.into(),
            question: question.into(),
            background: "Why it matters.".into(),
            pick: pick.into(),
            why: "the reason".into(),
        }
    }

    fn answer() -> Response {
        Response {
            ok: false,
            message: "for an older Kerdr".into(),
            confirmation: true,
            confirm: Some("0123456789abcdef".into()),
            asked: Some("fedcba9876543210".into()),
            session: Some("web".into()),
            questions: vec![
                question("1", "Whose is it?", "Kibbelz"),
                question("3", "Where does it run?", "The server"),
            ],
            ..Response::default()
        }
    }

    fn form(id: &str, response: &Response) -> Kickoff {
        let request = json!({"action": "dispatch", "id": id, "override": true, "confirm": "old", "asked": "old", "answers": []});
        let mut form =
            Kickoff::from_answer(0, &request, response, "#41".into(), "Checkout".into(), &[])
                .unwrap();
        form.ready_at = Instant::now();
        form
    }

    /// The box over a card it can start, as its answer opened it; and that card's number.
    fn open_on(response: &Response) -> (App, String) {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && !c.dismissed()
                    && !c.is_open_workspace()
                    && c.state.as_deref() != Some("shelved")
            })
            .expect("a waiting backlog card in the demo");
        let (machine, id) = (card.machine_index, card.detail.as_ref().unwrap().id.clone());
        let mut form = form(&id, response);
        form.machine = machine;
        app.mode = Mode::Kickoff {
            form,
            back: Box::new(Mode::Board),
            target: None,
        };
        (app, id)
    }

    fn open(response: &Response) -> App {
        open_on(response).0
    }

    /// The key the demo's card for record `id` has, which what's kept for it goes by.
    fn held(id: &str) -> String {
        format!("demo:{id}")
    }

    /// Presses a key, past the wait a new question has.
    fn key(app: &mut App, code: KeyCode) -> Action {
        if let Mode::Kickoff { form, .. } = &mut app.mode {
            form.ready_at = Instant::now();
        }
        let Mode::Kickoff { form, back, target } = app.mode.clone() else {
            panic!("the box is shut")
        };
        app.kickoff_key(press(code), form, back, target)
    }

    fn typed(app: &mut App, words: &str) {
        for c in words.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    fn sent(action: Action) -> Value {
        match action {
            Action::Mutate { request, .. } => request,
            other => panic!("nothing was sent: {other:?}"),
        }
    }

    #[test]
    fn it_takes_only_what_it_can_answer() {
        let request = json!({"action": "dispatch", "id": "0041"});
        assert!(
            Kickoff::from_answer(
                0,
                &request,
                &Response::default(),
                "#41".into(),
                "t".into(),
                &[]
            )
            .is_none()
        );
        let no_token = Response {
            asked: None,
            ..answer()
        };
        assert!(
            Kickoff::from_answer(0, &request, &no_token, "#41".into(), "t".into(), &[]).is_none()
        );
        let fresh = form("0041", &answer());
        // What the last try carried never goes again by accident, and the session it named does.
        assert_eq!(
            fresh.request,
            json!({"action": "dispatch", "id": "0041", "session": "web"})
        );
        assert_eq!(fresh.along.as_deref(), Some("0123456789abcdef"));
    }

    #[test]
    fn enter_takes_the_recommendation_and_typed_words_are_your_answer_and_the_last_starts_it() {
        let (mut app, id) = open_on(&answer());
        assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.question().num, "3");
        typed(&mut app, "  on the  laptop ");
        let request = sent(key(&mut app, KeyCode::Enter));
        assert_eq!(
            request,
            json!({"action": "dispatch", "id": id, "session": "web", "asked": "fedcba9876543210",
                   "answers": [{"num": "1", "take": true}, {"num": "3", "text": "on the laptop"}]})
        );
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn a_held_key_or_one_pressed_too_soon_never_answers() {
        let mut app = open(&answer());
        let Mode::Kickoff {
            mut form,
            back,
            target,
        } = app.mode.clone()
        else {
            panic!()
        };
        form.ready_at = Instant::now() + Duration::from_secs(60);
        app.mode = Mode::Kickoff {
            form: form.clone(),
            back: back.clone(),
            target: target.clone(),
        };
        app.kickoff_key(
            press(KeyCode::Enter),
            form.clone(),
            back.clone(),
            target.clone(),
        );
        let Mode::Kickoff { form: now, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(now.at, 0);
        form.ready_at = Instant::now();
        let held =
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat);
        app.kickoff_key(held, form, back, target);
        let Mode::Kickoff { form: now, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(now.at, 0);
    }

    #[test]
    fn up_goes_back_with_its_words_still_there() {
        let mut app = open(&answer());
        typed(&mut app, "mine");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Up);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!((form.at, form.typed()), (0, "mine"));
        key(&mut app, KeyCode::Up);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.at, 0, "there's nothing before the first");
    }

    #[test]
    fn going_back_to_a_question_needs_no_wait_before_enter_takes_it() {
        // Already read, unlike one that has just appeared (intent 0491).
        let mut app = open(&answer());
        typed(&mut app, "mine");
        key(&mut app, KeyCode::Enter);
        let Mode::Kickoff { form, .. } = &mut app.mode else {
            panic!()
        };
        assert_eq!(form.at, 1);
        form.ready_at = Instant::now();
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.at, 1, "the Enter counted");
        assert_eq!(form.drafts[0], "mine");
    }

    #[test]
    fn esc_keeps_what_is_answered_and_starts_nothing() {
        let mut app = open(&answer());
        assert!(
            matches!(key(&mut app, KeyCode::Esc), Action::None),
            "nothing answered, nothing sent"
        );
        assert!(matches!(app.mode, Mode::Board));

        let (mut app, id) = open_on(&answer());
        key(&mut app, KeyCode::Enter);
        typed(&mut app, "half typed");
        let request = sent(key(&mut app, KeyCode::Esc));
        assert_eq!(
            request,
            json!({"action": "answer", "id": id, "asked": "fedcba9876543210", "answers": [{"num": "1", "take": true}]})
        );
    }

    #[test]
    fn tab_starts_it_with_you_along_carrying_what_is_answered() {
        let (mut app, id) = open_on(&answer());
        let request = sent(key(&mut app, KeyCode::Tab));
        assert_eq!(
            request,
            json!({"action": "dispatch", "id": id, "session": "web", "override": true, "confirm": "0123456789abcdef"})
        );

        let mut app = open(&answer());
        key(&mut app, KeyCode::Enter);
        let request = sent(key(&mut app, KeyCode::Tab));
        assert_eq!(request["answers"], json!([{"num": "1", "take": true}]));
        assert_eq!(request["override"], json!(true));
    }

    /// A demo backlog card whose feed read these questions ahead, and the app it's in.
    fn preloaded(questions: Vec<Question>) -> (App, Card) {
        let (mut app, id) = open_on(&answer());
        app.mode = Mode::Board;
        let card = app
            .board
            .cards
            .iter_mut()
            .find(|c| c.detail.as_ref().is_some_and(|d| d.id == id))
            .unwrap();
        card.detail.as_mut().unwrap().kickoff = Some(crate::feed::Preloaded {
            asked: "feedread".into(),
            questions,
        });
        let card = card.clone();
        (app, card)
    }

    fn d(app: &mut App, card: &Card) -> Action {
        app.intent_action_on(card.clone(), "dispatch", json!({}))
    }

    fn showing(app: &App) -> &Kickoff {
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!("the box is shut: {:?}", app.mode)
        };
        form
    }

    #[test]
    fn d_opens_questions_read_ahead_at_once_and_only_checks_them_behind_the_box() {
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        // Nothing starts: it's the check, and the card shows no change.
        assert_eq!(request["action"], "kickoff");
        assert_eq!(request["id"], json!(card.detail.as_ref().unwrap().id));
        assert!(app.effect_for(&card.key, &request).is_none());
        let form = showing(&app);
        assert_eq!(form.questions.len(), 2);
        assert_eq!(form.asked, "feedread");
        // Tab isn't offered until the backend says it can be.
        assert!(form.along.is_none());
        // The answers go with the token of what he was shown, as a start.
        let start = sent({
            key(&mut app, KeyCode::Enter);
            key(&mut app, KeyCode::Enter)
        });
        assert_eq!(start["action"], "dispatch");
        assert_eq!(start["asked"], "feedread");
        assert_eq!(start["answers"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn the_check_keeps_his_place_and_words_when_the_questions_are_the_same() {
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        key(&mut app, KeyCode::Enter);
        typed(&mut app, "half a thought");
        app.finish_kickoff_check(3, &request, Ok(answer()));
        let form = showing(&app);
        assert_eq!((form.at, form.reached, form.machine), (1, 1, 3));
        assert_eq!(form.drafts[1], "half a thought");
        assert_eq!(form.asked, "fedcba9876543210");
        assert_eq!(form.along.as_deref(), Some("0123456789abcdef"));
        assert_eq!(form.request["session"], "web");
        assert!(form.note.is_none());
    }

    #[test]
    fn the_check_shows_changed_questions_as_they_are_now_and_says_so() {
        let (mut app, card) = preloaded(vec![
            question("1", "Whose is it?", "Kibbelz"),
            question("2", "An old question?", "Yes"),
        ]);
        let request = sent(d(&mut app, &card));
        typed(&mut app, "Mine");
        key(&mut app, KeyCode::Enter);
        typed(&mut app, "words for the old one");
        let mut now = answer();
        now.questions = vec![
            question("1", "Whose is it?", "Kibbelz"),
            question("2", "The question now?", "No"),
        ];
        app.finish_kickoff_check(0, &request, Ok(now));
        let form = showing(&app);
        assert_eq!(form.questions[1].question, "The question now?");
        assert_eq!((form.at, form.reached), (0, 0));
        assert_eq!(form.drafts[0], "Mine");
        assert_eq!(
            form.drafts[1], "",
            "never an answer to a question it wasn't for"
        );
        let note = form.note.as_deref().unwrap();
        assert!(note.starts_with(CHANGED), "{note}");
        assert!(note.ends_with("words for the old one"), "{note}");
        assert!(!note.contains("Mine"), "{note}");
    }

    #[test]
    fn answers_given_back_fill_in_a_box_opened_from_the_feed_and_survive_its_check() {
        // Intent 0591, B1-108: his answers to questions that changed, given back by an earlier
        // save, fill in the box d opens from the questions the feed read ahead, by number; and
        // when the check behind it finds them reworded again, they go on the new wording.
        let (mut app, card) = preloaded(vec![
            question("1", "Whose is it?", "Kibbelz"),
            question("3", "Where does it run, now?", "The server"),
        ]);
        app.kickoff_prefill.insert(
            card.key.clone(),
            vec![Prefill {
                num: "3".into(),
                text: Some("on  the laptop".into()),
            }],
        );
        let request = sent(d(&mut app, &card));
        assert_eq!(showing(&app).drafts, ["", "on the laptop"]);
        assert!(!app.kickoff_prefill.contains_key(&card.key));
        let mut now = answer();
        now.questions = vec![
            question("1", "Whose is it?", "Kibbelz"),
            question("3", "Where should it run?", "The server"),
        ];
        app.finish_kickoff_check(0, &request, Ok(now));
        let form = showing(&app);
        assert_eq!(form.drafts, ["", "on the laptop"]);
        assert_eq!(form.note.as_deref(), Some(CHANGED), "said twice");
    }

    #[test]
    fn the_check_says_only_that_they_changed_when_no_words_were_lost() {
        let (mut app, card) = preloaded(vec![question("1", "An old question?", "Yes")]);
        let request = sent(d(&mut app, &card));
        let mut now = answer();
        now.questions = vec![question("1", "The question now?", "No")];
        app.finish_kickoff_check(0, &request, Ok(now));
        assert_eq!(showing(&app).note.as_deref(), Some(CHANGED));
    }

    #[test]
    fn a_late_check_cant_touch_a_newer_box() {
        let (mut app, card) = preloaded(answer().questions);
        let first = sent(d(&mut app, &card));
        key(&mut app, KeyCode::Esc);
        let second = sent(d(&mut app, &card));
        assert_ne!(first["check"], second["check"]);
        // The newer check answers first; the older one, from a box he's left, changes nothing.
        app.finish_kickoff_check(0, &second, Ok(answer()));
        let mut old = answer();
        old.questions = vec![question("1", "What the older check read?", "No")];
        old.asked = Some("older".into());
        app.finish_kickoff_check(0, &first, Ok(old));
        let form = showing(&app);
        assert_eq!(form.questions.len(), 2);
        assert_eq!(form.asked, "fedcba9876543210");
    }

    #[test]
    fn a_token_the_check_disproves_cant_open_the_box_again() {
        // None left: d means start now, though the feed still has the questions.
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        let done = Response {
            ok: true,
            message: "#41 has no questions for you now; d starts it.".into(),
            ..Default::default()
        };
        app.finish_kickoff_check(0, &request, Ok(done));
        assert!(!matches!(app.mode, Mode::Kickoff { .. }));
        assert_eq!(sent(d(&mut app, &card))["action"], "dispatch");
        assert!(!matches!(app.mode, Mode::Kickoff { .. }));

        // A new token: answering it leaves the feed's old one no way back in either.
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        app.finish_kickoff_check(0, &request, Ok(answer()));
        assert_eq!(showing(&app).asked, "fedcba9876543210");
        key(&mut app, KeyCode::Esc);
        assert_eq!(sent(d(&mut app, &card))["action"], "dispatch");
    }

    #[test]
    fn words_for_a_changed_question_are_there_again_after_esc() {
        let (mut app, card) = preloaded(vec![question("1", "An old question?", "Yes")]);
        let request = sent(d(&mut app, &card));
        typed(&mut app, "words for the old one");
        let mut now = answer();
        now.questions = vec![question("1", "The question now?", "No")];
        app.finish_kickoff_check(0, &request, Ok(now.clone()));
        key(&mut app, KeyCode::Esc);
        assert!(!matches!(app.mode, Mode::Kickoff { .. }));
        // Reopened from the feed, once it has caught up: the words are in the note, no answer.
        let id = card.detail.as_ref().unwrap().id.clone();
        let kept = app.kickoff_kept[&card.key].clone();
        let form =
            Kickoff::from_answer(0, &json!({"id": id}), &now, "#41".into(), "t".into(), &kept)
                .unwrap();
        assert_eq!(form.drafts, [""]);
        assert!(form.note.unwrap().ends_with("words for the old one"));
        assert_eq!(form.orphaned, ["words for the old one"]);
    }

    #[test]
    fn the_box_goes_when_nothing_is_left_to_ask_or_it_cannot_be_answered() {
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        typed(&mut app, "kept");
        let done = Response {
            ok: true,
            message: "#41 has no questions for you now; d starts it.".into(),
            ..Default::default()
        };
        app.finish_kickoff_check(0, &request, Ok(done));
        assert!(matches!(app.mode, Mode::Board));
        assert!(app.message().unwrap().0.contains("d starts it"));
        assert_eq!(app.kickoff_kept[&card.key][0].2, "kept");
        // A refusal, in red: it started elsewhere, say.
        let request = sent(d(&mut app, &card));
        let refused = Response {
            message: "only an open backlog intent can start".into(),
            ..Default::default()
        };
        app.finish_kickoff_check(0, &request, Ok(refused));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn a_check_with_no_answer_leaves_the_box_up_and_says_so() {
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        app.finish_kickoff_check(0, &request, Err("its machine isn't answering".into()));
        assert_eq!(showing(&app).note.as_deref(), Some(UNCHECKED));
        assert_eq!(showing(&app).asked, "feedread");
        // Nor does a backend that doesn't know the request shut it.
        let old = Response {
            message: "unknown action".into(),
            ..Default::default()
        };
        app.finish_kickoff_check(0, &request, Ok(old));
        assert!(matches!(app.mode, Mode::Kickoff { .. }));
    }

    #[test]
    fn a_check_answered_after_he_left_changes_nothing() {
        let (mut app, card) = preloaded(answer().questions);
        let request = sent(d(&mut app, &card));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Board));
        app.finish_kickoff_check(0, &request, Ok(answer()));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn questions_he_answered_are_not_opened_from_the_feed_again() {
        let (mut app, card) = preloaded(answer().questions);
        sent(d(&mut app, &card));
        key(&mut app, KeyCode::Enter);
        sent(key(&mut app, KeyCode::Esc));
        // The feed still shows them until its next read: the start goes, and the backend says.
        let request = sent(d(&mut app, &card));
        assert_eq!(request["action"], "dispatch");
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn with_nothing_read_ahead_d_asks_the_backend_as_it_did() {
        let (mut app, mut card) = preloaded(answer().questions);
        card.detail.as_mut().unwrap().kickoff = None;
        assert_eq!(sent(d(&mut app, &card))["action"], "dispatch");
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn tab_is_refused_when_a_start_with_you_along_is_not_on_offer() {
        // A full session: answering takes no workspace, but starting it with you along would.
        let mut app = open(&Response {
            confirmation: false,
            confirm: None,
            ..answer()
        });
        assert!(matches!(key(&mut app, KeyCode::Tab), Action::None));
        assert!(matches!(app.mode, Mode::Kickoff { .. }));
    }

    #[test]
    fn a_question_with_no_recommendation_needs_your_own_words() {
        let mut none = question("1", "Whose is it?", "");
        none.why = String::new();
        let mut app = open(&Response {
            questions: vec![none],
            ..answer()
        });
        assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
        assert!(matches!(app.mode, Mode::Kickoff { .. }));
        typed(&mut app, "Mine");
        assert_eq!(
            sent(key(&mut app, KeyCode::Enter))["answers"],
            json!([{"num": "1", "text": "Mine"}])
        );
    }

    #[test]
    fn going_back_does_not_take_an_answer_away() {
        // ↑ then Esc: both answers given still go.
        let three = Response {
            questions: vec![
                question("1", "One?", "a"),
                question("2", "Two?", "b"),
                question("3", "Three?", "c"),
            ],
            ..answer()
        };
        let (mut app, _) = open_on(&three);
        typed(&mut app, "first");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Up);
        let request = sent(key(&mut app, KeyCode::Esc));
        assert_eq!(request["action"], "answer");
        assert_eq!(
            request["answers"],
            json!([{"num": "1", "text": "first"}, {"num": "2", "take": true}])
        );
        // ↑ then Tab, the same.
        let (mut app, _) = open_on(&three);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Up);
        let request = sent(key(&mut app, KeyCode::Tab));
        assert_eq!(request["answers"], json!([{"num": "1", "take": true}]));
    }

    #[test]
    fn a_box_opened_by_a_reopen_answers_with_a_start() {
        // A reopen of work that turned out never to have started.
        let request = json!({"action": "reopen", "id": "0041", "session": "web"});
        let form =
            Kickoff::from_answer(0, &request, &answer(), "#41".into(), "t".into(), &[]).unwrap();
        assert_eq!(form.request["action"], "dispatch");
    }

    #[test]
    fn it_opens_a_box_on_work_whose_record_changed_when_it_was_asked() {
        // Asking can queue it and give it a home, which a snapshot of the card would call changed.
        let (mut app, _) = open_on(&answer());
        let Mode::Kickoff { form, back, .. } = app.mode.clone() else {
            panic!()
        };
        let card = app
            .board
            .cards
            .iter()
            .find(|c| {
                c.detail
                    .as_ref()
                    .is_some_and(|d| Some(d.id.as_str()) == form.request["id"].as_str())
            })
            .unwrap();
        let mut stale = CardSnapshot::of(card);
        stale.home_session = Some("elsewhere".into());
        let mut ready = form;
        ready.ready_at = Instant::now();
        let action = app.kickoff_key(press(KeyCode::Tab), ready, back, Some(Box::new(stale)));
        assert!(matches!(action, Action::Mutate { .. }), "{action:?}");
    }

    #[test]
    fn what_you_typed_is_kept_until_it_is_saved_and_fills_the_box_for_the_same_question() {
        let (mut app, id) = open_on(&answer());
        typed(&mut app, "mine");
        key(&mut app, KeyCode::Enter);
        let request = sent(key(&mut app, KeyCode::Enter));
        assert_eq!(request["answers"][0]["text"], "mine");
        assert!(
            app.kickoff_kept_note(&request, Some(&held(&id)))
                .contains("kept")
        );
        let kept = app.kickoff_kept.get(&held(&id)).cloned().unwrap();
        let reopened =
            Kickoff::from_answer(0, &request, &answer(), "#41".into(), "t".into(), &kept).unwrap();
        assert_eq!(reopened.drafts[0], "mine");
        // A question reworded since isn't the one he answered.
        let reworded = Response {
            questions: vec![
                question("1", "Whose is it now?", "Kibbelz"),
                question("3", "Where does it run?", "The server"),
            ],
            ..answer()
        };
        let reopened =
            Kickoff::from_answer(0, &request, &reworded, "#41".into(), "t".into(), &kept).unwrap();
        assert_eq!(reopened.drafts[0], "");
    }

    #[test]
    fn esc_with_nothing_answered_still_keeps_what_was_typed() {
        let (mut app, id) = open_on(&answer());
        typed(&mut app, "half a thought");
        assert!(matches!(key(&mut app, KeyCode::Esc), Action::None));
        assert_eq!(app.kickoff_kept[&held(&id)][0].2, "half a thought");
    }

    #[test]
    fn text_from_a_record_reaches_the_screen_as_text_only() {
        let dirty = Response {
            questions: vec![question("1", "Whose\u{1b}[2J is it?", "Kib\u{7}belz")],
            note: Some("A\u{1b}]0;title\u{7} note".into()),
            ..answer()
        };
        let form = form("0041", &dirty);
        assert_eq!(form.question().question, "Whose[2J is it?");
        assert_eq!(form.question().pick, "Kibbelz");
        assert!(!form.note.unwrap().contains(char::is_control));
    }

    #[test]
    fn enter_shows_the_rest_before_it_can_take_a_recommendation_off_the_screen() {
        let mut app = open(&answer());
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        form.scroll_most.set(5);
        assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(
            (form.at, form.scroll.get()),
            (0, 5),
            "it scrolled, and took nothing"
        );
        key(&mut app, KeyCode::Enter);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.at, 1, "seen, it takes it");
        // Your own words need nothing scrolled.
        let mut app = open(&answer());
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        form.scroll_most.set(5);
        typed(&mut app, "mine");
        key(&mut app, KeyCode::Enter);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.at, 1);
    }

    #[test]
    fn answers_for_work_gone_from_the_board_are_not_sent() {
        let (mut app, _) = open_on(&answer());
        let Mode::Kickoff { form, .. } = &mut app.mode else {
            panic!()
        };
        form.request["id"] = json!("9999");
        form.ready_at = Instant::now();
        let Mode::Kickoff { form, back, target } = app.mode.clone() else {
            panic!()
        };
        assert!(matches!(
            app.kickoff_key(press(KeyCode::Tab), form, back, target),
            Action::None
        ));
        let said = app.message().unwrap().0;
        assert!(said.contains("gone from the board"), "{said}");
        assert!(said.contains("kept for its next d"), "{said}");
    }

    /// Intent 0591, K1-76: work that started while he answered gets his answers anyway, saved on
    /// it with their token, and nothing starts; never "select it again".
    #[test]
    fn answers_for_work_that_started_meanwhile_are_saved_on_it() {
        let (mut app, id) = open_on(&answer());
        key(&mut app, KeyCode::Enter);
        for card in &mut app.board.cards {
            if card.detail.as_ref().is_some_and(|d| d.id == id) {
                card.stage = Stage::Build;
                card.state = Some("active".into());
            }
        }
        let request = sent(key(&mut app, KeyCode::Esc));
        assert_eq!(
            request,
            json!({"action": "answer", "id": id, "asked": "fedcba9876543210", "answers": [{"num": "1", "take": true}]})
        );
        let said = app.message().unwrap().0;
        assert!(said.contains("saving your answers on it"), "{said}");
        // A start from the box sends the same, never a start.
        let (mut app, id) = open_on(&answer());
        key(&mut app, KeyCode::Enter);
        for card in &mut app.board.cards {
            if card.detail.as_ref().is_some_and(|d| d.id == id) {
                card.detail.as_mut().unwrap().disposition = Some("wont_do".into());
            }
        }
        let request = sent(key(&mut app, KeyCode::Enter));
        assert_eq!(request["action"], "answer");
        assert_eq!(request["answers"].as_array().map(Vec::len), Some(2));
        assert!(app.message().unwrap().0.contains("set aside meanwhile"));
    }

    #[test]
    fn typing_stays_in_the_box_and_paste_goes_in_as_one_line() {
        let mut app = open(&answer());
        typed(&mut app, "qdy");
        let Mode::Kickoff { form, .. } = &mut app.mode else {
            panic!()
        };
        assert_eq!(form.typed(), "qdy");
        App::kickoff_paste(form, " two\nlines ");
        assert_eq!(form.typed(), "qdytwo lines");
        key(&mut app, KeyCode::Backspace);
        let Mode::Kickoff { form, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(form.typed(), "qdytwo line");
    }
}
