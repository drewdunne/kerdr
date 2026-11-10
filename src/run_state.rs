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
//!
//! Release 1 of intent 0591 took away the smaller refusals around it, each by doing the step first:
//! a machine whose accounts haven't arrived is waited for ([`RunState::NoAccounts`]), a card in
//! won't do is brought back and then started, a conversation running outside herdr is moved into
//! herdr on the account picked, a backend too old to start on an account is brought up to date
//! first, and a card that changed while the picker was open has the account applied to how it is
//! now.
use super::*;
use crate::accounts::AccountAgent;
use crate::app::account_controls::MoveTarget;
use serde_json::json;

/// How a stopped card starts: the request ⏎ or `d` would send for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartHow {
    /// Work nobody started: `dispatch`, as `d` sends.
    #[default]
    Dispatch,
    /// Work that ran and has no agent now (its workspace closed, or open with its agent gone), or
    /// done work: `reopen`, as ⏎ sends, which resumes its conversation.
    Reopen,
    /// An open workspace with no intent and nothing running: `account-start`, a fresh agent of the
    /// account's own program in a new tab there, since there's no conversation to resume (intent
    /// 0591, K1-105).
    Fresh,
}

impl StartHow {
    pub fn action(self) -> &'static str {
        match self {
            StartHow::Dispatch => "dispatch",
            StartHow::Reopen => "reopen",
            StartHow::Fresh => "account-start",
        }
    }
}

/// A stopped card Kerdr can start, and where.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Start {
    pub how: StartHow,
    /// The machine whose backend starts it, and whose accounts the picker offers.
    pub machine: usize,
    /// The program a reopen resumes its conversation with, when the feed says: only that
    /// program's accounts can take the conversation, so only they're offered.
    pub program: Option<String>,
    /// It's in won't do: brought back first (`restore`), then started (intent 0591).
    pub restore: bool,
    /// Its conversation runs in a terminal outside herdr: the reopen moves it into herdr, onto the
    /// account picked (intent 0591).
    pub outside: bool,
    /// Its machine's backend can't take an account on a start yet (no `starts_on`): Kerdr brings
    /// its ark-sdlc up to date first, and starts it once its feed says it can (intent 0591).
    pub update: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Running(Box<AccountAgent>),
    Stopped(Start),
    /// The machine whose accounts the picker would offer hasn't sent them yet: s waits for them,
    /// then opens the picker (intent 0591).
    NoAccounts(usize),
    Unavailable(String),
}

/// How long s waits for a machine's accounts to arrive before it says they haven't (intent 0591).
pub const ACCOUNTS_WAIT: Duration = Duration::from_secs(60);
/// How long Kerdr waits for a backend it asked to update to say it can start on an account.
pub const UPDATE_WAIT: Duration = Duration::from_secs(90);
/// How long an account picked for a card waits for that card to settle (a start that turned out
/// to be running already, a picker answered as the card changed) before it's given up.
pub const APPLY_WAIT: Duration = Duration::from_secs(90);

/// s on a card whose machine's accounts haven't arrived: the card, its machine, and since when.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountsWait {
    pub key: String,
    pub machine: usize,
    pub since: Instant,
}

/// An account picked for a card that Kerdr still owes it, one for each card, each step waiting for
/// the one before (intent 0591, Codex's review): its machine's backend brought up to date, then the
/// card brought back from won't do, then started on that account, or moved to it once an agent runs
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct OwedAccount {
    pub key: String,
    pub to: String,
    pub since: Instant,
    /// Waiting for this machine's backend to say it can start on an account, after Kerdr asked it to
    /// update: its feed is read every few seconds meanwhile.
    pub update: Option<usize>,
    /// The request that asked for that update, by Kerdr's number for it, and what its answer said
    /// when it failed: only that request's answer touches this wait (Codex's review).
    pub asked: Option<u64>,
    pub failed: Option<String>,
    /// It's in won't do, and comes back first; and whether that `restore` has gone. A restore that
    /// went and is neither held nor sent on, with the card still in won't do, didn't happen: its own
    /// failure said why, and nothing starts.
    pub restore: bool,
    pub restore_sent: bool,
    /// Its start found its agent running already, so nothing restarted on the account picked: that
    /// agent moves to it once Kerdr sees it, and nothing is started again (intent 0591, K1-126).
    pub moving: bool,
}

impl OwedAccount {
    fn new(key: &str, to: &str) -> OwedAccount {
        OwedAccount {
            key: key.to_string(),
            to: to.to_string(),
            since: Instant::now(),
            update: None,
            asked: None,
            failed: None,
            restore: false,
            restore_sent: false,
            moving: false,
        }
    }
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
                return RunState::NoAccounts(card.machine_index);
            }
            if let Some(why) = self.agent_unclear(card) {
                return RunState::Unavailable(why);
            }
            let machine = self.machines[card.machine_index].name();
            let runs: Vec<String> = card
                .agents
                .iter()
                .map(|a| self.providers(card.machine_index).program_label(&a.agent))
                .filter(|p| !p.is_empty())
                .collect();
            let names: Vec<String> = self
                .offered(card.machine_index)
                .programs()
                .iter()
                .map(|id| self.providers(card.machine_index).program_label(id))
                .collect();
            let what = match runs.first() {
                Some(p) => p.clone(),
                None => "something".into(),
            };
            let accounts = if names.is_empty() {
                format!("none of {machine}'s accounts run")
            } else {
                format!(
                    "none of {machine}'s accounts run (they run {})",
                    names.join(", ")
                )
            };
            return RunState::Unavailable(format!(
                "{reference}'s pane runs {what}, which {accounts}, so s can't restart it on one; starting another would make a second copy."
            ));
        }
        if card.detail.is_none() {
            // An open workspace with nothing running and no intent: a fresh agent starts in a new
            // tab there, on the account picked, where its backend can (intent 0591, K1-105). Only
            // the workspace s was pressed on, on its own machine.
            if card.is_open_workspace()
                && self.actions_on()
                && self
                    .account_feed(card.machine_index)
                    .is_some_and(|f| f.starts_on(StartHow::Fresh.action()))
            {
                return RunState::Stopped(Start {
                    how: StartHow::Fresh,
                    machine: card.machine_index,
                    ..Start::default()
                });
            }
            return RunState::Unavailable(format!(
                "{} has no agent running and no intent to start: s starts or moves an intent's agent.",
                card.workspace_name()
            ));
        }
        // In won't do: s brings it back, then starts it, the picker being the one question
        // (intent 0591). As it would be once back.
        let restore = card.dismissed();
        let back = restore.then(|| {
            let mut c = card.clone();
            if let Some(d) = c.detail.as_mut() {
                d.disposition = None;
            }
            c
        });
        let card = back.as_ref().unwrap_or(card);
        // Its conversation runs outside herdr: the reopen moves it in, onto the account picked.
        let outside = card.outside_herdr().is_some() && !card.done();
        let how = if waiting_work(card) && !outside {
            StartHow::Dispatch
        } else if outside
            || card.reopenable()
            || (card.is_open_workspace() && (card.under_way() || card.done()))
        {
            // Open with its agent gone: the reopen resumes the conversation in a tab there.
            StartHow::Reopen
        } else {
            return RunState::Unavailable(match card.stage {
                Stage::Backlog => format!("{reference} has nothing to start yet."),
                _ => format!(
                    "{reference} is in {} with no agent and no workspace, and Kerdr doesn't know how to start it from there.",
                    card.stage.name()
                ),
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
            return RunState::NoAccounts(machine);
        };
        // A backend that can't take an account yet is brought up to date first (intent 0591).
        let update = !feed.starts_on(how.action());
        // The program its conversation ran, for a reopen, and for a dispatch too: work queued to be
        // reopened starts by d, and its backend resumes the conversation then.
        let program = card.resume_agent().map(str::to_string);
        RunState::Stopped(Start {
            how,
            machine,
            program,
            restore,
            outside,
            update,
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

    /// The program this agent has to stay on, in words, when `code` runs another: None for an
    /// account it may move to, and for a feed that doesn't say (intent 0591).
    pub fn held_to(
        &self,
        machine: usize,
        agent: &crate::accounts::AccountAgent,
        code: &str,
    ) -> Option<String> {
        if agent.programs.is_empty() {
            return None;
        }
        let runs = self.account_runs(machine, self.account(machine, code)?);
        if runs.is_empty() || runs.iter().any(|p| agent.programs.contains(p)) {
            return None;
        }
        let providers = self.providers(machine);
        let stays: Vec<String> = agent
            .programs
            .iter()
            .map(|p| providers.program_label(p))
            .collect();
        Some(stays.join(" or "))
    }

    /// `s` on a card: the picker its state calls for, or why there's none.
    pub(super) fn s_on_card(&mut self, card: Card) {
        // Work Kerdr follows (a start, a move into herdr) is left alone until it ends: its worker may
        // still be bringing the agent back (Codex's review).
        if self.follow_holds(&card, "s").is_some() {
            return;
        }
        match self.run_state(&card) {
            RunState::Running(agent) => {
                self.accounts_wait = None;
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
                self.accounts_wait = None;
                let choices = self.start_choices(&start);
                self.push_account_screen(AccountScreen::Start {
                    key: card.key,
                    reference: card.reference,
                    start,
                    choices,
                    sel: 0,
                });
            }
            // Its machine's accounts haven't come yet: wait for them, then open the picker, rather
            // than refuse (intent 0591).
            RunState::NoAccounts(machine) => {
                let name = self.machines[machine].name();
                // Accounts come with a machine's card feed: with none configured, none will.
                if self.machines[machine].machine.feed.is_none() {
                    self.say(
                        format!(
                            "{name} has no card feed configured, so it sends no accounts for s to offer on {}.",
                            card.reference
                        ),
                        Tone::Info,
                    );
                    return;
                }
                if !self.machines[machine].conn.is_live() {
                    self.say(
                        format!(
                            "{name} isn't answering, so it hasn't sent its accounts: s on {} offers them once it's back.",
                            card.reference
                        ),
                        Tone::Warn,
                    );
                    return;
                }
                self.accounts_wait = Some(AccountsWait {
                    key: card.key.clone(),
                    machine,
                    since: Instant::now(),
                });
                self.say(
                    format!(
                        "Waiting for {name}'s accounts; the picker for {} opens when they come. Esc on the board stops waiting.",
                        card.reference
                    ),
                    Tone::Info,
                );
            }
            RunState::Unavailable(why) => self.say(why, Tone::Info),
        }
    }

    /// Enter on the start picker: the card's start, as ⏎ or d would send it, on the account
    /// chosen ("list": its owner's list, over any account kept for it before). A card that changed
    /// while the picker was open has the account applied to how it is now: an agent running it now
    /// is moved to that account, and a card stopped elsewhere or differently is started there, the
    /// picker opening again only when that account can't take it now (intent 0591). Its own checks
    /// (a start already on its way, kickoff questions, a full session) still apply.
    pub(super) fn start_on_account(&mut self, key: &str, start: &Start, to: &str) -> Action {
        let Some(card) = self.card(key).cloned() else {
            self.say(
                "That card is gone from the board (finished or moved), so nothing started.",
                Tone::Warn,
            );
            return Action::None;
        };
        let reference = card.reference.clone();
        let start = match self.run_state(&card) {
            RunState::Stopped(now) if now == *start => now,
            RunState::Stopped(now) => {
                if to != "list" && !self.start_choices(&now).iter().any(|c| c == to) {
                    // It can't take that account as it is now: the picker again, with what it can.
                    let choices = self.start_choices(&now);
                    self.say(
                        format!(
                            "{reference} changed while you were choosing, and {to} can't take it now: choose again."
                        ),
                        Tone::Info,
                    );
                    self.push_account_screen(AccountScreen::Start {
                        key: card.key.clone(),
                        reference: reference.clone(),
                        start: now,
                        choices,
                        sel: 0,
                    });
                    return Action::None;
                }
                now
            }
            // An agent runs it now (started meanwhile): that agent moves to the account picked.
            RunState::Running(agent) => return self.move_to_account(&card, &agent, to),
            RunState::NoAccounts(machine) => {
                self.say(
                    format!(
                        "{reference} changed while you were choosing, and {} hasn't sent its accounts for how it is now; s on it opens the picker once they come.",
                        self.machines[machine].name()
                    ),
                    Tone::Info,
                );
                return Action::None;
            }
            RunState::Unavailable(why) => {
                self.say(
                    format!("{reference} changed while you were choosing: {why}"),
                    Tone::Info,
                );
                return Action::None;
            }
        };
        // Its backend can't take an account yet: brought up to date first, then started, never
        // sent the account before it can take it (intent 0591).
        if start.update {
            let name = self.machines[start.machine].name();
            let mut owed = OwedAccount::new(&card.key, to);
            owed.update = Some(start.machine);
            // Then brought back from won't do, if it's there, before its start.
            owed.restore = start.restore;
            self.owe(owed);
            self.say(
                format!(
                    "{name}'s backend can't start work on a chosen account yet: Kerdr is bringing its ark-sdlc up to date, then starts {reference} on {}. Esc on the board drops it.",
                    account_words(to)
                ),
                Tone::Info,
            );
            return Action::None;
        }
        // In won't do: brought back first; its start goes once that's answered (intent 0591).
        if start.restore {
            let mut owed = OwedAccount::new(&card.key, to);
            owed.restore = true;
            owed.restore_sent = true;
            self.owe(owed);
            self.say(
                format!(
                    "Bringing {reference} back from won't do, then starting it on {}.",
                    account_words(to)
                ),
                Tone::Info,
            );
            return self.intent_action_on(card, "restore", json!({}));
        }
        self.send_start(card, &start, to)
    }

    /// The start itself, as ⏎ or d sends it, with `account` added: "list" goes too, his owner's
    /// list, chosen over an account a queued start kept from before.
    fn send_start(&mut self, card: Card, start: &Start, to: &str) -> Action {
        let extra = json!({"account": to});
        if start.outside {
            self.say(
                format!(
                    "Moving {}'s conversation into herdr, onto {}…",
                    card.reference,
                    account_words(to)
                ),
                Tone::Info,
            );
        }
        match start.how {
            StartHow::Dispatch => self.intent_action_on(card, "dispatch", extra),
            StartHow::Reopen => self.reopen_action_with(&card, extra),
            StartHow::Fresh => self.start_fresh(card, to),
        }
    }

    /// A fresh agent in a new tab of this open workspace, on the account picked; you go in once it
    /// starts, while you're still on its card (intent 0591, K1-105).
    fn start_fresh(&mut self, card: Card, to: &str) -> Action {
        let (Some(session), Some(workspace)) = (card.session.clone(), card.workspace_id.clone())
        else {
            self.say(
                format!(
                    "{} isn't open any more, so nothing started.",
                    card.workspace_name()
                ),
                Tone::Info,
            );
            return Action::None;
        };
        if !self.can_write(card.machine_index) {
            return Action::None;
        }
        self.action_from = Some(CardSnapshot::of(&card));
        Action::Mutate {
            machine: card.machine_index,
            request: json!({"action":"account-start","session":session,"workspace":workspace,"to":to}),
        }
    }

    /// The running agent at this card moved to the account picked for it, as the move picker sends
    /// it. Nothing when it's on that account already.
    fn move_to_account(&mut self, card: &Card, agent: &AccountAgent, to: &str) -> Action {
        if to != "list" && agent.account == to {
            self.say(
                format!("{}'s agent is on {to} already.", card.reference),
                Tone::Info,
            );
            return Action::None;
        }
        self.say(
            format!(
                "{}'s agent is running now: moving it to {}.",
                card.reference,
                account_words(to)
            ),
            Tone::Info,
        );
        Action::Mutate {
            machine: card.machine_index,
            request: json!({"action":"account-move","pane":agent.pane,"session":agent.session,"conversation":agent.conversation,"to":to}),
        }
    }

    /// A start on an account sent on to the machine an intent lives on now, whose backend can't
    /// take an account yet: owed until that machine's ark-sdlc is brought up to date (intent 0591).
    /// False when the card isn't on the board.
    pub(super) fn owe_after_update(&mut self, id: &str, machine: usize, to: &str) -> bool {
        let Some(key) = self
            .board
            .cards
            .iter()
            .find(|c| c.detail.as_ref().is_some_and(|d| d.id == id))
            .map(|c| c.key.clone())
        else {
            return false;
        };
        let mut owed = OwedAccount::new(&key, to);
        owed.update = Some(machine);
        self.owe(owed);
        true
    }

    /// A start on an account that found the card's agent running already, so nothing restarted:
    /// that agent moves to the account picked once Kerdr sees it, as s on a running card does
    /// (intent 0591, K1-126). False when the card isn't on the board.
    pub(super) fn owe_move(&mut self, key: &str, to: &str) -> bool {
        if self.card(key).is_none() {
            return false;
        }
        let mut owed = OwedAccount::new(key, to);
        owed.moving = true;
        self.owe(owed);
        true
    }

    /// An account owed to a card, in place of one owed to it before; others' stay owed.
    fn owe(&mut self, owed: OwedAccount) {
        self.owed_accounts.retain(|o| o.key != owed.key);
        self.owed_accounts.push(owed);
    }

    /// What's owed to accounts picked with s, each turn of the board: for the first card whose next
    /// step is due, that step (its restore, its start, or the move once an agent runs it); and why
    /// one was given up. Only where an answer can't take keys meant for something else.
    pub fn account_owed(&mut self) -> Action {
        if !self.enter_can_land_here() {
            return Action::None;
        }
        // A move's question held while you were elsewhere: asked now (intent 0591, K2-27).
        if let Some(question) = self.held_move_question.take() {
            self.ask_held_move(question);
            return Action::None;
        }
        let mut i = 0;
        while i < self.owed_accounts.len() {
            match self.owed_step(i) {
                Owed::Wait => i += 1,
                Owed::Drop => {
                    self.owed_accounts.remove(i);
                }
                // A step on its way: it stays owed, and what follows waits for it.
                Owed::Send(action) => {
                    if !matches!(action, Action::None) {
                        return action;
                    }
                    i += 1;
                }
                Owed::Done(action) => {
                    self.owed_accounts.remove(i);
                    if !matches!(action, Action::None) {
                        return action;
                    }
                }
            }
        }
        Action::None
    }

    /// The next step owed to one card's account.
    fn owed_step(&mut self, i: usize) -> Owed {
        let owed = self.owed_accounts[i].clone();
        let Some(card) = self.card(&owed.key).cloned() else {
            return Owed::Drop;
        };
        let reference = card.reference.clone();
        let id = card
            .detail
            .as_ref()
            .map(|d| d.id.clone())
            .unwrap_or_default();
        let given_up = |app: &mut App, what: &str| {
            app.say(
                format!(
                    "{reference} didn't start on {}: {what}. s on it tries again.",
                    account_words(&owed.to)
                ),
                Tone::Warn,
            );
            Owed::Drop
        };
        // 1. Its machine's backend brought up to date.
        if let Some(machine) = owed.update {
            let name = self.machines[machine].name();
            if let Some(failed) = owed.failed {
                self.fail(format!(
                    "{reference} didn't start: Kerdr couldn't ask {name} to bring its ark-sdlc up to date ({failed}), and its backend can't start work on a chosen account yet. ⏎ starts it on its owner's list there."
                ));
                return Owed::Drop;
            }
            let ready = self
                .account_feed(machine)
                .is_some_and(|f| STARTS_ON.iter().all(|a| f.starts_on(a)));
            if !ready {
                if owed.since.elapsed() > UPDATE_WAIT {
                    self.fail(format!(
                        "{reference} didn't start: {name}'s copy of ark-sdlc still hasn't updated after {} s, so its backend can't start work on a chosen account. ⏎ starts it on its owner's list there; an agent there can say why that copy won't update.",
                        UPDATE_WAIT.as_secs()
                    ));
                    return Owed::Drop;
                }
                return Owed::Wait;
            }
            // Up to date: the next step has its own time.
            let o = &mut self.owed_accounts[i];
            o.update = None;
            o.since = Instant::now();
        }
        // 2. Brought back from won't do.
        if owed.restore && card.dismissed() {
            if !owed.restore_sent {
                let o = &mut self.owed_accounts[i];
                o.restore_sent = true;
                o.since = Instant::now();
                self.say(
                    format!(
                        "Bringing {reference} back from won't do, then starting it on {}.",
                        account_words(&owed.to)
                    ),
                    Tone::Info,
                );
                return Owed::Send(self.intent_action_on(card, "restore", json!({})));
            }
            // Gone: held while it's on its way (or answered yes and not in the feed yet), or sent on
            // to the machine the record lives on (Codex's review).
            let restoring = self
                .resolver
                .holds_where(|r| r["id"] == id.as_str() && r["action"] == "restore")
                || self
                    .resend
                    .iter()
                    .any(|(_, r, _)| r["id"] == id.as_str() && r["action"] == "restore")
                || self.resent.iter().any(|(_, r)| *r == id);
            if !restoring {
                // It didn't happen: its own failure said why.
                return Owed::Drop;
            }
            if self.owed_accounts[i].since.elapsed() > APPLY_WAIT {
                return given_up(self, "it didn't come back from won't do in time");
            }
            return Owed::Wait;
        }
        if self.still_changing_quietly(&card) {
            if self.owed_accounts[i].since.elapsed() > APPLY_WAIT {
                return given_up(self, "it never came to where it could start");
            }
            return Owed::Wait;
        }
        // Running already: only ever moved, never started again.
        if owed.moving {
            return match self.run_state(&card) {
                RunState::Running(agent) => {
                    Owed::Done(self.move_to_account(&card, &agent, &owed.to))
                }
                _ if self.owed_accounts[i].since.elapsed() > APPLY_WAIT => {
                    self.say(
                        format!(
                            "{reference} wasn't moved to {}: Kerdr hasn't seen its agent running. s on its card moves it once it shows.",
                            account_words(&owed.to)
                        ),
                        Tone::Warn,
                    );
                    Owed::Drop
                }
                _ => Owed::Wait,
            };
        }
        // 3. Started on that account, or the agent running it now moved there.
        match self.run_state(&card) {
            RunState::Running(agent) => Owed::Done(self.move_to_account(&card, &agent, &owed.to)),
            // It starts on a machine whose backend can't take an account yet (its restore was sent
            // on to an older one): that backend is brought up to date first, the account kept
            // (Codex's review).
            RunState::Stopped(start) if start.update && !start.restore => {
                let name = self.machines[start.machine].name();
                let o = &mut self.owed_accounts[i];
                o.update = Some(start.machine);
                o.asked = None;
                o.failed = None;
                o.since = Instant::now();
                self.say(
                    format!(
                        "{name}'s backend can't start work on a chosen account yet: Kerdr is bringing its ark-sdlc up to date, then starts {reference} on {}. Esc on the board drops it.",
                        account_words(&owed.to)
                    ),
                    Tone::Info,
                );
                Owed::Wait
            }
            RunState::Stopped(start) if !start.update && !start.restore => {
                if owed.to != "list" && !self.start_choices(&start).contains(&owed.to) {
                    self.say(
                        format!(
                            "{reference} can't start on {} as it is now: s on it offers what can.",
                            owed.to
                        ),
                        Tone::Info,
                    );
                    return Owed::Done(Action::None);
                }
                Owed::Done(self.send_start(card, &start, &owed.to))
            }
            _ => {
                if self.owed_accounts[i].since.elapsed() > APPLY_WAIT {
                    return given_up(self, "it never came to where it could start");
                }
                Owed::Wait
            }
        }
    }

    /// Whether a change to this card is still on its way, without saying so.
    fn still_changing_quietly(&self, card: &Card) -> bool {
        let Some(id) = card.detail.as_ref().map(|d| d.id.as_str()) else {
            return false;
        };
        self.resolver.starting(id).is_some()
            || self
                .resolver
                .unanswered_where(|r| r["id"] == id && r["action"] == "restore")
                .is_some()
            || self.following(id).is_some()
    }

    /// The request that asks an old backend to bring its ark-sdlc up to date, when it's due, with
    /// its machine: a read of the card's text (`edit` without `text`, which changes nothing), which
    /// ark-sdlc's backend pulls before. Sent once (intent 0591).
    pub fn update_due(&mut self) -> Option<(usize, serde_json::Value, u64)> {
        let i = self
            .owed_accounts
            .iter()
            .position(|o| o.update.is_some() && o.asked.is_none())?;
        let machine = self.owed_accounts[i].update?;
        let key = self.owed_accounts[i].key.clone();
        let id = self.card(&key)?.detail.as_ref()?.id.clone();
        self.update_asks += 1;
        let n = self.update_asks;
        // One update serves every card waiting on that machine now; one that waits later asks again.
        for o in self
            .owed_accounts
            .iter_mut()
            .filter(|o| o.update == Some(machine) && o.asked.is_none())
        {
            o.asked = Some(n);
        }
        Some((machine, json!({"action":"edit","id":id}), n))
    }

    /// That request's answer, by its number: only whether it reached the backend matters, since the
    /// pull comes before anything it says, and only to the waits it was asked for. Its machine's feed
    /// is read again meanwhile (accounts_awaited_on).
    pub fn update_answered(
        &mut self,
        machine: usize,
        n: u64,
        result: Result<crate::actions::Response, String>,
    ) {
        let Err(e) = result else {
            return;
        };
        let why = crate::errlog::last_part(&e, 200).to_string();
        for o in self
            .owed_accounts
            .iter_mut()
            .filter(|o| o.update == Some(machine) && o.asked == Some(n))
        {
            o.failed = Some(why.clone());
        }
    }

    /// The machines whose feed is worth reading often for s: one whose accounts it waits for, and
    /// those it asked to update.
    pub fn accounts_awaited_on(&self) -> Vec<usize> {
        let mut machines: Vec<usize> = self
            .accounts_wait
            .iter()
            .map(|w| w.machine)
            .chain(self.owed_accounts.iter().filter_map(|o| o.update))
            .filter(|&m| self.machines.get(m).is_some_and(|m| m.conn.is_live()))
            .collect();
        machines.sort_unstable();
        machines.dedup();
        machines
    }

    /// s waiting for a machine's accounts, each turn of the board: the picker once they've come,
    /// while you're still on that card and nowhere a picker would take keys meant for something
    /// else; after ACCOUNTS_WAIT, why not.
    pub fn accounts_wait_due(&mut self) {
        let Some(wait) = self.accounts_wait.clone() else {
            return;
        };
        if self.account_feed(wait.machine).is_some() {
            if !self.enter_can_land_here() {
                return;
            }
            self.accounts_wait = None;
            if let Some(card) = self
                .selected_for_control()
                .filter(|c| c.key == wait.key)
                .cloned()
            {
                self.s_on_card(card);
            }
            return;
        }
        if wait.since.elapsed() > ACCOUNTS_WAIT {
            self.accounts_wait = None;
            let reference = self
                .card(&wait.key)
                .map(|c| c.reference.clone())
                .unwrap_or_else(|| "the card".into());
            self.say(
                format!(
                    "{} still hasn't sent its accounts after a minute, so there's no picker for {reference}: ⏎ starts it on its owner's list, and s tries again.",
                    self.machines[wait.machine].name()
                ),
                Tone::Warn,
            );
        }
    }
}

/// One card's owed account, this turn: still waiting, given up, a step sent (it stays owed), or
/// its last step (it's paid).
enum Owed {
    Wait,
    Drop,
    Send(Action),
    Done(Action),
}

/// How a message names an account picked with s: its code, or "its owner's list".
fn account_words(to: &str) -> String {
    if to == "list" {
        "its owner's list".into()
    } else {
        to.to_string()
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
    fn an_older_backend_is_brought_up_to_date_and_then_sent_the_start() {
        let mut a = app();
        let k = backlog_card(&a);
        no_kickoff(&mut a, &k);
        let mi = a.card(&k).unwrap().machine_index;
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        // The picker opens anyway: the update is Kerdr's to do, not his.
        assert!(start_screen(&a).0.update);
        let at = start_screen(&a)
            .1
            .iter()
            .position(|c| c == "ARK_C")
            .unwrap();
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        // Nothing is sent that would start it on its owner's list without a word.
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let said = a.message().unwrap().0.to_string();
        assert!(said.contains("bringing its ark-sdlc up to date"), "{said}");
        // The update: a read its backend pulls before, sent once.
        let id = a.card(&k).unwrap().detail.as_ref().unwrap().id.clone();
        let (machine, request, n) = a.update_due().unwrap();
        assert_eq!((machine, request), (mi, json!({"action":"edit","id":id})));
        assert_eq!(a.update_due(), None);
        assert!(a.accounts_awaited_on().contains(&mi));
        a.update_answered(
            mi,
            n,
            Ok(crate::actions::parse_response(r#"{"ok":true,"message":""}"#).unwrap()),
        );
        assert_eq!(
            a.account_owed(),
            Action::None,
            "not before its feed says it can"
        );
        // Its feed says it can now: the start goes, on the account picked.
        a.machines[mi].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(machine, mi);
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
        assert!(a.owed_accounts.is_empty());
    }

    #[test]
    fn a_backend_that_never_updates_is_said_plainly_and_nothing_starts() {
        let mut a = app();
        let k = backlog_card(&a);
        let mi = a.card(&k).unwrap().machine_index;
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        a.update_due();
        a.owed_accounts[0].since -= UPDATE_WAIT + Duration::from_secs(1);
        assert_eq!(a.account_owed(), Action::None);
        let (said, tone) = a.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(said.contains("still hasn't updated"), "{said}");
        assert!(said.contains("its owner's list"), "{said}");
        assert!(a.owed_accounts.is_empty());

        // And when the request to update couldn't even be sent, it says that.
        a.message = None;
        press(&mut a, 's');
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let (_, _, n) = a.update_due().unwrap();
        a.update_answered(
            mi,
            n,
            Err("ssh: connect to host build-box: Connection refused".into()),
        );
        assert_eq!(a.account_owed(), Action::None);
        let said = a.message().unwrap().0;
        assert!(said.contains("couldn't ask"), "{said}");
    }

    #[test]
    fn a_card_an_agent_started_on_while_choosing_has_that_agent_moved() {
        let mut a = app();
        let k = closed_card(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        let at = start_screen(&a)
            .1
            .iter()
            .position(|c| c == "ARK_C")
            .unwrap();
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
        let running = a.move_agent(a.card(&k).unwrap()).unwrap();
        assert_ne!(running.account, "ARK_C");
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        // Not "press s again": the account picked goes to the agent running it now.
        let Action::Mutate { request, .. } = key(&mut a, KeyCode::Enter) else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(request["action"], "account-move");
        assert_eq!(request["to"], "ARK_C");
        assert_eq!(request["pane"], running.pane.as_str());
    }

    #[test]
    fn a_card_that_changed_so_the_account_cant_take_it_offers_the_picker_again() {
        let mut a = app();
        let k = backlog_card(&a);
        no_kickoff(&mut a, &k);
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.detail.as_mut().unwrap().resume = None;
        }
        select(&mut a, &k);
        press(&mut a, 's');
        // A fresh start takes Codex; the same card now resumes a Claude Code conversation.
        let at = start_screen(&a)
            .1
            .iter()
            .position(|c| c == "KBZ_X")
            .unwrap();
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            let d = c.detail.as_mut().unwrap();
            d.resume = serde_json::from_value(json!({"agent": "claude"})).ok();
        }
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let (start, choices) = start_screen(&a);
        assert_eq!(start.program.as_deref(), Some("claude"));
        assert!(!choices.contains(&"KBZ_X".to_string()), "{choices:?}");
        assert!(a.message().unwrap().0.contains("choose again"));
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
    fn a_move_to_a_machine_whose_backend_cant_take_an_account_updates_it_then_starts_there() {
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
        assert!(said.contains("bringing its ark-sdlc up to date"), "{said}");
        assert_eq!(a.update_due().map(|(m, ..)| m), Some(other));
        assert_eq!(a.account_owed(), Action::None);
        a.machines[other].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(machine, other, "it goes where the intent lives now");
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
    }

    /// Changes the feed's own record of this card, as its next read would bring it, so the board
    /// keeps it through rebuilds.
    fn in_feed(a: &mut App, key: &str, change: impl Fn(&mut crate::feed::FeedCard)) {
        let id = a.card(key).unwrap().detail.as_ref().unwrap().id.clone();
        for m in a.machines.iter_mut() {
            if let Some(f) = m.feed.as_mut() {
                for c in f.cards.iter_mut().filter(|c| c.id == id) {
                    change(c);
                }
            }
        }
        a.rebuild();
    }
    /// A request on its way, as the event loop sends it, and its answer, as the loop takes it.
    fn on_its_way(a: &mut App, machine: usize, request: &Value) {
        a.queue_action(machine, request.clone());
        a.resolver.ready();
    }
    fn answered(a: &mut App, request: &Value, reply: Value) {
        let n = a
            .resolver
            .unanswered_for(request["action"].as_str().unwrap(), request["id"].as_str())
            .expect("on its way");
        a.finish_job(
            n,
            request.clone(),
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }
    /// A backlog card in won't do, with no kickoff questions, its key.
    fn set_aside(a: &mut App) -> String {
        let k = backlog_card(a);
        in_feed(a, &k, |c| {
            c.disposition = Some("wont_do".into());
            c.kickoff = None;
        });
        assert!(a.card(&k).unwrap().dismissed());
        k
    }

    #[test]
    fn s_on_a_card_in_wont_do_brings_it_back_then_starts_it_on_the_account() {
        let mut a = app();
        let k = set_aside(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        assert!(start_screen(&a).0.restore);
        let (machine, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "restore");
        assert!(request.get("account").is_none());
        assert!(a.message().unwrap().0.contains("Bringing"));
        on_its_way(&mut a, machine, &request);
        // Nothing starts before the restore has answered.
        assert_eq!(a.account_owed(), Action::None);
        assert_eq!(a.owed_accounts.len(), 1);
        answered(&mut a, &request, json!({"ok":true,"message":"Restored."}));
        // Its feed shows it back.
        in_feed(&mut a, &k, |c| c.disposition = None);
        let Action::Mutate { request, .. } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
        assert!(a.owed_accounts.is_empty(), "paid once");
    }

    #[test]
    fn a_restore_that_failed_starts_nothing() {
        let mut a = app();
        let k = set_aside(&mut a);
        select(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        on_its_way(&mut a, machine, &request);
        answered(&mut a, &request, json!({"ok":false,"message":"no"}));
        assert_eq!(a.account_owed(), Action::None);
        assert!(a.owed_accounts.is_empty());
        assert_eq!(a.account_owed(), Action::None, "and nothing later either");
    }

    /// Codex's review: an old backend and won't do at once. The update comes first, then the
    /// restore, then the start, each after the one before.
    #[test]
    fn an_old_backend_and_wont_do_update_then_restore_then_start() {
        let mut a = app();
        let k = set_aside(&mut a);
        let mi = a.card(&k).unwrap().machine_index;
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        let start = start_screen(&a).0.clone();
        assert!(start.update && start.restore);
        let at = start_screen(&a)
            .1
            .iter()
            .position(|c| c == "ARK_C")
            .unwrap();
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert_eq!(a.update_due().map(|(m, ..)| m), Some(mi));
        assert_eq!(a.account_owed(), Action::None, "nothing before the update");
        a.machines[mi].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("no restore: {:?}", a.message())
        };
        assert_eq!(request["action"], "restore");
        on_its_way(&mut a, machine, &request);
        assert_eq!(
            a.account_owed(),
            Action::None,
            "nothing before the restore's answer"
        );
        answered(&mut a, &request, json!({"ok":true,"message":"Restored."}));
        in_feed(&mut a, &k, |c| c.disposition = None);
        let Action::Mutate { request, .. } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
    }

    /// Codex's review: a restore sent on to the machine the record lives on keeps its account, and
    /// the start goes there once it's back.
    #[test]
    fn a_restore_sent_on_to_another_machine_still_starts_there() {
        let mut a = app();
        let k = set_aside(&mut a);
        let here = a.card(&k).unwrap().machine_index;
        let other = (0..2).find(|&m| m != here).unwrap();
        select(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        on_its_way(&mut a, machine, &request);
        let home = format!("{}/web", a.machines[other].name());
        answered(
            &mut a,
            &request,
            json!({"ok":false,"message":"lives elsewhere","home":home}),
        );
        assert_eq!(a.account_owed(), Action::None);
        assert_eq!(a.owed_accounts.len(), 1, "the account is still owed");
        // The restore, sent on: it keeps its place, now for that machine.
        let id = request["id"].as_str().unwrap().to_string();
        assert!(
            a.resolver
                .holds_where(|r| r["id"] == id.as_str() && r["action"] == "restore")
        );
        let ready = a.resolver.ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].1, other, "sent on to where it lives");
        assert_eq!(a.account_owed(), Action::None);
        answered(&mut a, &request, json!({"ok":true,"message":"Restored."}));
        in_feed(&mut a, &k, |c| c.disposition = None);
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(machine, other, "where it lives");
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
    }

    /// Codex's review: an account owed to one card isn't dropped by one picked for another.
    #[test]
    fn accounts_owed_to_two_cards_are_each_paid() {
        let mut a = app();
        let first = set_aside(&mut a);
        select(&mut a, &first);
        press(&mut a, 's');
        let (m1, restore) = choose(&mut a, "ARK_C");
        on_its_way(&mut a, m1, &restore);
        // A second card, homed on the other machine, whose backend needs bringing up to date.
        let second = a
            .board
            .cards
            .iter()
            .find(|c| {
                c.key != first && waiting_work(c) && a.account_feed(c.machine_index).is_some()
            })
            .unwrap()
            .key
            .clone();
        let m2 = (0..2).find(|&m| m != m1).unwrap();
        let name = a.machines[m2].name();
        in_feed(&mut a, &second, |c| {
            c.kickoff = None;
            c.home = serde_json::from_value(json!({"machine": name, "session": "web"})).ok();
        });
        a.machines[m2].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &second);
        press(&mut a, 's');
        let at = start_screen(&a)
            .1
            .iter()
            .position(|c| c == "ARK_C")
            .unwrap();
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert_eq!(a.owed_accounts.len(), 2);
        // The first comes back and starts.
        answered(&mut a, &restore, json!({"ok":true,"message":"Restored."}));
        in_feed(&mut a, &first, |c| c.disposition = None);
        let Action::Mutate { request, .. } = a.account_owed() else {
            panic!("the first didn't start: {:?}", a.message())
        };
        assert_eq!(request["action"], "dispatch");
        let first_id = a.card(&first).unwrap().detail.as_ref().unwrap().id.clone();
        assert_eq!(request["id"], first_id.as_str());
        // The second, once its backend can.
        assert_eq!(a.update_due().map(|(m, ..)| m), Some(m2));
        a.machines[m2].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { request, .. } = a.account_owed() else {
            panic!("the second didn't start: {:?}", a.message())
        };
        let second_id = a.card(&second).unwrap().detail.as_ref().unwrap().id.clone();
        assert_eq!(request["id"], second_id.as_str());
        assert_eq!(request["account"], "ARK_C");
        assert!(a.owed_accounts.is_empty());
    }

    /// Picks this account in the open start picker, when the start must wait (nothing is sent).
    fn choose_waiting(a: &mut App, code: &str) {
        let at = start_screen(a).1.iter().position(|c| c == code).unwrap();
        for _ in 0..at {
            key(a, KeyCode::Down);
        }
        assert_eq!(key(a, KeyCode::Enter), Action::None);
    }

    /// Codex's second review: an update's answer touches only the waits its request was for. The
    /// first card's wait is dropped, a second card asks the same machine to update, and the first's
    /// request then fails late: the second still starts, once its own update has come.
    #[test]
    fn an_old_updates_failure_arriving_after_a_newer_request_leaves_the_newer_wait_alone() {
        let mut a = app();
        let first = backlog_card(&a);
        in_feed(&mut a, &first, |c| c.kickoff = None);
        let mi = a.card(&first).unwrap().machine_index;
        let name = a.machines[mi].name();
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &first);
        press(&mut a, 's');
        choose_waiting(&mut a, "ARK_C");
        let (m, _, old) = a.update_due().unwrap();
        assert_eq!(m, mi);
        // Esc drops the first card's wait, its update request still on its way.
        key(&mut a, KeyCode::Esc);
        assert!(a.owed_accounts.is_empty());
        // A second card, homed on the same machine, its account picked: a request of its own.
        let second = a
            .board
            .cards
            .iter()
            .find(|c| c.key != first && waiting_work(c))
            .unwrap()
            .key
            .clone();
        in_feed(&mut a, &second, |c| {
            c.kickoff = None;
            c.home = serde_json::from_value(json!({"machine": name, "session": "web"})).ok();
        });
        select(&mut a, &second);
        press(&mut a, 's');
        assert!(start_screen(&a).0.update);
        choose_waiting(&mut a, "ARK_C");
        let (m, _, new) = a.update_due().unwrap();
        assert_eq!(m, mi);
        assert_ne!(new, old, "a request of its own");
        // The first card's request fails, late: the second's wait is untouched.
        a.update_answered(mi, old, Err("ssh: connection timed out".into()));
        assert_eq!(a.owed_accounts.len(), 1);
        assert_eq!(a.owed_accounts[0].failed, None);
        assert_eq!(a.account_owed(), Action::None);
        assert_eq!(a.owed_accounts.len(), 1, "the second still waits");
        assert!(!a.failing(), "{:?}", a.message());
        // Its own request reached the backend, which can now: it starts, on the account picked.
        a.update_answered(
            mi,
            new,
            Ok(crate::actions::parse_response(r#"{"ok":true,"message":""}"#).unwrap()),
        );
        a.machines[mi].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("the second didn't start: {:?}", a.message())
        };
        let id = a.card(&second).unwrap().detail.as_ref().unwrap().id.clone();
        assert_eq!(machine, mi);
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["account"], "ARK_C");
        assert!(a.owed_accounts.is_empty());
    }

    /// Codex's second review: a card in won't do whose own machine can take an account, its restore
    /// sent on to the machine it lives on, whose backend can't yet. Back there, that backend is
    /// brought up to date, then the card starts there on the account picked.
    #[test]
    fn a_restore_sent_on_to_an_older_backend_updates_it_then_starts_there_on_the_account() {
        let mut a = app();
        let k = set_aside(&mut a);
        let here = a.card(&k).unwrap().machine_index;
        let other = (0..2).find(|&m| m != here).unwrap();
        a.machines[other].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        let start = start_screen(&a).0.clone();
        assert!(start.restore && !start.update, "no update asked for here");
        let (machine, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "restore");
        on_its_way(&mut a, machine, &request);
        let home = format!("{}/web", a.machines[other].name());
        answered(
            &mut a,
            &request,
            json!({"ok":false,"message":"lives elsewhere","home":home}),
        );
        let ready = a.resolver.ready();
        assert_eq!(ready[0].1, other, "sent on to where it lives");
        answered(&mut a, &request, json!({"ok":true,"message":"Restored."}));
        in_feed(&mut a, &k, |c| c.disposition = None);
        // Back, on a machine whose backend can't take an account yet: it's brought up to date.
        assert_eq!(a.account_owed(), Action::None);
        assert_eq!(a.owed_accounts.len(), 1, "the account is still owed");
        assert_eq!(a.owed_accounts[0].to, "ARK_C");
        let said = a.message().unwrap().0.to_string();
        assert!(said.contains("bringing its ark-sdlc up to date"), "{said}");
        let (m, update, n) = a.update_due().unwrap();
        assert_eq!(m, other);
        assert_eq!(update["action"], "edit");
        assert!(a.accounts_awaited_on().contains(&other));
        assert_eq!(
            a.account_owed(),
            Action::None,
            "not before its feed says it can"
        );
        a.update_answered(
            other,
            n,
            Ok(crate::actions::parse_response(r#"{"ok":true,"message":""}"#).unwrap()),
        );
        a.machines[other].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let Action::Mutate { machine, request } = a.account_owed() else {
            panic!("nothing started: {:?}", a.message())
        };
        assert_eq!(machine, other, "where it lives");
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
        assert!(a.owed_accounts.is_empty());
    }

    /// Codex's review: s's waits are read for every few seconds, ⏎ waiting or not.
    #[test]
    fn s_waiting_on_a_machine_has_its_feed_read_often_without_an_enter() {
        let mut a = app();
        let k = backlog_card(&a);
        let mi = a.card(&k).unwrap().machine_index;
        a.machines[mi].feed.as_mut().unwrap().starts_on.clear();
        select(&mut a, &k);
        press(&mut a, 's');
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.enter_when_open.is_none());
        assert!(
            a.start_awaited_on().contains(&mi),
            "{:?}",
            a.start_awaited_on()
        );
    }

    #[test]
    fn s_waits_for_a_machines_accounts_then_opens_the_picker() {
        let mut a = app();
        let k = backlog_card(&a);
        let here = a.card(&k).unwrap().machine_index;
        let other = (0..2).find(|&m| m != here).unwrap();
        let name = a.machines[other].name();
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.detail.as_mut().unwrap().home = serde_json::from_value(json!({"machine": name})).ok();
        }
        let feed = a.machines[other].feed.take();
        select(&mut a, &k);
        assert_eq!(press(&mut a, 's'), Action::None);
        assert!(matches!(a.mode, Mode::Board));
        let said = a.message().unwrap().0.to_string();
        assert!(
            said.starts_with(&format!("Waiting for {name}'s accounts")),
            "{said}"
        );
        assert_eq!(a.accounts_awaited_on(), vec![other]);
        a.accounts_wait_due();
        assert!(matches!(a.mode, Mode::Board));
        // They've come: the picker opens, on that machine's accounts.
        a.machines[other].feed = feed;
        a.accounts_wait_due();
        assert_eq!(start_screen(&a).0.machine, other);
        assert!(a.accounts_wait.is_none());
    }

    #[test]
    fn accounts_that_never_come_are_said_and_esc_stops_the_wait() {
        let mut a = app();
        let k = backlog_card(&a);
        let here = a.card(&k).unwrap().machine_index;
        let other = (0..2).find(|&m| m != here).unwrap();
        let name = a.machines[other].name();
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.detail.as_mut().unwrap().home = serde_json::from_value(json!({"machine": name})).ok();
        }
        a.machines[other].feed = None;
        select(&mut a, &k);
        press(&mut a, 's');
        a.accounts_wait.as_mut().unwrap().since -= ACCOUNTS_WAIT + Duration::from_secs(1);
        a.accounts_wait_due();
        let (said, tone) = a.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(said.contains("still hasn't sent its accounts"), "{said}");
        assert!(a.accounts_wait.is_none());
        press(&mut a, 's');
        assert!(a.accounts_wait.is_some());
        key(&mut a, KeyCode::Esc);
        assert!(a.accounts_wait.is_none());
    }

    #[test]
    fn s_on_a_conversation_running_outside_herdr_moves_it_in_on_the_account() {
        let mut a = app();
        let k = closed_card(&mut a);
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.detail.as_mut().unwrap().outside_herdr = Some(crate::feed::OutsideHerdr {
                terminal: Some("foot".into()),
                since: Some("2026-11-01T13:02:00+00:00".into()),
            });
        }
        select(&mut a, &k);
        press(&mut a, 's');
        let start = start_screen(&a).0.clone();
        assert!(start.outside);
        assert_eq!(start.how, StartHow::Reopen);
        let (_, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["account"], "ARK_C");
        assert!(a.message().unwrap().0.contains("into herdr"));
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
        // What he picked still holds: its agent moves to it once Kerdr sees it (intent 0591,
        // K1-126), and nothing is started again meanwhile.
        let said = a.message().unwrap().0.to_string();
        assert!(said.contains("wasn't restarted"), "{said}");
        assert!(said.contains("moves it to ARK_C once it sees it"), "{said}");
        assert_eq!(a.owed_accounts.len(), 1);
        assert!(a.owed_accounts[0].moving);
        assert_eq!(a.account_owed(), Action::None);
        // Its agent shows: it moves.
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
        let Action::Mutate { request, .. } = a.account_owed() else {
            panic!("nothing moved: {:?}", a.message())
        };
        assert_eq!(request["action"], "account-move");
        assert_eq!(request["to"], "ARK_C");
        assert!(a.owed_accounts.is_empty());
    }

    #[test]
    fn a_move_owed_to_an_agent_never_seen_gives_up_without_starting_anything() {
        let mut a = app();
        let k = closed_card(&mut a);
        assert!(a.owe_move(&k, "ARK_C"));
        assert_eq!(a.account_owed(), Action::None);
        a.owed_accounts[0].since -= APPLY_WAIT + Duration::from_secs(1);
        assert_eq!(a.account_owed(), Action::None);
        let (said, tone) = a.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(said.contains("wasn't moved to ARK_C"), "{said}");
        assert!(a.owed_accounts.is_empty());
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

    /// The demo's bare workspace, emptied of agents, on a machine whose backend can start a fresh
    /// agent there when `fresh` (intent 0591, K1-105).
    fn bare_workspace(a: &mut App, fresh: bool) -> String {
        let i = a
            .board
            .cards
            .iter()
            .position(|c| {
                c.source == crate::model::Source::Workspace
                    && c.is_open_workspace()
                    && a.account_feed(c.machine_index).is_some()
            })
            .expect("the demo has a bare workspace on a machine with accounts");
        a.board.cards[i].agents.clear();
        let mi = a.board.cards[i].machine_index;
        let mut starts: Vec<String> = STARTS_ON.iter().map(|s| s.to_string()).collect();
        if fresh {
            starts.push("account-start".into());
        }
        a.machines[mi].feed.as_mut().unwrap().starts_on = starts;
        a.board.cards[i].key.clone()
    }

    /// Intent 0591, K1-105: s on an empty workspace with no intent starts a fresh agent there on
    /// the account picked, and goes in while you're still on it.
    #[test]
    fn s_on_an_empty_workspace_starts_a_fresh_agent_there_and_goes_in() {
        let mut a = app();
        let k = bare_workspace(&mut a, true);
        let card = a.card(&k).unwrap().clone();
        select(&mut a, &k);
        a.s_on_card(card.clone());
        let (start, choices) = start_screen(&a);
        assert_eq!(start.how, StartHow::Fresh);
        assert_eq!(start.machine, card.machine_index);
        assert_eq!(choices.last().map(String::as_str), Some("list"));
        let at = choices.iter().position(|c| c != "list").unwrap();
        let code = choices[at].clone();
        for _ in 0..at {
            key(&mut a, KeyCode::Down);
        }
        let Action::Mutate { machine, request } = key(&mut a, KeyCode::Enter) else {
            panic!("nothing sent: {:?}", a.message())
        };
        assert_eq!(machine, card.machine_index);
        assert_eq!(
            request,
            json!({"action":"account-start","session":card.session,"workspace":card.workspace_id,"to":code})
        );
        assert_eq!(a.working_on(&request), "Starting a fresh agent…");
        let session = card.session.clone().unwrap();
        let workspace = card.workspace_id.clone().unwrap();
        let reply = json!({"ok":true,"session":session,"workspace":workspace,"label":"scratch","tab":"t9","message":"Started a new Claude Code agent in scratch on ARK_C."});
        a.finish_action(
            machine,
            crate::actions::with_op(request.clone()),
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
        let open = a.open_after.take().expect("went in");
        assert_eq!(
            (open.session.as_str(), open.workspace.as_str()),
            (session.as_str(), workspace.as_str())
        );
        assert_eq!(open.tab.as_deref(), Some("t9"));
        assert_eq!(open.from, k);
        // Gone on to another card meanwhile: said, with how to go in, and you stay put.
        select(&mut a, &k);
        a.s_on_card(card.clone());
        let Action::Mutate { machine, request } = key(&mut a, KeyCode::Enter) else {
            panic!()
        };
        let other = a
            .board
            .cards
            .iter()
            .find(|c| c.key != k)
            .unwrap()
            .key
            .clone();
        select(&mut a, &other);
        a.finish_action(
            machine,
            crate::actions::with_op(request),
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
        assert!(a.open_after.is_none());
        let (said, tone) = a.message().unwrap();
        assert_eq!(
            said,
            "Started a new Claude Code agent in scratch on ARK_C. ⏎ on its card goes in."
        );
        assert_eq!(tone, Tone::Good);
    }

    /// Intent 0591, K1-105: a backend that can't start a fresh agent there is never sent one; s
    /// says why as before. Nor is one started beside something running in it.
    #[test]
    fn an_empty_workspace_on_an_older_backend_keeps_the_refusal() {
        let mut a = app();
        let k = bare_workspace(&mut a, false);
        let card = a.card(&k).unwrap().clone();
        assert!(
            matches!(a.run_state(&card), RunState::Unavailable(why) if why.contains("no intent to start"))
        );
        let mut a = app();
        let k = bare_workspace(&mut a, true);
        let mut card = a.card(&k).unwrap().clone();
        card.workspace_id = None;
        assert!(matches!(a.run_state(&card), RunState::Unavailable(_)));
    }

    /// Codex's review: the yes to a start's question, given after its card moved home, starts it
    /// where it lives now on the account he picked, never on its owner's list.
    #[test]
    fn a_start_on_an_account_asked_again_after_its_card_moved_keeps_the_account() {
        let mut a = app();
        let k = backlog_card(&a);
        in_feed(&mut a, &k, |c| c.kickoff = None);
        select(&mut a, &k);
        press(&mut a, 's');
        let (machine, request) = choose(&mut a, "ARK_C");
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["account"], "ARK_C");
        on_its_way(&mut a, machine, &request);
        answered(
            &mut a,
            &request,
            json!({"ok":false,"confirmation":true,"message":"web has 3 workspaces. Start anyway?","used":3,"limit":3,"session":"web","confirm":"t-1"}),
        );
        assert!(matches!(a.mode, Mode::Confirm { .. }), "{:?}", a.message());
        // Its home moved meanwhile, to another machine that starts work on its accounts.
        let other = (0..a.machines.len())
            .find(|&m| m != machine && a.account(m, "ARK_C").is_some())
            .expect("another machine with ARK_C");
        a.machines[other].feed.as_mut().unwrap().starts_on =
            STARTS_ON.iter().map(|s| s.to_string()).collect();
        let name = a.machines[other].name();
        in_feed(&mut a, &k, |c| {
            c.home = Some(crate::feed::Home {
                machine: Some(name.clone()),
                session: Some("web".into()),
            })
        });
        if let Mode::Confirm { ready_at, .. } = &mut a.mode {
            *ready_at = None;
        }
        let Action::Mutate {
            machine: to,
            request: again,
        } = press(&mut a, 'y')
        else {
            panic!("the yes sent nothing: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(to, other);
        assert_eq!(again["action"], "dispatch");
        assert_eq!(again["account"], "ARK_C");
        assert!(again.get("confirm").is_none() && again.get("override").is_none());
    }
}
