//! Account screens keep selections and confirmations bound to a machine and a conversation.
use super::*;
use crate::accounts::{Account, AccountAgent, Owner, Usage};
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
    /// Whether p offers a pasted token: only where the backend can take one.
    pub paste: bool,
    /// Why the code typed wasn't sent, shown under the code box until the next one is.
    pub note: Option<String>,
    /// Machines after the first were picked because they needed a sign-in: each is skipped if
    /// its feed no longer says so by its turn. Machines chosen by hand (adding an account) aren't.
    pub recheck: bool,
}

/// One way to sign an account in, offered when it's already ready on the machine viewed.
#[derive(Clone, Debug, PartialEq)]
pub struct WhereChoice {
    pub label: String,
    pub machines: Vec<usize>,
}

/// claude.ai's emailed sign-in code: a few digits, typed in the browser. The code Kerdr wants is
/// the long one the page shows after Authorize, so this is caught before it's sent (intent 0327).
pub fn emailed_code(code: &str) -> bool {
    (4..=10).contains(&code.len()) && code.chars().all(|c| c.is_ascii_digit())
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
    /// Enter on an account already ready on the machine viewed: where to sign it in instead.
    Where {
        code: String,
        copy: Option<(String, String)>,
        message: String,
        choices: Vec<WhereChoice>,
        sel: usize,
    },
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

    /// An account's usage: the freshest reading of it in any machine's feed, whichever machine
    /// is shown, since usage belongs to the account. Machines are taken in the same order for
    /// every caller, so a tie gives the same figures on every tab.
    pub fn account_usage<'a>(&'a self, a: &'a Account) -> Option<&'a Usage> {
        crate::accounts::freshest(
            self.machines
                .iter()
                .filter_map(|m| m.feed.as_ref())
                .flat_map(|f| f.accounts.iter())
                .filter(|other| other.code == a.code)
                .filter_map(|other| other.usage.as_ref()),
        )
        .or(a.usage.as_ref())
    }

    /// An account's usage figures as `five│week`, from its freshest reading on any machine.
    pub fn account_numbers(&self, a: &Account) -> String {
        crate::accounts::numbers(self.account_usage(a), self.now())
    }

    /// The machine the accounts screen opens on: the board's machine tab when one is chosen, else
    /// the machine Kerdr runs on, whose browser a sign-in link opens. The selected card's machine
    /// comes only after that: a card picked for other reasons opened it on LAB-Server, already
    /// signed in, and Enter signed KBZ in there four times while the laptop needed it (intent 0327).
    pub fn account_machine(&self) -> Option<usize> {
        let with_accounts = |m: &crate::board::MachineState| {
            m.is_shown() && m.feed.as_ref().is_some_and(|f| !f.accounts.is_empty())
        };
        self.machine
            .as_ref()
            .and_then(|n| self.machine_index(n))
            .or_else(|| {
                self.machines
                    .iter()
                    .position(|m| m.machine.is_local() && with_accounts(m))
            })
            .or_else(|| self.selected().map(|c| c.machine_index))
            .or_else(|| self.machines.iter().position(with_accounts))
    }

    /// The machines other than `machine` where this account needs a sign-in (its feed says
    /// `sign_in` or `missing`), this machine first, with whether each is answering.
    fn needing(&self, code: &str, machine: usize) -> Vec<(usize, bool)> {
        let mut order: Vec<usize> = (0..self.machines.len()).collect();
        order.sort_by_key(|&i| !self.machines[i].machine.is_local());
        order
            .into_iter()
            .filter(|&i| i != machine && self.machines[i].is_shown())
            .filter(|&i| {
                self.account_feed(i)
                    .and_then(|f| f.accounts.iter().find(|x| x.code == code))
                    .is_some_and(|x| x.needs_signin())
            })
            .map(|i| (i, self.machines[i].conn.is_live()))
            .collect()
    }

    /// Where Enter signs an account in when it isn't ready on the machine viewed: there, then each
    /// other answering machine that needs it, in turn. Andrew took Tab between machines for the
    /// screen not noticing the others: "I thought the UI would detect which weren't signed in and
    /// just do those" (intent 0327). A copy is one machine's, so it signs in alone.
    pub fn signin_order(&self, code: &str, copy: bool, machine: usize) -> Vec<usize> {
        let mut machines = vec![machine];
        if !copy {
            machines.extend(
                self.needing(code, machine)
                    .into_iter()
                    .filter(|&(_, live)| live)
                    .map(|(i, _)| i),
            );
        }
        machines
    }

    /// What Enter offers instead of a sign-in when the account (or the copy) is already ready on
    /// the machine viewed: all the machines that need one in turn when there are several, each of
    /// them, and signing in again here. None when it isn't ready here, so Enter signs in at once.
    pub fn signin_choices(
        &self,
        code: &str,
        copy: Option<&(String, String)>,
        machine: usize,
    ) -> Option<(String, Vec<WhereChoice>)> {
        let a = self
            .account_feed(machine)?
            .accounts
            .iter()
            .find(|a| a.code == code)?;
        let here = self.machines.get(machine)?.name();
        if let Some((id, label)) = copy {
            let c = a.copy(id).filter(|c| a.copy_ready(c))?;
            let when = c
                .checked
                .as_deref()
                .map(|t| format!(" (checked {})", crate::accounts::reset_time(t)))
                .unwrap_or_default();
            let state = if c.works == Some(true) {
                "works"
            } else {
                "is signed in"
            };
            return Some((
                format!(
                    "The {label} copy of {code} on {here} already {state}{when}. Signing it in again replaces that sign-in."
                ),
                vec![WhereChoice {
                    label: format!("Sign the {label} copy in again on {here}"),
                    machines: vec![machine],
                }],
            ));
        }
        if !a.ready() {
            return None;
        }
        let need = self.needing(code, machine);
        let live: Vec<usize> = need.iter().filter(|(_, l)| *l).map(|(i, _)| *i).collect();
        let mut choices = Vec::new();
        if live.len() >= 2 {
            let names: Vec<String> = live.iter().map(|&i| self.machines[i].name()).collect();
            choices.push(WhereChoice {
                label: format!(
                    "All {}, one after another: {}",
                    live.len(),
                    names.join(", ")
                ),
                machines: live,
            });
        }
        choices.extend(need.iter().map(|&(i, answering)| {
            let status = self
                .account_feed(i)
                .and_then(|f| f.accounts.iter().find(|x| x.code == code))
                .map(|x| x.status_label())
                .unwrap_or_default();
            WhereChoice {
                label: format!(
                    "{:<22} {status}{}",
                    self.machines[i].name(),
                    if answering { "" } else { ", not answering" }
                ),
                machines: vec![i],
            }
        }));
        choices.push(WhereChoice {
            label: format!("{here} again, though it's ready"),
            machines: vec![machine],
        });
        // Only what feeds say: another machine may be out of usage, unknown, or not answering.
        let message = match need.len() {
            0 => format!(
                "{code} is already signed in and ready on {here}, and no other machine reports needing a sign-in. Sign it in again here anyway?"
            ),
            1 => format!(
                "{code} is already signed in and ready on {here}. This machine needs a sign-in:"
            ),
            n => format!(
                "{code} is already signed in and ready on {here}. These {n} machines need a sign-in:"
            ),
        };
        Some((message, choices))
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
                conversation: a.conversation.clone().unwrap_or_default(),
                ..AccountAgent::default()
            })
    }

    /// What S says when its list is saved unchanged. A list only picks the account agents start
    /// on, so when this owner has a running agent s could move, it says that's the key, naming
    /// the cards (F-0294: Andrew reached for S to move one agent and was told only "unchanged").
    pub fn unchanged_list_said(&self, machine: usize, owner: &Owner) -> String {
        let first = owner.models.first().map(String::as_str);
        let mut cards: Vec<(String, String)> = self
            .board
            .cards
            .iter()
            .filter(|c| c.machine_index == machine && c.detail.is_some())
            .filter(|c| {
                c.session.as_deref().is_some_and(|s| {
                    self.account_owner(machine, s).is_some_and(|o| {
                        o.id == owner.id || (o.parent.as_ref() == Some(&owner.id) && !o.own)
                    })
                })
            })
            .filter_map(|c| {
                let agent = self.move_agent(c)?;
                Some((c.reference.clone(), agent.account))
            })
            .collect();
        if cards.is_empty() {
            return "Model list unchanged.".into();
        }
        // The ones running off the list's first account first: those are what a move is for.
        cards.sort_by_key(|(_, account)| Some(account.as_str()) == first);
        let named: Vec<String> = cards
            .iter()
            .take(3)
            .map(|(r, a)| {
                if a.is_empty() {
                    r.clone()
                } else {
                    format!("{r} on {a}")
                }
            })
            .collect();
        format!(
            "Model list unchanged. It picks the account agents start on and moves none that's running: to move one, select its card and press s ({}{}).",
            named.join(", "),
            if cards.len() > 3 { ", …" } else { "" }
        )
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
        // An account not in the feed yet was just added here, and Kerdr adds token accounts.
        let paste = self
            .account_feed(machine)
            .and_then(|f| f.accounts.iter().find(|a| a.code == code))
            .is_none_or(|a| {
                a.takes_paste(
                    copy.as_ref()
                        .and_then(|(id, _)| a.copies.iter().find(|c| &c.id == id)),
                )
            });
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
            paste,
            note: None,
            recheck: false,
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

    /// A sign-in whose later machines were picked because they needed one: by each one's turn,
    /// one signed in meanwhile is skipped rather than signed in again unasked.
    fn begin_needed_signin(
        &mut self,
        code: String,
        copy: Option<(String, String)>,
        machines: Vec<usize>,
    ) -> Action {
        let action = self.begin_signin(code, copy, machines);
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut self.mode
        {
            s.recheck = true;
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
                        self.say(
                            if card.cancellable() {
                                "This card's agent isn't running, so there's nothing to move: ⏎ goes in, X cancels it."
                            } else {
                                "This card has no agent running to move."
                            },
                            Tone::Info,
                        );
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
                    || (!s.editing
                        && (key.code == KeyCode::Char('r')
                            || (s.paste && key.code == KeyCode::Char('p')))))
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
                                            if let Some((message, choices)) =
                                                self.signin_choices(&code, copy.as_ref(), *machine)
                                            {
                                                self.push_account_screen(AccountScreen::Where {
                                                    code,
                                                    copy,
                                                    message,
                                                    choices,
                                                    sel: 0,
                                                });
                                                return Action::None;
                                            }
                                            let machines =
                                                self.signin_order(&code, copy.is_some(), *machine);
                                            return self.begin_needed_signin(code, copy, machines);
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
                            let said = self.unchanged_list_said(*machine, owner);
                            self.say(said, Tone::Info);
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
                    KeyCode::Char('p') if !s.editing && s.paste => {
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
                            } else if emailed_code(s.page_code.trim()) {
                                // Kept here: Claude Code would refuse it and end the sign-in.
                                s.note = Some("That's the short code claude.ai emails you: type it on the claude.ai page in your browser. Here, paste the long code the page shows after you press Authorize.".into());
                                s.page_code.clear();
                                s.editing = true;
                            } else {
                                request = Some((
                                    machine,
                                    json!({"action":"account-signin","ticket":ticket,"step":"finish","page_code":s.page_code.trim()}),
                                ));
                                s.page_code.clear();
                                s.editing = false;
                                s.note = None;
                            }
                        }
                    }
                    _ => {}
                }
            }
            AccountScreen::Where {
                code,
                copy,
                choices,
                sel,
                ..
            } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(choice) = choices.get(*sel) {
                        let (code, copy, machines) =
                            (code.clone(), copy.clone(), choice.machines.clone());
                        // The sign-in opens over the list, so Esc from it goes back there.
                        self.mode = *back;
                        return self.begin_needed_signin(code, copy, machines);
                    }
                }
                _ => {}
            },
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
        if let Some(i) = self
            .account_cancels
            .iter()
            .position(|(mi, _)| self.machines.get(*mi).is_some_and(|m| m.conn.is_live()))
        {
            let (mi, request) = self.account_cancels.remove(i);
            return self.send_account(mi, request);
        }
        while let Some((mi, request)) = self.account_queue.pop_front() {
            if request["action"] == "account-signin"
                && request["step"] == "cancel"
                && !self.machines.get(mi).is_some_and(|m| m.conn.is_live())
            {
                // Refused now, it would be lost and leave that machine's sign-in running.
                self.account_cancels.push((mi, request));
                continue;
            }
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
            // What each machine of the sign-in says now, read before the screen is borrowed.
            let now: Vec<(bool, String)> = match &self.mode {
                Mode::Accounts {
                    screen: AccountScreen::Signin(s),
                    ..
                } => s
                    .machines
                    .iter()
                    .map(|&mi| {
                        let needs = self
                            .account_feed(mi)
                            .and_then(|f| f.accounts.iter().find(|a| a.code == s.code))
                            .is_none_or(|a| a.needs_signin());
                        (needs, self.machines[mi].name())
                    })
                    .collect(),
                _ => Vec::new(),
            };
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
                    let mut skipped = Vec::new();
                    while s.recheck
                        && s.current < s.machines.len()
                        && now.get(s.current).is_some_and(|(needs, _)| !needs)
                    {
                        skipped.push(now[s.current].1.clone());
                        s.current += 1;
                    }
                    if !skipped.is_empty() {
                        s.message = format!(
                            "{} {} signed in meanwhile, so {} skipped.",
                            response.message,
                            skipped.join(" and "),
                            if skipped.len() == 1 {
                                "it was"
                            } else {
                                "they were"
                            }
                        );
                    }
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
    fn set_status(a: &mut App, machine: usize, code: &str, status: &str) {
        a.machines[machine]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == code)
            .unwrap()
            .status = status.into();
    }
    /// Signs in ARK, a token account, on the laptop, where it needs it: KBZ is a saved sign-in,
    /// which takes no pasted token.
    fn token_signin(a: &mut App) -> Action {
        set_status(a, 0, "ARK", "sign_in");
        a.open_accounts();
        key(a, KeyCode::Down);
        key(a, KeyCode::Enter)
    }
    fn signin_screen(a: &App) -> &Signin {
        let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &a.mode
        else {
            panic!("expected the sign-in, got {:?}", a.mode)
        };
        s
    }
    fn where_screen(a: &App) -> (&str, &[WhereChoice]) {
        let Mode::Accounts {
            screen: AccountScreen::Where {
                message, choices, ..
            },
            ..
        } = &a.mode
        else {
            panic!("expected where to sign in, got {:?}", a.mode)
        };
        (message, choices)
    }
    fn started() -> Value {
        json!({"ok":true,"message":"Authorize in your browser.","status":"waiting","url":"https://example.com/authorize?state=example","ticket":"login-ticket","needs_code":true})
    }

    #[test]
    fn token_paste_leaves_only_metadata_in_ui_and_result_state() {
        let mut a = app();
        let action = token_signin(&mut a);
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
        set_conversation(&mut a, "c-one");
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
        assert_eq!(r["conversation"], "c-one");
    }
    fn set_conversation(a: &mut App, conv: &str) {
        let key = a.sel.key.clone().unwrap();
        for c in a.board.cards.iter_mut().filter(|c| c.key == key) {
            for g in c.agents.iter_mut().filter(|g| g.pane_id == "w1:p1") {
                g.conversation = Some(conv.into());
            }
        }
    }
    #[test]
    fn an_unrecorded_agent_replaced_in_its_pane_is_not_moved() {
        let mut a = app();
        pick(&mut a);
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .retain(|a| !(a.session == "api" && a.pane == "w1:p1"));
        set_conversation(&mut a, "c-one");
        press(&mut a, 's');
        set_conversation(&mut a, "c-two");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.message().unwrap().0.contains("conversation changed"));
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
    fn s_upper_saved_unchanged_points_to_s_on_a_card_when_an_agent_could_move() {
        // F-0294: Andrew pressed S to move one running agent, and "Model list unchanged" alone
        // read as "can't be done".
        let mut a = app();
        let owner = a
            .account_feed(1)
            .unwrap()
            .owners
            .iter()
            .find(|o| o.id == "platform")
            .unwrap()
            .clone();
        let open = |a: &mut App, owner: &crate::accounts::Owner| {
            a.push_account_screen(AccountScreen::Models {
                machine: 1,
                models: owner.models.clone(),
                owner: owner.clone(),
                sel: 0,
                move_waiting: false,
            });
        };
        open(&mut a, &owner);
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let said = a.message().unwrap().0;
        assert!(
            said.starts_with("Model list unchanged. It picks the account agents start on and moves none that's running: to move one, select its card and press s ("),
            "{said}"
        );
        // The build box's Platform sessions: the ones off the list's first account (KBZ) first.
        assert!(said.contains(" on ARK"), "{said}");
        // With no agent there to move, it's only unchanged.
        for c in a.board.cards.iter_mut() {
            c.agents.clear();
        }
        open(&mut a, &owner);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.message().unwrap().0, "Model list unchanged.");
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
        assert_eq!(token_signin(&mut a), Action::None);
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
        let action = token_signin(&mut a);
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
        // The agents' copy takes a pasted token; root's doesn't. Refused by Claude, it isn't ready.
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap()
            .copies[0]
            .works = Some(false);
        open_copies(&mut a, "agents");
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
            json!({"action":"account-signin","code":"ARK","step":"start","copy":"agents"})
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
        assert_eq!((copy.as_deref(), *machine), (Some("agents"), 1));
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
            json!({"action":"account-signin","code":"ARK","step":"paste","token":"fixture-token","copy":"agents"})
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

    /// Intent 0327: on All, with a LAB-Server card selected, A opened on LAB-Server and Enter signed
    /// KBZ in there though the laptop was the one that needed it.
    #[test]
    fn accounts_open_on_this_machine_unless_a_machine_tab_is_chosen() {
        let mut a = app();
        let card = a
            .board
            .cards
            .iter()
            .find(|c| c.machine_index == 1)
            .unwrap()
            .clone();
        a.sel = Selection {
            col: card.stage.index(),
            key: Some(card.key.clone()),
        };
        assert_eq!(a.selected().unwrap().machine_index, 1);
        assert!(a.machine.is_none());
        assert_eq!(
            a.account_machine(),
            Some(0),
            "All opens on the machine Kerdr runs on"
        );
        a.machine = Some("build-box".into());
        assert_eq!(
            a.account_machine(),
            Some(1),
            "a machine tab is a choice, and wins"
        );
        // With no account information here, the selected card's machine is next.
        a.machine = None;
        a.machines[0].feed.as_mut().unwrap().accounts.clear();
        assert_eq!(a.account_machine(), Some(1));
    }

    #[test]
    fn enter_on_an_account_ready_here_offers_the_machines_that_need_one() {
        let mut a = app();
        set_status(&mut a, 1, "KBZ", "ready");
        set_status(&mut a, 0, "KBZ", "sign_in");
        a.machine = Some("build-box".into());
        a.open_accounts();
        assert_eq!(
            key(&mut a, KeyCode::Enter),
            Action::None,
            "nothing starts by itself"
        );
        let (message, choices) = where_screen(&a);
        assert!(
            message.contains("KBZ is already signed in and ready on build-box"),
            "{message}"
        );
        assert_eq!(choices.len(), 2);
        assert!(choices[0].label.starts_with("laptop") && choices[0].label.contains("✕ sign in"));
        assert_eq!(choices[0].machines, vec![0]);
        assert_eq!(
            choices[1].machines,
            vec![1],
            "signing in again here is offered last"
        );
        assert!(choices[1].label.contains("though it's ready"));
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 0, "the machine that needed it");
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"KBZ","step":"start"})
        );
        assert_eq!(signin_screen(&a).machines, vec![0]);
        a.busy = false;
        a.account_pending = None;
        key(&mut a, KeyCode::Esc);
        assert!(
            matches!(
                &a.mode,
                Mode::Accounts {
                    screen: AccountScreen::List { machine: 1, .. },
                    ..
                }
            ),
            "Esc from the sign-in goes back to the list, not the question: {:?}",
            a.mode
        );
    }

    #[test]
    fn ready_everywhere_asks_before_signing_in_again_and_several_can_go_in_turn() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ", "ready");
        // Out of usage isn't signed out: it's never offered, and never called ready either.
        set_status(&mut a, 1, "KBZ", "out");
        a.open_accounts();
        key(&mut a, KeyCode::Enter);
        let (message, choices) = where_screen(&a);
        assert!(
            message.contains("no other machine reports needing a sign-in"),
            "{message}"
        );
        assert!(!message.contains("every other machine"));
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].machines, vec![0]);
        key(&mut a, KeyCode::Esc);
        // Two more signed out: all of them in turn comes first, then each, then here again.
        a.machines[2].feed = a.machines[1].feed.clone();
        a.machines[2].conn = a.machines[1].conn.clone();
        set_status(&mut a, 1, "KBZ", "sign_in");
        set_status(&mut a, 2, "KBZ", "missing");
        a.open_accounts();
        key(&mut a, KeyCode::Enter);
        let (message, choices) = where_screen(&a);
        assert!(
            message.contains("These 2 machines need a sign-in"),
            "{message}"
        );
        let machines: Vec<Vec<usize>> = choices.iter().map(|c| c.machines.clone()).collect();
        assert_eq!(machines, vec![vec![1, 2], vec![1], vec![2], vec![0]]);
        assert!(choices[0].label.starts_with("All 2, one after another"));
        let (mi, _) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 1);
        assert_eq!(signin_screen(&a).machines, vec![1, 2]);
        // One not answering is offered, said so, and left out of all in turn.
        let mut a = app();
        a.machines[2].feed = a.machines[1].feed.clone();
        set_status(&mut a, 0, "KBZ", "ready");
        set_status(&mut a, 1, "KBZ", "sign_in");
        set_status(&mut a, 2, "KBZ", "sign_in");
        a.machines[2].conn = crate::poll::Conn::Connecting;
        a.open_accounts();
        key(&mut a, KeyCode::Enter);
        let (_, choices) = where_screen(&a);
        let machines: Vec<Vec<usize>> = choices.iter().map(|c| c.machines.clone()).collect();
        assert_eq!(machines, vec![vec![1], vec![2], vec![0]]);
        assert!(
            choices[1].label.ends_with(", not answering"),
            "{}",
            choices[1].label
        );
    }

    /// Andrew: "I didn't realize I had to 'tab' through the various machines at the top. I thought
    /// the UI would detect which weren't signed in and just do those" (intent 0327).
    #[test]
    fn enter_signs_in_here_then_every_other_machine_that_needs_it() {
        let mut a = app();
        a.machines[2].feed = a.machines[1].feed.clone();
        a.machines[2].conn = a.machines[1].conn.clone();
        a.machines[3].feed = a.machines[1].feed.clone();
        set_status(&mut a, 0, "KBZ", "sign_in");
        set_status(&mut a, 1, "KBZ", "ready");
        set_status(&mut a, 2, "KBZ", "missing");
        set_status(&mut a, 3, "KBZ", "sign_in");
        a.machines[3].conn = crate::poll::Conn::Connecting;
        assert_eq!(a.signin_order("KBZ", false, 0), vec![0, 2]);
        assert_eq!(
            a.signin_order("KBZ", true, 0),
            vec![0],
            "a copy is one machine's"
        );
        a.open_accounts();
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 0, "the machine viewed first");
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"KBZ","step":"start"})
        );
        assert_eq!(signin_screen(&a).machines, vec![0, 2]);
        let action = Action::Mutate {
            machine: mi,
            request: start,
        };
        complete(&mut a, action, started());
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        let (mi, r) = sent(a.account_tick());
        assert_eq!(mi, 2, "then the next that needs it");
        assert_eq!(r["step"], "start");
    }

    #[test]
    fn a_copy_that_works_asks_before_signing_in_again_and_one_that_doesnt_starts() {
        let mut a = app();
        open_copies(&mut a, "agents");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let (message, choices) = where_screen(&a);
        assert!(
            message.contains("The Your agents copy of ARK on build-box already works"),
            "{message}"
        );
        assert_eq!(choices.len(), 1);
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 1);
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"ARK","step":"start","copy":"agents"})
        );
        let mut a = app();
        open_copies(&mut a, "tenants");
        assert_eq!(sent(key(&mut a, KeyCode::Enter)).1["copy"], "tenants");
        // The copy launches use, never tested, while the account is ready: asked about too.
        let mut a = app();
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap()
            .copies[0]
            .works = None;
        open_copies(&mut a, "agents");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let (message, _) = where_screen(&a);
        assert!(message.contains("already is signed in"), "{message}");
    }

    #[test]
    fn p_token_only_for_token_accounts_and_copies_that_take_one() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        assert!(!signin_screen(&a).paste, "KBZ is a saved sign-in");
        assert_eq!(press(&mut a, 'p'), Action::None);
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if !s.failed && s.ticket.is_some()),
            "p on a saved sign-in neither cancels it nor opens the paste: {:?}",
            a.mode
        );
        assert!(a.account_queue.is_empty());
        let mut a = app();
        let action = token_signin(&mut a);
        complete(&mut a, action, started());
        assert!(signin_screen(&a).paste);
        press(&mut a, 'p');
        assert!(matches!(
            &a.mode,
            Mode::Accounts {
                screen: AccountScreen::PasteToken { .. },
                ..
            }
        ));
        // Root's copy says it takes none; a copy that says nothing follows its account's kind.
        let mut a = app();
        let ark = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap();
        ark.copies[1].paste = Some(false);
        ark.copies[0].works = Some(false);
        open_copies(&mut a, "tenants");
        key(&mut a, KeyCode::Enter);
        assert!(!signin_screen(&a).paste);
        // A feed from before paste: root's copy still offers none, the agents' copy one.
        let mut a = app();
        let ark = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap();
        assert!(ark.copies.iter().all(|c| c.paste.is_none()));
        assert!(ark.takes_paste(Some(&ark.copies[0])) && !ark.takes_paste(Some(&ark.copies[1])));
        open_copies(&mut a, "tenants");
        key(&mut a, KeyCode::Enter);
        assert!(!signin_screen(&a).paste);
        let mut a = app();
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap()
            .copies[0]
            .works = Some(false);
        open_copies(&mut a, "agents");
        key(&mut a, KeyCode::Enter);
        assert!(signin_screen(&a).paste);
    }

    #[test]
    fn a_short_all_digit_code_is_caught_before_it_is_sent() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        a.on_paste("123456");
        assert_eq!(
            key(&mut a, KeyCode::Enter),
            Action::None,
            "the emailed code isn't sent"
        );
        let s = signin_screen(&a);
        assert!(
            s.note
                .as_deref()
                .unwrap()
                .contains("short code claude.ai emails you")
        );
        assert!(
            s.note
                .as_deref()
                .unwrap()
                .contains("long code the page shows after you press Authorize")
        );
        assert!(s.page_code.is_empty() && s.editing && !s.failed);
        a.on_paste("Ab3-long-code#state-xyz");
        let (_, finish) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(finish["page_code"], "Ab3-long-code#state-xyz");
        assert!(signin_screen(&a).note.is_none());
        assert!(emailed_code("004211") && emailed_code("1234"));
        assert!(!emailed_code("123") && !emailed_code("12345678901") && !emailed_code("12ab56"));
    }

    /// Codex's second look: a machine picked because it needed a sign-in, but signed in by the
    /// time its turn comes, is skipped rather than signed in again unasked.
    #[test]
    fn a_machine_signed_in_meanwhile_is_skipped_in_turn() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ", "sign_in");
        set_status(&mut a, 1, "KBZ", "sign_in");
        a.open_accounts();
        let action = key(&mut a, KeyCode::Enter);
        assert_eq!(signin_screen(&a).machines, vec![0, 1]);
        assert!(signin_screen(&a).recheck);
        complete(&mut a, action, started());
        set_status(&mut a, 1, "KBZ", "ready");
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"KBZ signed in on laptop"}),
        );
        assert_eq!(a.account_tick(), Action::None, "no start for build-box");
        let s = signin_screen(&a);
        assert_eq!(s.current, 2);
        assert!(
            s.message
                .contains("build-box signed in meanwhile, so it was skipped"),
            "{}",
            s.message
        );
    }

    #[test]
    fn esc_while_a_machine_is_not_answering_cancels_once_it_answers() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        let live = a.machines[0].conn.clone();
        a.machines[0].conn = crate::poll::Conn::Connecting;
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None, "nothing can reach it yet");
        assert_eq!(
            a.account_cancels.len(),
            1,
            "the cancel is kept, not dropped"
        );
        assert_eq!(a.account_tick(), Action::None);
        a.machines[0].conn = live;
        let (mi, r) = sent(a.account_tick());
        assert_eq!(mi, 0);
        assert_eq!(
            r,
            json!({"action":"account-signin","step":"cancel","ticket":"login-ticket"})
        );
        assert!(a.account_cancels.is_empty());
    }
}
