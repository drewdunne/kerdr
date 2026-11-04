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
use crate::actions::{Question, Response};
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
        let drafts = drafts_for(&questions, kept);
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
            note: response.note.as_deref().map(clean),
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
        self.note = response.note.as_deref().map(clean);
        let now = cleaned(&response.questions);
        if now != self.questions {
            let kept = self.kept();
            self.drafts = drafts_for(&now, &kept);
            self.questions = now;
            self.at = 0;
            self.reached = 0;
            self.note = Some(CHANGED.into());
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
        let response = Response {
            asked: Some(preloaded.asked.clone()),
            questions: preloaded.questions.clone(),
            ..Default::default()
        };
        let kept = self
            .kickoff_kept
            .get(&detail.id)
            .cloned()
            .unwrap_or_default();
        let form = Kickoff::from_answer(
            machine,
            request,
            &response,
            card.reference.clone(),
            card.title.clone(),
            &kept,
        )?;
        let id = detail.id.clone();
        self.mode = Mode::Kickoff {
            form,
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
        };
        Some(Action::Mutate {
            machine,
            request: json!({"action": "kickoff", "id": id}),
        })
    }

    /// The check's answer, while its box is still up; once it's gone, nothing it says matters: his
    /// answers went with their token, and the backend checks that itself.
    pub(super) fn finish_kickoff_check(
        &mut self,
        machine: usize,
        request: &Value,
        result: Result<Response, String>,
    ) {
        let Mode::Kickoff { form, back, .. } = &mut self.mode else {
            return;
        };
        if form.request["id"] != request["id"] {
            return;
        }
        match result {
            Ok(response) if !response.questions.is_empty() && response.asked.is_some() => {
                form.checked(machine, &response);
            }
            Err(_) => form.note = Some(UNCHECKED.into()),
            // A backend older than this request: as good as no answer.
            Ok(response) if !response.ok && response.message == "unknown action" => {
                form.note = Some(UNCHECKED.into());
            }
            // None left to ask, or it can't be answered now: the box goes, keeping what's typed.
            Ok(response) => {
                let (form, back) = (form.clone(), back.clone());
                self.keep_kickoff(&form);
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
                    self.keep_kickoff(&form);
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

    /// Keeps what's typed in the box until it's saved, for when `d` opens it again.
    fn keep_kickoff(&mut self, form: &Kickoff) {
        let Some(id) = form.request["id"].as_str() else {
            return;
        };
        let kept = form.kept();
        if kept.is_empty() {
            self.kickoff_kept.remove(id);
        } else {
            self.kickoff_kept.insert(id.to_string(), kept);
        }
    }

    /// What a failure adds when it has words of yours in hand: where they went.
    pub(super) fn kickoff_kept_note(&self, request: &Value) -> &'static str {
        let holds = matches!(request["action"].as_str(), Some("dispatch" | "answer"))
            && request.get("answers").is_some()
            && request["id"]
                .as_str()
                .is_some_and(|id| self.kickoff_kept.contains_key(id));
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
        self.keep_kickoff(&form);
        if !self.kickoff_still_applies(&form) {
            self.say(
                "That intent changed while you were answering, so nothing was sent. Select it again.",
                Tone::Warn,
            );
            return Action::None;
        }
        let answered = if matches!(how, Leaving::Start) {
            form.questions.len()
        } else {
            form.reached
        };
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
        assert_eq!(form.drafts[1], "");
        assert_eq!(form.note.as_deref(), Some(CHANGED));
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
        assert_eq!(
            app.kickoff_kept[&request["id"].as_str().unwrap().to_string()][0].2,
            "kept"
        );
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
        assert!(app.kickoff_kept_note(&request).contains("kept"));
        let kept = app.kickoff_kept.get(&id).cloned().unwrap();
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
        assert_eq!(app.kickoff_kept[&id][0].2, "half a thought");
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
    fn answers_for_work_that_changed_meanwhile_are_not_sent() {
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
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("changed while you were answering")
        );
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
