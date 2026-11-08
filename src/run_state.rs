//! Where a card's agent stands, which decides what `s` does on it (intent 0580).
//!
//! `s` used to work only on a card an agent was running: it moved that agent to another account,
//! restarting it there with its conversation. A card with nothing running said there was nothing
//! to move. Now every card is in one of three states, and `s` does what its state allows:
//!
//! - [`RunState::Running`]: an agent the machine's accounts run is at it. `s` moves it, as before.
//! - [`RunState::Stopped`]: nothing runs it, and Kerdr can start it, the way `d` (a fresh start)
//!   or ⏎ (a reopen, resuming its conversation) would. `s` asks which account, then starts it on
//!   that one through the same request, with `account` added.
//! - [`RunState::Unavailable`]: neither, with the reason in words.
use super::*;
use crate::accounts::AccountAgent;
use crate::app::account_controls::MoveTarget;
use serde_json::json;

/// How a stopped card starts: the request ⏎ or `d` would send for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartHow {
    /// Work nobody started: `dispatch`, as `d` sends.
    Dispatch,
    /// Work that ran and has no agent now (its workspace closed, or open with its agent gone), or
    /// done work: `reopen`, as ⏎ sends, which resumes its conversation.
    Reopen,
}

impl StartHow {
    pub fn action(self) -> &'static str {
        match self {
            StartHow::Dispatch => "dispatch",
            StartHow::Reopen => "reopen",
        }
    }
}

/// A stopped card Kerdr can start, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub how: StartHow,
    /// The machine whose backend starts it, and whose accounts the picker offers.
    pub machine: usize,
    /// The program a reopen resumes its conversation with, when the feed says: only that
    /// program's accounts can take the conversation, so only they're offered.
    pub program: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Running(Box<AccountAgent>),
    Stopped(Start),
    Unavailable(String),
}

/// The feed's word for what takes `account` on a start (`starts_on`), from backends that can.
pub const STARTS_ON: [&str; 2] = ["dispatch", "reopen"];

impl App {
    /// The state `s` acts on for this card.
    pub fn run_state(&self, card: &Card) -> RunState {
        if let Some(agent) = self.move_agent(card) {
            return RunState::Running(Box::new(agent));
        }
        let reference = &card.reference;
        if !card.agents.is_empty() {
            // Something is at it, just not an agent s can move: never a second copy beside it.
            if self.account_feed(card.machine_index).is_none() {
                return RunState::Unavailable(
                    "This card's machine hasn't sent its accounts yet.".into(),
                );
            }
            let names: Vec<String> = self
                .offered(card.machine_index)
                .programs()
                .iter()
                .map(|id| self.providers(card.machine_index).program_label(id))
                .collect();
            return RunState::Unavailable(if names.is_empty() {
                "s moves agents its machine's accounts run; this card has none.".into()
            } else {
                format!(
                    "s moves agents its machine's accounts run ({}); this card has none.",
                    names.join(", ")
                )
            });
        }
        if card.detail.is_none() {
            return RunState::Unavailable(format!(
                "{} has no agent running and no intent to start: s starts or moves an intent's agent.",
                card.workspace_name()
            ));
        }
        if card.dismissed() {
            return RunState::Unavailable(format!(
                "{reference} is in won't do; u brings it back, then s starts it on an account."
            ));
        }
        if card.outside_herdr().is_some() {
            return RunState::Unavailable(format!(
                "{reference}'s conversation runs in a terminal outside herdr: s would start a second copy. ⏎ says where it is."
            ));
        }
        let how = if waiting_work(card) {
            StartHow::Dispatch
        } else if card.reopenable()
            || (card.is_open_workspace() && (card.under_way() || card.done()))
        {
            // Open with its agent gone: the reopen resumes the conversation in a tab there.
            StartHow::Reopen
        } else {
            return RunState::Unavailable(match card.stage {
                Stage::Backlog => format!("{reference} has nothing to start yet."),
                _ => format!("{reference} has no agent running and nothing s can start."),
            });
        };
        if !self.actions_on() {
            return RunState::Unavailable(format!(
                "{reference} has no agent running; with [actions] configured, s starts it on an account."
            ));
        }
        // Where the start goes, as ⏎ and d send it (home_machine): where a refusal said it lives
        // now, else its home machine, else the card's own.
        let home = card
            .detail
            .as_ref()
            .and_then(|d| d.home.as_ref())
            .and_then(|h| h.machine.as_deref());
        let machine = match (self.moved_target(card), home) {
            (Some(to), _) => to,
            (None, Some(name)) => match self.machine_index(name) {
                Some(mi) => mi,
                None => {
                    return RunState::Unavailable(format!(
                        "{reference}'s home machine {name} is not configured. Add it before starting it."
                    ));
                }
            },
            (None, None) => card.machine_index,
        };
        let Some(feed) = self.account_feed(machine) else {
            return RunState::Unavailable(format!(
                "{} hasn't sent its accounts yet, so s can't offer one: ⏎ starts {reference} on its owner's list.",
                self.machines[machine].name()
            ));
        };
        if !feed.starts_on(how.action()) {
            return RunState::Unavailable(format!(
                "{}'s backend can't start work on a chosen account yet: update it. ⏎ starts {reference} on its owner's list.",
                self.machines[machine].name()
            ));
        }
        // The program its conversation ran, for a reopen, and for a dispatch too: work queued to be
        // reopened starts by d, and its backend resumes the conversation then.
        let program = card.resume_agent().map(str::to_string);
        RunState::Stopped(Start {
            how,
            machine,
            program,
        })
    }

    /// The accounts a stopped card's picker offers: those of the machine it starts on, of the
    /// program a reopen resumes with when that's known, then "list" (its owner's list, as ⏎ or d).
    pub fn start_choices(&self, start: &Start) -> Vec<String> {
        let mut choices: Vec<String> =
            self.account_feed(start.machine)
                .into_iter()
                .flat_map(|f| &f.accounts)
                .filter(|a| {
                    start.program.as_deref().is_none_or(|p| {
                        self.account_runs(start.machine, a).contains(&p.to_string())
                    })
                })
                .map(|a| a.code.clone())
                .collect();
        choices.push("list".into());
        choices
    }

    /// The programs an account runs: its own `runs`, else its provider's `programs`, as the
    /// feed describes them.
    fn account_runs(&self, machine: usize, a: &crate::accounts::Account) -> Vec<String> {
        if !a.runs.is_empty() {
            return vec![a.runs.clone()];
        }
        self.account_feed(machine)
            .into_iter()
            .flat_map(|f| &f.providers)
            .filter(|p| p.id == a.provider)
            .flat_map(|p| p.programs.clone())
            .collect()
    }

    /// `s` on a card: the picker its state calls for, or why there's none.
    pub(super) fn s_on_card(&mut self, card: Card) {
        match self.run_state(&card) {
            RunState::Running(agent) => {
                let machine = card.machine_index;
                let mut choices: Vec<String> = self
                    .account_feed(machine)
                    .into_iter()
                    .flat_map(|f| &f.accounts)
                    .map(|a| a.code.clone())
                    .collect();
                choices.sort_by_key(|a| a == &agent.account);
                choices.push("list".into());
                self.push_account_screen(AccountScreen::Move {
                    target: Box::new(MoveTarget {
                        machine,
                        key: card.key,
                        reference: card.reference,
                        agent: *agent,
                    }),
                    choices,
                    sel: 0,
                });
            }
            RunState::Stopped(start) => {
                let choices = self.start_choices(&start);
                self.push_account_screen(AccountScreen::Start {
                    key: card.key,
                    reference: card.reference,
                    start,
                    choices,
                    sel: 0,
                });
            }
            RunState::Unavailable(why) => self.say(why, Tone::Info),
        }
    }

    /// Enter on the start picker: the card's start, as ⏎ or d would send it, on the account
    /// chosen ("list": its owner's list, over any account kept for it before). Only while the card is still stopped the same way; its own
    /// checks (a start already on its way, kickoff questions, a full session) still apply.
    pub(super) fn start_on_account(&mut self, key: &str, start: &Start, to: &str) -> Action {
        let Some(card) = self.card(key).cloned() else {
            self.say("That card is gone from the board.", Tone::Warn);
            return Action::None;
        };
        if self.run_state(&card) != RunState::Stopped(start.clone()) {
            self.say(
                format!("{} changed. Press s on it again.", card.reference),
                Tone::Warn,
            );
            return Action::None;
        }
        // "list" goes too: his owner's list, chosen over an account a queued start kept from before.
        let extra = json!({"account": to});
        match start.how {
            StartHow::Dispatch => self.intent_action_on(card, "dispatch", extra),
            StartHow::Reopen => self.reopen_action_with(&card, extra),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::account_controls::AccountScreen;

    use serde_json::Value;

    fn app() -> App {
        let mut a = App::new(Config::default(), crate::demo::machines(false));
        a.cfg.actions.command = Some("fixture".into());
        a
    }
    fn key(a: &mut App, k: KeyCode) -> Action {
        a.on_key(KeyEvent::new(k, KeyModifiers::NONE))
    }
    fn press(a: &mut App, c: char) -> Action {
        key(a, KeyCode::Char(c))
    }
    fn select(a: &mut App, key: &str) {
        let c = a.card(key).unwrap();
        a.sel = Selection {
            col: c.stage.index(),
            key: Some(key.to_string()),
        };
    }
    /// A card nobody started, on a machine whose feed carries accounts.
    fn backlog_card(a: &App) -> String {
        a.board
            .cards
            .iter()
            .find(|c| waiting_work(c) && a.account_feed(c.machine_index).is_some())
            .expect("the demo has backlog work")
            .key
            .clone()
    }
    /// A card under way whose workspace closed, its agent's conversation Claude Code's.
    fn closed_card(a: &mut App) -> String {
        let i = a
            .board
            .cards
            .iter()
            .position(|c| {
                c.detail.is_some()
                    && c.stage == Stage::Build
                    && a.account_feed(c.machine_index).is_some()
            })
            .unwrap();
        let card = &mut a.board.cards[i];
        card.workspace_id = None;
        card.session = None;
        card.agents.clear();
        let d = card.detail.as_mut().unwrap();
        d.home = None;
        d.resume = serde_json::from_value(json!({"agent": "claude"})).ok();
        card.key.clone()
    }
    /// No questions read ahead, so the start goes out as it is rather than behind a box.
    fn no_kickoff(a: &mut App, key: &str) {
        for c in a.board.cards.iter_mut().filter(|c| c.key == key) {
            if let Some(d) = c.detail.as_mut() {
                d.kickoff = None;
            }
        }
    }
    fn start_screen(a: &App) -> (&Start, &[String]) {
        let Mode::Accounts {
            screen: AccountScreen::Start { start, choices, .. },
            ..
        } = &a.mode
        else {
            panic!("expected the start picker, got {:?}", a.mode)
        };
        (start, choices)
    }
    fn choose(a: &mut App, code: &str) -> (usize, Value) {
        let at = start_screen(a).1.iter().position(|c| c == code).unwrap();
        for _ in 0..at {
            key(a, KeyCode::Down);
        }
        let Action::Mutate { machine, request } = key(a, KeyCode::Enter) else {
            panic!("expected a request, got {:?}", a.message())
        };
        (machine, request)
    }

    #[test]
    fn s_on_work_nobody_started_starts_it_on_the_account_chosen() {
        let mut a = app();
        let k = backlog_card(&a);
        select(&mut a, &k);
        no_kickoff(&mut a, &k);
        press(&mut a, 's');
        let (start, choices) = start_screen(&a);
        assert_eq!(start.how, StartHow::Dispatch);
        assert_eq!(choices.last().map(String::as_str), Some("list"));
        assert!(
            choices.contains(&"KBZ_X".to_string()),
            "a fresh start takes any program"
        );
        let (_, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
        assert!(!matches!(a.mode, Mode::Accounts { .. }));
    }

    #[test]
    fn following_the_list_sends_the_start_d_would() {
        let mut a = app();
        let k = backlog_card(&a);
        select(&mut a, &k);
        no_kickoff(&mut a, &k);
        press(&mut a, 's');
        let (_, request) = choose(&mut a, "list");
        assert_eq!(request["action"], "dispatch");
        // Named, so it wins over an account a queued start kept from an earlier s.
        assert_eq!(request["account"], "list", "{request}");
    }

    #[test]
    fn s_on_work_whose_workspace_closed_resumes_it_on_its_programs_accounts() {
        let mut a = app();
        let k = closed_card(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        let (start, choices) = start_screen(&a);
        assert_eq!(start.how, StartHow::Reopen);
        assert_eq!(start.program.as_deref(), Some("claude"));
        // Codex can't take a Claude Code conversation, so it isn't offered.
        assert!(!choices.contains(&"KBZ_X".to_string()), "{choices:?}");
        let (_, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["account"], "ARK_C");
    }

    #[test]
    fn s_on_an_open_workspace_whose_agent_ended_resumes_it_there() {
        let mut a = app();
        let i = a
            .board
            .cards
            .iter()
            .position(|c| c.reference == "#44")
            .unwrap();
        a.board.cards[i].agents.clear();
        let k = a.board.cards[i].key.clone();
        assert!(a.board.cards[i].is_open_workspace());
        select(&mut a, &k);
        press(&mut a, 's');
        assert_eq!(start_screen(&a).0.how, StartHow::Reopen);
        let (_, request) = choose(&mut a, "KBZ_C");
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["account"], "KBZ_C");
    }

    #[test]
    fn s_on_a_running_agent_still_moves_it() {
        let mut a = app();
        let k = a
            .board
            .cards
            .iter()
            .find(|c| c.reference == "#44")
            .unwrap()
            .key
            .clone();
        select(&mut a, &k);
        press(&mut a, 's');
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Move { .. },
                ..
            }
        ));
    }

    #[test]
    fn an_older_backend_is_never_sent_an_account_to_start_on() {
        let mut a = app();
        let k = backlog_card(&a);
        let mi = a.card(&k).unwrap().machine_index;
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        assert_eq!(press(&mut a, 's'), Action::None);
        assert!(!matches!(a.mode, Mode::Accounts { .. }));
        let said = a.message().unwrap().0;
        assert!(said.contains("update it"), "{said}");
    }

    #[test]
    fn a_card_that_changed_since_s_starts_nothing() {
        let mut a = app();
        let k = closed_card(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        // Its workspace opened meanwhile, with an agent in it.
        let other = a
            .board
            .cards
            .iter()
            .find(|c| c.reference == "#44")
            .unwrap()
            .clone();
        let card = a.board.cards.iter_mut().find(|c| c.key == k).unwrap();
        card.workspace_id = other.workspace_id;
        card.session = other.session;
        card.agents = other.agents;
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.message().unwrap().0.contains("changed"));
    }

    #[test]
    fn a_question_on_the_start_keeps_the_account_for_the_yes() {
        let mut a = app();
        let k = backlog_card(&a);
        select(&mut a, &k);
        no_kickoff(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        a.finish_action(
            machine,
            request,
            Ok(crate::actions::parse_response(
                &json!({"ok":false,"confirmation":true,"confirm":"0123456789abcdef","message":"web has 3 workspaces. Start anyway?","session":"web","used":3}).to_string(),
            )
            .unwrap()),
        );
        let Mode::Confirm { request, .. } = &a.mode else {
            panic!("expected the question, got {:?}", a.mode)
        };
        assert_eq!(request["account"], "ARK_C");
    }

    #[test]
    fn s_in_the_backlog_screen_acts_on_the_backlog_row() {
        let mut a = app();
        let k = backlog_card(&a);
        no_kickoff(&mut a, &k);
        let row = a
            .backlog_cards()
            .iter()
            .position(|&i| a.board.cards[i].key == k)
            .unwrap();
        a.sel.key = None;
        a.mode = Mode::Backlog { sel: row };
        press(&mut a, 's');
        assert_eq!(start_screen(&a).0.how, StartHow::Dispatch);
    }

    #[test]
    fn upper_s_with_no_session_chosen_edits_the_selected_cards_owner() {
        let mut a = app();
        assert!(!matches!(a.scope(), Scope::Session(..)));
        let k = a
            .board
            .cards
            .iter()
            .find(|c| c.reference == "#44")
            .unwrap()
            .key
            .clone();
        select(&mut a, &k);
        press(&mut a, 'S');
        let Mode::Accounts {
            screen: AccountScreen::Models { owner, .. },
            ..
        } = &a.mode
        else {
            panic!("expected the model list, got {:?}", a.message())
        };
        // #44 runs in the build box's api session, which Platform owns.
        assert_eq!(owner.id, "platform");
    }

    #[test]
    fn upper_s_on_a_card_with_no_session_uses_its_owner_by_name() {
        let mut a = app();
        let k = closed_card(&mut a);
        let card = a.board.cards.iter_mut().find(|c| c.key == k).unwrap();
        let d = card.detail.as_mut().unwrap();
        d.tenant = Some("shop".into());
        select(&mut a, &k);
        press(&mut a, 'S');
        let Mode::Accounts {
            screen: AccountScreen::Models { owner, .. },
            ..
        } = &a.mode
        else {
            panic!("expected the model list, got {:?}", a.message())
        };
        assert_eq!(owner.id, "shop");
    }

    fn finish(a: &mut App, machine: usize, request: Value, reply: Value) {
        a.finish_action(
            machine,
            request,
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }

    #[test]
    fn work_whose_write_up_failed_starts_as_d_would() {
        let mut a = app();
        let k = backlog_card(&a);
        no_kickoff(&mut a, &k);
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.detail.as_mut().unwrap().intake = Some("failed".into());
        }
        select(&mut a, &k);
        press(&mut a, 's');
        assert_eq!(start_screen(&a).0.how, StartHow::Dispatch);
    }

    #[test]
    fn an_account_whose_program_only_its_provider_names_is_matched_by_it() {
        let mut a = app();
        let k = closed_card(&mut a);
        let mi = a.card(&k).unwrap().machine_index;
        for acc in &mut a.machines[mi].feed.as_mut().unwrap().accounts {
            acc.runs.clear();
        }
        select(&mut a, &k);
        press(&mut a, 's');
        let choices = start_screen(&a).1;
        assert!(choices.contains(&"ARK_C".to_string()), "{choices:?}");
        assert!(!choices.contains(&"KBZ_X".to_string()), "{choices:?}");
    }

    #[test]
    fn a_move_to_a_machine_whose_backend_cant_take_an_account_starts_nothing() {
        let mut a = app();
        let k = backlog_card(&a);
        no_kickoff(&mut a, &k);
        let here = a.card(&k).unwrap().machine_index;
        let other = (0..2).find(|&m| m != here).unwrap();
        a.machines[other].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        let home = format!("{}/web", a.machines[other].name());
        finish(
            &mut a,
            machine,
            request,
            json!({"ok":false,"message":"lives elsewhere","home":home}),
        );
        assert!(a.resent.is_empty(), "the start went on to an older backend");
        let said = a.message().unwrap().0;
        assert!(
            said.contains("can't start work on a chosen account"),
            "{said}"
        );
        // And s on it now asks that machine, whose backend can't, so s offers nothing.
        let card = a.card(&k).unwrap().clone();
        assert!(
            matches!(a.run_state(&card), RunState::Unavailable(ref why) if why.contains("update it")),
            "{:?}",
            a.run_state(&card)
        );
        assert_eq!(press(&mut a, 's'), Action::None);
        assert!(!matches!(a.mode, Mode::Accounts { .. }));
    }

    #[test]
    fn a_reopen_that_restarted_nothing_says_so_rather_than_go_in() {
        let mut a = app();
        let k = closed_card(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        finish(
            &mut a,
            machine,
            request,
            json!({"ok":true,"message":"#1 is open already, in web. Its agent was running already, so it stays on its own account, not ARK_C: s on its card moves it.","session":"web","workspace":"w9","account_unchanged":true}),
        );
        assert_eq!(a.open_after, None);
        assert!(a.message().unwrap().0.contains("not ARK_C"));
    }

    #[test]
    fn upper_s_never_takes_a_home_session_from_another_machine() {
        let mut a = app();
        let k = closed_card(&mut a);
        let card = a.board.cards.iter_mut().find(|c| c.key == k).unwrap();
        let d = card.detail.as_mut().unwrap();
        // Its home is a machine this board doesn't have, whose session shares a name with one here.
        d.home = serde_json::from_value(json!({"machine":"elsewhere","session":"web"})).ok();
        d.tenant = None;
        d.organization = Some("platform".into());
        select(&mut a, &k);
        press(&mut a, 'S');
        let Mode::Accounts {
            screen: AccountScreen::Models { owner, .. },
            ..
        } = &a.mode
        else {
            panic!("expected the model list, got {:?}", a.message())
        };
        assert_eq!(owner.id, "platform", "web's owner here isn't this card's");
    }
}
