//! A bare workspace's conversation resumed in a new workspace of its session, once that workspace
//! has closed (intent 0591, K1-03, K1-08, K3-03): `account-resume`, to a backend that lists it in
//! `starts_on`. Never on its own: its workspace may have closed because its agent wrapped, so Kerdr
//! asks first, and the backend checks it isn't running anywhere else before it resumes.

use super::*;
use crate::app::account_controls::{AccountScreen, MoveTarget};
use serde_json::{Value, json};

/// The action a backend lists in `starts_on` when it resumes a conversation with no intent.
pub const RESUME: &str = "account-resume";

/// A conversation a bare workspace ran, as the board last saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BareResume {
    pub machine: usize,
    pub session: String,
    pub conversation: String,
    /// The closed workspace's name, which the new one takes.
    pub label: String,
    /// herdr's name for the program it ran, for the words.
    pub program: String,
    /// Its card's key, for going on to the next card once it's open.
    pub key: String,
}

impl BareResume {
    /// The request that resumes it, on the account picked, or on its owner's list.
    pub fn request(&self, to: Option<&str>) -> Value {
        let mut request = json!({
            "action": RESUME,
            "session": self.session,
            "conversation": self.conversation,
            "label": self.label,
        });
        if let Some(to) = to.filter(|t| *t != "list") {
            request["to"] = json!(to);
        }
        request
    }
}

impl App {
    /// ⏎ found a bare workspace closed (K3-03, K1-08): asks to resume its conversation, when its
    /// backend can. Whether it asked.
    pub fn offer_resume_closed(&mut self, card: &Card) -> bool {
        let Some(resume) = self.bare_resume_of(card) else {
            return false;
        };
        let said = format!(
            "{} has closed since the board last read it.",
            card.workspace_name()
        );
        self.offer_resume(&resume, None, &said)
    }

    /// The conversation a bare workspace's card ran, when the board knows exactly one: never a
    /// guess between several.
    pub fn bare_resume_of(&self, card: &Card) -> Option<BareResume> {
        if card.detail.is_some() || card.source != crate::model::Source::Workspace {
            return None;
        }
        let session = card.session.clone()?;
        let mut known = card
            .agents
            .iter()
            .filter_map(|a| Some((a.conversation.clone()?, a.agent.clone())))
            .filter(|(c, _)| !c.is_empty());
        let (conversation, program) = known.next()?;
        if known.any(|(c, _)| c != conversation) {
            return None;
        }
        Some(BareResume {
            machine: card.machine_index,
            session,
            conversation,
            label: card.workspace_name(),
            program,
            key: card.key.clone(),
        })
    }

    /// The same, for the agent a move was picked for on a bare workspace.
    pub fn bare_resume_of_target(&self, target: &MoveTarget) -> Option<BareResume> {
        if !target.key.starts_with("ws:")
            || target.agent.conversation.is_empty()
            || target.agent.session.is_empty()
        {
            return None;
        }
        Some(BareResume {
            machine: target.machine,
            session: target.agent.session.clone(),
            conversation: target.agent.conversation.clone(),
            label: target.reference.clone(),
            program: target.agent.program.clone(),
            key: target.key.clone(),
        })
    }

    /// Whether this machine's backend resumes a conversation with no intent.
    pub fn resumes_on(&self, machine: usize) -> bool {
        self.actions_on()
            && self
                .account_feed(machine)
                .is_some_and(|f| f.starts_on(RESUME))
    }

    /// Asks whether to resume it, led by what happened (`said`): on the account picked, or its
    /// owner's list. Its yes sends `account-resume`, and Kerdr goes in once it's open, as ⏎ meant.
    /// Whether it asked: a backend that doesn't resume, or a machine that can't take it, asks
    /// nothing, and the caller says what it always said.
    pub fn offer_resume(&mut self, resume: &BareResume, to: Option<&str>, said: &str) -> bool {
        self.offer_resume_when(resume, to, said, true)
    }

    /// The same, asked now or, when `now` is false (it came while he typed), held with what it
    /// would resume and asked once he's back where it can't take keys meant for something else
    /// (Codex's fourth look).
    pub fn offer_resume_when(
        &mut self,
        resume: &BareResume,
        to: Option<&str>,
        said: &str,
        now: bool,
    ) -> bool {
        if !self.resumes_on(resume.machine) || !self.machines[resume.machine].conn.is_live() {
            return false;
        }
        let request = resume.request(to);
        let program = match self
            .providers(resume.machine)
            .program_label(&resume.program)
        {
            p if p.is_empty() => "agent's".to_string(),
            p => p,
        };
        let on = match request["to"].as_str() {
            Some(code) => format!(" on {code}"),
            None => String::new(),
        };
        let message = format!(
            "{} Resume its {program} conversation in a new workspace in {}{on}? Kerdr checks first that it isn't running anywhere else, and goes in once it's open.",
            said.trim_end(),
            resume.session
        );
        // What its yes goes into once it's open: the card it was, gone from the board.
        let gone = CardSnapshot {
            key: resume.key.clone(),
            machine: resume.machine,
            home_machine: None,
            home_session: Some(resume.session.clone()),
            resume_agent: None,
            resume_conversation: Some(resume.conversation.clone()),
            workspace: None,
            waiting: false,
            done: false,
            dismissed: false,
        };
        self.keep_asked(resume.machine, &request, None, Some(gone), None);
        let question = AccountScreen::Confirm {
            machine: resume.machine,
            request,
            message,
            yes: "resume it".into(),
            no: "leave it".into(),
            target: None,
            ready_at: Instant::now() + Duration::from_millis(500),
            choices: vec![],
        };
        if now {
            self.push_account_screen(question);
        } else {
            self.hold_question(question);
            self.say(
                format!(
                    "{} Kerdr offers to resume it once you're back on the board.",
                    said.trim_end()
                ),
                Tone::Info,
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::Source;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let mut a = App::new(Config::machine_split(), crate::demo::machines(false));
        a.cfg.actions.command = Some("fixture".into());
        a
    }

    /// A bare workspace whose agent's conversation the board knows, on a machine whose backend
    /// resumes one when `resumes`.
    fn bare(a: &mut App, resumes: bool) -> Card {
        let i = a
            .board
            .cards
            .iter()
            .position(|c| {
                c.source == Source::Workspace
                    && c.is_open_workspace()
                    && !c.agents.is_empty()
                    && a.account_feed(c.machine_index).is_some()
            })
            .expect("the demo has a bare workspace with an agent on a machine with accounts");
        a.board.cards[i].agents.truncate(1);
        a.board.cards[i].agents[0].conversation = Some("c-0123".into());
        let mi = a.board.cards[i].machine_index;
        let feed = a.machines[mi].feed.as_mut().unwrap();
        feed.starts_on.retain(|s| s != RESUME);
        if resumes {
            feed.starts_on.push(RESUME.into());
        }
        a.board.cards[i].clone()
    }

    fn key(a: &mut App, k: KeyCode) -> Action {
        a.on_key(KeyEvent::new(k, KeyModifiers::NONE))
    }

    fn settle(a: &mut App) {
        if let Mode::Accounts {
            screen: AccountScreen::Confirm { ready_at, .. },
            ..
        } = &mut a.mode
        {
            *ready_at = Instant::now() - Duration::from_secs(1);
        }
    }

    fn asked(a: &App) -> (Value, String) {
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                request, message, ..
            },
            ..
        } = &a.mode
        else {
            panic!("nothing asked: {:?} {:?}", a.mode, a.message())
        };
        (request.clone(), message.clone())
    }

    fn answer(a: &mut App, action: Action, reply: Value) {
        let Action::Mutate { machine, request } = action else {
            panic!("nothing sent: {action:?} {:?}", a.message())
        };
        a.finish_action(
            machine,
            crate::actions::with_op(request),
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }

    fn opened(card: &Card) -> Value {
        json!({"ok":true,"session":card.session,"workspace":"w77","label":card.workspace_name(),"message":"Resumed that Claude Code conversation in a new workspace."})
    }

    /// Takes the card off the board, as a read once its workspace closed does.
    fn closes(a: &mut App, card: &Card) {
        a.board.cards.retain(|c| c.key != card.key);
    }

    #[test]
    fn a_closed_bare_workspace_asks_to_resume_its_conversation_and_goes_in_after_the_yes() {
        // K3-03: ⏎ found it closed. Asked, never resumed on its own; the yes sends
        // account-resume for its conversation, and Kerdr goes in once it's open.
        let mut a = app();
        let card = bare(&mut a, true);
        closes(&mut a, &card);
        assert!(a.offer_resume_closed(&card));
        let (request, message) = asked(&a);
        assert_eq!(
            request,
            json!({"action":"account-resume","session":card.session,"conversation":"c-0123","label":card.workspace_name()})
        );
        assert!(message.contains("Resume its"), "{message}");
        assert_eq!(
            key(&mut a, KeyCode::Char('y')),
            Action::None,
            "answered too soon"
        );
        settle(&mut a);
        let yes = key(&mut a, KeyCode::Char('y'));
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        answer(&mut a, yes, opened(&card));
        let open = a.open_after.take().expect("didn't go in");
        assert_eq!(open.workspace, "w77");
        assert_eq!(Some(open.session), card.session);
        // n leaves it: nothing sent.
        let mut a = app();
        let card = bare(&mut a, true);
        assert!(a.offer_resume_closed(&card));
        settle(&mut a);
        assert_eq!(key(&mut a, KeyCode::Char('n')), Action::None);
        assert!(matches!(a.mode, Mode::Board));
    }

    #[test]
    fn a_backend_that_doesnt_resume_or_a_card_with_no_single_conversation_asks_nothing() {
        let mut a = app();
        let card = bare(&mut a, false);
        assert!(!a.offer_resume_closed(&card));
        assert!(matches!(a.mode, Mode::Board));
        let mut a = app();
        let mut card = bare(&mut a, true);
        card.agents[0].conversation = None;
        assert!(!a.offer_resume_closed(&card));
        let mut two = card.agents[0].clone();
        card.agents[0].conversation = Some("c-1".into());
        two.conversation = Some("c-2".into());
        card.agents.push(two);
        assert!(!a.offer_resume_closed(&card), "a guess between two");
        assert!(matches!(a.mode, Mode::Board));
    }

    #[test]
    fn a_waited_jump_whose_bare_workspace_closed_meanwhile_asks_to_resume_it() {
        // K1-03: ⏎ waited for its machine, and its workspace had closed by then.
        let mut a = app();
        let card = bare(&mut a, true);
        a.wait_to_jump(&card);
        closes(&mut a, &card);
        a.mode = Mode::Board;
        assert_eq!(a.pending_jump_ready(), None);
        let (request, message) = asked(&a);
        assert_eq!(request["action"], "account-resume");
        assert_eq!(request["conversation"], "c-0123");
        assert!(
            message.starts_with(&format!("{} closed while", card.workspace_name())),
            "{message}"
        );
        // An older backend: said as before.
        let mut a = app();
        let card = bare(&mut a, false);
        a.wait_to_jump(&card);
        closes(&mut a, &card);
        a.mode = Mode::Board;
        assert_eq!(a.pending_jump_ready(), None);
        assert!(matches!(a.mode, Mode::Board));
        assert!(
            a.message()
                .unwrap()
                .0
                .contains("nothing of it is left to open")
        );
    }

    #[test]
    fn s_on_a_bare_workspace_whose_agent_is_gone_offers_to_resume_it_on_the_account_picked() {
        // s picked an account for its agent, and the move found it gone: resumed on that
        // account, after a yes.
        let mut a = app();
        let card = bare(&mut a, true);
        a.sel = Selection {
            col: a.col_of(&card),
            key: Some(card.key.clone()),
        };
        a.s_on_card(card.clone());
        let Mode::Accounts {
            screen: AccountScreen::Move { choices, .. },
            ..
        } = &a.mode
        else {
            panic!("no move picker: {:?} {:?}", a.mode, a.message())
        };
        let to = choices[0].clone();
        let action = key(&mut a, KeyCode::Enter);
        answer(
            &mut a,
            action,
            json!({"ok":false,"not_running":true,"why":"gone","message":"The conversation you picked isn't running anywhere on this machine now, so nothing moved."}),
        );
        let (request, message) = asked(&a);
        assert_eq!(request["action"], "account-resume");
        assert_eq!(request["conversation"], "c-0123");
        assert_eq!(
            request["to"].as_str(),
            (to != "list").then_some(to.as_str())
        );
        assert!(
            message.starts_with("The conversation you picked"),
            "{message}"
        );
        settle(&mut a);
        let Action::Mutate { request: sent, .. } = key(&mut a, KeyCode::Char('y')) else {
            panic!("the yes sent nothing")
        };
        assert_eq!(sent, request);
        // Gone before ⏎ in the picker: the same offer.
        let mut a = app();
        let card = bare(&mut a, true);
        a.sel = Selection {
            col: a.col_of(&card),
            key: Some(card.key.clone()),
        };
        a.s_on_card(card.clone());
        closes(&mut a, &card);
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert_eq!(asked(&a).0["action"], "account-resume");
    }

    #[test]
    fn a_resume_offer_that_comes_while_he_types_is_made_once_he_is_back_even_with_its_card_gone() {
        // Codex's fourth look: the move's answer finds its agent gone while he's typing a new
        // intent. The offer isn't put over his typing, nor dropped: it waits with the
        // conversation it would resume, and is made once he's back, though the card has gone.
        let mut a = app();
        let card = bare(&mut a, true);
        a.sel = Selection {
            col: a.col_of(&card),
            key: Some(card.key.clone()),
        };
        a.s_on_card(card.clone());
        let action = key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Esc);
        key(&mut a, KeyCode::Char('c'));
        assert!(matches!(a.mode, Mode::Compose { .. }), "{:?}", a.mode);
        answer(
            &mut a,
            action,
            json!({"ok":false,"not_running":true,"why":"gone","message":"The conversation you picked isn't running anywhere on this machine now, so nothing moved."}),
        );
        assert!(matches!(a.mode, Mode::Compose { .. }), "{:?}", a.mode);
        assert!(
            a.message()
                .unwrap()
                .0
                .ends_with("Kerdr offers to resume it once you're back on the board."),
            "{:?}",
            a.message()
        );
        closes(&mut a, &card);
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_owed(), Action::None);
        let (request, _) = asked(&a);
        assert_eq!(request["action"], "account-resume");
        assert_eq!(request["conversation"], "c-0123");
        settle(&mut a);
        let Action::Mutate { request: sent, .. } = key(&mut a, KeyCode::Char('y')) else {
            panic!("the yes sent nothing")
        };
        assert_eq!(sent, request);
        // Resumed: gone into as ⏎ meant, from what was kept with the offer.
        answer(
            &mut a,
            Action::Mutate {
                machine: card.machine_index,
                request: sent,
            },
            opened(&card),
        );
        assert_eq!(
            a.open_after.take().map(|o| o.workspace).as_deref(),
            Some("w77")
        );
    }

    #[test]
    fn a_resume_that_asks_or_needs_a_sign_in_still_goes_in_once_it_resumes() {
        let mut a = app();
        let card = bare(&mut a, true);
        closes(&mut a, &card);
        assert!(a.offer_resume_closed(&card));
        settle(&mut a);
        let yes = key(&mut a, KeyCode::Char('y'));
        // Its backend's own question: asked in the same box, its yes the same request with its
        // token.
        answer(
            &mut a,
            yes,
            json!({"ok":false,"confirmation":true,"confirm":"r-1","message":"That Claude conversation may still be open somewhere else. Resume it anyway?"}),
        );
        let (request, _) = asked(&a);
        assert_eq!(request["confirm"], "r-1");
        settle(&mut a);
        let yes = key(&mut a, KeyCode::Char('y'));
        // Its account isn't signed in there: the box that signs it in, then the resume again.
        answer(
            &mut a,
            yes,
            json!({"ok":false,"sign_in":true,"code":"ARK_C","message":"ARK_C needs signing in first; nothing started"}),
        );
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Where { .. },
                ..
            }
        ));
        let start = key(&mut a, KeyCode::Enter);
        answer(
            &mut a,
            start,
            json!({"ok":true,"message":"Authorize in your browser.","status":"waiting","url":"https://example.com/authorize?state=example","ticket":"login-ticket","needs_code":true}),
        );
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now();
        }
        let poll = a.account_tick();
        answer(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"ARK_C is signed in."}),
        );
        assert!(
            a.message().unwrap().0.contains("so the resume goes on"),
            "{:?}",
            a.message()
        );
        let again = a.account_tick();
        let Action::Mutate { request: sent, .. } = &again else {
            panic!("not sent again: {:?}", a.message())
        };
        assert_eq!(sent["action"], "account-resume");
        assert_eq!(sent["confirm"], "r-1");
        answer(&mut a, again, opened(&card));
        assert!(a.open_after.take().is_some(), "didn't go in");
    }
}
