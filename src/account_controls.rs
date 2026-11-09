//! Account screens keep selections and confirmations bound to a machine and a conversation.
use super::*;
use crate::accounts::{Account, AccountAgent, Figures, Owner, Provider, Usage, Window};
use crate::actions::Response;
use crate::feed::Feed;
use crate::providers::Providers;
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
    /// For a provider whose page takes a code: the code to type there, and when it runs out.
    /// Nothing comes back to paste, so there's no code box.
    pub user_code: Option<String>,
    pub expires: Option<String>,
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
    /// For an account just added, not in any feed yet: its provider and kind, from which each
    /// machine's own feed says whether its sign-in takes a token.
    pub added: Option<(String, String)>,
}

/// One way to sign an account in, offered when it's already ready on the machine viewed.
#[derive(Clone, Debug, PartialEq)]
pub struct WhereChoice {
    pub label: String,
    pub machines: Vec<usize>,
}

/// What the code box says when it keeps back an emailed code, for a backend too old to say it.
pub const EMAILED_CODE: &str = "That looks like the short code a sign-in page emails you: type it on the page in your browser. Here, paste the long code the page shows after you allow it.";

/// An emailed sign-in code: a few digits, typed in the browser. The code an older backend wants
/// is the long one the page shows after you allow it, so this is caught before it's sent to one
/// (intent 0327); a backend that describes its providers checks codes itself.
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
    /// Whether the code to type on the provider's page has run out: r then starts again.
    pub fn expired(&self, now: chrono::DateTime<chrono::Local>) -> bool {
        self.user_code.is_some()
            && self
                .expires
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t <= now)
    }

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
    /// `s` on a stopped card: which account to start it on (intent 0580).
    Start {
        key: String,
        reference: String,
        start: crate::app::run_state::Start,
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
    /// `+` asks which provider first, since the provider decides the program.
    Provider {
        machine: usize,
        choices: Vec<Provider>,
        sel: usize,
    },
    Add(Box<AddAccount>),
    Rename {
        machine: usize,
        code: String,
        name: String,
    },
    Signin(Box<Signin>),
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

/// The add form, for one provider: code, name, kind (in that provider's words) and machines.
#[derive(Clone, Debug)]
pub struct AddAccount {
    pub machine: usize,
    pub provider: Provider,
    pub code: String,
    pub name: String,
    /// What an empty code or name stands for: the owner's code, `_` and the provider's letter,
    /// and the owner's name with the provider's.
    pub suggested: (String, String),
    pub kind: usize,
    /// The answers to the provider's own `fields`, in its order.
    pub extra: Vec<String>,
    /// Rows, in order: code, name, each of the provider's fields, each kind, each machine.
    pub field: usize,
    pub machines: Vec<usize>,
    pub chosen: Vec<bool>,
}

impl AddAccount {
    /// The first of the provider's own fields; the text rows end where the kinds start.
    pub const FIELDS_AT: usize = 2;

    pub fn kinds_at(&self) -> usize {
        Self::FIELDS_AT + self.provider.fields.len()
    }

    pub fn machines_at(&self) -> usize {
        self.kinds_at() + self.provider.kinds.len()
    }

    /// The most a text row takes, typed or pasted: a code or name 80 characters, the provider's
    /// own fields (an address, a model's name) 200.
    pub fn limit(at: usize) -> usize {
        if at < Self::FIELDS_AT { 80 } else { 200 }
    }

    /// The text being typed in row `at`, if it's a text row.
    fn text_mut(&mut self, at: usize) -> Option<&mut String> {
        match at {
            0 => Some(&mut self.code),
            1 => Some(&mut self.name),
            at if at < self.kinds_at() => self.extra.get_mut(at - Self::FIELDS_AT),
            _ => None,
        }
    }

    /// The provider's fields that were answered, as the request's `fields`; a required one left
    /// empty, if any.
    pub fn answers(&self) -> (serde_json::Map<String, Value>, Option<&str>) {
        let mut out = serde_json::Map::new();
        let mut missing = None;
        for (f, value) in self.provider.fields.iter().zip(&self.extra) {
            if value.trim().is_empty() {
                if f.required && missing.is_none() {
                    missing = Some(if f.label.is_empty() {
                        f.id.as_str()
                    } else {
                        f.label.as_str()
                    });
                }
            } else {
                out.insert(f.id.clone(), json!(value.trim()));
            }
        }
        (out, missing)
    }

    fn rows(&self) -> usize {
        self.machines_at() + self.machines.len()
    }

    /// The code and name it will add: what was typed, else the suggestion.
    pub fn code(&self) -> &str {
        if self.code.is_empty() {
            &self.suggested.0
        } else {
            &self.code
        }
    }

    pub fn name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.suggested.1
        } else {
            self.name.trim()
        }
    }

    pub fn kind(&self) -> &str {
        self.provider
            .kinds
            .get(self.kind)
            .map_or("", String::as_str)
    }

    /// Codes are the owner's code, `_` and the provider's letter (`KBZ_X`); a provider a feed
    /// doesn't describe takes the older 2–4 capitals.
    pub fn code_ok(&self) -> bool {
        code_ok(self.code(), &self.provider.letter)
    }
}

/// An account code for a provider with this letter: 2–4 capitals or digits starting with a
/// capital, `_`, then the letter. With no letter, the older 2–4 capitals.
pub fn code_ok(code: &str, letter: &str) -> bool {
    if letter.is_empty() {
        return (2..=4).contains(&code.len()) && code.chars().all(|c| c.is_ascii_uppercase());
    }
    let Some(owner) = code.strip_suffix(letter).and_then(|c| c.strip_suffix('_')) else {
        return false;
    };
    (2..=4).contains(&owner.len())
        && owner.starts_with(|c: char| c.is_ascii_uppercase())
        && owner
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
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
        self.account_reading(a).map(|(u, _)| u).or(a.usage.as_ref())
    }

    /// That freshest reading, and whether the feed it came from describes the account's
    /// provider: a reading from an older feed keeps the older five-hour and weekly pair, whatever
    /// another machine's feed says of the provider.
    fn account_reading<'a>(&'a self, a: &'a Account) -> Option<(&'a Usage, bool)> {
        let readings: Vec<(&Usage, bool)> = self
            .machines
            .iter()
            .filter_map(|m| m.feed.as_ref())
            .flat_map(|f| {
                f.accounts
                    .iter()
                    .filter(|other| other.code == a.code)
                    .filter_map(move |other| {
                        let described = f.providers.iter().any(|p| p.id == other.provider);
                        Some((other.usage.as_ref()?, described))
                    })
            })
            .collect();
        let best = crate::accounts::freshest(readings.iter().map(|(u, _)| *u))?;
        readings.into_iter().find(|(u, _)| std::ptr::eq(*u, best))
    }

    /// What providers and programs are called, from every machine's feed, `machine`'s first.
    pub fn providers(&self, machine: usize) -> Providers<'_> {
        let mut order: Vec<usize> = (0..self.machines.len()).collect();
        order.sort_by_key(|&i| i != machine);
        Providers::new(order.into_iter().filter_map(|i| self.account_feed(i)))
    }

    /// The same from every machine's feed in the board's order, for what belongs to an account
    /// wherever it's shown (its usage windows).
    pub fn all_providers(&self) -> Providers<'_> {
        Providers::new((0..self.machines.len()).filter_map(|i| self.account_feed(i)))
    }

    /// What `machine`'s own backend can do: the providers `+` offers there, their code rules, how
    /// its accounts sign in and take a paste, the programs `s` moves. Never another machine's.
    pub fn offered(&self, machine: usize) -> Providers<'_> {
        Providers::new(self.account_feed(machine))
    }

    /// An account in a machine's feed, by its code.
    pub fn account(&self, machine: usize, code: &str) -> Option<&Account> {
        self.account_feed(machine)?
            .accounts
            .iter()
            .find(|a| a.code == code)
    }

    /// Whether any machine's feed has an account with this code.
    fn code_taken(&self, code: &str) -> bool {
        self.machines
            .iter()
            .filter_map(|m| m.feed.as_ref())
            .any(|f| f.accounts.iter().any(|a| a.code == code))
    }

    /// The codes of a machine's accounts of one provider and kind.
    pub fn codes_of(&self, machine: usize, provider: &str, kind: &str) -> Vec<String> {
        self.account_feed(machine)
            .into_iter()
            .flat_map(|f| &f.accounts)
            .filter(|a| a.provider == provider && a.kind == kind)
            .map(|a| a.code.clone())
            .collect()
    }

    /// The usage windows to show for an account (crate::accounts::shown_windows), and the
    /// provider description they were read by: none for a reading from an older feed.
    fn account_shown(&self, a: &Account) -> (Vec<Window>, Option<&Provider>) {
        let provider = self.all_providers().get(&a.provider);
        let (usage, provider) = match self.account_reading(a) {
            Some((u, described)) => (Some(u), provider.filter(|_| described)),
            None => (a.usage.as_ref(), provider),
        };
        (crate::accounts::shown_windows(usage, provider), provider)
    }

    pub fn account_windows(&self, a: &Account) -> Vec<Window> {
        self.account_shown(a).0
    }

    /// The windows its provider can have that this account's plan lacks, by name.
    pub fn account_lacking(&self, a: &Account) -> Vec<String> {
        let (windows, provider) = self.account_shown(a);
        crate::accounts::lacking(provider, &windows)
    }

    /// An account's figures, one per window it has, from its freshest reading on any machine:
    /// `86│100`, or `7 this week` for a plan with only a weekly limit.
    pub fn account_numbers(&self, a: &Account) -> String {
        self.account_figures(a, Figures::Worded)
    }

    pub fn account_figures(&self, a: &Account, figures: Figures) -> String {
        crate::accounts::numbers(&self.account_windows(a), figures, self.now())
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
            // On the owner split, the machine the chosen owner's agents run on, even before its
            // accounts are read: never another machine's in its place (intent 0595).
            .or_else(|| self.owner_machine())
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
            .filter(|&i| self.account(i, code).is_some_and(|x| x.needs_signin()))
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
                .account(i, code)
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

    /// The feed's record of the agent at this card that matters: the one waiting for usage, else
    /// the one in the card's own tab, else the first (friction F-0378: a workspace with Codex in
    /// its first tab and Claude Code out of usage in its second is about the second).
    pub fn account_agent(&self, card: &Card) -> Option<&AccountAgent> {
        let feed = self.account_feed(card.machine_index)?;
        let session = card.session.as_deref()?;
        let records: Vec<(&AccountAgent, &str)> = card
            .agents
            .iter()
            .filter_map(|a| {
                feed.agent_on_pane(session, &a.pane_id, a.conversation.as_deref())
                    .map(|r| (r, a.pane_id.as_str()))
            })
            .collect();
        let own = self.jump_tab(card).0;
        let in_own = |pane: &str| own.is_some() && self.agent_tab(card, pane) == own;
        records
            .iter()
            .find(|(r, _)| r.waiting_for_usage())
            .or_else(|| records.iter().find(|(_, pane)| in_own(pane)))
            .or_else(|| records.first())
            .map(|(r, _)| *r)
    }

    /// The herdr tab a pane of this card's is in, from its machine's last read.
    pub fn agent_tab(&self, card: &Card, pane: &str) -> Option<String> {
        self.machines
            .get(card.machine_index)?
            .snapshot(card.session.as_deref()?)?
            .agents
            .iter()
            .find(|a| a.pane_id == pane)
            .and_then(|a| a.tab_id.clone())
    }

    /// The agents at this card s could move, each with the tab it's in: the feed's record of each
    /// pane, or one with nothing recorded yet of a program the machine's accounts run. The engine
    /// moves that by its pane, takes its conversation from herdr and records it from then on; moving
    /// is how an account gets set (intent 0264).
    fn movable_agents(&self, card: &Card) -> Vec<(AccountAgent, Option<String>)> {
        let Some(feed) = self.account_feed(card.machine_index) else {
            return Vec::new();
        };
        let Some(session) = card.session.clone() else {
            return Vec::new();
        };
        let programs = self.offered(card.machine_index).programs();
        card.agents
            .iter()
            .filter(|a| !a.pane_id.is_empty())
            .filter_map(|a| {
                let agent =
                    match feed.agent_on_pane(&session, &a.pane_id, a.conversation.as_deref()) {
                        Some(r) => r.clone(),
                        None if programs.contains(&a.agent) => AccountAgent {
                            pane: a.pane_id.clone(),
                            session: session.clone(),
                            conversation: a.conversation.clone().unwrap_or_default(),
                            program: a.agent.clone(),
                            ..AccountAgent::default()
                        },
                        None => return None,
                    };
                Some((agent, self.agent_tab(card, &a.pane_id)))
            })
            .collect()
    }

    /// The agent s moves (friction F-0378): the one waiting for usage, else the one in the tab ⏎
    /// lands on for this card, else the only one. Never the first of several in different tabs
    /// when nothing says which: [`App::agent_unclear`] says why instead.
    pub fn move_agent(&self, card: &Card) -> Option<AccountAgent> {
        let agents = self.movable_agents(card);
        let waiting: Vec<&(AccountAgent, Option<String>)> = agents
            .iter()
            .filter(|(a, _)| a.waiting_for_usage())
            .collect();
        let own = self.jump_tab(card).0;
        let pick = |among: &[&(AccountAgent, Option<String>)]| -> Option<AccountAgent> {
            if let [one] = among {
                return Some(one.0.clone());
            }
            if let Some(own) = &own
                && let Some(a) = among.iter().find(|(_, tab)| tab.as_ref() == Some(own))
            {
                return Some(a.0.clone());
            }
            // Several in one known tab are one place: the first, as herdr lists them. A tab herdr
            // didn't name (an older herdr) says nothing about where it is, so it's never taken for
            // the same tab as another (Codex's review).
            let first = among.first().and_then(|(_, t)| t.as_ref())?;
            among
                .iter()
                .all(|(_, t)| t.as_ref() == Some(first))
                .then(|| among[0].0.clone())
        };
        if !waiting.is_empty() {
            return pick(&waiting);
        }
        let all: Vec<&(AccountAgent, Option<String>)> = agents.iter().collect();
        pick(&all)
    }

    /// Why s doesn't move any of this card's agents when it has several it could, in different
    /// tabs, and nothing says which is its own (friction F-0378): never a guess.
    pub(super) fn agent_unclear(&self, card: &Card) -> Option<String> {
        let agents = self.movable_agents(card);
        if agents.len() < 2 || self.move_agent(card).is_some() {
            return None;
        }
        let named: Vec<String> = agents
            .iter()
            .map(|(a, tab)| {
                let program = self
                    .agent_program(card.machine_index, a)
                    .unwrap_or_else(|| "an agent".into());
                match tab.as_deref().and_then(|t| t.rsplit_once(":t")) {
                    Some((_, n)) => format!("{program} in tab {n}"),
                    None => program,
                }
            })
            .collect();
        Some(format!(
            "{}'s workspace runs {}, and nothing says which is its own, so s doesn't guess which to move: ⏎ goes in, where you can see which one needs it.",
            card.reference,
            named.join(" and ")
        ))
    }

    /// The program an agent runs, in words: what the feed records, else herdr's name for it.
    pub fn agent_program(&self, machine: usize, agent: &AccountAgent) -> Option<String> {
        (!agent.program.is_empty()).then(|| self.providers(machine).program_label(&agent.program))
    }

    /// What `+` suggests for a new account of this provider: the viewed session's owner's code
    /// (else the machine owner's, else the next organization's whose code isn't taken), `_` and
    /// the provider's letter, and that owner's name with the provider's.
    fn suggested_account(&self, machine: usize, provider: &Provider) -> (String, String) {
        let Some(feed) = self.account_feed(machine) else {
            return Default::default();
        };
        let viewed = self.viewed_owner().filter(|(mi, _)| *mi == machine);
        let machine_owner = feed
            .machine_owner
            .as_ref()
            .and_then(|id| feed.owners.iter().find(|o| &o.id == id));
        let owners = viewed
            .map(|(_, o)| o)
            .into_iter()
            .chain(machine_owner.cloned())
            .chain(feed.owners.iter().filter(|o| o.parent.is_none()).cloned());
        let mut first = None;
        for o in owners {
            let Some(code) = o.code.as_deref().filter(|c| !c.is_empty()) else {
                continue;
            };
            let name = format!("{} {}", o.name, provider.name());
            if provider.letter.is_empty() {
                return (String::new(), name);
            }
            let code = format!("{code}_{}", provider.letter);
            if !self.code_taken(&code) {
                return (code, name);
            }
            first.get_or_insert((String::new(), name));
        }
        first.unwrap_or_default()
    }

    fn open_add(&mut self, machine: usize, provider: Provider) {
        // Only machines whose own backend can add this provider: an older one can't sign in a
        // provider it doesn't know.
        let machines: Vec<_> = self
            .machines
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_shown() && m.machine.feed.is_some())
            .map(|(i, _)| i)
            .filter(|&i| i == machine || self.offered(i).list().iter().any(|p| p.id == provider.id))
            .collect();
        let chosen = machines
            .iter()
            .map(|&i| self.machines[i].conn.is_live())
            .collect();
        // The first kind no account of this provider has yet: a second "this machine's own
        // sign-in" is rarely what's wanted.
        let kind = provider
            .kinds
            .iter()
            .position(|k| self.codes_of(machine, &provider.id, k).is_empty())
            .unwrap_or(0);
        let suggested = self.suggested_account(machine, &provider);
        self.push_account_screen(AccountScreen::Add(Box::new(AddAccount {
            machine,
            code: String::new(),
            name: String::new(),
            suggested,
            kind,
            extra: vec![String::new(); provider.fields.len()],
            provider,
            field: 0,
            machines,
            chosen,
        })));
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

    pub(super) fn push_account_screen(&mut self, screen: AccountScreen) {
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
        if let Scope::Session(machine, session) = self.scope() {
            let mi = self.machine_index(machine)?;
            return Some((mi, self.account_owner(mi, session)?.clone()));
        }
        // The owner the top bar is on (intent 0595), from the feed of the machine its agents run
        // on, else of any machine that lists it.
        let id = self.scope().owner()?;
        let listed = |mi: usize| {
            self.account_feed(mi)
                .and_then(|f| f.owners.iter().find(|o| o.id == id))
                .map(|o| (mi, o.clone()))
        };
        self.owner_machine()
            .and_then(listed)
            .or_else(|| (0..self.machines.len()).find_map(listed))
    }

    /// Whose list S edits: the viewed session's owner when [ ] chose a session, or the owner the
    /// top bar is on, else the selected card's (intent 0580), so it never asks for a session
    /// first. A card's owner is its session's, else its home session's, else the tenant or
    /// organization its record names.
    pub fn models_owner(&self) -> Option<(usize, Owner)> {
        if matches!(self.scope(), Scope::Session(..)) {
            return self.viewed_owner();
        }
        if self
            .scope()
            .owner()
            .is_some_and(|o| o != crate::owners::UNOWNED)
            && let Some(found) = self.viewed_owner()
        {
            return Some(found);
        }
        self.card_owner(self.selected_for_control()?)
    }

    fn card_owner(&self, card: &Card) -> Option<(usize, Owner)> {
        let home = card.detail.as_ref().and_then(|d| d.home.as_ref());
        // Its home session is looked up only on its own home machine: the same name on another
        // machine can belong to someone else. A home machine Kerdr doesn't have gives no session.
        let home_machine = match home.and_then(|h| h.machine.as_deref()) {
            Some(name) => self.machine_index(name),
            None => Some(card.machine_index),
        };
        let sessions = [
            (Some(card.machine_index), card.session.as_deref()),
            (home_machine, home.and_then(|h| h.session.as_deref())),
        ];
        for (mi, session) in sessions {
            if let (Some(mi), Some(session)) = (mi, session)
                && let Some(owner) = self.account_owner(mi, session)
            {
                return Some((mi, owner.clone()));
            }
        }
        let machine = home_machine.unwrap_or(card.machine_index);
        let d = card.detail.as_ref()?;
        let named = [d.tenant.as_deref(), d.organization.as_deref()];
        let feed = self.account_feed(machine)?;
        named
            .into_iter()
            .flatten()
            .filter(|n| !n.is_empty())
            .find_map(|n| {
                feed.owners
                    .iter()
                    .find(|o| o.id.eq_ignore_ascii_case(n) || o.name.eq_ignore_ascii_case(n))
            })
            .map(|o| (machine, o.clone()))
    }

    /// What S says when there's no list to open: never "choose a session first".
    fn no_models_owner_said(&self) -> String {
        match self.selected_for_control() {
            None if matches!(self.scope(), Scope::Session(..)) => {
                "This session has no owner with a list of models yet.".into()
            }
            None => "Select a card first: S edits the list of models its owner's agents start on."
                .into(),
            Some(card) => format!(
                "{}'s machine hasn't said who owns it, so there's no list of models to edit yet.",
                card.reference
            ),
        }
    }

    /// Whether the machine's backend has a current account to show and set; an older one says
    /// so when c or C is pressed, and nothing is sent.
    fn knows_current(&mut self, machine: usize) -> bool {
        if self
            .account_feed(machine)
            .is_some_and(|f| f.knows_current())
        {
            return true;
        }
        self.say(
            "This machine's backend has no current account yet: update it to set one here.",
            Tone::Info,
        );
        false
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

    /// Whether a question the backend asked about this move (with a token of its own) may still be
    /// put and answered. As target_current, except that an agent the picker saw on its first turn,
    /// with no conversation yet (and so no account on record), is the same agent once herdr learns
    /// its conversation: the backend's token names that agent's process, and refuses there one that
    /// replaced it in the pane (intent 0591, Codex's review).
    fn target_answerable(&self, target: &MoveTarget) -> bool {
        if self.target_current(target) {
            return true;
        }
        if !target.agent.conversation.is_empty() {
            return false;
        }
        self.card(&target.key)
            .filter(|c| c.machine_index == target.machine)
            .and_then(|c| self.move_agent(c))
            .is_some_and(|a| {
                a.pane == target.agent.pane
                    && a.session == target.agent.session
                    && (target.agent.account.is_empty() || a.account == target.agent.account)
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
        let paste = self
            .signin_takes_paste(machine, &code, copy.as_ref())
            .unwrap_or(false);
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
            user_code: None,
            expires: None,
            page_code: String::new(),
            editing: false,
            poll_at: Instant::now() + Duration::from_secs(2),
            failed: false,
            message: "Starting sign-in…".into(),
            paste,
            note: None,
            recheck: false,
            added: None,
        };
        let request = signin.start();
        self.push_account_screen(AccountScreen::Signin(Box::new(signin)));
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

    /// Whether p offers a pasted token for this sign-in on `machine`, by that machine's feed;
    /// None for an account not in it yet (just added: its add form says).
    fn signin_takes_paste(
        &self,
        machine: usize,
        code: &str,
        copy: Option<&(String, String)>,
    ) -> Option<bool> {
        let a = self.account(machine, code)?;
        let c = copy.and_then(|(id, _)| a.copies.iter().find(|c| &c.id == id));
        Some(self.offered(machine).takes_paste(a, c))
    }

    /// The same for an account just added, which no feed has yet, from its provider and kind.
    fn added_takes_paste(&self, machine: usize, code: &str, added: &(String, String)) -> bool {
        self.offered(machine).takes_paste(
            &Account {
                code: code.to_string(),
                provider: added.0.clone(),
                kind: added.1.clone(),
                ..Account::default()
            },
            None,
        )
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
        // On a card's own screen, only s: its selected row's agent.
        let card_screen = matches!(self.mode, Mode::Card { .. }) && key.code == KeyCode::Char('s');
        if !card_screen
            && !matches!(
                self.mode,
                Mode::Board
                    | Mode::Peek { .. }
                    | Mode::Backlog { .. }
                    | Mode::Misc { .. }
                    | Mode::Done { .. }
            )
        {
            return None;
        }
        match key.code {
            KeyCode::Char('A') => self.open_accounts(),
            KeyCode::Char('S') => {
                if let Some((machine, owner)) = self.models_owner() {
                    self.push_account_screen(AccountScreen::Models {
                        machine,
                        models: owner.models.clone(),
                        owner,
                        sel: 0,
                        move_waiting: false,
                    });
                } else {
                    self.say(self.no_models_owner_said(), Tone::Info);
                }
            }
            KeyCode::Char('s') => {
                if let Some(card) = self.selected_for_control().cloned() {
                    self.s_on_card(card);
                } else {
                    self.say(
                        "Select a card first: s starts or moves its agent.",
                        Tone::Info,
                    );
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
                AccountScreen::Add(_)
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
                    // C takes the current account off, from any line (intent 0455). It always asks: the
                    // backend's answer, not this machine's feed, says whether one was set.
                    KeyCode::Char('C') => {
                        if self.knows_current(*machine) {
                            request =
                                Some((*machine, json!({"action":"account-current","clear":true})))
                        }
                    }
                    KeyCode::Char('+') => {
                        let machine = *machine;
                        let mut choices = self.offered(machine).list();
                        self.mode = Mode::Accounts { screen, back };
                        match choices.len() {
                            0 => self.say(
                                "This machine's feed names no provider to add an account of.",
                                Tone::Warn,
                            ),
                            1 => self.open_add(machine, choices.remove(0)),
                            _ => self.push_account_screen(AccountScreen::Provider {
                                machine,
                                choices,
                                sel: 0,
                            }),
                        }
                        return Action::None;
                    }
                    _ => {
                        let copy_id = match &row {
                            Some(Row::Copy(_, id)) => Some(id.clone()),
                            _ => None,
                        };
                        match row {
                            Some(Row::Account(code) | Row::Copy(code, _)) => {
                                let a = self.account(*machine, &code).cloned();
                                let copy = copy_id.and_then(|id| {
                                    let c = a.as_ref()?.copy(&id)?;
                                    Some((c.id.clone(), c.name().to_string()))
                                });
                                if let Some(a) = a {
                                    match key.code {
                                        // A provider with no sign-in (a local model) is tested.
                                        KeyCode::Enter
                                            if !self.offered(*machine).signs_in(&a.provider) =>
                                        {
                                            request = Some((
                                                *machine,
                                                for_copy(
                                                    json!({"action":"account-test","code":code}),
                                                    copy.as_ref().map(|(id, _)| id.as_str()),
                                                ),
                                            ))
                                        }
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
                                        // c makes it the current account, for every
                                        // machine (intent 0455); the backend says if it already was.
                                        KeyCode::Char('c') => {
                                            if self.knows_current(*machine) {
                                                request = Some((
                                                    *machine,
                                                    json!({"action":"account-current","to":code}),
                                                ));
                                            }
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
            AccountScreen::Start {
                key: card,
                start,
                choices,
                sel,
                ..
            } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(to) = choices.get(*sel).cloned() {
                        let (card, start) = (card.clone(), start.clone());
                        // Back where s was pressed, so the start answers as ⏎'s or d's would.
                        self.mode = *back;
                        return self.start_on_account(&card, &start, &to);
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
            AccountScreen::Provider {
                machine,
                choices,
                sel,
            } => match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(provider) = choices.get(*sel).cloned() {
                        let machine = *machine;
                        // The form replaces the question, so Esc from it goes back to the list.
                        self.mode = *back;
                        self.open_add(machine, provider);
                        return Action::None;
                    }
                }
                _ => {}
            },
            AccountScreen::Add(add) => {
                let rows = add.rows();
                let at = add.field;
                let kinds = add.kinds_at()..add.machines_at();
                match key.code {
                    KeyCode::Tab => add.field = (at + 1) % rows,
                    KeyCode::BackTab => add.field = (at + rows - 1) % rows,
                    KeyCode::Char(' ') if kinds.contains(&at) => add.kind = at - kinds.start,
                    KeyCode::Char(' ') if at >= kinds.end => {
                        add.chosen[at - kinds.end] = !add.chosen[at - kinds.end]
                    }
                    KeyCode::Backspace if at < kinds.start => {
                        if let Some(value) = add.text_mut(at) {
                            value.pop();
                        }
                    }
                    KeyCode::Char(c)
                        if at < kinds.start && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let Some(value) = add
                            .text_mut(at)
                            .filter(|v| v.chars().count() < AddAccount::limit(at))
                        {
                            value.push(c);
                        }
                    }
                    KeyCode::Enter => {
                        let example = if add.provider.letter.is_empty() {
                            "2–4 capitals".to_string()
                        } else {
                            format!(
                                "the owner's code, _ and {} (like KBZ_{})",
                                add.provider.letter, add.provider.letter
                            )
                        };
                        let (answers, missing) = add.answers();
                        if !add.code_ok() || add.name().is_empty() || !add.chosen.contains(&true) {
                            self.say(
                                format!(
                                    "Use a code of {example}, a name, and at least one machine."
                                ),
                                Tone::Warn,
                            );
                        } else if let Some(missing) = missing {
                            self.say(
                                format!("{} needs {missing}.", add.provider.name()),
                                Tone::Warn,
                            );
                        } else if self.code_taken(add.code()) {
                            self.say(
                                "That code exists. Select its account and press e to rename it.",
                                Tone::Warn,
                            );
                        } else {
                            let mut value = json!({"action":"account-add","code":add.code(),"name":add.name(),"provider":add.provider.id});
                            // A provider with no kinds leaves the kind to its backend.
                            if !add.kind().is_empty() {
                                value["kind"] = json!(add.kind());
                            }
                            if !answers.is_empty() {
                                value["fields"] = Value::Object(answers);
                            }
                            request = Some((add.machine, value));
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
                    if let Some(a) = self.account(*machine, code) {
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
                // A backend that describes its providers checks a code itself and waits on.
                let checks_codes = match self.account(machine, &s.code) {
                    Some(a) => self.offered(machine).checks_codes_here(&a.provider),
                    None => !self.offered(machine).listed(),
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
                    // Where the page takes a code, c copies that code and l the link.
                    KeyCode::Char('c') if !s.editing && s.user_code.is_some() => {
                        return s.user_code.clone().map_or(Action::None, Action::CopyCode);
                    }
                    KeyCode::Char('c' | 'l') if !s.editing => {
                        return s.url.clone().map_or(Action::None, Action::CopyLink);
                    }
                    KeyCode::Char('o') if !s.editing => {
                        return s.url.clone().map_or(Action::None, Action::OpenLink);
                    }
                    KeyCode::Char('r') if !s.editing && (s.failed || s.expired(self.now())) => {
                        s.failed = false;
                        s.url = None;
                        s.page_code.clear();
                        s.needs_code = false;
                        s.user_code = None;
                        s.expires = None;
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
                            } else if checks_codes && emailed_code(s.page_code.trim()) {
                                // Kept here: an older backend would end the sign-in on it.
                                s.note = Some(EMAILED_CODE.into());
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
                    if target.as_ref().is_none_or(|t| self.target_answerable(t)) {
                        // A yes is a new request: never the question's op (intent 0591).
                        request = Some((*machine, crate::actions::without_op(pending)));
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
                AccountScreen::Add(add) if add.field < add.kinds_at() => {
                    let at = add.field;
                    let limit = AddAccount::limit(at);
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if text.chars().count() > limit {
                        self.say(
                            format!("That's longer than {limit} characters, so it wasn't pasted."),
                            Tone::Warn,
                        );
                    } else if let Some(value) = add.text_mut(at) {
                        *value = text;
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
        // The answer's request carries the op Kerdr gave it as it went (intent 0591).
        let pending = self.account_pending.take().filter(|p| {
            p.machine == machine
                && crate::actions::without_op(&p.request)
                    == crate::actions::without_op(&crate::actions::request_metadata(&request))
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
        if action == "account-current" && response.ok {
            // Shown at once on the machine asked; the others' feeds follow as their machines do.
            let current = if request["clear"] == true {
                None
            } else {
                response.current.clone()
            };
            if let Some(feed) = self.machines.get_mut(machine).and_then(|m| m.feed.as_mut())
                && (current.is_some() || request["clear"] == true)
            {
                feed.current = Some(current);
            }
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
                        let needs = self.account(mi, &s.code).is_none_or(|a| a.needs_signin());
                        (needs, self.machines[mi].name())
                    })
                    .collect(),
                _ => Vec::new(),
            };
            // The paste rule for each machine in turn, read before the screen is borrowed.
            let paste_next: Vec<Option<bool>> = match &self.mode {
                Mode::Accounts {
                    screen: AccountScreen::Signin(s),
                    ..
                } => s
                    .machines
                    .iter()
                    .map(|&mi| {
                        self.signin_takes_paste(mi, &s.code, s.copy.as_ref())
                            .or_else(|| {
                                let added = s.added.as_ref()?;
                                Some(self.added_takes_paste(mi, &s.code, added))
                            })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if let Mode::Accounts {
                screen: AccountScreen::Signin(s),
                ..
            } = &mut self.mode
            {
                if request["step"] == "finish" && !response.ok && response.status.is_none() {
                    // The backend refused that code and waits on for the right one: say why
                    // under the box, and keep it open.
                    s.note = Some(response.message.clone());
                    s.editing = true;
                    s.poll_at = Instant::now() + Duration::from_secs(2);
                    self.keep_error(&response.message, "warn");
                    return;
                }
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
                    s.user_code = None;
                    s.expires = None;
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
                        // Each machine's own feed says whether its sign-in takes a token.
                        if let Some(Some(paste)) = paste_next.get(s.current) {
                            s.paste = *paste;
                        }
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
                    } else if response
                        .user_code
                        .as_deref()
                        .is_some_and(|c| !user_code_ok(c))
                    {
                        // A code to type that isn't one (letters, digits and dashes, 4–32) is
                        // never shown or copied: without it the sign-in can't finish.
                        s.failed = true;
                        s.message = "The sign-in gave a code to type that Kerdr won't show. Press r to start again, or sign in on that machine itself.".into();
                        if let Some(ticket) = response.ticket {
                            self.account_queue.push_back((
                                machine,
                                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                            ));
                        }
                    } else {
                        s.ticket = response.ticket;
                        s.url = response.url;
                        s.user_code = response.user_code;
                        s.expires = response.expires;
                        s.needs_code = response.needs_code && s.user_code.is_none();
                        self.signin_open = s.url.clone();
                    }
                }
                s.poll_at = Instant::now() + Duration::from_secs(2);
            }
            return;
        }
        // Its agent wasn't running, so the backend started it on the account picked instead, as a
        // reopen (intent 0591): taken as that reopen's answer, followed while it goes on, and gone
        // into once it's done while you're still on its card.
        if action == "account-move"
            && response.ok
            && !response.confirmation
            && (response.follow.is_some()
                || (response.session.is_some() && response.workspace.is_some()))
        {
            let key = match pending.as_ref().and_then(|p| p.screen.as_ref()) {
                Some(AccountScreen::Move { target, .. }) => Some(target.key.clone()),
                _ => None,
            };
            let id = response
                .follow
                .as_ref()
                .and_then(|f| f.id.clone())
                .or_else(|| response.id.clone())
                .or_else(|| {
                    key.as_deref()
                        .and_then(|k| self.card(k))
                        .and_then(|c| c.detail.as_ref())
                        .map(|d| d.id.clone())
                });
            if let Some(id) = id {
                if visible && let Mode::Accounts { back, .. } = self.mode.clone() {
                    self.mode = *back;
                }
                let mut reopen = json!({"action":"reopen","id":id});
                if let Some(to) = request["to"].as_str() {
                    reopen["account"] = json!(to);
                }
                // Where you were, only while its picker is still open: once Esc has closed it, the
                // start still goes on and is followed, but its answer takes you nowhere (Codex's
                // review).
                self.action_from = key
                    .as_deref()
                    .filter(|k| visible && self.sel.key.as_deref() == Some(*k))
                    .and_then(|k| self.card(k))
                    .map(CardSnapshot::of);
                self.queue_action(machine, reopen.clone());
                let n = self.resolver.last_added();
                self.finish_job(n, reopen, Ok(response));
                return;
            }
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
                    let name = self.machines[machine].name();
                    self.fail(format!(
                        "{name}'s backend asked the same question again after your yes, so nothing moved. That's a fault there, and it's in the error log; s on the card offers the move again."
                    ));
                    return;
                }
                let target = target.clone();
                // A question about the start made because its agent wasn't running (it names the
                // intent or its session, as a start's question does; a move's names neither) is
                // about that start, not the agent: its yes isn't refused once the board shows the
                // agent gone (intent 0591).
                let starts = response.id.is_some() || response.session.is_some();
                if starts || self.target_answerable(&target) {
                    let mut request = request;
                    request["confirm"] = json!(token);
                    self.push_account_screen(AccountScreen::Confirm {
                        machine,
                        request,
                        message: response.message,
                        yes: response.yes.unwrap_or_else(|| "move when it stops".into()),
                        no: response.no.unwrap_or_else(|| "leave it".into()),
                        target: (!starts).then_some(target),
                        ready_at: Instant::now() + Duration::from_millis(500),
                    });
                    return;
                }
            }
        } else if response.ok && visible {
            match pending.and_then(|p| p.screen) {
                Some(AccountScreen::Add(add)) => {
                    let selected = add
                        .machines
                        .iter()
                        .zip(&add.chosen)
                        .filter_map(|(m, c)| c.then_some(*m))
                        .collect::<Vec<_>>();
                    // Pop details before opening the sequence; the account now exists.
                    if let Mode::Accounts { back, .. } = self.mode.clone() {
                        self.mode = *back;
                    }
                    // A provider with no sign-in (a local model) is ready to test once added.
                    if add.provider.signin.is_none() && self.offered(machine).listed() {
                        self.say(
                            format!(
                                "Added {}. {} accounts don't sign in: t tests it, and S adds it to a list.",
                                add.code(),
                                add.provider.name()
                            ),
                            Tone::Good,
                        );
                        return;
                    }
                    let added = (add.provider.id.clone(), add.kind().to_string());
                    // Its first machine's own backend says whether it takes a token there.
                    let paste = selected.first().is_some_and(|&mi| {
                        self.signin_takes_paste(mi, add.code(), None)
                            .unwrap_or_else(|| self.added_takes_paste(mi, add.code(), &added))
                    });
                    let action = self.begin_signin(add.code().to_string(), None, selected);
                    if let Mode::Accounts {
                        screen: AccountScreen::Signin(s),
                        ..
                    } = &mut self.mode
                    {
                        s.paste = paste;
                        s.added = Some(added);
                    }
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

/// A code to type on a provider's page, as sign-ins give them (`K7QD-3MPWX`): nothing else is
/// shown as one or sent to the clipboard.
pub fn user_code_ok(code: &str) -> bool {
    (4..=32).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
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
        (AccountScreen::Add(a), AccountScreen::Add(b)) => {
            a.code() == b.code() && a.provider.id == b.provider.id
        }
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
        let mut a = App::new(Config::machine_split(), crate::demo::machines(false));
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
        // As start_action sends it: an account request goes with an op of its own (intent 0591).
        let r = crate::actions::with_op(r);
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
    /// Signs in ARK_C, a token account, on the laptop, where it needs it: KBZ_C is a saved sign-in,
    /// which takes no pasted token.
    fn token_signin(a: &mut App) -> Action {
        set_status(a, 0, "ARK_C", "sign_in");
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
        assert_eq!(rows[*sel], Row::Account("ARK_C".into()));
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
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C"})
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
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C","confirm":"opaque-ticket"})
        );
        assert!(r.get("override").is_none());
    }
    const FOLLOW: &str = "f-0123456789abcdef0123";

    /// #44's key and record id.
    fn card_44(a: &App) -> (String, String) {
        let c = a.board.cards.iter().find(|c| c.reference == "#44").unwrap();
        (c.key.clone(), c.detail.as_ref().unwrap().id.clone())
    }

    /// The account move s sends on #44 for ARK_C, its Move picker open.
    fn move_44(a: &mut App) -> Action {
        pick(a);
        press(a, 's');
        let action = key(a, KeyCode::Enter);
        let (_, r) = sent(action.clone());
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C"})
        );
        action
    }

    /// The backend's answer when its agent wasn't running: the start it made instead, followed.
    fn started_instead(id: &str) -> Value {
        json!({"ok":true,
               "message":"Its agent wasn't running, so it's being started on ARK_C: Starting api; ⏎ again in a minute goes in.",
               "id":id,"session":"api",
               "follow":{"token":FOLLOW,"what":"start","id":id,"session":"api","say":"Starting api, then #44…","limit":300}})
    }

    /// Followed work's end, as its progress answer brings it.
    fn followed_to(a: &mut App, reply: Value) {
        for f in &mut a.follows {
            f.next = Instant::now();
        }
        let due = a.progress_due();
        assert_eq!(due.len(), 1, "one progress request due");
        assert_eq!(due[0].2, json!({"action":"progress","follow":FOLLOW}));
        assert!(a.progress_answered(
            FOLLOW,
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap())
        ));
    }

    /// Piece A's backend (intent 0591): s on a card whose agent turns out not to be running starts
    /// it on the account picked, as a reopen, and answers with `follow`. Kerdr follows it as that
    /// reopen and goes in once it's done, while you're still on the card.
    #[test]
    fn an_account_move_whose_agent_wasnt_running_is_followed_and_goes_in_when_done() {
        let mut a = app();
        let (k, id) = card_44(&a);
        let action = move_44(&mut a);
        complete(&mut a, action, started_instead(&id));
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        assert_eq!(a.follows.len(), 1, "followed");
        assert_eq!(
            crate::actions::without_op(&a.follows[0].request),
            json!({"action":"reopen","id":id,"account":"ARK_C"})
        );
        assert_eq!(a.message().unwrap().0, "Starting api, then #44…");
        followed_to(
            &mut a,
            json!({"ok":true,"state":"done","message":"Reopened #44 in api on ARK_C.","id":id,
                   "session":"api","workspace":"w9","label":"44-again"}),
        );
        assert!(a.follows.is_empty());
        let open = a.open_after.clone().expect("in, once it's done");
        assert_eq!(
            (
                open.session.as_str(),
                open.workspace.as_str(),
                open.label.as_str(),
                open.from.as_str()
            ),
            ("api", "w9", "44-again", k.as_str())
        );
    }

    /// The same start answered at once with its workspace (no follow): in, as on a reopen's answer.
    #[test]
    fn an_account_move_answered_with_a_workspace_goes_in() {
        let mut a = app();
        let (k, id) = card_44(&a);
        let action = move_44(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":true,"message":"Its agent wasn't running, so it's being started on ARK_C: Reopened #44 in api.",
                   "id":id,"session":"api","workspace":"w9","label":"44-again"}),
        );
        assert!(a.follows.is_empty());
        let open = a.open_after.clone().expect("in, as on a reopen's answer");
        assert_eq!(
            (open.workspace.as_str(), open.from.as_str()),
            ("w9", k.as_str())
        );
    }

    /// That start's question (a full session, say) comes back with a token of its own and "yes" and
    /// "no": the yes goes back as the same account move with that token, even once the board shows
    /// the agent gone, and the start it answers is followed and gone into.
    #[test]
    fn a_question_from_a_start_made_for_a_move_round_trips_and_its_yes_goes_in() {
        let mut a = app();
        let (k, id) = card_44(&a);
        let action = move_44(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":false,"confirmation":true,"confirm":"cont-0123456789abcdef","yes":"yes","no":"no",
                   "id":id,"session":"api","used":2,"limit":2,
                   "message":"Its agent wasn't running, so it's being started on ARK_C: api has 2 workspaces. Start anyway?"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                yes, no, message, ..
            },
            ..
        } = &a.mode
        else {
            panic!("no question: {:?}", a.message());
        };
        assert_eq!((yes.as_str(), no.as_str()), ("yes", "no"));
        assert!(
            message.ends_with("api has 2 workspaces. Start anyway?"),
            "{message}"
        );
        // The board catches up: no agent runs it now.
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.agents.clear();
        }
        assert!(a.move_agent(a.card(&k).unwrap()).is_none());
        settle(&mut a);
        let yes = press(&mut a, 'y');
        let (_, r) = sent(yes.clone());
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C","confirm":"cont-0123456789abcdef"})
        );
        complete(&mut a, yes, started_instead(&id));
        assert_eq!(a.follows.len(), 1, "the start its yes made is followed");
        followed_to(
            &mut a,
            json!({"ok":true,"state":"done","message":"Started #44 in api on ARK_C.","id":id,
                   "session":"api","workspace":"w9","label":"44-again"}),
        );
        assert_eq!(a.open_after.clone().expect("in, once it's done").from, k);
    }

    /// s on an agent still on its first turn: the backend asks, where it used to queue the move.
    /// Its yes goes back as the same move with its token; the backend keeps the pid, its start and
    /// whether it hands over with that token, so the yes needs nothing else. Both wordings: a move,
    /// and a hand-over to another program.
    #[test]
    fn a_first_turn_question_round_trips_and_its_yes_queues_the_move() {
        for handover in [false, true] {
            let mut a = app();
            pick(&mut a);
            press(&mut a, 's');
            if handover {
                press(&mut a, 'j');
            }
            let action = key(&mut a, KeyCode::Enter);
            let (_, asked) = sent(action.clone());
            let to = asked["to"].as_str().unwrap().to_string();
            let (text, yes_label) = if handover {
                (
                    format!(
                        "It's still on its first turn, so herdr doesn't know its conversation yet. This agent runs Claude Code. {to} runs Codex, so its conversation can't carry over: a fresh Codex agent starts in the same pane and picks up from the intent's log, its pinboard and the end of this conversation. Hand it over to Codex once that turn ends?"
                    ),
                    "hand over when it stops",
                )
            } else {
                (
                    format!(
                        "It's still on its first turn, so herdr doesn't know its conversation yet. Move it to {to} once that turn ends? It won't be cut off."
                    ),
                    "move when it stops",
                )
            };
            complete(
                &mut a,
                action,
                json!({"ok":false,"confirmation":text,"yes":yes_label,"no":"leave it",
                       "confirm":"turn-0123456789abcdef","message":text}),
            );
            let Mode::Accounts {
                screen:
                    AccountScreen::Confirm {
                        yes, no, message, ..
                    },
                ..
            } = &a.mode
            else {
                panic!("no question: {:?}", a.message());
            };
            assert_eq!((yes.as_str(), no.as_str()), (yes_label, "leave it"));
            assert_eq!(message, &text);
            settle(&mut a);
            let yes = press(&mut a, 'y');
            let (_, r) = sent(yes.clone());
            let mut expected = asked.clone();
            expected["confirm"] = json!("turn-0123456789abcdef");
            assert_eq!(r, expected, "the same move, with its token");
            let word = if handover { "hands over" } else { "moves" };
            let said = format!(
                "it's still on its first turn, so herdr doesn't know its conversation yet: it {word} to {to} once that turn ends"
            );
            complete(
                &mut a,
                yes,
                json!({"ok":true,"state":"queued","message":said}),
            );
            assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
            assert!(a.follows.is_empty());
            assert_eq!(a.message().unwrap().0, said);
        }
    }

    /// Codex's review: an agent on its first turn, which the picker saw with no conversation, whose
    /// turn ends (herdr learns its conversation) before the question comes, or before y. Its
    /// question is still put, and its yes still goes, with its token: the backend's token names the
    /// agent's process, and refuses there one that replaced it.
    #[test]
    fn a_first_turn_question_still_answers_once_the_agents_conversation_is_known() {
        for before_question in [true, false] {
            let mut a = app();
            pick(&mut a);
            // On its first turn: no record of it yet, and herdr doesn't know its conversation.
            a.machines[1]
                .feed
                .as_mut()
                .unwrap()
                .agents
                .retain(|r| !(r.session == "api" && r.pane == "w1:p1"));
            let k = a.sel.key.clone().unwrap();
            for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
                for g in c.agents.iter_mut().filter(|g| g.pane_id == "w1:p1") {
                    g.conversation = None;
                }
            }
            press(&mut a, 's');
            let action = key(&mut a, KeyCode::Enter);
            let (_, asked) = sent(action.clone());
            assert_eq!(asked["conversation"], "", "the picker saw no conversation");
            let to = asked["to"].as_str().unwrap().to_string();
            if before_question {
                set_conversation(&mut a, "c-known");
            }
            let text = format!(
                "It's still on its first turn, so herdr doesn't know its conversation yet. Move it to {to} once that turn ends? It won't be cut off."
            );
            complete(
                &mut a,
                action,
                json!({"ok":false,"confirmation":text,"yes":"move when it stops","no":"leave it",
                       "confirm":"turn-0123456789abcdef","message":text}),
            );
            assert!(
                matches!(
                    a.mode,
                    Mode::Accounts {
                        screen: AccountScreen::Confirm { .. },
                        ..
                    }
                ),
                "the question wasn't put: {:?}",
                a.message()
            );
            if !before_question {
                set_conversation(&mut a, "c-known");
            }
            settle(&mut a);
            let (_, r) = sent(press(&mut a, 'y'));
            let mut expected = asked.clone();
            expected["confirm"] = json!("turn-0123456789abcdef");
            assert_eq!(r, expected);
        }
        // An agent the picker saw with its conversation, holding another one now, is still refused.
        let mut a = app();
        move_question(&mut a);
        set_conversation(&mut a, "c-another");
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(a.message().unwrap().0.contains("conversation changed"));
    }

    /// Codex's review: Esc on the picker while its move is on its way, and the backend starts the
    /// card instead. The start goes on and is followed, but its answer never takes you anywhere:
    /// neither one answered at once with its workspace, nor a followed one when it's done.
    #[test]
    fn escaping_the_picker_lets_the_start_made_instead_go_on_without_taking_you_anywhere() {
        for followed in [false, true] {
            let mut a = app();
            let (_, id) = card_44(&a);
            let action = move_44(&mut a);
            key(&mut a, KeyCode::Esc);
            assert!(matches!(a.mode, Mode::Board));
            if followed {
                complete(&mut a, action, started_instead(&id));
                assert_eq!(a.follows.len(), 1, "still followed");
                assert!(a.enter_when_open.is_none(), "no ⏎ remembered");
                followed_to(
                    &mut a,
                    json!({"ok":true,"state":"done","message":"Reopened #44 in api on ARK_C.","id":id,
                           "session":"api","workspace":"w9","label":"44-again"}),
                );
                assert!(a.follows.is_empty());
            } else {
                complete(
                    &mut a,
                    action,
                    json!({"ok":true,"message":"Its agent wasn't running, so it's being started on ARK_C: Reopened #44 in api.",
                           "id":id,"session":"api","workspace":"w9","label":"44-again"}),
                );
            }
            assert_eq!(a.open_after, None, "Esc'd: nowhere to go");
            assert!(a.enter_when_open.is_none());
            assert!(matches!(a.mode, Mode::Board));
            let said = a.message().unwrap().0.to_string();
            assert!(said.ends_with("⏎ on its card goes in."), "{said}");
        }
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
    fn s_says_why_when_a_card_has_no_agent_its_accounts_run() {
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
                g.agent = "some-other-agent".into();
            }
        }
        press(&mut a, 's');
        assert!(matches!(a.mode, Mode::Board));
        // The programs come from the feed's providers, in its words.
        let said = a.message().unwrap().0;
        assert!(said.contains("(they run Claude Code, Codex)"), "{said}");
        assert!(said.contains("second copy"), "{said}");
    }

    #[test]
    fn s_moves_a_codex_agent_with_no_record_and_names_its_program() {
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
        let Mode::Accounts {
            screen: AccountScreen::Move { target, .. },
            ..
        } = &a.mode
        else {
            panic!("s didn't open the picker: {:?}", a.message());
        };
        assert_eq!(target.agent.program, "codex");
        assert_eq!(
            a.agent_program(target.machine, &target.agent).as_deref(),
            Some("Codex")
        );
    }

    #[test]
    fn a_move_to_another_program_asks_as_a_hand_over() {
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        // The picker lists each account's provider; the agent's own account comes last.
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("KBZ_X   Codex"), "{screen}");
        assert!(screen.contains("(on it now)"), "{screen}");
        press(&mut a, 'j');
        let action = key(&mut a, KeyCode::Enter);
        let Action::Mutate { request, .. } = &action else {
            panic!("no move sent");
        };
        assert_eq!(request["to"], "KBZ_X");
        complete(
            &mut a,
            action,
            json!({"ok":false,"confirmation":"This agent runs Claude Code. KBZ_X runs Codex, so its conversation can't carry over. Hand it over to Codex?","yes":"hand it over","no":"leave it","confirm":"handover-token","message":"move?"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm { yes, request, .. },
            ..
        } = &a.mode
        else {
            panic!("no question: {:?}", a.message());
        };
        assert_eq!(yes, "hand it over");
        assert_eq!(request["confirm"], "handover-token");
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("over to Codex?"), "{screen}");
        assert!(screen.contains("y hand it over"), "{screen}");
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
            models: vec!["KBZ_C".into(), "ARK_C".into()],
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
            models: vec!["KBZ_C".into()],
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
        // The build box's Platform sessions: the ones off the list's first account (KBZ_C) first.
        assert!(said.contains(" on ARK_C"), "{said}");
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
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Models { models, sel: 1, .. }, .. } if models == &["KBZ_C", "ARK_C", "KBZ_X"])
        );
        assert_eq!(press(&mut a, 'd'), Action::None);
        let (_, request) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(request["models"], json!(["KBZ_C", "KBZ_X"]));
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
            models: vec!["ARK_C".into()],
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
        assert_eq!(models, &["ARK_C", "KBZ_C"]);
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
        let action = a.begin_signin("ARK_C".into(), None, vec![0, 1]);
        complete(&mut a, action, started());
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None);
        let action = a.begin_signin("ARK_C".into(), None, vec![0, 1]);
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
            json!({"action":"account-signin","code":"ARK_C","step":"start"})
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

    /// A feed where the pane's first record is one left behind by another conversation.
    fn stale_record_first(a: &mut App) {
        let feed = a.machines[1].feed.as_mut().unwrap();
        let real = feed
            .agents
            .iter()
            .position(|f| f.session == "api" && f.pane == "w1:p1")
            .unwrap();
        feed.agents[real].conversation = "c-real".into();
        feed.agents[real].account = "REAL_C".into();
        let mut stale = feed.agents[real].clone();
        stale.conversation = "c-left-behind".into();
        stale.account = "OLD_C".into();
        feed.agents.insert(0, stale);
    }
    #[test]
    fn a_card_shows_the_account_of_the_conversation_in_its_pane() {
        let mut a = app();
        pick(&mut a);
        stale_record_first(&mut a);
        set_conversation(&mut a, "c-real");
        let card = a.selected().unwrap();
        assert_eq!(a.account_agent(card).unwrap().account, "REAL_C");
    }
    #[test]
    fn a_conversation_with_no_record_shows_no_account_rather_than_a_stale_one() {
        let mut a = app();
        pick(&mut a);
        stale_record_first(&mut a);
        set_conversation(&mut a, "c-started-by-hand");
        let card = a.selected().unwrap();
        assert!(a.account_agent(card).is_none());
    }
    #[test]
    fn until_herdr_knows_the_conversation_the_newest_record_stands() {
        let mut a = app();
        pick(&mut a);
        stale_record_first(&mut a);
        // A feed lists its agents newest first.
        let card = a.selected().unwrap();
        assert!(card.agents.iter().all(|g| g.conversation.is_none()));
        assert_eq!(a.account_agent(card).unwrap().account, "OLD_C");
    }
    /// #44's workspace with Codex in its first tab and Claude Code in its second, the second out
    /// of usage or not (friction F-0378).
    fn two_tabs(a: &mut App, second_waiting: bool) -> String {
        pick(a);
        let key = a.sel.key.clone().unwrap();
        let snap = a.machines[1]
            .data
            .as_mut()
            .unwrap()
            .snapshots
            .get_mut("api")
            .unwrap()
            .as_mut()
            .unwrap();
        for g in snap.agents.iter_mut().filter(|g| g.pane_id == "w1:p1") {
            g.tab_id = Some("w1:t1".into());
            g.agent = Some("codex".into());
        }
        let mut second = snap
            .agents
            .iter()
            .find(|g| g.pane_id == "w1:p1")
            .unwrap()
            .clone();
        second.pane_id = "w1:p9".into();
        second.tab_id = Some("w1:t2".into());
        second.agent = Some("claude".into());
        snap.agents.push(second);
        let feed = a.machines[1].feed.as_mut().unwrap();
        for r in feed
            .agents
            .iter_mut()
            .filter(|r| r.session == "api" && r.pane == "w1:p1")
        {
            r.waiting = None;
            r.program = "codex".into();
            r.account = "KBZ_X".into();
        }
        let mut claude = feed
            .agents
            .iter()
            .find(|r| r.session == "api" && r.pane == "w1:p1")
            .unwrap()
            .clone();
        claude.pane = "w1:p9".into();
        claude.conversation = "c-claude".into();
        claude.program = "claude".into();
        claude.account = "KBZ_C".into();
        claude.handed_from = None;
        claude.waiting = second_waiting.then(|| crate::accounts::Waiting {
            why: "usage".into(),
            until: None,
        });
        feed.agents.push(claude);
        a.rebuild();
        assert_eq!(a.card(&key).unwrap().agents.len(), 2, "both tabs' agents");
        key
    }
    #[test]
    fn s_moves_the_agent_out_of_usage_not_the_first_tabs() {
        let mut a = app();
        let k = two_tabs(&mut a, true);
        let card = a.card(&k).unwrap();
        assert_eq!(a.account_agent(card).unwrap().pane, "w1:p9");
        press(&mut a, 's');
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["action"], "account-move");
        assert_eq!(r["pane"], "w1:p9");
        assert_eq!(r["conversation"], "c-claude");
    }
    #[test]
    fn s_moves_the_agent_of_the_cards_own_tab() {
        let mut a = app();
        let k = two_tabs(&mut a, false);
        for c in a.board.cards.iter_mut().filter(|c| c.key == k) {
            c.own_tab = Some("w1:t2".into());
        }
        let card = a.card(&k).unwrap();
        assert_eq!(a.move_agent(card).unwrap().pane, "w1:p9");
    }
    #[test]
    fn s_never_guesses_among_agents_in_different_tabs() {
        let mut a = app();
        let k = two_tabs(&mut a, false);
        let card = a.card(&k).unwrap();
        assert!(a.move_agent(card).is_none());
        press(&mut a, 's');
        assert!(matches!(a.mode, Mode::Board));
        let said = a.message().unwrap().0;
        assert!(
            said.contains("Codex in tab 1 and Claude Code in tab 2"),
            "{said}"
        );
        assert!(said.contains("doesn't guess"), "{said}");
    }
    /// The two tabs' agents with no tab named, as an older herdr lists them.
    fn tabs_unnamed(a: &mut App, same: Option<&str>) {
        let snap = a.machines[1]
            .data
            .as_mut()
            .unwrap()
            .snapshots
            .get_mut("api")
            .unwrap()
            .as_mut()
            .unwrap();
        for g in snap
            .agents
            .iter_mut()
            .filter(|g| g.pane_id == "w1:p1" || g.pane_id == "w1:p9")
        {
            g.tab_id = same.map(str::to_string);
        }
        a.rebuild();
    }
    #[test]
    fn s_never_takes_agents_in_tabs_herdr_didnt_name_for_one_place() {
        let mut a = app();
        let k = two_tabs(&mut a, false);
        tabs_unnamed(&mut a, None);
        let card = a.card(&k).unwrap();
        assert_eq!(card.agents.len(), 2);
        assert!(a.move_agent(card).is_none(), "no tab says they're together");
        // s says why and sends nothing, rather than moving the first.
        assert_eq!(press(&mut a, 's'), Action::None);
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        let said = a.message().unwrap().0;
        assert!(said.contains("Codex and Claude Code"), "{said}");
        assert!(said.contains("doesn't guess"), "{said}");
        // Two in one tab herdr named are one place: the first, as before.
        tabs_unnamed(&mut a, Some("w1:t1"));
        let card = a.card(&k).unwrap();
        assert_eq!(a.move_agent(card).unwrap().pane, "w1:p1");
    }
    #[test]
    fn s_moves_the_agent_really_in_the_pane() {
        let mut a = app();
        pick(&mut a);
        stale_record_first(&mut a);
        set_conversation(&mut a, "c-real");
        press(&mut a, 's');
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["action"], "account-move");
        assert_eq!(r["conversation"], "c-real");
        assert_eq!(r["pane"], "w1:p1");
    }
    #[test]
    fn s_on_a_conversation_with_no_record_moves_it_and_not_a_stale_one() {
        let mut a = app();
        pick(&mut a);
        stale_record_first(&mut a);
        set_conversation(&mut a, "c-started-by-hand");
        press(&mut a, 's');
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["conversation"], "c-started-by-hand");
    }
    #[test]
    fn queued_and_stale_moves_leave_the_account_to_the_feed() {
        for (state, message) in [
            ("queued", "moving to ARK_C"),
            ("stale", "the conversation changed"),
        ] {
            let mut a = app();
            pick(&mut a);
            press(&mut a, 's');
            let action = key(&mut a, KeyCode::Enter);
            complete(
                &mut a,
                action,
                json!({"ok":true,"state":state,"account":"ARK_C","message":message}),
            );
            assert_eq!(
                a.account_agent(a.selected().unwrap()).unwrap().account,
                "KBZ_C"
            );
            assert_eq!(a.message().unwrap().0, message);
        }
    }

    #[test]
    fn a_move_problem_needs_attention_and_is_shown_in_the_picker() {
        let mut a = app();
        let problem = "restart on ARK_C failed; retry the move";
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
    fn a_failure_the_signin_box_says_itself_is_kept_once() {
        let (_dir, log, path) = crate::errlog::tests::log();
        let mut a = app();
        a.demo = false;
        a.error_log = log;
        a.machines[0].conn = crate::poll::Conn::Connecting;
        assert_eq!(token_signin(&mut a), Action::None);
        assert_eq!(press(&mut a, 'r'), Action::None);
        a.keep_box_errors();
        a.keep_box_errors();
        let got: Vec<_> = crate::errlog::tests::lines(&path)
            .into_iter()
            .filter(|l| l["action"] == "account-signin")
            .collect();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0]["message"]
                .as_str()
                .unwrap()
                .contains("not answering")
        );
        assert_eq!(got[0]["machine"], a.machines[0].name().as_str());
        // Another warning meanwhile, an answer about something else: still only the once, and that
        // answer's stderr isn't put on it.
        a.say("Something else went wrong.", Tone::Warn);
        a.keep_box_errors();
        a.answer_stderr = "another's stderr".into();
        a.finish_action(
            0,
            serde_json::json!({"action":"dispatch","id":"0009"}),
            Err("no".into()),
        );
        let lines = crate::errlog::tests::lines(&path);
        let signins: Vec<_> = lines
            .iter()
            .filter(|l| l["action"] == "account-signin")
            .collect();
        assert_eq!(signins.len(), 1, "{lines:?}");
        assert!(signins[0].get("stderr").is_none(), "{lines:?}");
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
            json!({"ok":false,"status":"failed","message":"that signed in as X, not KBZ_C's Y"}),
        );
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if s.failed && s.current == 0 && s.message.contains("not KBZ_C"))
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
        a.saved_models(0, &json!({"owner":"studio","models":["KBZ_C"]}));
        assert_eq!(a.account_feed(0).unwrap().owners[0].models, ["KBZ_C"]);
        assert_eq!(a.account_feed(1).unwrap().owners, other);
    }

    fn open_copies(a: &mut App, copy: &str) {
        let rows = a.account_rows(1);
        let sel = rows
            .iter()
            .position(|r| *r == Row::Copy("ARK_C".into(), copy.into()))
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
        assert!(rows.contains(&Row::Copy("ARK_C".into(), "agents".into())));
        assert!(rows.contains(&Row::Copy("ARK_C".into(), "tenants".into())));
        assert!(!rows.contains(&Row::Account("ARK_C".into())));
        assert!(a.account_rows(0).contains(&Row::Account("ARK_C".into())));
        let ark = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK_C")
            .unwrap();
        ark.copies.truncate(1);
        let rows = a.account_rows(1);
        assert!(rows.contains(&Row::Account("ARK_C".into())));
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
                json!({"action":"account-test","code":"ARK_C","copy":"tenants"})
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
            json!({"action":"account-test","code":"ARK_C","copy":"agents"})
        );
        complete(&mut a, test, json!({"ok":true,"message":"works"}));
        press(&mut a, 'j');
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 1);
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"ARK_C","step":"start","copy":"tenants"})
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
            .find(|x| x.code == "ARK_C")
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
            json!({"action":"account-signin","code":"ARK_C","step":"start","copy":"agents"})
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
            json!({"action":"account-signin","code":"ARK_C","step":"paste","token":"fixture-token","copy":"agents"})
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
                .find(|x| x.code == "ARK_C")
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
        assert_eq!(selected(&a), Row::Account("ARK_C".into()));
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK_C")
            .unwrap()
            .copies = copies;
        a.rebuild();
        assert_eq!(selected(&a), Row::Copy("ARK_C".into(), "agents".into()));
    }

    /// Intent 0327: on All, with a LAB-Server card selected, A opened on LAB-Server and Enter signed
    /// KBZ_C in there though the laptop was the one that needed it.
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
        set_status(&mut a, 1, "KBZ_C", "ready");
        set_status(&mut a, 0, "KBZ_C", "sign_in");
        a.machine = Some("build-box".into());
        a.open_accounts();
        assert_eq!(
            key(&mut a, KeyCode::Enter),
            Action::None,
            "nothing starts by itself"
        );
        let (message, choices) = where_screen(&a);
        assert!(
            message.contains("KBZ_C is already signed in and ready on build-box"),
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
            json!({"action":"account-signin","code":"KBZ_C","step":"start"})
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
        set_status(&mut a, 0, "KBZ_C", "ready");
        // Out of usage isn't signed out: it's never offered, and never called ready either.
        set_status(&mut a, 1, "KBZ_C", "out");
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
        set_status(&mut a, 1, "KBZ_C", "sign_in");
        set_status(&mut a, 2, "KBZ_C", "missing");
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
        set_status(&mut a, 0, "KBZ_C", "ready");
        set_status(&mut a, 1, "KBZ_C", "sign_in");
        set_status(&mut a, 2, "KBZ_C", "sign_in");
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
        set_status(&mut a, 0, "KBZ_C", "sign_in");
        set_status(&mut a, 1, "KBZ_C", "ready");
        set_status(&mut a, 2, "KBZ_C", "missing");
        set_status(&mut a, 3, "KBZ_C", "sign_in");
        a.machines[3].conn = crate::poll::Conn::Connecting;
        assert_eq!(a.signin_order("KBZ_C", false, 0), vec![0, 2]);
        assert_eq!(
            a.signin_order("KBZ_C", true, 0),
            vec![0],
            "a copy is one machine's"
        );
        a.open_accounts();
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 0, "the machine viewed first");
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"KBZ_C","step":"start"})
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
            message.contains("The Your agents copy of ARK_C on build-box already works"),
            "{message}"
        );
        assert_eq!(choices.len(), 1);
        let (mi, start) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(mi, 1);
        assert_eq!(
            start,
            json!({"action":"account-signin","code":"ARK_C","step":"start","copy":"agents"})
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
            .find(|x| x.code == "ARK_C")
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
        assert!(!signin_screen(&a).paste, "KBZ_C is a saved sign-in");
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
            .find(|x| x.code == "ARK_C")
            .unwrap();
        ark.copies[1].paste = Some(false);
        ark.copies[0].works = Some(false);
        open_copies(&mut a, "tenants");
        key(&mut a, KeyCode::Enter);
        assert!(!signin_screen(&a).paste);
        // A feed from before paste: root's copy still offers none, the agents' copy one.
        let a = app();
        let ark = a.account(1, "ARK_C").unwrap();
        assert!(ark.copies.iter().all(|c| c.paste.is_none()));
        let p = a.offered(1);
        assert!(
            p.takes_paste(ark, Some(&ark.copies[0])) && !p.takes_paste(ark, Some(&ark.copies[1]))
        );
        let mut a = a;
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
            .find(|x| x.code == "ARK_C")
            .unwrap()
            .copies[0]
            .works = Some(false);
        open_copies(&mut a, "agents");
        key(&mut a, KeyCode::Enter);
        assert!(signin_screen(&a).paste);
    }

    #[test]
    fn a_short_all_digit_code_is_caught_before_it_is_sent() {
        // An older backend, which describes no providers, ends the sign-in on a wrong code, so
        // Kerdr keeps an emailed one back itself.
        let mut a = app();
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.providers.clear();
            }
        }
        let action = signin(&mut a);
        complete(&mut a, action, started());
        a.on_paste("123456");
        assert_eq!(
            key(&mut a, KeyCode::Enter),
            Action::None,
            "the emailed code isn't sent"
        );
        let s = signin_screen(&a);
        assert_eq!(s.note.as_deref(), Some(EMAILED_CODE));
        assert!(!EMAILED_CODE.contains("claude"));
        assert!(s.page_code.is_empty() && s.editing && !s.failed);
        a.on_paste("Ab3-long-code#state-xyz");
        let (_, finish) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(finish["page_code"], "Ab3-long-code#state-xyz");
        assert!(signin_screen(&a).note.is_none());
        assert!(emailed_code("004211") && emailed_code("1234"));
        assert!(!emailed_code("123") && !emailed_code("12345678901") && !emailed_code("12ab56"));
    }

    /// A backend that describes its providers checks a code itself: Kerdr sends it, and a
    /// refusal keeps the box open with the backend's words while the sign-in waits on.
    #[test]
    fn a_backend_that_describes_its_providers_checks_codes_itself() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        a.on_paste("123456");
        let action = key(&mut a, KeyCode::Enter);
        let (_, finish) = sent(action.clone());
        assert_eq!(finish["page_code"], "123456");
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"that's the short code the page emails you; this wants the long one"}),
        );
        let s = signin_screen(&a);
        assert!(!s.failed && s.editing && s.ticket.is_some());
        assert_eq!(
            s.note.as_deref(),
            Some("that's the short code the page emails you; this wants the long one")
        );
        // A refusal that ends it still says failed.
        a.on_paste("wrong-account-code");
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":false,"status":"failed","message":"that signed in as someone else"}),
        );
        assert!(signin_screen(&a).failed);
    }

    /// Codex's second look: a machine picked because it needed a sign-in, but signed in by the
    /// time its turn comes, is skipped rather than signed in again unasked.
    #[test]
    fn a_machine_signed_in_meanwhile_is_skipped_in_turn() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ_C", "sign_in");
        set_status(&mut a, 1, "KBZ_C", "sign_in");
        a.open_accounts();
        let action = key(&mut a, KeyCode::Enter);
        assert_eq!(signin_screen(&a).machines, vec![0, 1]);
        assert!(signin_screen(&a).recheck);
        complete(&mut a, action, started());
        set_status(&mut a, 1, "KBZ_C", "ready");
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"done","message":"KBZ_C signed in on laptop"}),
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

    fn add_screen(a: &App) -> &AddAccount {
        match &a.mode {
            Mode::Accounts {
                screen: AccountScreen::Add(add),
                ..
            } => add,
            other => panic!("not the add form: {other:?}"),
        }
    }

    fn type_text(a: &mut App, text: &str) {
        for c in text.chars() {
            press(a, c);
        }
    }

    /// Intent 0356: + asks the provider first, then suggests the owner's code and its letter.
    #[test]
    fn plus_asks_the_provider_then_suggests_the_owners_code_and_its_letter() {
        let mut a = app();
        a.open_accounts();
        press(&mut a, '+');
        let Mode::Accounts {
            screen: AccountScreen::Provider { choices, .. },
            ..
        } = &a.mode
        else {
            panic!("+ didn't ask which provider: {:?}", a.message());
        };
        assert_eq!(
            choices.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["claude", "codex"]
        );
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(
            screen.contains("Add an account: which provider?"),
            "{screen}"
        );
        assert!(screen.contains("Codex      runs Codex"), "{screen}");
        press(&mut a, 'j');
        key(&mut a, KeyCode::Enter);
        let add = add_screen(&a);
        assert_eq!(add.provider.id, "codex");
        // The machine's owner is Example Studio, code ARK; KBZ_X already uses this machine's
        // own Codex sign-in, so a sign-in of its own is chosen.
        assert_eq!(
            add.suggested,
            ("ARK_X".to_string(), "Example Studio Codex".to_string())
        );
        assert_eq!(add.kind(), "token");
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("Add a Codex account"), "{screen}");
        assert!(!screen.contains("Claude account"), "{screen}");
        assert!(screen.contains("KBZ_X uses it"), "{screen}");
        assert!(
            screen.contains("a sign-in of its own, per machine"),
            "{screen}"
        );
        // A code without the provider's letter is stopped here.
        type_text(&mut a, "ARK_C");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(
            a.message().unwrap().0.contains("_ and X"),
            "{:?}",
            a.message()
        );
        for _ in 0..5 {
            key(&mut a, KeyCode::Backspace);
        }
        // Empty, the suggestion stands.
        let action = key(&mut a, KeyCode::Enter);
        let (machine, request) = sent(action.clone());
        assert_eq!(machine, 0);
        assert_eq!(
            request,
            json!({"action":"account-add","code":"ARK_X","name":"Example Studio Codex","provider":"codex","kind":"token"})
        );
        complete(&mut a, action, json!({"ok":true,"message":"added ARK_X"}));
        // Its sign-in starts; a Codex sign-in takes no pasted token.
        assert_eq!(signin_screen(&a).code, "ARK_X");
        assert!(!signin_screen(&a).paste);
    }

    #[test]
    fn an_added_account_its_feed_already_has_follows_the_feeds_paste() {
        let mut a = app();
        a.open_accounts();
        press(&mut a, '+');
        key(&mut a, KeyCode::Enter);
        assert_eq!(add_screen(&a).provider.id, "claude");
        type_text(&mut a, "NEW_C");
        let action = key(&mut a, KeyCode::Enter);
        // A refresh brings the account before the add answers, saying it takes no token here.
        let published: Account = serde_json::from_value(json!({"code":"NEW_C","name":"New",
            "provider":"claude","kind":"token","runs":"claude","status":"missing","paste":false}))
        .unwrap();
        a.machines[0]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .push(published);
        complete(&mut a, action, json!({"ok":true,"message":"added NEW_C"}));
        assert_eq!(signin_screen(&a).code, "NEW_C");
        assert!(!signin_screen(&a).paste);
    }

    #[test]
    fn a_code_typed_on_the_providers_page_is_shown_and_copied_with_nothing_to_paste() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ_X", "sign_in");
        a.open_accounts();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        let action = key(&mut a, KeyCode::Enter);
        let (_, request) = sent(action.clone());
        assert_eq!(request["code"], "KBZ_X");
        let expires = (a.now() + chrono::Duration::minutes(15)).to_rfc3339();
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"waiting","ticket":"device-ticket","url":"https://auth.example.com/codex/device",
                   "needs_code":false,"user_code":"K7QD-3MPWX","expires":expires,
                   "message":"Open the link and type the code K7QD-3MPWX on the page."}),
        );
        let s = signin_screen(&a);
        assert_eq!(s.user_code.as_deref(), Some("K7QD-3MPWX"));
        assert!(!s.needs_code && !s.paste);
        assert_eq!(press(&mut a, 'c'), Action::CopyCode("K7QD-3MPWX".into()));
        assert_eq!(
            press(&mut a, 'l'),
            Action::CopyLink("https://auth.example.com/codex/device".into())
        );
        // No code box: Tab opens nothing and typing goes nowhere.
        key(&mut a, KeyCode::Tab);
        press(&mut a, 'x');
        assert!(!signin_screen(&a).editing && signin_screen(&a).page_code.is_empty());
        let screen = crate::ui::tests_render(&a, 100, 40);
        assert!(screen.contains("K7QD-3MPWX"), "{screen}");
        assert!(screen.contains("type the code on the page"), "{screen}");
        assert!(
            screen.contains("On the page at auth.example.com, type this code."),
            "{screen}"
        );
        assert!(screen.contains("Nothing to paste back here"), "{screen}");
        assert!(!screen.contains("long code from the page"), "{screen}");
        // Polling carries on, and done moves to the next machine, the code cleared.
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now() - Duration::from_secs(1);
        }
        let poll = a.account_tick();
        assert_eq!(sent(poll.clone()).1["step"], "poll");
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        assert!(signin_screen(&a).user_code.is_none());
    }

    #[test]
    fn a_code_that_isnt_one_is_never_shown_or_copied() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ_X", "sign_in");
        a.open_accounts();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        let action = key(&mut a, KeyCode::Enter);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"waiting","ticket":"t","url":"https://auth.example.com/device","needs_code":false,"user_code":"rm -rf ~; echo","message":"m"}),
        );
        // It can't finish without the code, so it says so and stops, cancelling that ticket.
        let s = signin_screen(&a);
        assert!(s.user_code.is_none() && s.failed);
        assert!(s.message.contains("won't show"), "{}", s.message);
        let (_, cancel) = sent(a.account_tick());
        assert_eq!(cancel["step"], "cancel");
        assert_eq!(cancel["ticket"], "t");
        assert!(user_code_ok("K7QD-3MPWX") && !user_code_ok("abc") && !user_code_ok("a b c d"));
    }

    #[test]
    fn a_providers_own_fields_are_asked_checked_and_sent() {
        let mut a = app();
        let local: Provider = serde_json::from_value(json!({"id":"local","label":"Local","letter":"L",
            "kinds":["endpoint"],"kind_labels":{"endpoint":"a model on your network"},"programs":["codex"],
            "signin":null,"windows":[],"fields":[{"id":"endpoint","label":"address","required":true},
            {"id":"model","label":"model","required":false}]}))
        .unwrap();
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.providers.push(local.clone());
            }
        }
        a.open_accounts();
        press(&mut a, '+');
        press(&mut a, 'j');
        press(&mut a, 'j');
        key(&mut a, KeyCode::Enter);
        assert_eq!(add_screen(&a).provider.id, "local");
        assert_eq!(add_screen(&a).suggested.0, "ARK_L");
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("address"), "{screen}");
        assert!(screen.contains("a model on your network"), "{screen}");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.message().unwrap().0.contains("Local needs address"));
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Tab);
        type_text(&mut a, "http://studio-gpu:8000/v1");
        let action = key(&mut a, KeyCode::Enter);
        let (_, request) = sent(action.clone());
        assert_eq!(request["kind"], "endpoint");
        assert_eq!(
            request["fields"],
            json!({"endpoint":"http://studio-gpu:8000/v1"})
        );
        // Added, it starts no sign-in: its provider has none.
        complete(&mut a, action, json!({"ok":true,"message":"added ARK_L"}));
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::List { .. },
                ..
            }
        ));
        assert!(a.message().unwrap().0.contains("don't sign in"));
        // Once the feed has it, Enter tests it, and says so.
        let added: Account = serde_json::from_value(json!({"code":"ARK_L","name":"Studio GPU",
            "provider":"local","kind":"endpoint","runs":"codex","status":"ready"}))
        .unwrap();
        a.machines[0].feed.as_mut().unwrap().accounts.push(added);
        a.refresh_accounts();
        if let Mode::Accounts {
            screen: AccountScreen::List { rows, sel, .. },
            ..
        } = &mut a.mode
        {
            *sel = rows
                .iter()
                .position(|r| *r == Row::Account("ARK_L".into()))
                .unwrap();
        }
        let screen = crate::ui::tests_render(&a, 176, 44);
        assert!(screen.contains("it needs no sign-in"), "{screen}");
        let (_, test) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(test, json!({"action":"account-test","code":"ARK_L"}));
        // On one copy's line, Enter tests that copy, as t does.
        for acc in &mut a.machines[0].feed.as_mut().unwrap().accounts {
            if acc.code == "ARK_L" {
                acc.copies = serde_json::from_value(json!([
                    {"id":"agents","label":"Your agents","stored":true,"works":true},
                    {"id":"tenants","label":"Tenants","stored":true,"works":null}
                ]))
                .unwrap();
            }
        }
        a.refresh_accounts();
        if let Mode::Accounts {
            screen: AccountScreen::List { rows, sel, .. },
            ..
        } = &mut a.mode
        {
            *sel = rows
                .iter()
                .position(|r| *r == Row::Copy("ARK_L".into(), "tenants".into()))
                .unwrap();
        }
        let (_, test) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(
            test,
            json!({"action":"account-test","code":"ARK_L","copy":"tenants"})
        );
        let (_, t) = sent(press(&mut a, 't'));
        assert_eq!(t, test);
        // A pasted address longer than the field takes is refused, not cut short.
        press(&mut a, '+');
        press(&mut a, 'j');
        press(&mut a, 'j');
        key(&mut a, KeyCode::Enter);
        key(&mut a, KeyCode::Tab);
        key(&mut a, KeyCode::Tab);
        a.on_paste(&"x".repeat(201));
        assert!(add_screen(&a).extra[0].is_empty());
        assert!(a.message().unwrap().0.contains("longer than 200"));
        a.on_paste(&"x".repeat(200));
        assert_eq!(add_screen(&a).extra[0].len(), 200);
    }

    #[test]
    fn an_older_machine_offers_only_what_its_own_backend_can_add() {
        let mut a = app();
        // The build box runs an older backend: no providers, Claude accounts only.
        let old = a.machines[1].feed.as_mut().unwrap();
        old.providers.clear();
        old.accounts.retain(|acc| acc.provider == "claude");
        a.open_accounts();
        key(&mut a, KeyCode::Tab);
        press(&mut a, '+');
        let add = add_screen(&a);
        assert_eq!(add.machine, 1);
        // Straight to the form: its only provider, by the older rules, though the laptop's feed
        // describes Codex and Claude's letter.
        assert_eq!(add.provider.id, "claude");
        assert!(add.provider.letter.is_empty());
        assert_eq!(add.provider.kinds, ["token"]);
        // Names still come from the newer feed.
        assert_eq!(a.providers(1).label("claude"), "Claude");
        // s moves only the programs its own accounts run.
        assert_eq!(a.offered(1).programs(), ["claude"]);
        // Adding Codex on the laptop leaves the older build box out: it can't sign one in.
        key(&mut a, KeyCode::Esc);
        key(&mut a, KeyCode::BackTab);
        press(&mut a, '+');
        press(&mut a, 'j');
        key(&mut a, KeyCode::Enter);
        let add = add_screen(&a);
        assert_eq!((add.machine, add.provider.id.as_str()), (0, "codex"));
        assert_eq!(add.machines, [0]);
        let action = key(&mut a, KeyCode::Enter);
        complete(&mut a, action, json!({"ok":true,"message":"added ARK_X"}));
        assert_eq!(signin_screen(&a).machines, [0]);
        assert_eq!(
            signin_screen(&a).added,
            Some(("codex".into(), "token".into()))
        );
        assert!(!signin_screen(&a).paste);
    }

    #[test]
    fn a_reading_from_an_older_feed_keeps_both_windows_whatever_another_feed_says() {
        let mut a = app();
        let old = a.machines[1].feed.as_mut().unwrap();
        old.providers.clear();
        for acc in &mut old.accounts {
            if acc.code == "ARK_C" {
                // The build box's reading is newest, and has no five-hour figure.
                acc.usage = Some(
                    serde_json::from_value(json!({"week":7,"seen":"2099-01-01T00:00:00Z"}))
                        .unwrap(),
                );
            }
        }
        let ark = a.account(0, "ARK_C").unwrap().clone();
        assert_eq!(a.account_figures(&ark, Figures::Short), "—│7");
        assert!(a.account_lacking(&ark).is_empty());
        // The same reading from a feed that describes Claude is a weekly-only plan.
        a.machines[1].feed.as_mut().unwrap().providers = crate::demo::providers();
        assert_eq!(a.account_figures(&ark, Figures::Short), "7");
    }

    #[test]
    fn paste_is_worked_out_for_each_machine_in_turn() {
        let mut a = app();
        set_status(&mut a, 0, "ARK_C", "sign_in");
        set_status(&mut a, 1, "ARK_C", "sign_in");
        // The build box's backend says its copy of ARK_C takes no token.
        for acc in &mut a.machines[1].feed.as_mut().unwrap().accounts {
            if acc.code == "ARK_C" {
                acc.copies.clear();
                acc.paste = Some(false);
            }
        }
        let action = token_signin(&mut a);
        assert_eq!(signin_screen(&a).machines, [0, 1]);
        assert!(signin_screen(&a).paste);
        complete(&mut a, action, started());
        let action = a.account_queue.pop_front().map(|(m, r)| Action::Mutate {
            machine: m,
            request: r,
        });
        assert!(action.is_none(), "nothing queued before the first finishes");
        // The laptop finishes; the build box's turn comes with its own answer.
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now() - Duration::from_secs(1);
        }
        let poll = a.account_tick();
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"signed in"}),
        );
        assert_eq!(signin_screen(&a).current, 1);
        assert!(!signin_screen(&a).paste);
        // An account-level paste covers its first copy too, whatever its kind.
        let mut endpoint = a.account(0, "ARK_C").unwrap().clone();
        endpoint.kind = "endpoint".into();
        endpoint.paste = Some(true);
        endpoint.copies =
            serde_json::from_value(json!([{"id":"agents","stored":true},{"id":"tenants"}]))
                .unwrap();
        let p = a.offered(0);
        assert!(p.takes_paste(&endpoint, Some(&endpoint.copies[0])));
        assert!(!p.takes_paste(&endpoint, Some(&endpoint.copies[1])));
    }

    #[test]
    fn r_starts_a_typed_code_sign_in_again_once_its_code_has_run_out() {
        let mut a = app();
        set_status(&mut a, 0, "KBZ_X", "sign_in");
        a.open_accounts();
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        let action = key(&mut a, KeyCode::Enter);
        let gone = (a.now() - chrono::Duration::minutes(1)).to_rfc3339();
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"waiting","ticket":"old","url":"https://auth.example.com/device","needs_code":false,"user_code":"K7QD-3MPWX","expires":gone,"message":"m"}),
        );
        assert!(!signin_screen(&a).failed);
        let screen = crate::ui::tests_render(&a, 100, 40);
        assert!(screen.contains("r starts again."), "{screen}");
        assert!(screen.contains("r start again    esc stop"), "{screen}");
        // r cancels the old ticket first, then starts again.
        let (_, cancel) = sent(press(&mut a, 'r'));
        assert_eq!(cancel["step"], "cancel");
        assert_eq!(cancel["ticket"], "old");
        assert!(signin_screen(&a).user_code.is_none());
    }

    #[test]
    fn a_feed_without_providers_adds_as_before() {
        let mut a = app();
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.providers.clear();
                f.accounts.retain(|acc| acc.provider == "claude");
                for o in &mut f.owners {
                    o.code = None;
                }
            }
        }
        a.open_accounts();
        press(&mut a, '+');
        let add = add_screen(&a);
        assert_eq!(add.provider.id, "claude");
        assert!(add.suggested.0.is_empty());
        type_text(&mut a, "NEW");
        key(&mut a, KeyCode::Tab);
        type_text(&mut a, "Studio");
        let action = key(&mut a, KeyCode::Enter);
        let (_, request) = sent(action.clone());
        assert_eq!(
            request,
            json!({"action":"account-add","code":"NEW","name":"Studio","provider":"claude","kind":"token"})
        );
        complete(&mut a, action, json!({"ok":true,"message":"added NEW"}));
        assert!(signin_screen(&a).paste);
    }

    #[test]
    fn account_codes_follow_their_providers_letter() {
        assert!(code_ok("KBZ_X", "X") && code_ok("ANET_C", "C") && code_ok("P4M_C", "C"));
        assert!(!code_ok("KBZ_C", "X") && !code_ok("KBZX", "X") && !code_ok("K_X", "X"));
        assert!(!code_ok("4BZ_X", "X") && !code_ok("ANETS_X", "X") && !code_ok("kbz_x", "X"));
        assert!(code_ok("KBZ", "") && !code_ok("KBZ_C", ""));
    }

    /// Intent 0455: c makes the selected account current, C clears it, each saying what it did,
    /// and the answer shows at once on the machine asked.
    #[test]
    fn c_sets_the_current_account_and_shift_c_clears_it() {
        let mut a = app();
        knows_current(&mut a);
        a.open_accounts();
        let action = press(&mut a, 'c');
        let (mi, request) = sent(action.clone());
        assert_eq!(mi, 0);
        assert_eq!(request, json!({"action":"account-current","to":"KBZ_C"}));
        complete(
            &mut a,
            action,
            json!({"ok":true,"message":"KBZ_C (Personal) is the current account","current":{"account":"KBZ_C","name":"Personal","since":"2026-11-02T10:00:00-06:00","status":"out","until":"2026-11-03T09:00:00-06:00","machine":"laptop"}}),
        );
        let current = a.machines[0]
            .feed
            .as_ref()
            .unwrap()
            .current_account()
            .cloned()
            .unwrap();
        assert_eq!(current.account, "KBZ_C");
        assert!(!current.starts_sessions());
        assert!(current.held_back("here").unwrap().starts_with("out until"));
        assert!(
            a.message().unwrap().0.contains("is the current account"),
            "{:?}",
            a.message()
        );
        let action = press(&mut a, 'C');
        assert_eq!(
            sent(action.clone()).1,
            json!({"action":"account-current","clear":true})
        );
        complete(
            &mut a,
            action,
            json!({"ok":true,"message":"No current account now","current":null}),
        );
        assert!(
            a.machines[0]
                .feed
                .as_ref()
                .unwrap()
                .current_account()
                .is_none()
        );
    }

    /// A refused set changes nothing shown.
    #[test]
    fn a_refused_current_account_leaves_the_feed_as_it_was() {
        let mut a = app();
        knows_current(&mut a);
        a.open_accounts();
        let action = press(&mut a, 'c');
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"no account KBZ_C"}),
        );
        assert!(
            a.machines[0]
                .feed
                .as_ref()
                .unwrap()
                .current_account()
                .is_none()
        );
    }

    /// Every feed says null: a backend that has a current account, none set.
    fn knows_current(a: &mut App) {
        for m in a.machines.iter_mut() {
            if let Some(f) = m.feed.as_mut() {
                f.current = Some(None);
            }
        }
    }

    /// Codex's review: a feed not yet caught up never stops c or C. Set on the laptop, the build
    /// box's feed still says none, and C there still asks its backend, which knows better; c on
    /// the account a feed already calls current asks too.
    #[test]
    fn c_and_shift_c_ask_the_backend_whatever_a_feed_still_says() {
        let mut a = app();
        knows_current(&mut a);
        a.open_accounts();
        let action = press(&mut a, 'c');
        complete(
            &mut a,
            action,
            json!({"ok":true,"message":"set","current":{"account":"KBZ_C","status":"ready"}}),
        );
        key(&mut a, KeyCode::Tab);
        let (mi, request) = sent(press(&mut a, 'C'));
        assert_eq!(mi, 1);
        assert_eq!(request, json!({"action":"account-current","clear":true}));
        a.account_pending = None;
        key(&mut a, KeyCode::Tab);
        let (mi, request) = sent(press(&mut a, 'c'));
        assert_eq!(mi, 0);
        assert_eq!(request, json!({"action":"account-current","to":"KBZ_C"}));
    }

    /// An older backend, whose feed has no `current` at all: c and C send nothing and say why.
    #[test]
    fn an_older_backend_is_never_sent_a_current_account() {
        let mut a = app();
        a.open_accounts();
        assert!(matches!(press(&mut a, 'c'), Action::None));
        assert!(matches!(press(&mut a, 'C'), Action::None));
        assert!(a.message().unwrap().0.contains("no current account yet"));
    }
}
