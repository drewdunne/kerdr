//! Account screens keep selections and confirmations bound to a machine and a conversation.
use super::*;
use crate::accounts::{AccountAgent, Owner};
use crate::actions::Response;
use crate::feed::Feed;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Account(String),
    Owner(String),
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
    pub code: String,
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
        token: Secret,
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
        let mut rows: Vec<Row> = feed
            .accounts
            .iter()
            .map(|a| Row::Account(a.code.clone()))
            .collect();
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
                        .and_then(|r| rows.iter().position(|x| x == &r))
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
            .and_then(|c| self.account_agent(c))
            .is_some_and(|a| {
                a.pane == target.agent.pane
                    && a.session == target.agent.session
                    && a.conversation == target.agent.conversation
                    && a.account == target.agent.account
            })
    }

    fn send_account(&mut self, machine: usize, request: Value) -> Action {
        if !self.can_write(machine) {
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

    fn begin_signin(&mut self, code: String, machines: Vec<usize>) -> Action {
        let Some(&machine) = machines.first() else {
            return Action::None;
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        self.push_account_screen(AccountScreen::Signin(Signin {
            code: code.clone(),
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
        }));
        self.send_account(
            machine,
            json!({"action":"account-signin","code":code,"step":"start"}),
        )
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
                    if let Some(agent) = self.account_agent(&card).cloned() {
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
                    } else {
                        self.say("This card has no recorded account yet.", Tone::Info);
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
        // Never repeat mutations from a held key; movement remains available while writing.
        if self.busy {
            let typing_poll = self
                .account_pending
                .as_ref()
                .is_some_and(|p| p.request["step"] == "poll")
                && matches!(&screen, AccountScreen::Signin(_));
            if !typing_poll || matches!(key.code, KeyCode::Enter | KeyCode::Char('p' | 'r')) {
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
                token,
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
                    let request = json!({"action":"account-signin","code":code,"step":"paste","token":std::mem::take(&mut token.0)});
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
                        match row {
                            Some(Row::Account(code)) => {
                                let a = self
                                    .account_feed(*machine)
                                    .and_then(|f| f.accounts.iter().find(|a| a.code == code))
                                    .cloned();
                                if let Some(a) = a {
                                    match key.code {
                                        KeyCode::Enter => {
                                            return self.begin_signin(code, vec![*machine]);
                                        }
                                        KeyCode::Char('t') => {
                                            request = Some((
                                                *machine,
                                                json!({"action":"account-test","code":code}),
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
                                        KeyCode::Backspace if owner.parent.is_some() => {
                                            request = Some((
                                                *machine,
                                                json!({"action":"account-models","owner":id,"inherit":true}),
                                            ))
                                        }
                                        KeyCode::Enter | KeyCode::Char('J' | 'K' | 'd')
                                            if !models.is_empty() =>
                                        {
                                            match key.code {
                                                KeyCode::Enter => {
                                                    let code = models.remove(*model);
                                                    models.insert(0, code);
                                                    *model = 0;
                                                }
                                                KeyCode::Char('d') => {
                                                    models.remove(*model);
                                                }
                                                KeyCode::Char('J') if *model + 1 < models.len() => {
                                                    models.swap(*model, *model + 1);
                                                    *model += 1;
                                                }
                                                KeyCode::Char('K') if *model > 0 => {
                                                    models.swap(*model, *model - 1);
                                                    *model -= 1;
                                                }
                                                _ => {}
                                            }
                                            if models.is_empty() {
                                                self.say("Keep one model, or Backspace to follow the parent.",Tone::Info);
                                            } else {
                                                request = Some((
                                                    *machine,
                                                    json!({"action":"account-models","owner":id,"models":models}),
                                                ));
                                            }
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
                                json!({"action":"account-move","pane":target.agent.pane,"session":target.agent.session,"to":to}),
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
            } => match key.code {
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
                KeyCode::Backspace if owner.parent.is_some() => {
                    request = Some((
                        *machine,
                        json!({"action":"account-models","owner":owner.id,"inherit":true}),
                    ))
                }
                KeyCode::Enter if !models.is_empty() => {
                    request = Some((
                        *machine,
                        json!({"action":"account-models","owner":owner.id,"models":models}),
                    ))
                }
                _ => {}
            },
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
                            || code.len() > 12
                            || !code
                                .chars()
                                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                            || name.trim().is_empty()
                            || !chosen.contains(&true)
                        {
                            self.say("Use a code of 1–12 uppercase letters, digits or _, a name, and at least one machine.",Tone::Warn);
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
                    KeyCode::Tab => s.editing = !s.editing,
                    KeyCode::Char('p') if !s.editing => {
                        if let Some(ticket) = s.ticket.take() {
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
                        self.mode = Mode::Accounts { screen, back };
                        self.push_account_screen(AccountScreen::PasteToken {
                            machine,
                            code,
                            token: Secret::default(),
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
                        s.ticket = None;
                        s.url = None;
                        request = Some((
                            machine,
                            json!({"action":"account-signin","code":s.code,"step":"start"}),
                        ));
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
        while let Some((mi, request)) = self.account_queue.pop_front() {
            if request["action"] == "account-signin"
                && request["step"] == "start"
                && !matches!(&self.mode,Mode::Accounts{screen:AccountScreen::Signin(s),..} if s.machines.get(s.current)==Some(&mi) && request["code"]==s.code)
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
                if let Mode::Accounts {
                    screen: AccountScreen::Signin(s),
                    ..
                } = &mut self.mode
                {
                    s.failed = true;
                    s.message = e.clone();
                }
                self.say(e, Tone::Warn);
                return;
            }
        };
        let action = request["action"].as_str().unwrap_or("");
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
                if !response.ok {
                    self.say(
                        format!("Could not cancel sign-in: {}", response.message),
                        Tone::Warn,
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
                } else if response.status.as_deref() == Some("done") {
                    s.current += 1;
                    s.ticket = None;
                    s.url = None;
                    s.page_code.clear();
                    s.editing = false;
                    if let Some(&mi) = s.machines.get(s.current) {
                        self.account_queue.push_back((
                            mi,
                            json!({"action":"account-signin","code":s.code,"step":"start"}),
                        ));
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
        if action == "account-move" && response.confirmation && visible {
            if let (
                Some(token),
                Mode::Accounts {
                    screen: AccountScreen::Move { target, .. },
                    ..
                },
            ) = (response.confirm, &self.mode)
            {
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
                    let action = self.begin_signin(code, selected);
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
                        self.account_batch_messages.clear();
                        for (mi, m) in self
                            .machines
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m.is_shown() && m.machine.feed.is_some())
                        {
                            if m.conn.is_live() {
                                self.account_queue.push_back((
                                    mi,
                                    json!({"action":"account-move-waiting","owner":owner.id}),
                                ));
                            } else {
                                self.account_batch_messages
                                    .push(format!("{}: offline, not moved", m.name()));
                            }
                        }
                    }
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
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

fn same_screen(a: &AccountScreen, b: &AccountScreen) -> bool {
    match (a, b) {
        (AccountScreen::Signin(a), AccountScreen::Signin(b)) => {
            a.code == b.code
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
            json!({"ok":true,"message":"cancelled","status":"done"}),
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
            json!({"action":"account-move","pane":"w1:p1","session":"api","to":"ARK"})
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
            json!({"action":"account-move","pane":"w1:p1","session":"api","to":"ARK","confirm":"opaque-ticket"})
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
            models: owner.models.clone(),
            owner,
            sel: 0,
            move_waiting: true,
        });
        let action = key(&mut a, KeyCode::Enter);
        complete(&mut a, action, json!({"ok":false,"message":"write failed"}));
        assert!(a.account_queue.is_empty());
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
        let action = a.begin_signin("ARK".into(), vec![0, 1]);
        complete(&mut a, action, started());
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None);
        let action = a.begin_signin("ARK".into(), vec![0, 1]);
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
}
