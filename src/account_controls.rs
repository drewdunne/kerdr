//! Account screens keep selections and confirmations bound to a machine and a conversation.
use super::*;
use crate::accounts::{AccountAgent, Owner};
use crate::actions::Response;
use crate::feed::Feed;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Account(String),
    /// One copy of an account's sign-in on this machine: its code, then the copy's id.
    Copy(String, String),
    Owner(String),
}

impl Row {
    fn code(&self) -> Option<&str> {
        match self {
            Row::Account(code) | Row::Copy(code, _) => Some(code),
            Row::Owner(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MoveTarget {
    pub machine: usize,
    pub key: String,
    pub reference: String,
    pub agent: AccountAgent,
}

#[derive(Clone, Debug)]
pub struct Signin {
    pub deferred: Option<KeyCode>,
    pub restart: bool,
    pub code: String,
    /// The copy being signed in (its id and label), when the account has copies.
    pub copy: Option<(String, String)>,
    pub machines: Vec<usize>,
    pub current: usize,
    pub ticket: Option<String>,
    pub url: Option<String>,
    pub needs_code: bool,
    pub page_code: String,
    pub editing: bool,
    pub poll_at: Instant,
    pub failed: bool,
    pub message: String,
}

/// Names the copy a request is for; no copy means the account's default copy, as before.
fn for_copy(mut request: Value, copy: Option<&str>) -> Value {
    if let Some(copy) = copy {
        request["copy"] = json!(copy);
    }
    request
}

impl Signin {
    fn start(&self) -> Value {
        for_copy(
            json!({"action":"account-signin","code":self.code,"step":"start"}),
            self.copy.as_ref().map(|(id, _)| id.as_str()),
        )
    }
}

/// A typed credential is masked and excluded from Debug output.
#[derive(Clone, Default)]
pub struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, Debug)]
pub enum AccountScreen {
    PasteToken {
        machine: usize,
        code: String,
        copy: Option<String>,
        token: Secret,
        submit: bool,
    },
    List {
        machine: usize,
        rows: Vec<Row>,
        sel: usize,
        model: usize,
    },
    Move {
        target: Box<MoveTarget>,
        choices: Vec<String>,
        sel: usize,
    },
    Models {
        machine: usize,
        owner: Owner,
        models: Vec<String>,
        sel: usize,
        move_waiting: bool,
    },
    AddModel {
        machine: usize,
        choices: Vec<String>,
        sel: usize,
    },
    Add {
        machine: usize,
        code: String,
        name: String,
        field: usize,
        machines: Vec<usize>,
        chosen: Vec<bool>,
    },
    Rename {
        machine: usize,
        code: String,
        name: String,
    },
    Signin(Signin),
    Confirm {
        machine: usize,
        request: Value,
        message: String,
        yes: String,
        no: String,
        target: Option<Box<MoveTarget>>,
        ready_at: Instant,
    },
}

#[derive(Clone, Debug)]
pub struct Pending {
    pub machine: usize,
    pub request: Value,
    pub screen: Option<AccountScreen>,
}

impl App {
    fn queue_waiting_moves(&mut self, owner: &str) {
        self.account_batch_messages.clear();
        for (mi, m) in self
            .machines
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_shown() && m.machine.feed.is_some())
        {
            if m.conn.is_live() {
                self.account_queue
                    .push_back((mi, json!({"action":"account-move-waiting","owner":owner})));
            } else {
                self.account_batch_messages
                    .push(format!("{}: offline, not moved", m.name()));
            }
        }
        if self.account_queue.is_empty() && !self.account_batch_messages.is_empty() {
            self.say(self.account_batch_messages.join("; "), Tone::Warn);
        }
    }

    fn saved_models(&mut self, machine: usize, request: &Value) {
        let id = request["owner"].as_str().unwrap_or("");
        if let Some(feed) = self.machines.get_mut(machine).and_then(|m| m.feed.as_mut()) {
            let Some(index) = feed.owners.iter().position(|o| o.id == id) else {
                return;
            };
            let inherit = request["inherit"] == true;
            let models: Vec<String> = if inherit {
                feed.owners
                    .iter()
                    .find(|o| Some(&o.id) == feed.owners[index].parent.as_ref())
                    .map(|o| o.models.clone())
                    .unwrap_or_default()
            } else {
                request["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            };
            feed.owners[index].models = models.clone();
            feed.owners[index].own = !inherit;
            for owner in &mut feed.owners {
                if !owner.own && owner.parent.as_deref() == Some(id) {
                    owner.models = models.clone();
                }
            }
        }
    }
    pub fn account_feed(&self, machine: usize) -> Option<&Feed> {
        self.machines.get(machine)?.feed.as_ref()
    }

    pub fn account_machine(&self) -> Option<usize> {
        self.machine
            .as_ref()
            .and_then(|n| self.machine_index(n))
            .or_else(|| self.selected().map(|c| c.machine_index))
            .or_else(|| {
                self.machines.iter().position(|m| {
                    m.is_shown() && m.feed.as_ref().is_some_and(|f| !f.accounts.is_empty())
                })
            })
    }

    pub fn account_agent(&self, card: &Card) -> Option<&AccountAgent> {
        let feed = self.account_feed(card.machine_index)?;
        card.agents.iter().find_map(|a| {
            feed.agents.iter().find(|f| {
                f.pane == a.pane_id && Some(f.session.as_str()) == card.session.as_deref()
            })
        })
    }

    /// The agent s moves: the one the feed records on this card, or else the card's Claude Code
    /// agent with nothing recorded yet. The engine moves that by its pane, takes its conversation
    /// from herdr and records it from then on; moving is how an account gets set (intent 0264).
    pub fn move_agent(&self, card: &Card) -> Option<AccountAgent> {
        if let Some(agent) = self.account_agent(card) {
            return Some(agent.clone());
        }
        self.account_feed(card.machine_index)?;
        let session = card.session.clone()?;
        card.agents
            .iter()
            .find(|a| a.agent == "claude" && !a.pane_id.is_empty())
            .map(|a| AccountAgent {
                pane: a.pane_id.clone(),
                session,
                ..AccountAgent::default()
            })
    }

    pub fn account_owner(&self, machine: usize, session: &str) -> Option<&Owner> {
        let f = self.account_feed(machine)?;
        f.owners
            .iter()
            .find(|o| o.sessions.iter().any(|s| s == session))
            .or_else(|| {
                (session == "default")
                    .then(|| {
                        f.owners
                            .iter()
                            .find(|o| Some(&o.id) == f.machine_owner.as_ref())
                    })
                    .flatten()
            })
    }

    pub fn account_rows(&self, machine: usize) -> Vec<Row> {
        let Some(feed) = self.account_feed(machine) else {
            return vec![];
        };
        let mut rows: Vec<Row> = Vec::new();
        for a in &feed.accounts {
            let copies = a.shown_copies();
            if copies.is_empty() {
                rows.push(Row::Account(a.code.clone()));
            } else {
                // The copy lines are the stops: each one signs in and tests on its own.
                rows.extend(
                    copies
                        .iter()
                        .map(|c| Row::Copy(a.code.clone(), c.id.clone())),
                );
            }
        }
        for o in feed.owners.iter().filter(|o| o.parent.is_none()) {
            rows.push(Row::Owner(o.id.clone()));
            rows.extend(
                feed.owners
                    .iter()
                    .filter(|t| t.parent.as_ref() == Some(&o.id))
                    .map(|t| Row::Owner(t.id.clone())),
            );
        }
        for o in &feed.owners {
            if !rows.contains(&Row::Owner(o.id.clone())) {
                rows.push(Row::Owner(o.id.clone()));
            }
        }
        rows
    }

    fn push_account_screen(&mut self, screen: AccountScreen) {
        self.mode = Mode::Accounts {
            screen,
            back: Box::new(self.mode.clone()),
        };
    }

    pub(super) fn refresh_accounts(&mut self) {
        fn refresh(app: &App, mode: &mut Mode) {
            if let Mode::Accounts { screen, back } = mode {
                if let AccountScreen::List {
                    machine, rows, sel, ..
                } = screen
                {
                    let selected = rows.get(*sel).cloned();
                    *rows = app.account_rows(*machine);
                    *sel = selected
                        .and_then(|r| {
                            rows.iter().position(|x| x == &r).or_else(|| {
                                let code = r.code()?;
                                rows.iter().position(|x| x.code() == Some(code))
                            })
                        })
                        .unwrap_or((*sel).min(rows.len().saturating_sub(1)));
                }
                refresh(app, back);
            }
        }
        let mut mode = std::mem::replace(&mut self.mode, Mode::Board);
        refresh(self, &mut mode);
        self.mode = mode;
    }

    pub fn open_accounts(&mut self) {
        let machine = self.account_machine().unwrap_or(0);
        let rows = self.account_rows(machine);
        self.push_account_screen(AccountScreen::List {
            machine,
            rows,
            sel: 0,
            model: 0,
        });
    }

    fn viewed_owner(&self) -> Option<(usize, Owner)> {
        // S belongs to the viewed session, never whichever card happens to be selected.
        if let Scope::Session(machine, session) = self.scope() {
            let mi = self.machine_index(machine)?;
            return Some((mi, self.account_owner(mi, session)?.clone()));
        }
        None
    }

    fn target_current(&self, target: &MoveTarget) -> bool {
        self.card(&target.key)
            .filter(|c| c.machine_index == target.machine)
            .and_then(|c| self.move_agent(c))
            .is_some_and(|a| {
                a.pane == target.agent.pane
                    && a.session == target.agent.session
                    && a.conversation == target.agent.conversation
                    && a.account == target.agent.account
            })
    }

    fn send_account(&mut self, machine: usize, request: Value) -> Action {
        if !self.can_write(machine) {
            if request["action"] == "account-signin"
                && let Mode::Accounts {
                    screen: AccountScreen::Signin(s),
                    ..
                } = &mut self.mode
            {
                s.failed = true;
                s.restart = false;
                s.message =
                    "This machine is not answering. Reconnect, then press r to try sign-in again."
                        .into();
            }
            return Action::None;
        }
        self.account_pending = Some(Pending {
            machine,
            request: crate::actions::request_metadata(&request),
            screen: match &self.mode {
                Mode::Accounts { screen, .. } => {
                    let mut snapshot = screen.clone();
                    match &mut snapshot {
                        AccountScreen::PasteToken { token, .. } => token.0.clear(),
                        AccountScreen::Signin(s) => s.page_code.clear(),
                        _ => {}
                    }
                    Some(snapshot)
                }
                _ => None,
            },
        });
        Action::Mutate { machine, request }
    }

    fn begin_signin(
        &mut self,
        code: String,
        copy: Option<(String, String)>,
        machines: Vec<usize>,
    ) -> Action {
        let Some(&machine) = machines.first() else {
            return Action::None;
        };
        let signin = Signin {
            deferred: None,
            restart: false,
            code,
            copy,
            machines,
            current: 0,
            ticket: None,
            url: None,
            needs_code: false,
            page_code: String::new(),
            editing: false,
            poll_at: Instant::now() + Duration::from_secs(2),
            failed: false,
            message: "Starting sign-in…".into(),
        };
        let request = signin.start();
        self.push_account_screen(AccountScreen::Signin(signin));
        let action = self.send_account(machine, request);
        if action == Action::None
            && let Mode::Accounts {
                screen: AccountScreen::Signin(s),
                ..
            } = &mut self.mode
        {
            s.failed = true;
            s.message =
                "This machine is not answering. Reconnect, then press r to try sign-in again."
                    .into();
        }
        action
    }

    pub(super) fn account_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Mode::Accounts { screen, back } = self.mode.clone() {
            return Some(self.account_screen_key(key, screen, back));
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        if !matches!(
            self.mode,
            Mode::Board | Mode::Peek { .. } | Mode::Backlog { .. } | Mode::Misc { .. }
        ) {
            return None;
        }
        match key.code {
            KeyCode::Char('A') => self.open_accounts(),
            KeyCode::Char('S') => {
                if let Some((machine, owner)) = self.viewed_owner() {
                    self.push_account_screen(AccountScreen::Models {
                        machine,
                        models: owner.models.clone(),
                        owner,
                        sel: 0,
                        move_waiting: false,
                    });
                } else {
                    self.say(
                        "Choose a session with [ ] first; its owner sets the list.",
                        Tone::Info,
                    );
                }
            }
            KeyCode::Char('s') => {
                if let Some(card) = self.selected().cloned() {
                    if let Some(agent) = self.move_agent(&card) {
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
                                agent,
                            }),
                            choices,
                            sel: 0,
                        });
                    } else if card.agents.is_empty() {
                        self.say("This card has no agent running to move.", Tone::Info);
                    } else if self.account_feed(card.machine_index).is_none() {
                        self.say(
                            "This card's machine hasn't sent its accounts yet.",
                            Tone::Info,
                        );
                    } else {
                        self.say(
                            "s moves Claude Code agents; this card has none.",
                            Tone::Info,
                        );
                    }
                }
            }
            _ => return None,
        }
        Some(Action::None)
    }

    fn account_screen_key(
        &mut self,
        key: KeyEvent,
        mut screen: AccountScreen,
        back: Box<Mode>,
    ) -> Action {
        if key.code == KeyCode::Esc {
            if let AccountScreen::Signin(s) = &screen
                && let (Some(&machine), Some(ticket)) =
                    (s.machines.get(s.current), s.ticket.as_ref())
            {
                self.account_queue.push_back((
                    machine,
                    json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                ));
            }
            self.mode = *back;
            return Action::None;
        }
        // Keep sign-in input responsive while a background poll finishes.
        if self.busy {
            if let AccountScreen::PasteToken { submit, .. } = &mut screen
                && self
                    .account_pending
                    .as_ref()
                    .is_some_and(|p| p.request["step"] == "cancel")
                && key.code == KeyCode::Enter
            {
                *submit = true;
                self.mode = Mode::Accounts { screen, back };
                return Action::None;
            }
            let typing_poll = self
                .account_pending
                .as_ref()
                .is_some_and(|p| p.request["step"] == "poll")
                && matches!(&screen, AccountScreen::Signin(_));
            if typing_poll
                && let AccountScreen::Signin(s) = &mut screen
                && (key.code == KeyCode::Enter
                    || (!s.editing && matches!(key.code, KeyCode::Char('p' | 'r'))))
            {
                s.deferred = Some(key.code);
                self.mode = Mode::Accounts { screen, back };
                return Action::None;
            }
            if !typing_poll {
                return Action::None;
            }
        }
        if key.kind == KeyEventKind::Repeat
            && !matches!(
                &screen,
                AccountScreen::Add { .. }
                    | AccountScreen::Rename { .. }
                    | AccountScreen::PasteToken { .. }
                    | AccountScreen::Signin(_)
            )
        {
            return Action::None;
        }
        let mut request: Option<(usize, Value)> = None;
        match &mut screen {
            AccountScreen::PasteToken {
                machine,
                code,
                copy,
                token,
                ..
            } => match key.code {
                KeyCode::Backspace => {
                    token.0.pop();
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL) && token.0.len() < 8192 =>
                {
                    token.0.push(c)
                }
                KeyCode::Enter if !token.0.trim().is_empty() => {
                    let request = for_copy(
                        json!({"action":"account-signin","code":code,"step":"paste","token":token.0.trim()}),
                        copy.as_deref(),
                    );
                    token.0.clear();
                    let machine = *machine;
                    self.mode = *back;
                    if let Mode::Accounts {
                        screen: AccountScreen::Signin(s),
                        ..
                    } = &mut self.mode
                    {
                        s.failed = false;
                    }
                    return self.send_account(machine, request);
                }
                _ => {}
            },
            AccountScreen::List {
                machine,
                rows,
                sel,
                model,
            } => {
                let row = rows.get(*sel).cloned();
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        *sel = (*sel + 1).min(rows.len().saturating_sub(1));
                        *model = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        *sel = sel.saturating_sub(1);
                        *model = 0;
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        let available: Vec<_> = self
                            .machines
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m.is_shown() && m.feed.is_some())
                            .map(|(i, _)| i)
                            .collect();
                        if !available.is_empty() {
                            let i = available.iter().position(|i| i == machine).unwrap_or(0);
                            let n = if key.code == KeyCode::BackTab {
                                (i + available.len() - 1) % available.len()
                            } else {
                                (i + 1) % available.len()
                            };
                            *machine = available[n];
                            *rows = self.account_rows(*machine);
                            *sel = 0;
                            *model = 0;
                        }
                    }
                    KeyCode::Char('+') => {
                        let machines: Vec<_> = self
                            .machines
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m.is_shown() && m.machine.feed.is_some())
                            .map(|(i, _)| i)
                            .collect();
                        let chosen = machines
                            .iter()
                            .map(|&i| self.machines[i].conn.is_live())
                            .collect();
                        self.push_account_screen(AccountScreen::Add {
                            machine: *machine,
                            code: String::new(),
                            name: String::new(),
                            field: 0,
                            machines,
                            chosen,
                        });
                        return Action::None;
                    }
                    _ => {
                        let copy_id = match &row {
                            Some(Row::Copy(_, id)) => Some(id.clone()),
                            _ => None,
                        };
                        match row {
                            Some(Row::Account(code) | Row::Copy(code, _)) => {
                                let a = self
                                    .account_feed(*machine)
                                    .and_then(|f| f.accounts.iter().find(|a| a.code == code))
                                    .cloned();
                                let copy = copy_id.and_then(|id| {
                                    let c = a.as_ref()?.copy(&id)?;
                                    Some((c.id.clone(), c.name().to_string()))
                                });
                                if let Some(a) = a {
                                    match key.code {
                                        KeyCode::Enter => {
                                            return self.begin_signin(code, copy, vec![*machine]);
                                        }
                                        KeyCode::Char('t') => {
                                            request = Some((
                                                *machine,
                                                for_copy(
                                                    json!({"action":"account-test","code":code}),
                                                    copy.as_ref().map(|(id, _)| id.as_str()),
                                                ),
                                            ))
                                        }
                                        KeyCode::Char('e') => {
                                            self.push_account_screen(AccountScreen::Rename {
                                                machine: *machine,
                                                code,
                                                name: a.name,
                                            });
                                            return Action::None;
                                        }
                                        KeyCode::Char('x') => {
                                            self.push_account_screen(AccountScreen::Confirm { machine:*machine, request:json!({"action":"account-remove","code":code}), message:format!("Remove {} ({})?",a.code,a.name), yes:"remove".into(), no:"keep it".into(), target:None, ready_at:Instant::now()+Duration::from_millis(500) });
                                            return Action::None;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some(Row::Owner(id)) => {
                                if let Some(owner) = self
                                    .account_feed(*machine)
                                    .and_then(|f| f.owners.iter().find(|o| o.id == id))
                                    .cloned()
                                {
                                    let mut models = owner.models.clone();
                                    *model = (*model).min(models.len().saturating_sub(1));
                                    match key.code {
                                        KeyCode::Left | KeyCode::Char('h') => {
                                            *model = model.saturating_sub(1)
                                        }
                                        KeyCode::Right | KeyCode::Char('l') => {
                                            *model =
                                                (*model + 1).min(models.len().saturating_sub(1))
                                        }
                                        KeyCode::Char('S' | 'a') => {
                                            self.push_account_screen(AccountScreen::Models {
                                                machine: *machine,
                                                owner,
                                                models,
                                                sel: *model,
                                                move_waiting: false,
                                            });
                                            if key.code == KeyCode::Char('a') {
                                                return self
                                                    .account_key(KeyEvent::new(
                                                        KeyCode::Char('a'),
                                                        KeyModifiers::NONE,
                                                    ))
                                                    .unwrap_or(Action::None);
                                            }
                                            return Action::None;
                                        }
                                        KeyCode::Backspace
                                        | KeyCode::Enter
                                        | KeyCode::Char('J' | 'K' | 'd') => {
                                            let key = key.code;
                                            // Edit a visible draft; no central write until Save.
                                            if key == KeyCode::Enter && *model < models.len() {
                                                let code = models.remove(*model);
                                                models.insert(0, code);
                                                *model = 0;
                                            }
                                            self.push_account_screen(AccountScreen::Models {
                                                machine: *machine,
                                                owner,
                                                models,
                                                sel: *model,
                                                move_waiting: false,
                                            });
                                            if key != KeyCode::Enter {
                                                return self
                                                    .account_key(KeyEvent::new(
                                                        key,
                                                        KeyModifiers::NONE,
                                                    ))
                                                    .unwrap_or(Action::None);
                                            }
                                            return Action::None;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            AccountScreen::Move {
                target,
                choices,
                sel,
            } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    if self.target_current(target) {
                        if let Some(to) = choices.get(*sel) {
                            request = Some((
                                target.machine,
                                json!({"action":"account-move","pane":target.agent.pane,"session":target.agent.session,"conversation":target.agent.conversation,"to":to}),
                            ));
                        }
                    } else {
                        self.say(
                            "That conversation changed. Open the move picker again.",
                            Tone::Warn,
                        );
                    }
                }
                _ => {}
            },
            AccountScreen::Models {
                machine,
                owner,
                models,
                sel,
                move_waiting,
            } => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        *sel = (*sel + 1).min(models.len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                    KeyCode::Char('J') if *sel + 1 < models.len() => {
                        models.swap(*sel, *sel + 1);
                        *sel += 1;
                    }
                    KeyCode::Char('K') if *sel > 0 && *sel < models.len() => {
                        models.swap(*sel, *sel - 1);
                        *sel -= 1;
                    }
                    KeyCode::Char('d') if models.len() > 1 && *sel < models.len() => {
                        models.remove(*sel);
                        *sel = (*sel).min(models.len() - 1);
                    }
                    KeyCode::Char(' ') => *move_waiting = !*move_waiting,
                    KeyCode::Char('a') => {
                        let choices = self
                            .account_feed(*machine)
                            .into_iter()
                            .flat_map(|f| &f.accounts)
                            .filter(|a| !models.contains(&a.code))
                            .map(|a| a.code.clone())
                            .collect();
                        self.push_account_screen(AccountScreen::AddModel {
                            machine: *machine,
                            choices,
                            sel: 0,
                        });
                        return Action::None;
                    }
                    KeyCode::Backspace if owner.parent.is_some() && owner.own => {
                        self.push_account_screen(AccountScreen::Confirm {
                        machine: *machine,
                        request: json!({"action":"account-models","owner":owner.id,"inherit":true}),
                        message: format!("Make {} follow its organization's model list?", owner.name),
                        yes: "follow organization".into(), no: "keep this list".into(), target: None,
                        ready_at: Instant::now() + Duration::from_millis(500),
                    });
                        return Action::None;
                    }
                    KeyCode::Enter if !models.is_empty() => {
                        if *models == owner.models && (owner.own || owner.parent.is_none()) {
                            self.say("Model list unchanged.", Tone::Info);
                            if *move_waiting {
                                self.queue_waiting_moves(&owner.id);
                            }
                            self.mode = *back;
                            return Action::None;
                        }
                        let value =
                            if *models == owner.models && !owner.own && owner.parent.is_some() {
                                json!({"action":"account-models","owner":owner.id,"inherit":true})
                            } else {
                                json!({"action":"account-models","owner":owner.id,"models":models})
                            };
                        request = Some((*machine, value));
                    }
                    _ => {}
                }
            }
            AccountScreen::AddModel { choices, sel, .. } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    let mut restored = *back;
                    if let (
                        Some(code),
                        Mode::Accounts {
                            screen: AccountScreen::Models { models, sel, .. },
                            ..
                        },
                    ) = (choices.get(*sel), &mut restored)
                    {
                        models.push(code.clone());
                        *sel = models.len() - 1;
                    }
                    self.mode = restored;
                    return Action::None;
                }
                _ => {}
            },
            AccountScreen::Add {
                machine,
                code,
                name,
                field,
                machines,
                chosen,
            } => {
                let fields = 2 + machines.len();
                match key.code {
                    KeyCode::Tab => *field = (*field + 1) % fields,
                    KeyCode::BackTab => *field = (*field + fields - 1) % fields,
                    KeyCode::Char(' ') if *field >= 2 => chosen[*field - 2] = !chosen[*field - 2],
                    KeyCode::Backspace if *field < 2 => {
                        if *field == 0 {
                            code.pop();
                        } else {
                            name.pop();
                        }
                    }
                    KeyCode::Char(c)
                        if *field < 2 && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        let value = if *field == 0 { code } else { name };
                        if value.len() < 80 {
                            value.push(c);
                        }
                    }
                    KeyCode::Enter => {
                        if code.is_empty()
                            || !(2..=4).contains(&code.len())
                            || !code.chars().all(|c| c.is_ascii_uppercase())
                            || name.trim().is_empty()
                            || !chosen.contains(&true)
                        {
                            self.say("Use a code of 2–4 uppercase letters, a name, and at least one machine.",Tone::Warn);
                        } else if self
                            .machines
                            .iter()
                            .filter_map(|m| m.feed.as_ref())
                            .any(|f| f.accounts.iter().any(|a| &a.code == code))
                        {
                            self.say(
                                "That code exists. Select its account and press e to rename it.",
                                Tone::Warn,
                            );
                        } else {
                            request = Some((
                                *machine,
                                json!({"action":"account-add","code":code,"name":name.trim(),"provider":"claude","kind":"token"}),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            AccountScreen::Rename {
                machine,
                code,
                name,
            } => match key.code {
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL) && name.len() < 80 =>
                {
                    name.push(c)
                }
                KeyCode::Enter if !name.trim().is_empty() => {
                    if let Some(a) = self
                        .account_feed(*machine)
                        .and_then(|f| f.accounts.iter().find(|a| &a.code == code))
                    {
                        request = Some((
                            *machine,
                            json!({"action":"account-add","code":code,"name":name.trim(),"provider":a.provider,"kind":a.kind}),
                        ));
                    }
                }
                _ => {}
            },
            AccountScreen::Signin(s) => {
                let Some(&machine) = s.machines.get(s.current) else {
                    self.mode = *back;
                    return Action::None;
                };
                match key.code {
                    KeyCode::Tab if s.needs_code => s.editing = !s.editing,
                    KeyCode::Char('p') if !s.editing => {
                        if let Some(ticket) = &s.ticket {
                            self.account_queue.push_back((
                                machine,
                                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                            ));
                        }
                        s.url = None;
                        s.failed = true;
                        s.message =
                            "Browser sign-in cancelled. Paste a token, or r to start again.".into();
                        let code = s.code.clone();
                        let copy = s.copy.as_ref().map(|(id, _)| id.clone());
                        self.mode = Mode::Accounts { screen, back };
                        self.push_account_screen(AccountScreen::PasteToken {
                            machine,
                            code,
                            copy,
                            token: Secret::default(),
                            submit: false,
                        });
                        return Action::None;
                    }
                    KeyCode::Char('c') if !s.editing => {
                        return s.url.clone().map_or(Action::None, Action::CopyLink);
                    }
                    KeyCode::Char('o') if !s.editing => {
                        return s.url.clone().map_or(Action::None, Action::OpenLink);
                    }
                    KeyCode::Char('r') if !s.editing && s.failed => {
                        s.failed = false;
                        s.url = None;
                        s.page_code.clear();
                        s.needs_code = false;
                        if let Some(ticket) = &s.ticket {
                            s.restart = true;
                            s.message = "Stopping the previous sign-in…".into();
                            request = Some((
                                machine,
                                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                            ));
                        } else {
                            request = Some((machine, s.start()));
                        }
                    }
                    KeyCode::Backspace if s.editing => {
                        s.page_code.pop();
                    }
                    KeyCode::Char(c)
                        if s.editing
                            && s.page_code.len() < 4096
                            && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        s.page_code.push(c)
                    }
                    KeyCode::Enter if !s.failed => {
                        if let Some(ticket) = &s.ticket {
                            if s.page_code.trim().is_empty() {
                                request = Some((
                                    machine,
                                    json!({"action":"account-signin","ticket":ticket,"step":"poll"}),
                                ));
                            } else {
                                request = Some((
                                    machine,
                                    json!({"action":"account-signin","ticket":ticket,"step":"finish","page_code":s.page_code.trim()}),
                                ));
                                s.page_code.clear();
                                s.editing = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            AccountScreen::Confirm {
                machine,
                request: pending,
                target,
                ready_at,
                ..
            } => match key.code {
                KeyCode::Char('n') => {
                    self.mode = *back;
                    return Action::None;
                }
                KeyCode::Char('y') if Instant::now() >= *ready_at => {
                    if target.as_ref().is_none_or(|t| self.target_current(t)) {
                        request = Some((*machine, pending.clone()));
                        self.mode = *back;
                        if let Some((mi, r)) = request {
                            return self.send_account(mi, r);
                        }
                        return Action::None;
                    } else {
                        self.say(
                            "That conversation changed. Open the move picker again.",
                            Tone::Warn,
                        );
                        self.mode = *back;
                        return Action::None;
                    }
                }
                _ => {}
            },
        }
        self.mode = Mode::Accounts { screen, back };
        request.map_or(Action::None, |(mi, r)| self.send_account(mi, r))
    }

    pub(super) fn account_paste(&mut self, text: &str) -> bool {
        let can_type = !self.busy
            || self
                .account_pending
                .as_ref()
                .is_some_and(|p| matches!(p.request["step"].as_str(), Some("poll" | "cancel")));
        if can_type && let Mode::Accounts { screen, .. } = &mut self.mode {
            let clean = || {
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect::<String>()
            };
            match screen {
                AccountScreen::Add {
                    code, name, field, ..
                } if *field < 2 => {
                    if *field == 0 {
                        *code = clean();
                    } else {
                        *name = clean();
                    }
                    return true;
                }
                AccountScreen::Rename { name, .. } => {
                    *name = clean();
                    return true;
                }
                _ => {}
            }
        }
        if let Mode::Accounts {
            screen: AccountScreen::PasteToken { token, .. },
            ..
        } = &mut self.mode
        {
            if can_type {
                token.0 = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(8192)
                    .collect();
            }
            return true;
        }
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut self.mode
        {
            if can_type && s.needs_code {
                s.page_code = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(4096)
                    .collect();
                s.editing = true;
            }
            return true;
        }
        false
    }

    /// Called by the event loop; only polls a visible sign-in, never retries an unknown write.
    pub fn account_tick(&mut self) -> Action {
        if self.busy {
            return Action::None;
        }
        if let Mode::Accounts {
            screen: AccountScreen::PasteToken { submit, .. },
            ..
        } = &mut self.mode
            && std::mem::take(submit)
        {
            return self
                .account_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap_or(Action::None);
        }
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut self.mode
            && let Some(key) = s.deferred.take()
        {
            return self
                .account_key(KeyEvent::new(key, KeyModifiers::NONE))
                .unwrap_or(Action::None);
        }
        while let Some((mi, request)) = self.account_queue.pop_front() {
            if request["action"] == "account-signin"
                && request["step"] == "start"
                && !matches!(&self.mode,Mode::Accounts{screen:AccountScreen::Signin(s),..} if s.machines.get(s.current)==Some(&mi) && request==s.start())
            {
                continue;
            }
            let action = self.send_account(mi, request.clone());
            if action == Action::None && request["action"] == "account-move-waiting" {
                self.account_batch_messages.push(format!(
                    "{}: disconnected, not moved",
                    self.machines[mi].name()
                ));
                if self.account_queue.is_empty() {
                    self.say(self.account_batch_messages.join("; "), Tone::Warn);
                }
            }
            if action == Action::None
                && request["action"] == "account-signin"
                && request["step"] == "start"
                && let Mode::Accounts {
                    screen: AccountScreen::Signin(s),
                    ..
                } = &mut self.mode
            {
                s.failed = true;
                s.message =
                    "This machine is not answering. Reconnect, then press r to try sign-in again."
                        .into();
            }
            return action;
        }
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut self.mode
            && !s.failed
            && s.page_code.is_empty()
            && Instant::now() >= s.poll_at
            && let (Some(&mi), Some(ticket)) = (s.machines.get(s.current), s.ticket.clone())
        {
            s.poll_at = Instant::now() + Duration::from_secs(2);
            return self.send_account(
                mi,
                json!({"action":"account-signin","ticket":ticket,"step":"poll"}),
            );
        }
        Action::None
    }

    pub(super) fn finish_account_action(
        &mut self,
        machine: usize,
        request: Value,
        result: Result<Response, String>,
    ) {
        let pending = self.account_pending.take().filter(|p| {
            p.machine == machine && p.request == crate::actions::request_metadata(&request)
        });
        let visible = match (&pending, &self.mode) {
            (Some(p), Mode::Accounts { screen, .. }) => p
                .screen
                .as_ref()
                .is_some_and(|old| same_screen(old, screen)),
            _ => false,
        };
        let response = match result {
            Ok(r) => r,
            Err(e) => {
                if request["action"] == "account-move-waiting" {
                    self.account_batch_messages
                        .push(format!("{}: {e}", self.machines[machine].name()));
                    if self.account_queue.is_empty() {
                        self.say(self.account_batch_messages.join("; "), Tone::Warn);
                    }
                    return;
                }
                if visible
                    && let Mode::Accounts {
                        screen: AccountScreen::Signin(s),
                        ..
                    } = &mut self.mode
                {
                    s.failed = true;
                    s.editing = false;
                    s.message = e.clone();
                }
                self.say(e, Tone::Warn);
                return;
            }
        };
        let action = request["action"].as_str().unwrap_or("");
        if action == "account-models" && response.ok {
            self.saved_models(machine, &request);
        }
        if action == "account-move-waiting" {
            self.account_batch_messages.push(format!(
                "{}: {}",
                self.machines[machine].name(),
                response.message
            ));
            if self.account_queue.is_empty() {
                self.say(self.account_batch_messages.join("; "), Tone::Info);
            }
            return;
        }
        if action == "account-signin" {
            if request["step"] == "cancel" {
                if visible
                    && let Mode::Accounts {
                        screen: AccountScreen::Signin(s),
                        ..
                    } = &mut self.mode
                    && s.restart
                {
                    s.restart = false;
                    if response.ok {
                        s.ticket = None;
                        s.failed = false;
                        self.account_queue.push_back((machine, s.start()));
                    } else {
                        s.failed = true;
                        s.message = format!("Could not cancel sign-in: {}", response.message);
                    }
                }
                if !response.ok {
                    if let Mode::Accounts {
                        screen: AccountScreen::PasteToken { submit, .. },
                        ..
                    } = &mut self.mode
                    {
                        *submit = false;
                    }
                    self.say(
                        format!("Could not cancel sign-in: {}", response.message),
                        Tone::Warn,
                    );
                } else {
                    clear_cancelled_ticket(
                        &mut self.mode,
                        request["ticket"].as_str().unwrap_or(""),
                    );
                }
                return;
            }
            // Esc while start was in flight must not leave a login process or pop a browser later.
            if !visible {
                if request["step"] == "start"
                    && let Some(ticket) = response.ticket
                {
                    self.account_queue.push_back((
                        machine,
                        json!({"action":"account-signin","ticket":ticket,"step":"cancel"}),
                    ));
                }
                return;
            }
            if let Mode::Accounts {
                screen: AccountScreen::Signin(s),
                ..
            } = &mut self.mode
            {
                s.message = response.message.clone();
                if !response.ok || !matches!(response.status.as_deref(), Some("waiting" | "done")) {
                    s.failed = true;
                    s.editing = false;
                    if request["step"] == "start" && response.ticket.is_some() {
                        s.ticket = response.ticket;
                    }
                } else if response.status.as_deref() == Some("done") {
                    s.deferred = None;
                    s.current += 1;
                    s.ticket = None;
                    s.url = None;
                    s.page_code.clear();
                    s.editing = false;
                    s.needs_code = false;
                    if let Some(&mi) = s.machines.get(s.current) {
                        self.account_queue.push_back((mi, s.start()));
                    }
                } else if request["step"] == "start" {
                    let valid = response
                        .url
                        .as_deref()
                        .is_some_and(crate::accounts::valid_signin_url);
                    if !valid || response.ticket.as_deref().is_none_or(str::is_empty) {
                        s.failed = true;
                        s.message =
                            "The sign-in did not return a valid HTTPS link and ticket.".into();
                        if let Some(ticket) = response.ticket {
                            self.account_queue.push_back((
                                machine,
                                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                            ));
                        }
                    } else {
                        s.ticket = response.ticket;
                        s.url = response.url;
                        s.needs_code = response.needs_code;
                        self.signin_open = s.url.clone();
                    }
                }
                s.poll_at = Instant::now() + Duration::from_secs(2);
            }
            return;
        }
        if action == "account-move" && response.confirmation && !visible {
            self.say(
                "That move needs confirmation. Open the move picker again to review it.",
                Tone::Warn,
            );
            return;
        }
        if action == "account-move" && response.confirmation && visible {
            if let (
                Some(token),
                Mode::Accounts {
                    screen: AccountScreen::Move { target, .. },
                    ..
                },
            ) = (response.confirm, &self.mode)
            {
                if request["confirm"].as_str() == Some(&token) {
                    self.say("The backend repeated the same move question. Refresh the account state before trying again.", Tone::Warn);
                    return;
                }
                let target = target.clone();
                if self.target_current(&target) {
                    let mut request = request;
                    request["confirm"] = json!(token);
                    self.push_account_screen(AccountScreen::Confirm {
                        machine,
                        request,
                        message: response.message,
                        yes: response.yes.unwrap_or_else(|| "move when it stops".into()),
                        no: response.no.unwrap_or_else(|| "leave it".into()),
                        target: Some(target),
                        ready_at: Instant::now() + Duration::from_millis(500),
                    });
                    return;
                }
            }
        } else if response.ok && visible {
            match pending.and_then(|p| p.screen) {
                Some(AccountScreen::Add {
                    code,
                    machines,
                    chosen,
                    ..
                }) => {
                    let selected = machines
                        .into_iter()
                        .zip(chosen)
                        .filter_map(|(m, c)| c.then_some(m))
                        .collect::<Vec<_>>();
                    // Pop details before opening the sequence; the account now exists.
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
                    }
                    let action = self.begin_signin(code, None, selected);
                    if let Action::Mutate { machine, request } = action {
                        self.account_queue.push_back((machine, request));
                    }
                }
                Some(AccountScreen::Models {
                    owner,
                    move_waiting,
                    ..
                }) if action == "account-models" => {
                    if move_waiting {
                        self.queue_waiting_moves(&owner.id);
                    }
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
                    }
                    if move_waiting
                        && self.account_queue.is_empty()
                        && !self.account_batch_messages.is_empty()
                    {
                        return;
                    }
                }
                Some(AccountScreen::Rename { .. }) => {
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
                    }
                }
                Some(AccountScreen::Move { .. }) if action == "account-move" => {
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
                    }
                }
                _ => {}
            }
        }
        self.say(
            response.message,
            if response.ok { Tone::Good } else { Tone::Warn },
        );
    }
}

fn clear_cancelled_ticket(mode: &mut Mode, ticket: &str) {
    if let Mode::Accounts { screen, back } = mode {
        if let AccountScreen::Signin(s) = screen
            && s.ticket.as_deref() == Some(ticket)
        {
            s.ticket = None;
        }
        clear_cancelled_ticket(back, ticket);
    }
}

fn same_screen(a: &AccountScreen, b: &AccountScreen) -> bool {
    match (a, b) {
        (AccountScreen::Signin(a), AccountScreen::Signin(b)) => {
            a.code == b.code
                && a.copy == b.copy
                && a.machines == b.machines
                && a.current == b.current
                && a.ticket == b.ticket
        }
        (AccountScreen::Move { target: a, .. }, AccountScreen::Move { target: b, .. }) => {
            a.key == b.key && a.machine == b.machine && a.agent == b.agent
        }
        (AccountScreen::Add { code: a, .. }, AccountScreen::Add { code: b, .. }) => a == b,
        (AccountScreen::Models { owner: a, .. }, AccountScreen::Models { owner: b, .. }) => {
            a.id == b.id
        }
        (AccountScreen::Rename { code: a, .. }, AccountScreen::Rename { code: b, .. }) => a == b,
        (AccountScreen::List { machine: a, .. }, AccountScreen::List { machine: b, .. }) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn sent(action: Action) -> (usize, Value) {
        let Action::Mutate { machine, request } = action else {
            panic!("expected action, got {action:?}")
        };
        (machine, request)
    }
    fn complete(a: &mut App, action: Action, reply: Value) {
        let (mi, r) = sent(action);
        a.finish_action(
            mi,
            r,
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }
    fn pick(a: &mut App) {
        let c = a.board.cards.iter().find(|c| c.reference == "#44").unwrap();
        a.sel = Selection {
            col: c.stage.index(),
            key: Some(c.key.clone()),
        };
    }
    fn move_question(a: &mut App) {
        pick(a);
        press(a, 's');
        let action = key(a, KeyCode::Enter);
        complete(
            a,
            action,
            json!({"ok":false,"message":"move?","confirmation":"Mid-step; restarting ends jobs.","confirm":"opaque-ticket","yes":"move when it stops","no":"leave it"}),
        );
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
    fn signin(a: &mut App) -> Action {
        a.open_accounts();
        key(a, KeyCode::Enter)
    }
    fn started() -> Value {
        json!({"ok":true,"message":"Authorize in your browser.","status":"waiting","url":"https://example.com/authorize?state=example","ticket":"login-ticket","needs_code":true})
    }

    #[test]
    fn token_paste_leaves_only_metadata_in_ui_and_result_state() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        press(&mut a, 'p');
        let cancel = a.account_tick();
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"message":"cancelled","status":"cancelled"}),
        );
        let secret = "fixture-only-private-value";
        a.on_paste(secret);
        assert!(!format!("{:?}", a.mode).contains(secret));
        let action = key(&mut a, KeyCode::Enter);
        let (mi, request) = sent(action);
        assert_eq!(request["token"], secret);
        assert_eq!(request["step"], "paste");
        assert_eq!(mi, 0);
        assert!(!format!("{:?} {:?}", a.mode, a.account_pending).contains(secret));
        let metadata = crate::actions::request_metadata(&request);
        assert!(metadata.get("token").is_none());
        a.finish_action(
            mi,
            metadata,
            Ok(Response {
                ok: true,
                message: "signed in".into(),
                status: Some("done".into()),
                ..Default::default()
            }),
        );
        let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &a.mode
        else {
            panic!()
        };
        assert_eq!(s.current, 1);
    }
    #[test]
    fn refreshing_accounts_keeps_the_selected_identity_and_adds_new_codes() {
        let mut a = app();
        a.open_accounts();
        press(&mut a, 'j');
        a.machines[0].feed.as_mut().unwrap().accounts.insert(
            0,
            crate::accounts::Account {
                code: "NEW".into(),
                ..Default::default()
            },
        );
        a.rebuild();
        let Mode::Accounts {
            screen: AccountScreen::List { rows, sel, .. },
            ..
        } = &a.mode
        else {
            panic!()
        };
        assert_eq!(rows[*sel], Row::Account("ARK".into()));
        assert!(rows.contains(&Row::Account("NEW".into())));
    }

    #[test]
    fn move_question_only_appears_after_choosing_and_uses_its_opaque_token() {
        let mut a = app();
        pick(&mut a);
        assert_eq!(press(&mut a, 's'), Action::None);
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Move { .. },
                ..
            }
        ));
        let action = key(&mut a, KeyCode::Enter);
        let (mi, r) = sent(action.clone());
        assert_eq!(mi, 1);
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK"})
        );
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"fallback","confirmation":"Mid-step","confirm":"opaque-ticket"}),
        );
        assert_eq!(press(&mut a, 'y'), Action::None);
        settle(&mut a);
        let (_, r) = sent(press(&mut a, 'y'));
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK","confirm":"opaque-ticket"})
        );
        assert!(r.get("override").is_none());
    }
    #[test]
    fn a_changed_conversation_cannot_use_an_old_confirmation() {
        let mut a = app();
        move_question(&mut a);
        settle(&mut a);
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .iter_mut()
            .find(|a| a.session == "api" && a.pane == "w1:p1")
            .unwrap()
            .conversation = "replacement".into();
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(a.message().unwrap().0.contains("conversation changed"));
    }
    #[test]
    fn s_moves_an_agent_with_no_recorded_account() {
        let mut a = app();
        pick(&mut a);
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .retain(|a| !(a.session == "api" && a.pane == "w1:p1"));
        press(&mut a, 's');
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Move { .. },
                ..
            }
        ));
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["action"], "account-move");
        assert_eq!(r["pane"], "w1:p1");
        assert_eq!(r["session"], "api");
        assert_eq!(r["conversation"], "");
    }
    #[test]
    fn s_says_why_when_a_card_has_no_claude_agent() {
        let mut a = app();
        pick(&mut a);
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .retain(|a| !(a.session == "api" && a.pane == "w1:p1"));
        let key = a.sel.key.clone().unwrap();
        for c in a.board.cards.iter_mut().filter(|c| c.key == key) {
            for g in &mut c.agents {
                g.agent = "codex".into();
            }
        }
        press(&mut a, 's');
        assert!(matches!(a.mode, Mode::Board));
        assert!(a.message().unwrap().0.contains("Claude Code"));
    }
    #[test]
    fn escaping_a_move_ignores_its_late_question() {
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Mid-step","confirmation":"Mid-step","confirm":"ticket"}),
        );
        assert!(matches!(a.mode, Mode::Board));
    }
    #[test]
    fn s_uses_the_viewed_session_not_the_selected_card() {
        let mut a = app();
        pick(&mut a);
        a.machine = Some("laptop".into());
        a.rebuild();
        a.tab = a
            .tabs
            .iter()
            .position(|s| matches!(s,Scope::Session(_,s)if s=="web"))
            .unwrap();
        press(&mut a, 'S');
        let Mode::Accounts {
            screen: AccountScreen::Models { owner, machine, .. },
            ..
        } = &a.mode
        else {
            panic!()
        };
        assert_eq!(owner.id, "shop");
        assert_eq!(*machine, 0);
    }
    #[test]
    fn saving_models_moves_waiting_only_after_success_and_on_each_machine() {
        let mut a = app();
        let owner = a.account_feed(0).unwrap().owners[0].clone();
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: vec!["KBZ".into(), "ARK".into()],
            owner,
            sel: 0,
            move_waiting: true,
        });
        let action = key(&mut a, KeyCode::Enter);
        assert!(a.account_queue.is_empty());
        complete(&mut a, action, json!({"ok":true,"message":"saved"}));
        assert_eq!(a.account_queue.len(), 2);
        for mi in [0, 1] {
            let (m, r) = sent(a.account_tick());
            assert_eq!(m, mi);
            assert_eq!(r, json!({"action":"account-move-waiting","owner":"studio"}));
            a.finish_action(
                m,
                r,
                Ok(Response {
                    ok: true,
                    message: "moved".into(),
                    ..Default::default()
                }),
            );
        }
    }
    #[test]
    fn a_failed_model_save_does_not_move_sessions() {
        let mut a = app();
        let owner = a.account_feed(0).unwrap().owners[0].clone();
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: vec!["KBZ".into()],
            owner,
            sel: 0,
            move_waiting: true,
        });
        let action = key(&mut a, KeyCode::Enter);
        complete(&mut a, action, json!({"ok":false,"message":"write failed"}));
        assert!(a.account_queue.is_empty());
    }

    #[test]
    fn unchanged_inherited_models_stay_inherited_when_moving_waiters() {
        let mut a = app();
        let owner = a
            .account_feed(0)
            .unwrap()
            .owners
            .iter()
            .find(|o| o.id == "shop")
            .unwrap()
            .clone();
        assert!(!owner.own);
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: owner.models.clone(),
            owner,
            sel: 0,
            move_waiting: true,
        });
        let action = key(&mut a, KeyCode::Enter);
        let (_, request) = sent(action.clone());
        assert_eq!(
            request,
            json!({"action":"account-models","owner":"shop","inherit":true})
        );
        complete(&mut a, action, json!({"ok":true,"message":"unchanged"}));
        assert_eq!(a.account_queue.len(), 2);
        assert!(
            !a.account_feed(0)
                .unwrap()
                .owners
                .iter()
                .find(|o| o.id == "shop")
                .unwrap()
                .own
        );
    }

    #[test]
    fn owner_row_reorders_and_drops_edit_one_draft_before_saving() {
        let mut a = app();
        a.open_accounts();
        if let Mode::Accounts {
            screen: AccountScreen::List { sel, rows, .. },
            ..
        } = &mut a.mode
        {
            *sel = rows
                .iter()
                .position(|r| *r == Row::Owner("studio".into()))
                .unwrap();
        }
        assert_eq!(press(&mut a, 'J'), Action::None);
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Models { models, sel: 1, .. }, .. } if models == &["KBZ", "ARK"])
        );
        assert_eq!(press(&mut a, 'd'), Action::None);
        let (_, request) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(request["models"], json!(["KBZ"]));
    }

    #[test]
    fn account_screen_preserves_a_misc_peek_across_refresh() {
        let mut a = app();
        press(&mut a, 'i');
        key(&mut a, KeyCode::Char(' '));
        assert!(matches!(a.mode, Mode::Peek { .. }));
        let target = a.sel.key.clone();
        a.open_accounts();
        a.rebuild();
        key(&mut a, KeyCode::Esc);
        assert!(matches!(a.mode, Mode::Peek { .. }));
        assert_eq!(a.sel.key, target);
    }

    #[test]
    fn retry_cancels_the_existing_ticket_before_starting_again() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        let poll = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            poll,
            json!({"ok":false,"status":"failed","message":"wrong code"}),
        );
        let cancel = press(&mut a, 'r');
        let (_, request) = sent(cancel.clone());
        assert_eq!(request["step"], "cancel");
        assert_eq!(request["ticket"], "login-ticket");
        assert!(a.account_queue.is_empty());
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        let (_, request) = sent(a.account_tick());
        assert_eq!(request["step"], "start");
    }

    #[test]
    fn enter_during_poll_is_sent_after_the_poll_and_typing_suspends_polling() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        let poll = key(&mut a, KeyCode::Enter);
        a.busy = true;
        a.on_paste("page-code");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"waiting","message":"waiting"}),
        );
        let (_, request) = sent(a.account_tick());
        assert_eq!(request["step"], "finish");
        assert_eq!(request["page_code"], "page-code");
        a.on_paste("another-code");
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now() - Duration::from_secs(1);
        }
        assert_eq!(a.account_tick(), Action::None);
    }
    #[test]
    fn add_model_edits_the_draft_and_escape_sends_nothing() {
        let mut a = app();
        let owner = a.account_feed(0).unwrap().owners[0].clone();
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: vec!["ARK".into()],
            owner,
            sel: 0,
            move_waiting: false,
        });
        press(&mut a, 'a');
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let Mode::Accounts {
            screen: AccountScreen::Models { models, .. },
            ..
        } = &a.mode
        else {
            panic!()
        };
        assert_eq!(models, &["ARK", "KBZ"]);
        assert_eq!(key(&mut a, KeyCode::Esc), Action::None);
    }
    #[test]
    fn sign_in_is_machine_bound_and_copies_the_link_only_when_requested() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        assert!(a.signin_open.is_some());
        let copy = press(&mut a, 'c');
        assert!(matches!(copy, Action::CopyLink(_)));
        a.on_paste("page-code\r\n");
        let (mi, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 0);
        assert_eq!(
            r,
            json!({"action":"account-signin","step":"finish","ticket":"login-ticket","page_code":"page-code"})
        );
        let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &a.mode
        else {
            panic!()
        };
        assert!(s.page_code.is_empty());
    }
    #[test]
    fn escape_while_starting_cancels_the_late_ticket_without_opening_a_browser() {
        let mut a = app();
        let action = signin(&mut a);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        complete(&mut a, action, started());
        assert!(a.signin_open.is_none());
        let (mi, r) = sent(a.account_tick());
        assert_eq!(mi, 0);
        assert_eq!(
            r,
            json!({"action":"account-signin","step":"cancel","ticket":"login-ticket"})
        );
    }
    #[test]
    fn sequence_uses_a_new_ticket_on_each_machine_and_escape_cancels_queued_start() {
        let mut a = app();
        let action = a.begin_signin("ARK".into(), None, vec![0, 1]);
        complete(&mut a, action, started());
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None);
        let action = a.begin_signin("ARK".into(), None, vec![0, 1]);
        complete(&mut a, action, started());
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        let (mi, r) = sent(a.account_tick());
        assert_eq!(mi, 1);
        assert_eq!(
            r,
            json!({"action":"account-signin","code":"ARK","step":"start"})
        );
    }
    #[test]
    fn non_web_signin_link_is_refused_and_ticket_cancelled() {
        let mut a = app();
        let action = signin(&mut a);
        let mut reply = started();
        reply["url"] = json!("file:///tmp/script");
        complete(&mut a, action, reply);
        assert!(a.signin_open.is_none());
        assert_eq!(a.account_queue.front().unwrap().1["step"], "cancel");
    }
    #[test]
    fn unknown_signin_result_stops_polling_and_does_not_retry() {
        let mut a = app();
        let action = signin(&mut a);
        let (mi, r) = sent(action);
        a.finish_action(mi, r, Err("timed out; result unknown".into()));
        assert_eq!(a.account_tick(), Action::None);
    }
    #[test]
    fn account_lookup_distinguishes_identical_panes_in_different_sessions() {
        let mut a = app();
        pick(&mut a);
        let card = a.selected().unwrap();
        let agent = a.account_agent(card).unwrap();
        assert_eq!(agent.session, "api");
        assert!(agent.waiting_for_usage());
        assert_eq!(card.attention, Attention::Waiting);
        assert!(!card.needs_you());
    }

    #[test]
    fn queued_and_stale_moves_leave_the_account_to_the_feed() {
        for (state, message) in [
            ("queued", "moving to ARK"),
            ("stale", "the conversation changed"),
        ] {
            let mut a = app();
            pick(&mut a);
            press(&mut a, 's');
            let action = key(&mut a, KeyCode::Enter);
            complete(
                &mut a,
                action,
                json!({"ok":true,"state":state,"account":"ARK","message":message}),
            );
            assert_eq!(
                a.account_agent(a.selected().unwrap()).unwrap().account,
                "KBZ"
            );
            assert_eq!(a.message().unwrap().0, message);
        }
    }

    #[test]
    fn a_move_problem_needs_attention_and_is_shown_in_the_picker() {
        let mut a = app();
        let problem = "restart on ARK failed; retry the move";
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .iter_mut()
            .find(|a| a.session == "api" && a.pane == "w1:p1")
            .unwrap()
            .problem = Some(problem.into());
        a.rebuild();
        pick(&mut a);
        assert!(a.selected().unwrap().needs_you());
        assert_eq!(a.selected().unwrap().why, problem);
        press(&mut a, 's');
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &a)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains(problem));
    }

    #[test]
    fn offline_retry_and_token_submission_stay_retryable() {
        let mut a = app();
        a.machines[0].conn = crate::poll::Conn::Connecting;
        assert_eq!(signin(&mut a), Action::None);
        assert_eq!(press(&mut a, 'r'), Action::None);
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.failed && s.message.contains("not answering"))
        );
        press(&mut a, 'p');
        a.on_paste("fixture-private-value");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.failed)
        );
        assert!(!format!("{:?}", a.mode).contains("fixture-private-value"));
        a.machines[0].conn = crate::poll::Conn::Live {
            latency: Duration::ZERO,
        };
        assert_eq!(sent(press(&mut a, 'r')).1["step"], "start");
    }

    #[test]
    fn tab_cannot_pause_a_signin_without_a_code_field() {
        let mut a = app();
        let action = signin(&mut a);
        let mut response = started();
        response["needs_code"] = json!(false);
        complete(&mut a, action, response);
        key(&mut a, KeyCode::Tab);
        press(&mut a, 'z');
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            assert!(!s.editing);
            assert!(s.page_code.is_empty());
            s.poll_at = Instant::now() - Duration::from_secs(1);
        }
        assert_eq!(sent(a.account_tick()).1["step"], "poll");
    }

    #[test]
    fn token_enter_during_cancel_is_deferred_and_trims_the_paste() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        press(&mut a, 'p');
        let cancel = a.account_tick();
        a.busy = true;
        a.on_paste("  fixture-private-value  ");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        let (_, request) = sent(a.account_tick());
        assert_eq!(request["token"], "fixture-private-value");
        assert!(!format!("{:?}", a.mode).contains("fixture-private-value"));
    }

    #[test]
    fn identity_mismatch_and_refused_cancel_do_not_advance_signin() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        a.on_paste("page-code");
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":false,"status":"failed","message":"that signed in as X, not KBZ's Y"}),
        );
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.failed && s.current == 0 && s.message.contains("not KBZ"))
        );
        let cancel = press(&mut a, 'r');
        complete(
            &mut a,
            cancel,
            json!({"ok":false,"message":"cancel refused"}),
        );
        assert!(a.account_queue.is_empty());
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.failed && s.ticket.is_some() && s.message.contains("cancel refused"))
        );
        key(&mut a, KeyCode::Esc);
        assert!(!matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Signin(_),
                ..
            }
        ));
    }

    #[test]
    fn unchanged_organization_does_not_write_and_saved_lists_do_not_rewrite_other_feeds() {
        let mut a = app();
        let mut owner = a.account_feed(0).unwrap().owners[0].clone();
        owner.own = false;
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: owner.models.clone(),
            owner,
            sel: 0,
            move_waiting: false,
        });
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.account_pending.is_none());
        let other = a.account_feed(1).unwrap().owners.clone();
        a.saved_models(0, &json!({"owner":"studio","models":["KBZ"]}));
        assert_eq!(a.account_feed(0).unwrap().owners[0].models, ["KBZ"]);
        assert_eq!(a.account_feed(1).unwrap().owners, other);
    }

    fn open_copies(a: &mut App, copy: &str) {
        let rows = a.account_rows(1);
        let sel = rows
            .iter()
            .position(|r| *r == Row::Copy("ARK".into(), copy.into()))
            .unwrap();
        a.push_account_screen(AccountScreen::List {
            machine: 1,
            rows,
            sel,
            model: 0,
        });
    }

    #[test]
    fn two_copies_are_two_rows_and_one_copy_is_todays_display() {
        let mut a = app();
        let rows = a.account_rows(1);
        assert!(rows.contains(&Row::Copy("ARK".into(), "agents".into())));
        assert!(rows.contains(&Row::Copy("ARK".into(), "tenants".into())));
        assert!(!rows.contains(&Row::Account("ARK".into())));
        assert!(a.account_rows(0).contains(&Row::Account("ARK".into())));
        let ark = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap();
        ark.copies.truncate(1);
        let rows = a.account_rows(1);
        assert!(rows.contains(&Row::Account("ARK".into())));
        assert!(!rows.iter().any(|r| matches!(r, Row::Copy(..))));
    }

    #[test]
    fn enter_and_t_on_a_copy_line_name_that_copy() {
        let mut a = app();
        open_copies(&mut a, "tenants");
        let test = press(&mut a, 't');
        assert_eq!(
            sent(test.clone()),
            (
                1,
                json!({"action":"account-test","code":"ARK","copy":"tenants"})
            )
        );
        complete(
            &mut a,
            test,
            json!({"ok":true,"message":"The tenants' copy works","status":"ready"}),
        );
        press(&mut a, 'k');
        let test = press(&mut a, 't');
        assert_eq!(
            sent(test.clone()).1,
            json!({"action":"account-test","code":"ARK","copy":"agents"})
        );
        complete(&mut a, test, json!({"ok":true,"message":"works"}));
        press(&mut a, 'j');
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 1);
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"ARK","step":"start","copy":"tenants"})
        );
        assert!(matches!(
            &a.mode,
            Mode::Accounts { screen: AccountScreen::Signin(s), .. }
                if s.copy == Some(("tenants".into(), "Tenants".into())) && s.machines == [1]
        ));
    }

    #[test]
    fn retry_paste_and_finish_stay_on_the_copy_being_signed_in() {
        let mut a = app();
        open_copies(&mut a, "tenants");
        let action = key(&mut a, KeyCode::Enter);
        complete(&mut a, action, started());
        a.on_paste("page-code");
        let finish = key(&mut a, KeyCode::Enter);
        assert_eq!(
            sent(finish.clone()).1,
            json!({"action":"account-signin","step":"finish","ticket":"login-ticket","page_code":"page-code"}),
            "a ticket is already bound to its copy"
        );
        complete(
            &mut a,
            finish,
            json!({"ok":false,"status":"failed","message":"the code was refused"}),
        );
        let cancel = press(&mut a, 'r');
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        assert_eq!(
            sent(a.account_tick()).1,
            json!({"action":"account-signin","code":"ARK","step":"start","copy":"tenants"})
        );
        complete_pending(&mut a, started());
        press(&mut a, 'p');
        let Mode::Accounts {
            screen: AccountScreen::PasteToken { copy, machine, .. },
            ..
        } = &a.mode
        else {
            panic!("expected the token paste")
        };
        assert_eq!((copy.as_deref(), *machine), (Some("tenants"), 1));
        let cancel = a.account_tick();
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        a.on_paste("  fixture-token  ");
        let (_, paste) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(
            paste,
            json!({"action":"account-signin","code":"ARK","step":"paste","token":"fixture-token","copy":"tenants"})
        );
    }

    fn complete_pending(a: &mut App, reply: Value) {
        let pending = a.account_pending.clone().expect("a request in flight");
        a.finish_action(
            pending.machine,
            pending.request,
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }

    #[test]
    fn a_finished_copy_signs_in_nothing_else() {
        let mut a = app();
        open_copies(&mut a, "tenants");
        let action = key(&mut a, KeyCode::Enter);
        complete(&mut a, action, started());
        let poll = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        assert!(a.account_queue.is_empty());
        assert!(matches!(
            &a.mode,
            Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.current == 1
        ));
    }

    #[test]
    fn refreshing_keeps_the_account_selected_when_its_copies_come_or_go() {
        let mut a = app();
        open_copies(&mut a, "tenants");
        let copies = std::mem::take(
            &mut a.machines[1]
                .feed
                .as_mut()
                .unwrap()
                .accounts
                .iter_mut()
                .find(|x| x.code == "ARK")
                .unwrap()
                .copies,
        );
        a.rebuild();
        let selected = |a: &App| match &a.mode {
            Mode::Accounts {
                screen: AccountScreen::List { rows, sel, .. },
                ..
            } => rows[*sel].clone(),
            _ => panic!(),
        };
        assert_eq!(selected(&a), Row::Account("ARK".into()));
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap()
            .copies = copies;
        a.rebuild();
        assert_eq!(selected(&a), Row::Copy("ARK".into(), "agents".into()));
    }
}
