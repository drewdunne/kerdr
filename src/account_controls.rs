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

/// A move whose account isn't signed in on its machine: the sign-in first, then the move again.
#[derive(Clone, Debug, PartialEq)]
pub struct AfterSignin {
    pub machine: usize,
    pub code: String,
    pub request: Value,
}

/// What an account question needs beyond its words, kept apart from the screen it's asked on, so
/// a yes given over the board, a question that changed, a sign-in on the way, n or Esc never loses
/// it (Codex's review): the agent or card it came from, what follows once it's settled, and the
/// question itself while it's held for him. One for each question, found by its machine and its
/// request without any answer's fields. It's settled when its question is done with: n or Esc on
/// it, the answer to its yes (unless that asks again or waits on a sign-in), or its sign-in left;
/// never because something else covers it on screen (Codex's third look).
#[derive(Clone, Debug)]
pub struct AskedAbout {
    pub machine: usize,
    /// The request as asked, without its answers (move_asked).
    pub asked: Value,
    /// The agent a move was for, even when its question is about a start made instead: a changed
    /// question is asked again for it at once.
    pub target: Option<Box<MoveTarget>>,
    /// The card a fresh agent's start was asked from: gone into once it starts, while you're
    /// still on it.
    pub from: Option<CardSnapshot>,
    /// The owner whose waiting sessions move once a saved list's question is settled, yes or no:
    /// "move waiting sessions" was ticked.
    pub then_waiting: Option<String>,
    /// The question, its token in its request, while it waits for him to be back where it can't
    /// take keys meant for something else. Held ones are asked in the order they came, each its
    /// own: a second never replaces or drops the first (Codex's third look). Any screen an answer
    /// brings is held the same way: a sign-in's, or an offer to resume (Codex's fourth look).
    pub held: Option<Box<AccountScreen>>,
    /// A held sign-in's request that goes again once it's signed in, waiting with it until it's
    /// shown.
    pub signin: Option<AfterSignin>,
}

/// What waits on a sign-in, in words: the step that goes on once it's done, and what's said when
/// it's left (Round 2b).
fn waiting_on_signin(request: &Value) -> (&'static str, &'static str) {
    match request["action"].as_str() {
        Some("account-start") => ("start", "Nothing started"),
        Some(crate::app::resume::RESUME) => ("resume", "Nothing was resumed"),
        _ => ("move", "Nothing moved"),
    }
}

/// A request to ask again with a backend's new token: without any earlier answer's fields (its
/// op, choice, override, queue or token), so the answer given to the new question is the only one
/// it carries (Codex's review).
fn reasked(request: &Value, token: &str) -> Value {
    let mut request = move_asked(request);
    if let Some(fields) = request.as_object_mut() {
        fields.remove("override");
        fields.remove("queue");
    }
    request["confirm"] = json!(token);
    request
}

/// One running agent a saved list left on another account: how it's named, and the move that
/// puts it on the list (intent 0591, B3-42).
#[derive(Clone, Debug, PartialEq)]
pub struct OffAgent {
    pub name: String,
    pub request: Value,
}

/// The moves to the list he said yes to, after a saved list left running agents on other
/// accounts: one at a time, each one's question asked before the next is sent; Esc on any of them
/// stops the rest (intent 0591, B3-42).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OffListMoves {
    pub machine: usize,
    pub total: usize,
    /// Not sent yet, in the backend's order.
    pub left: std::collections::VecDeque<OffAgent>,
    /// Sent: its answer, its question or its sign-in still to come.
    pub current: Option<OffAgent>,
    pub moved: Vec<String>,
    /// Moving as soon as they can: when they next stop, or once a job ends.
    pub later: Vec<String>,
    /// Left where they were by his n.
    pub kept: Vec<String>,
    /// Each one that didn't move, and why, as a sentence.
    pub failed: Vec<String>,
    /// Each one whose answer was lost, so nobody can tell whether it moved, as a sentence: never
    /// counted as moved or as not moved (Codex's third look).
    pub unknown: Vec<String>,
}

/// How one of those moves ended.
enum OffListEnd {
    Moved,
    Later,
    Kept,
    Failed(String),
    /// Its answer was lost: it may have moved.
    Unknown(String),
}

impl OffListMoves {
    fn end(&mut self, how: OffListEnd) {
        let Some(agent) = self.current.take() else {
            return;
        };
        match how {
            OffListEnd::Moved => self.moved.push(agent.name),
            OffListEnd::Later => self.later.push(agent.name),
            OffListEnd::Kept => self.kept.push(agent.name),
            OffListEnd::Failed(why) => self.failed.push(format!(
                "{} didn't move: {}.",
                capitalized(&agent.name),
                sentence_part(&why)
            )),
            OffListEnd::Unknown(why) => self.unknown.push(format!(
                "Kerdr can't tell whether {} moved: {}. The accounts screen (A) shows where it is.",
                agent.name,
                sentence_part(lost_why(&why))
            )),
        }
    }

    /// What's said once they've all been asked, or he stopped: how many moved, and which didn't.
    fn summary(&self, stopped: &[String]) -> (String, Tone) {
        let one = |n: usize, single: &'static str, many: &'static str| {
            if n == 1 { single } else { many }
        };
        let done = self.moved.len() + self.later.len();
        let mut said = format!("{done} of {} moved to the list", self.total);
        let later = || {
            format!(
                "{} as soon as {} can",
                and_list(&self.later),
                one(self.later.len(), "it", "they")
            )
        };
        said += &match (self.moved.is_empty(), self.later.is_empty()) {
            (true, true) => ".".to_string(),
            (false, true) => format!(": {}.", and_list(&self.moved)),
            (true, false) => format!(": {}.", later()),
            (false, false) => format!(": {} now, and {}.", and_list(&self.moved), later()),
        };
        for failed in self.failed.iter().chain(&self.unknown) {
            said += &format!(" {failed}");
        }
        if !self.kept.is_empty() {
            let n = self.kept.len();
            said += &format!(
                " {} stayed where {} {}, as you said.",
                capitalized(&and_list(&self.kept)),
                one(n, "it", "they"),
                one(n, "was", "were")
            );
        }
        if !stopped.is_empty() {
            said += &format!(
                " You stopped there, so {} {} moved.",
                and_list(stopped),
                one(stopped.len(), "wasn't", "weren't")
            );
        }
        let tone = if !self.failed.is_empty() || !self.unknown.is_empty() {
            Tone::Warn
        } else if done == self.total {
            Tone::Good
        } else {
            Tone::Info
        };
        (said, tone)
    }
}

/// Why an answer was lost, without the transport's own "can't tell whether it happened" and where
/// to look, which the summary says in its own words.
fn lost_why(error: &str) -> &str {
    error
        .split_once(crate::actions::UNKNOWN)
        .map_or(error, |(why, _)| why)
        .trim_end_matches([' ', ';', ',', '.'])
}

/// Words that start a sentence: their first letter a capital.
fn capitalized(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// A backend's sentence as part of another: its full stop dropped, and its first letter lowered
/// unless the first word is a name ("ARK_C", "#41", "Kerdr").
fn sentence_part(said: &str) -> String {
    let said = said.trim().trim_end_matches('.').trim_end();
    let first = said.split_whitespace().next().unwrap_or_default();
    let mut letters = first.chars();
    let plain_word = letters.next().is_some_and(char::is_uppercase)
        && letters.clone().count() > 0
        && letters.all(char::is_lowercase)
        && first != "Kerdr";
    if !plain_word {
        return said.to_string();
    }
    let mut chars = said.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// A move to the list, as Kerdr sends it for one agent a saved list left elsewhere (intent 0591,
/// B3-42).
fn off_list_request(agent: &crate::actions::OffList) -> Value {
    let mut request = json!({"action":"account-move","pane":agent.pane,"to":"list"});
    if !agent.session.is_empty() {
        request["session"] = json!(agent.session);
    }
    if !agent.conversation.is_empty() {
        request["conversation"] = json!(agent.conversation);
    }
    request
}

/// A move as it was asked for, without what its question's answers add: the same move, whichever
/// answer, token or op it went with.
fn move_asked(request: &Value) -> Value {
    let mut request = crate::actions::without_op(request);
    if let Some(fields) = request.as_object_mut() {
        fields.remove("confirm");
        fields.remove("choice");
    }
    request
}

/// How the agent a move was chosen for stands now (intent 0591, K2-13, K2-64).
enum MoveNow {
    /// As it was, or the same agent now that its conversation is known.
    Same(AccountAgent),
    /// The same conversation, on another account than when it was chosen.
    AccountChanged(AccountAgent),
    /// Its pane runs another conversation.
    NewConversation(AccountAgent),
    /// No agent of it is running.
    Gone,
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
    /// A fresh sign-in has started by itself in place of one that ended (intent 0591): only once.
    pub started_fresh: bool,
    /// He chose a token instead of the browser: its stopping isn't a failure (intent 0591).
    pub set_aside: bool,
    /// A newer sign-in of the same account took its place: not a failure either (intent 0591).
    pub replaced: bool,
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
    /// Whether it stopped because something went wrong, rather than by his own choice or for a
    /// newer sign-in: only then is it drawn as a failure (intent 0591).
    pub fn went_wrong(&self) -> bool {
        self.failed && !self.set_aside && !self.replaced
    }

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
        /// The answers its backend offers beyond yes and no (intent 0591): each sends the request
        /// again with its token and `choice`.
        choices: Vec<crate::actions::Choice>,
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

/// Names as a sentence lists them: "A", "A and B", "A, B and C".
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl App {
    fn queue_waiting_moves(&mut self, owner: &str) {
        self.account_batch_messages.clear();
        self.account_batch_failed = false;
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

    /// x's question on an account says what removing it leaves behind (intent 0591, K2-12): the
    /// owners' lists it comes off, whether it was the current account, and the other machines
    /// where Kerdr sees a sign-in of it saved, since only this machine's is forgotten.
    fn remove_question(&self, machine: usize, code: &str, name: &str) -> String {
        let mut said = format!("Remove {code} ({name})?");
        let Some(feed) = self.account_feed(machine) else {
            return said;
        };
        let lists: Vec<String> = feed
            .owners
            .iter()
            .filter(|o| (o.own || o.parent.is_none()) && o.models.iter().any(|m| m == code))
            .map(|o| format!("{}'s", o.name))
            .collect();
        if !lists.is_empty() {
            let s = if lists.len() == 1 { "" } else { "s" };
            said.push_str(&format!(
                " It comes off {} model list{s}.",
                and_list(&lists)
            ));
        }
        if feed
            .current
            .as_ref()
            .and_then(|c| c.as_ref())
            .is_some_and(|c| c.account == code)
        {
            said.push_str(" It's the current account, so each owner's list chooses again.");
        }
        let others: Vec<String> = (0..self.machines.len())
            .filter(|&m| m != machine)
            .filter(|&m| {
                self.account_feed(m).is_some_and(|f| {
                    f.accounts
                        .iter()
                        .any(|a| a.code == code && a.copies.iter().any(|c| c.stored))
                })
            })
            .map(|m| self.machines[m].name())
            .collect();
        if !others.is_empty() {
            said.push_str(&format!(
                " Only this machine's saved sign-in goes: it stays signed in on {}.",
                and_list(&others)
            ));
        }
        said
    }

    /// Backspace's question on a tenant's own list shows that list, so it could be typed back,
    /// and the one it would follow instead (intent 0591, K2-14).
    fn follow_question(&self, machine: usize, owner: &crate::accounts::Owner) -> String {
        let mut said = format!(
            "Make {} follow its organization's model list? Its own list goes: {}.",
            owner.name,
            owner.models.join(" → ")
        );
        if let Some(org) = self.account_feed(machine).and_then(|f| {
            f.owners
                .iter()
                .find(|o| Some(&o.id) == owner.parent.as_ref())
        }) && !org.models.is_empty()
        {
            said.push_str(&format!(
                " It would follow {}'s: {}.",
                org.name,
                org.models.join(" → ")
            ));
        }
        said
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

    /// How the agent a move was chosen for stands now.
    fn move_now(&self, target: &MoveTarget) -> MoveNow {
        let Some(now) = self
            .card(&target.key)
            .filter(|c| c.machine_index == target.machine)
            .and_then(|c| self.move_agent(c))
        else {
            return MoveNow::Gone;
        };
        if self.target_answerable(target) {
            MoveNow::Same(now)
        } else if now.pane == target.agent.pane
            && now.session == target.agent.session
            && now.conversation == target.agent.conversation
        {
            MoveNow::AccountChanged(now)
        } else {
            MoveNow::NewConversation(now)
        }
    }

    /// A move's question held while you were elsewhere, asked once you're back where it can't
    /// take keys meant for something else, for the agent as it is now (intent 0591, K2-27).
    pub(crate) fn ask_held_move(&mut self, question: AccountScreen) {
        let AccountScreen::Confirm {
            target: Some(target),
            ..
        } = &question
        else {
            // Asked now, not when it came: a key pressed as it opens never answers it.
            let mut question = question;
            if let AccountScreen::Confirm { ready_at, .. } = &mut question {
                *ready_at = Instant::now() + Duration::from_millis(500);
            }
            return self.push_account_screen(question);
        };
        let reference = target.reference.clone();
        let (machine, request) = match &question {
            AccountScreen::Confirm {
                machine, request, ..
            } => (*machine, request.clone()),
            _ => return,
        };
        match self.move_now(target) {
            MoveNow::Same(_) | MoveNow::AccountChanged(_) => {
                let mut question = question;
                if let AccountScreen::Confirm { ready_at, .. } = &mut question {
                    *ready_at = Instant::now() + Duration::from_millis(500);
                }
                self.push_account_screen(question);
            }
            // A question that no longer fits is done with (Codex's third look).
            MoveNow::NewConversation(_) => {
                self.settle_question(machine, &request);
                self.say(
                    format!(
                        "{reference}'s move asked you something, but its pane runs another conversation now, so the question no longer fits; s on its card offers the move for this one."
                    ),
                    Tone::Info,
                );
            }
            MoveNow::Gone => {
                self.settle_question(machine, &request);
                self.say(
                    format!(
                        "{reference}'s move asked you something, but its agent isn't running any more, so nothing moved; s on its card starts it on an account."
                    ),
                    Tone::Info,
                );
            }
        }
    }

    /// The name a saved list's left-out agent goes by: its card's, else its intent's, else the
    /// account it's on and where (intent 0591, B3-42).
    fn off_list_name(&self, machine: usize, agent: &crate::actions::OffList) -> String {
        let on_card = |c: &&Card| {
            c.machine_index == machine
                && (agent.session.is_empty() || c.session.as_deref() == Some(&agent.session))
                && c.agents.iter().any(|a| a.pane_id == agent.pane)
        };
        if let Some(card) = self
            .board
            .cards
            .iter()
            .filter(on_card)
            .find(|c| c.detail.is_some())
            .or_else(|| self.board.cards.iter().find(on_card))
        {
            return card.reference.clone();
        }
        if let Some(n) = agent
            .intent
            .as_deref()
            .map(|i| i.trim_start_matches('0'))
            .filter(|n| !n.is_empty())
        {
            return format!("#{n}");
        }
        let on = if agent.account.is_empty() {
            "an agent".to_string()
        } else {
            format!("the agent on {}", agent.account)
        };
        if agent.session.is_empty() {
            on
        } else {
            format!("{on} in {}", agent.session)
        }
    }

    /// A list saved with running agents left on accounts that aren't on it: asked once whether to
    /// move them to it now, where an answer can't take keys meant for something else (intent
    /// 0591, B3-42). A backend that doesn't list them asks nothing, as before.
    fn ask_off_list(
        &mut self,
        machine: usize,
        request: &Value,
        agents: &[crate::actions::OffList],
        visible: bool,
    ) {
        if agents.is_empty() || self.off_list_moves.is_some() {
            return;
        }
        let id = request["owner"].as_str().unwrap_or_default();
        let owner = self
            .account_feed(machine)
            .and_then(|f| f.owners.iter().find(|o| o.id == id))
            .map(|o| o.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string());
        let named: Vec<OffAgent> = agents
            .iter()
            .map(|a| OffAgent {
                name: self.off_list_name(machine, a),
                request: off_list_request(a),
            })
            .collect();
        let names: Vec<String> = named.iter().map(|a| a.name.clone()).collect();
        let mut accounts: Vec<String> = agents
            .iter()
            .map(|a| a.account.clone())
            .filter(|a| !a.is_empty())
            .collect();
        accounts.sort();
        accounts.dedup();
        let one = named.len() == 1;
        let on = match accounts.len() {
            0 => "on an account that isn't on it".to_string(),
            1 => format!("on {}, which isn't on it", accounts[0]),
            _ => format!("on {}, which aren't on it", and_list(&accounts)),
        };
        let message = format!(
            "{owner}'s list is saved. On {}, {} {} still running {on}. Move {} to the list now?",
            self.machines[machine].name(),
            and_list(&names),
            if one { "is" } else { "are" },
            if one { "it" } else { "them" },
        );
        let question = AccountScreen::Confirm {
            machine,
            request: json!({
                "action": crate::app::MOVE_OFF_LIST,
                "agents": named
                    .iter()
                    .map(|a| json!({"name": a.name, "request": a.request}))
                    .collect::<Vec<_>>(),
            }),
            message,
            yes: if one { "move it" } else { "move them" }.into(),
            no: if one { "leave it" } else { "leave them" }.into(),
            target: None,
            ready_at: Instant::now() + Duration::from_millis(500),
            choices: vec![],
        };
        if visible || self.enter_can_land_here() {
            self.push_account_screen(question);
        } else {
            self.hold_question(question);
        }
    }

    /// His yes to moving them: the first move goes, and each next one once the one before has
    /// its answer (intent 0591, B3-42).
    fn start_off_list(&mut self, machine: usize, request: &Value) {
        let left: std::collections::VecDeque<OffAgent> = request["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(OffAgent {
                    name: a["name"].as_str()?.to_string(),
                    request: a["request"].clone(),
                })
            })
            .collect();
        if left.is_empty() {
            return;
        }
        self.off_list_moves = Some(OffListMoves {
            machine,
            total: left.len(),
            left,
            ..OffListMoves::default()
        });
        self.off_list_next();
    }

    /// The next move to the list, or, with none left, what came of them all.
    fn off_list_next(&mut self) {
        let Some(moves) = self.off_list_moves.as_mut() else {
            return;
        };
        if let Some(next) = moves.left.pop_front() {
            self.account_queue
                .push_back((moves.machine, next.request.clone()));
            moves.current = Some(next);
            return;
        }
        if let Some(moves) = self.off_list_moves.take() {
            let (said, tone) = moves.summary(&[]);
            self.say(said, tone);
        }
    }

    /// The name of the agent this request moves, when it's the move to the list going now.
    fn off_list_current(&self, machine: usize, request: &Value) -> Option<String> {
        let moves = self.off_list_moves.as_ref()?;
        let agent = moves.current.as_ref()?;
        (moves.machine == machine && move_asked(request) == agent.request)
            .then(|| agent.name.clone())
    }

    /// The move going now, ended this way, and the next one sent.
    fn off_list_ended(&mut self, how: OffListEnd) {
        if let Some(moves) = self.off_list_moves.as_mut() {
            moves.end(how);
        }
        self.off_list_next();
    }

    /// Whether this request, or a question about it, is still anywhere: on its way, queued, held,
    /// waiting on its sign-in, or on screen at any depth, covered by another screen or not (Codex's
    /// third look). Compared without the fields an answer adds.
    fn question_anywhere(&self, machine: usize, asked: &Value) -> bool {
        let this = |m: usize, request: &Value| m == machine && move_asked(request) == *asked;
        let on_screen = |screen: &AccountScreen| matches!(screen, AccountScreen::Confirm { machine, request, .. } if this(*machine, request));
        let mut shown = false;
        let mut mode = &self.mode;
        while let Mode::Accounts { screen, back } = mode {
            shown |= on_screen(screen);
            mode = back;
        }
        shown
            || self
                .account_pending
                .as_ref()
                .is_some_and(|p| this(p.machine, &p.request))
            || self.account_queue.iter().any(|(m, r)| this(*m, r))
            || self
                .account_later
                .iter()
                .any(|(p, _)| this(p.machine, &p.request))
            || self
                .after_signin
                .as_ref()
                .is_some_and(|a| this(a.machine, &a.request))
            || self
                .asked_about
                .iter()
                .any(|k| k.machine == machine && k.asked == *asked && k.held.is_some())
    }

    /// Keeps what an account question needs beyond its words, merged into what's kept for it
    /// already (Codex's review). Said to go on: an answer that keeps it doesn't settle it.
    pub(crate) fn keep_asked(
        &mut self,
        machine: usize,
        request: &Value,
        target: Option<Box<MoveTarget>>,
        from: Option<CardSnapshot>,
        then_waiting: Option<String>,
    ) {
        let asked = move_asked(request);
        self.asked_now.push((machine, asked.clone()));
        if let Some(kept) = self
            .asked_about
            .iter_mut()
            .find(|k| k.machine == machine && k.asked == asked)
        {
            kept.target = target.or(kept.target.take());
            kept.from = from.or(kept.from.take());
            kept.then_waiting = then_waiting.or(kept.then_waiting.take());
            return;
        }
        if target.is_some() || from.is_some() || then_waiting.is_some() {
            self.asked_about.push(AskedAbout {
                machine,
                asked,
                target,
                from,
                then_waiting,
                held: None,
                signin: None,
            });
        }
    }

    /// What's kept for the account question about this request.
    fn asked_for(&self, machine: usize, request: &Value) -> Option<&AskedAbout> {
        let asked = move_asked(request);
        self.asked_about
            .iter()
            .find(|k| k.machine == machine && k.asked == asked)
    }

    /// An account question held until he's back where it can't take keys meant for something
    /// else: kept with its own record, after any held before it, never in place of one (Codex's
    /// third look).
    pub(crate) fn hold_question(&mut self, question: AccountScreen) {
        let AccountScreen::Confirm {
            machine, request, ..
        } = &question
        else {
            return;
        };
        let (machine, request) = (*machine, request.clone());
        self.hold_screen(machine, &request, question, None);
    }

    /// Any screen an account request's answer brings (a question, a sign-in, an offer to resume),
    /// held with that request's record until he's back where it can't take keys meant for
    /// something else, after any held before it, with the request a sign-in sends again once it's
    /// done (Codex's fourth look).
    fn hold_screen(
        &mut self,
        machine: usize,
        request: &Value,
        screen: AccountScreen,
        signin: Option<AfterSignin>,
    ) {
        let asked = move_asked(request);
        self.asked_now.push((machine, asked.clone()));
        let mut kept = match self
            .asked_about
            .iter()
            .position(|k| k.machine == machine && k.asked == asked)
        {
            Some(i) => self.asked_about.remove(i),
            None => AskedAbout {
                machine,
                asked,
                target: None,
                from: None,
                then_waiting: None,
                held: None,
                signin: None,
            },
        };
        kept.held = Some(Box::new(screen));
        kept.signin = signin;
        self.asked_about.push(kept);
    }

    /// Shows what an answer brought now, where he is, or holds it until he's back where it can't
    /// take keys meant for something else: never dropped for not being in sight (Codex's fourth
    /// look). Whether it's shown now.
    fn show_or_hold(
        &mut self,
        machine: usize,
        request: &Value,
        screen: AccountScreen,
        signin: Option<AfterSignin>,
        visible: bool,
    ) -> bool {
        if visible || self.enter_can_land_here() {
            if signin.is_some() {
                self.after_signin = signin;
            }
            self.push_account_screen(screen);
            return true;
        }
        self.hold_screen(machine, request, screen, signin);
        false
    }

    /// The card the account request on its way is for, when it's on the board: a move's agent's,
    /// a fresh start's workspace. Shown as being tried again while it's sent again by itself
    /// (intent 0591).
    pub(crate) fn account_card_in_flight(&self) -> Option<String> {
        let p = self.account_pending.as_ref()?;
        let key = match &p.screen {
            Some(AccountScreen::Move { target, .. }) => Some(target.key.clone()),
            _ => self.asked_for(p.machine, &p.request).and_then(|k| {
                k.from
                    .as_ref()
                    .map(|f| f.key.clone())
                    .or_else(|| k.target.as_ref().map(|t| t.key.clone()))
            }),
        }?;
        self.card(&key).is_some().then_some(key)
    }

    /// The question held longest, taken to be asked now. Its record stays while it holds what the
    /// question's answer needs.
    pub(crate) fn take_held_question(&mut self) -> Option<AccountScreen> {
        // A sign-in waits while another's request waits on its own (there's one at a time).
        let busy_signin = self.after_signin.is_some();
        let i = self
            .asked_about
            .iter()
            .position(|k| k.held.is_some() && !(busy_signin && k.signin.is_some()))?;
        let question = self.asked_about[i].held.take()?;
        if let Some(after) = self.asked_about[i].signin.take() {
            self.after_signin = Some(after);
        }
        let kept = &self.asked_about[i];
        if kept.target.is_none() && kept.from.is_none() && kept.then_waiting.is_none() {
            self.asked_about.remove(i);
        }
        Some(*question)
    }

    /// The questions held for him, in the order they'll be asked.
    pub fn held_questions(&self) -> impl Iterator<Item = &AccountScreen> {
        self.asked_about.iter().filter_map(|k| k.held.as_deref())
    }

    /// The question about this request is done with (n or Esc on it, its answer, or its sign-in
    /// left): what follows it happens, and nothing more is kept for it (Codex's third look).
    fn settle_question(&mut self, machine: usize, request: &Value) {
        let asked = move_asked(request);
        let Some(i) = self
            .asked_about
            .iter()
            .position(|k| k.machine == machine && k.asked == asked)
        else {
            return;
        };
        let kept = self.asked_about.remove(i);
        if let Some(owner) = kept.then_waiting {
            self.queue_waiting_moves(&owner);
        }
    }

    /// A question dropped without an answer (its screen gone from under it some other way than n,
    /// Esc or y): settled as if left, so nothing is kept for it. A covered question is still on
    /// screen, and held ones wait for him.
    fn settle_dropped(&mut self) {
        let dropped: Vec<(usize, Value)> = self
            .asked_about
            .iter()
            .filter(|k| k.held.is_none() && !self.question_anywhere(k.machine, &k.asked))
            .map(|k| (k.machine, k.asked.clone()))
            .collect();
        for (machine, asked) in dropped {
            self.settle_question(machine, &asked);
        }
    }

    /// Moves to the list he stopped: Esc on the one going now, or leaving its sign-in. The rest
    /// aren't sent, and what came of them is said (intent 0591, B3-42).
    fn off_list_stop(&mut self) {
        let Some(mut moves) = self.off_list_moves.take() else {
            return;
        };
        let stopped: Vec<String> = moves
            .current
            .take()
            .into_iter()
            .chain(moves.left.drain(..))
            .map(|a| a.name)
            .collect();
        let (said, tone) = moves.summary(&stopped);
        self.say(said, tone);
    }

    /// The same, for a move to the list whose question is nowhere any more without his n, Esc or
    /// an answer (its screen gone from under it some other way): the rest aren't sent.
    fn off_list_left(&mut self) {
        let Some(moves) = &self.off_list_moves else {
            return;
        };
        let Some(agent) = &moves.current else {
            return;
        };
        if self.question_anywhere(moves.machine, &agent.request) {
            return;
        }
        self.off_list_stop();
    }

    /// After a sign-in's answer: once the sign-in a move waits on is done on its machine, the
    /// sign-in closes and the move goes again by itself as a new request, from the screen under it
    /// (the move picker it was asked in, as a rule) (intent 0591, B3-10). A sign-in that failed keeps the move waiting
    /// while r can try again; leaving it drops the move.
    fn signin_then_move(&mut self) {
        let Some(after) = self.after_signin.clone() else {
            return;
        };
        let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            back,
        } = &self.mode
        else {
            return;
        };
        if s.code != after.code
            || !s.machines.contains(&after.machine)
            || s.failed
            || s.current < s.machines.len()
        {
            return;
        }
        self.after_signin = None;
        self.mode = (**back).clone();
        self.say(
            format!(
                "{} is signed in on {}, so the {} goes on.",
                after.code,
                self.machines[after.machine].name(),
                waiting_on_signin(&after.request).0
            ),
            Tone::Info,
        );
        self.account_queue.push_back((after.machine, after.request));
    }

    /// A move waiting on a sign-in he has left (the box asking where to sign in, or the sign-in
    /// itself): dropped, and said (intent 0591, B3-10).
    fn drop_left_signin(&mut self) {
        let Some(after) = &self.after_signin else {
            return;
        };
        let on_it = match &self.mode {
            Mode::Accounts {
                screen: AccountScreen::Where { code, .. },
                ..
            } => *code == after.code,
            Mode::Accounts {
                screen: AccountScreen::Signin(s),
                ..
            } => s.code == after.code,
            _ => false,
        };
        if on_it {
            return;
        }
        let Some(after) = self.after_signin.take() else {
            return;
        };
        self.say(
            format!(
                "{}: {} isn't signed in on {}.",
                waiting_on_signin(&after.request).1,
                after.code,
                self.machines
                    .get(after.machine)
                    .map(|m| m.name())
                    .unwrap_or_default()
            ),
            Tone::Info,
        );
        // Its question is done with; a move to the list stops the rest (Codex's third look).
        self.settle_question(after.machine, &after.request);
        if self
            .off_list_current(after.machine, &after.request)
            .is_some()
        {
            self.off_list_stop();
        }
    }

    /// The move picker's agent, as it is now, in place (intent 0591, K2-13).
    fn retarget_picker(&mut self, agent: AccountAgent) {
        if let Mode::Accounts {
            screen: AccountScreen::Move { target, .. },
            ..
        } = &mut self.mode
        {
            target.agent = agent;
        }
    }

    /// Every account request is made here (Codex's fourth look). What it's asked from is kept with
    /// it as it's made: the screen it came from, and for a move from the picker its agent, in the
    /// move's own record. One made while another is on its way waits its turn with all of that,
    /// and is never sent over the one on its way.
    pub(crate) fn send_account(&mut self, machine: usize, request: Value) -> Action {
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
        // What the request came from is kept in its record, found by the request when its answer
        // comes, never in a slot another answer could take (Codex's third look).
        let origin = Pending {
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
        };
        if request["action"] == "account-move"
            && let Some(AccountScreen::Move { target, .. }) = &origin.screen
            && request["pane"].as_str() == Some(target.agent.pane.as_str())
        {
            let target = target.clone();
            self.keep_asked(machine, &request, Some(target), None, None);
        }
        if self.busy {
            self.account_later.push_back((origin, request));
            return Action::None;
        }
        self.account_pending = Some(origin);
        Action::Mutate { machine, request }
    }

    /// Whether this request waits its turn behind the one on its way.
    fn account_waits(&self, machine: usize, request: &Value) -> bool {
        let request = crate::actions::request_metadata(request);
        self.account_later
            .iter()
            .any(|(p, _)| p.machine == machine && p.request == request)
    }

    /// Whether an account reply is the answer to the request on its way: matched before anything
    /// about that request is touched (its busy line, its "trying again", what it was asked from),
    /// so another request's answer coming first leaves it all as it was (Codex's fourth look).
    /// With none known to be on its way, it's taken as that one's, as before.
    pub(crate) fn account_reply_is_ours(&self, machine: usize, request: &Value) -> bool {
        self.account_pending.as_ref().is_none_or(|p| {
            p.machine == machine
                && crate::actions::without_op(&p.request)
                    == crate::actions::without_op(&crate::actions::request_metadata(request))
        })
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
            started_fresh: false,
            set_aside: false,
            replaced: false,
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
        let action = self.send_account(machine, request.clone());
        if action == Action::None
            && !self.account_waits(machine, &request)
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
            // Esc on a question leaves it: it's settled, and a move to the list stops the rest
            // (Codex's third look).
            if let AccountScreen::Confirm {
                machine, request, ..
            } = &screen
            {
                self.settle_question(*machine, request);
                if self.off_list_current(*machine, request).is_some() {
                    self.off_list_stop();
                }
            }
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
                                            let message =
                                                self.remove_question(*machine, &a.code, &a.name);
                                            self.push_account_screen(AccountScreen::Confirm { machine:*machine, request:json!({"action":"account-remove","code":code}), message, yes:"remove".into(), no:"keep it".into(), target:None, ready_at:Instant::now()+Duration::from_millis(500), choices:vec![] });
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
                    // The agent changed while he chose (intent 0591, K2-13): the same conversation
                    // on another account still moves where he picked; another conversation in its
                    // pane is shown in the picker, for Enter again; none running ends it.
                    let to_now = choices.get(*sel).cloned();
                    match self.move_now(target) {
                        MoveNow::AccountChanged(now) => {
                            if to_now.as_deref() == Some(now.account.as_str()) {
                                self.say(
                                    format!(
                                        "{}'s agent is on {} already: it moved there while you chose.",
                                        target.reference, now.account
                                    ),
                                    Tone::Info,
                                );
                                self.mode = *back;
                                return Action::None;
                            }
                            self.say(
                                format!(
                                    "{}'s agent is on {} now; moving it where you picked.",
                                    target.reference, now.account
                                ),
                                Tone::Info,
                            );
                            target.agent = now;
                        }
                        MoveNow::NewConversation(now) => {
                            self.say(
                                format!(
                                    "{} changed while you chose: its pane runs a new conversation now, shown here. ⏎ moves this one.",
                                    target.reference
                                ),
                                Tone::Info,
                            );
                            target.agent = now;
                            self.mode = Mode::Accounts { screen, back };
                            return Action::None;
                        }
                        MoveNow::Gone => {
                            self.mode = *back;
                            // A bare workspace's agent, its conversation known: offered to resume
                            // in a new workspace on the account picked (Round 2b, K1-08).
                            let said = format!(
                                "{}'s agent isn't running any more, so there's nothing to move.",
                                target.reference
                            );
                            if let Some(resume) = self.bare_resume_of_target(target)
                                && self.offer_resume(&resume, to_now.as_deref(), &said)
                            {
                                return Action::None;
                            }
                            self.say(
                                format!(
                                    "{}'s agent isn't running any more, so there's nothing to move; s on its card starts it on an account.",
                                    target.reference
                                ),
                                Tone::Info,
                            );
                            return Action::None;
                        }
                        // Its conversation may have become known since: the move names it.
                        MoveNow::Same(now) => target.agent = now,
                    }
                    if self.target_current(target) {
                        if let Some(to) = choices.get(*sel) {
                            if let Some(stays) = self.held_to(target.machine, &target.agent, to) {
                                self.say(
                                    format!(
                                        "{} has to stay on {stays}: its safety settings only work there. Pick a {stays} account.",
                                        target.reference
                                    ),
                                    Tone::Warn,
                                );
                                return Action::None;
                            }
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
                KeyCode::Backspace if owner.parent.is_some() && owner.own => {
                    self.push_account_screen(AccountScreen::Confirm {
                        machine: *machine,
                        request: json!({"action":"account-models","owner":owner.id,"inherit":true}),
                        message: self.follow_question(*machine, owner),
                        yes: "follow organization".into(),
                        no: "keep this list".into(),
                        target: None,
                        ready_at: Instant::now() + Duration::from_millis(500),
                        choices: vec![],
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
                    let value = if *models == owner.models && !owner.own && owner.parent.is_some() {
                        json!({"action":"account-models","owner":owner.id,"inherit":true})
                    } else {
                        json!({"action":"account-models","owner":owner.id,"models":models})
                    };
                    request = Some((*machine, value));
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
                                format!(
                                    "{} needs {missing}: type it in the {missing} row.",
                                    add.provider.name()
                                ),
                                Tone::Warn,
                            );
                            // The cursor goes to that row (intent 0591).
                            if let Some(row) = add
                                .provider
                                .fields
                                .iter()
                                .zip(&add.extra)
                                .position(|(f, value)| f.required && value.trim().is_empty())
                            {
                                add.field = AddAccount::FIELDS_AT + row;
                            }
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
                        s.set_aside = true;
                        s.message =
                            "Browser sign-in set aside for a token. r starts it again.".into();
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
                        s.set_aside = false;
                        s.replaced = false;
                        // Asked for by hand: its link opens, and it may start afresh once more.
                        s.started_fresh = false;
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
                                    json!({"action":"account-signin","ticket":ticket,"step":"finish","page_code":code_from_paste(s.page_code.trim())}),
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
                choices,
                ..
            } => match key.code {
                KeyCode::Char('n') => {
                    self.mode = *back;
                    let (machine, request) = (*machine, pending.clone());
                    self.settle_question(machine, &request);
                    // One of the moves to the list: this one stays, and the next is asked.
                    if self.off_list_current(machine, &request).is_some() {
                        self.off_list_ended(OffListEnd::Kept);
                    }
                    return Action::None;
                }
                // Longer than the screen holds, it scrolls (Codex's review).
                KeyCode::PageDown | KeyCode::Down => self.question_view.down(3),
                KeyCode::PageUp | KeyCode::Up => self.question_view.up(3),
                // An answer while some of it is still below shows the rest first.
                KeyCode::Char(c)
                    if Instant::now() >= *ready_at
                        && self.question_view.unseen()
                        && (c == 'y'
                            || crate::actions::choice_keys(choices).contains(&Some(c))) =>
                {
                    self.question_view.to_end();
                    *ready_at = Instant::now() + Duration::from_millis(500);
                }
                // Yes, or one of the answers its backend offers (intent 0591): the same request
                // with its token, and that choice.
                KeyCode::Char(c)
                    if Instant::now() >= *ready_at
                        && (c == 'y'
                            || crate::actions::choice_keys(choices).contains(&Some(c))) =>
                {
                    // Kerdr's own question: its yes moves them to the list, one at a time.
                    if pending["action"] == crate::app::MOVE_OFF_LIST {
                        let (machine, request) = (*machine, pending.clone());
                        self.mode = *back;
                        self.settle_question(machine, &request);
                        self.start_off_list(machine, &request);
                        return Action::None;
                    }
                    let chosen = choices
                        .iter()
                        .zip(crate::actions::choice_keys(choices))
                        .find(|(_, k)| *k == Some(c) && c != 'y')
                        .map(|(choice, _)| choice.id.clone());
                    let now = target
                        .as_ref()
                        .map(|t| (t.reference.clone(), self.move_now(t)));
                    match now {
                        // The same agent, or the same conversation on another account: its backend
                        // checks its token and asks again if that changed what it asked.
                        None | Some((_, MoveNow::Same(_) | MoveNow::AccountChanged(_))) => {
                            // A yes is a new request: never the question's op (intent 0591), and
                            // only this answer, never one given to an earlier question (Codex's
                            // review).
                            let mut answer = crate::actions::without_op(pending);
                            if let Some(fields) = answer.as_object_mut() {
                                fields.remove("choice");
                            }
                            if let Some(id) = chosen {
                                answer["choice"] = json!(id);
                            }
                            request = Some((*machine, answer));
                            let target = target.clone();
                            self.mode = *back;
                            if let Some((mi, r)) = request {
                                // What the question came from goes with its yes: the agent a
                                // question it brings is about, and (in send_account) the card a
                                // start goes into.
                                self.keep_asked(mi, &r, target, None, None);
                                return self.send_account(mi, r);
                            }
                            return Action::None;
                        }
                        // Another conversation: the picker, when it's under the question, shows it
                        // for Enter again (intent 0591, K2-13).
                        Some((reference, MoveNow::NewConversation(agent))) => {
                            self.mode = *back;
                            let (machine, request) = (*machine, pending.clone());
                            self.settle_question(machine, &request);
                            self.retarget_picker(agent);
                            let picker = matches!(
                                self.mode,
                                Mode::Accounts {
                                    screen: AccountScreen::Move { .. },
                                    ..
                                }
                            );
                            let next = if picker {
                                "⏎ moves this one."
                            } else {
                                "s on its card offers the move for this one."
                            };
                            self.say(
                                format!(
                                    "{reference} changed while you decided: its pane runs a new conversation now, so nothing moved. {next}"
                                ),
                                Tone::Info,
                            );
                            return Action::None;
                        }
                        Some((reference, MoveNow::Gone)) => {
                            self.mode = *back;
                            let (machine, request) = (*machine, pending.clone());
                            self.settle_question(machine, &request);
                            self.say(
                                format!(
                                    "{reference}'s agent isn't running any more, so nothing moved; s on its card starts it on an account."
                                ),
                                Tone::Info,
                            );
                            return Action::None;
                        }
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
        self.drop_left_signin();
        self.off_list_left();
        self.settle_dropped();
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
            // A move to the list that can't go now: said with the others, and the next one goes.
            if action == Action::None && self.off_list_current(mi, &request).is_some() {
                let away = format!("{} isn't answering", self.machines[mi].name());
                self.off_list_ended(OffListEnd::Failed(away));
            }
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
        // Asked for while another was on its way: sent now, with what it was asked from (Codex's
        // fourth look).
        if let Some((origin, request)) = self.account_later.pop_front() {
            let machine = origin.machine;
            if !self.can_write(machine) {
                return Action::None;
            }
            self.account_pending = Some(origin);
            return Action::Mutate { machine, request };
        }
        // The code to type ran out with the page unused: a new one, by itself, once (intent 0591).
        let now = self.now();
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut self.mode
            && !s.failed
            && !s.started_fresh
            && !s.restart
            && s.page_code.is_empty()
            && !s.editing
            && s.expired(now)
            && let (Some(&mi), Some(ticket)) = (s.machines.get(s.current), s.ticket.clone())
        {
            s.started_fresh = true;
            s.restart = true;
            s.url = None;
            s.needs_code = false;
            s.user_code = None;
            s.expires = None;
            s.message = "Starting a fresh sign-in…".into();
            self.say("That code ran out; here's a new one.", Tone::Info);
            return self.send_account(
                mi,
                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
            );
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
        let Some(name) = self.off_list_current(machine, &request) else {
            return self.account_answer(machine, request, result);
        };
        // One of the moves to the list he said yes to (intent 0591, B3-42). Its question is asked
        // now, named, and answered before the next move is sent.
        if let Ok(response) = &result
            && response.confirmation
        {
            self.account_pending.take_if(|p| {
                p.machine == machine
                    && crate::actions::without_op(&p.request)
                        == crate::actions::without_op(&crate::actions::request_metadata(&request))
            });
            let Some(token) = response
                .confirm
                .clone()
                .filter(|t| request["confirm"].as_str() != Some(t.as_str()))
            else {
                return self.off_list_ended(OffListEnd::Failed(
                    "its backend asked the same question again after your yes".into(),
                ));
            };
            let request = reasked(&request, &token);
            let message = if response.message.contains(&name) {
                response.message.clone()
            } else {
                format!("{name}: {}", response.message)
            };
            let question = AccountScreen::Confirm {
                machine,
                request,
                message,
                yes: response.yes.clone().unwrap_or_else(|| "move it".into()),
                no: response.no.clone().unwrap_or_else(|| "leave it".into()),
                target: None,
                ready_at: Instant::now() + Duration::from_millis(500),
                choices: response.choices.clone(),
            };
            if self.enter_can_land_here() {
                self.push_account_screen(question);
            } else {
                // Held with any others, each asked in turn (Codex's third look).
                self.hold_question(question);
                self.say(
                    format!(
                        "{name}'s move to the list asks you something first; it's asked once you're back on the board."
                    ),
                    Tone::Info,
                );
            }
            return;
        }
        let how = match &result {
            // Its answer was lost: it may have moved, so it's said as not known, never as not
            // moved (Codex's third look).
            Err(e) if crate::actions::result_unknown(e) => OffListEnd::Unknown(e.clone()),
            Err(e) => OffListEnd::Failed(account_error(&request, e)),
            Ok(r) if r.ok && r.state.as_deref() == Some("queued") => OffListEnd::Later,
            Ok(r) if r.ok => OffListEnd::Moved,
            Ok(r) => OffListEnd::Failed(r.message.clone()),
        };
        let asked = move_asked(&request);
        self.account_answer(machine, request, result);
        // Its account isn't signed in there: the move goes again by itself once it is, and the
        // next waits for that.
        if self
            .after_signin
            .as_ref()
            .is_some_and(|a| a.machine == machine && move_asked(&a.request) == asked)
        {
            return;
        }
        self.off_list_ended(how);
    }

    /// An account request's answer. Its question, if it had one, is settled by it, unless the
    /// answer asks again, is held for him, or waits on a sign-in (Codex's third look).
    fn account_answer(&mut self, machine: usize, request: Value, result: Result<Response, String>) {
        let asked = move_asked(&request);
        self.asked_now.clear();
        self.answer_account(machine, request, result);
        let goes_on = self
            .asked_now
            .iter()
            .any(|(m, a)| *m == machine && *a == asked);
        self.asked_now.clear();
        if !goes_on {
            self.settle_question(machine, &asked);
        }
    }

    fn answer_account(&mut self, machine: usize, request: Value, result: Result<Response, String>) {
        // The request this answers, matched before anything is taken: another request's answer
        // leaves what's kept for this one where it is (Codex's third look). The answer's request
        // carries the op Kerdr gave it as it went (intent 0591).
        let pending = self.account_pending.take_if(|p| {
            p.machine == machine
                && crate::actions::without_op(&p.request)
                    == crate::actions::without_op(&crate::actions::request_metadata(&request))
        });
        // A fresh agent's start or a resume (intent 0591, K1-105, K1-03): the card it was asked
        // on, for going in, from its own record.
        let start_from = if matches!(
            request["action"].as_str(),
            Some("account-start" | crate::app::resume::RESUME)
        ) {
            self.asked_for(machine, &request)
                .and_then(|k| k.from.clone())
        } else {
            None
        };
        let visible = match (&pending, &self.mode) {
            (Some(p), Mode::Accounts { screen, .. }) => p
                .screen
                .as_ref()
                .is_some_and(|old| same_screen(old, screen)),
            _ => false,
        };
        // The agent a move was asked for: its picker's, else what's kept for its question, which
        // outlives the picker (Codex's review).
        let move_target = match pending.as_ref().and_then(|p| p.screen.as_ref()) {
            Some(AccountScreen::Move { target, .. }) => Some(target.clone()),
            _ => self
                .asked_for(machine, &request)
                .and_then(|k| k.target.clone()),
        };
        // A list saved with "move waiting sessions" ticked: they move once it's saved, after any
        // question it brings (Codex's review).
        let then_waiting = match pending.as_ref().and_then(|p| p.screen.as_ref()) {
            Some(AccountScreen::Models {
                owner,
                move_waiting: true,
                ..
            }) => Some(owner.id.clone()),
            _ => None,
        };
        let response = match result {
            Ok(r) => r,
            Err(e) => {
                // r, or a code that ran out, goes on to start whatever its cancel came to, no answer
                // included: a start replaces any sign-in still running (intent 0591). If the machine
                // is truly away, the start says so.
                if request["action"] == "account-signin"
                    && request["step"] == "cancel"
                    && visible
                    && let Mode::Accounts {
                        screen: AccountScreen::Signin(s),
                        ..
                    } = &mut self.mode
                    && s.restart
                {
                    s.restart = false;
                    s.ticket = None;
                    s.failed = false;
                    self.account_queue.push_back((machine, s.start()));
                    return;
                }
                // Named by what was asked, in words for the accounts screen (intent 0591).
                let e = account_error(&request, &e);
                if request["action"] == "account-move-waiting" {
                    self.account_batch_failed = true;
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
        // A list is saved even when its backend then asks about the agents left running elsewhere
        // (intent 0591, B3-42).
        if action == "account-models" && (response.ok || response.confirmation) {
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
            // A machine whose part didn't happen makes the whole summary a warning, not news
            // (intent 0591).
            self.account_batch_failed |= !response.ok;
            if self.account_queue.is_empty() {
                let tone = if self.account_batch_failed {
                    Tone::Warn
                } else {
                    Tone::Info
                };
                self.say(self.account_batch_messages.join("; "), tone);
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
                    // A start replaces any sign-in still running, so r goes on whatever the
                    // cancel said (intent 0591).
                    s.ticket = None;
                    s.failed = false;
                    self.account_queue.push_back((machine, s.start()));
                    return;
                }
                // Asked to stop one that has gone already: that's what was asked for.
                let gone =
                    response.message.contains("no such") || response.message.contains("had ended");
                if !response.ok && !gone {
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
                // It ended by itself, or a passing failure outlasted its retries: a fresh sign-in
                // starts in its place, once, in place of a dead end (intent 0591).
                if !response.ok
                    && (response.again || response.retry.is_some())
                    && !s.started_fresh
                    && matches!(request["step"].as_str(), Some("start" | "poll" | "finish"))
                {
                    s.started_fresh = true;
                    s.ticket = None;
                    s.url = None;
                    s.page_code.clear();
                    s.needs_code = false;
                    s.user_code = None;
                    s.expires = None;
                    s.editing = false;
                    s.failed = false;
                    s.note = None;
                    s.message = "Starting a fresh sign-in…".into();
                    let start = s.start();
                    self.account_queue.push_back((machine, start));
                    self.say(fresh_signin_words(&response.message), Tone::Info);
                    return;
                }
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
                if !response.ok
                    || !(response.signin_under_way() || response.status.as_deref() == Some("done"))
                {
                    s.failed = true;
                    s.replaced = response.replaced;
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
                } else if response.status.as_deref() == Some("starting")
                    && response.url.is_none()
                    && response
                        .ticket
                        .as_deref()
                        .or(s.ticket.as_deref())
                        .is_some_and(|t| !t.is_empty())
                {
                    // A slow program with no link yet: its ticket is kept, and a poll brings the
                    // link (intent 0591).
                    if response.ticket.is_some() {
                        s.ticket = response.ticket;
                    }
                } else if request["step"] == "start" || (s.url.is_none() && response.url.is_some())
                {
                    let ticket = response.ticket.or_else(|| s.ticket.clone());
                    let valid = response
                        .url
                        .as_deref()
                        .is_some_and(crate::accounts::valid_signin_url);
                    if !valid || ticket.as_deref().is_none_or(str::is_empty) {
                        s.failed = true;
                        let name = self.machines[machine].name();
                        s.message = format!(
                            "{name}'s backend sent a sign-in link Kerdr won't open (not a plain https link with a ticket), so that sign-in was cancelled. r tries again; if it happens again, that backend needs a fix."
                        );
                        if let Some(ticket) = ticket {
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
                        let name = self.machines[machine].name();
                        s.message = format!(
                            "{name}'s backend sent a code to type that Kerdr can't show safely, so that sign-in was cancelled. r tries again."
                        );
                        if let Some(ticket) = ticket {
                            self.account_queue.push_back((
                                machine,
                                json!({"action":"account-signin","step":"cancel","ticket":ticket}),
                            ));
                        }
                    } else {
                        s.ticket = ticket;
                        s.url = response.url;
                        s.user_code = response.user_code;
                        s.expires = response.expires;
                        s.needs_code = response.needs_code && s.user_code.is_none();
                        // One Kerdr started by itself shows its link and leaves the browser be:
                        // nobody may be at the machine by then (intent 0591).
                        if !s.started_fresh {
                            self.signin_open = s.url.clone();
                        }
                    }
                }
                s.poll_at = Instant::now() + Duration::from_secs(2);
            }
            self.signin_then_move();
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
        // A fresh agent started in an empty workspace: gone into, as a reopen is, while you're
        // still on its card; otherwise said, with how to go in (intent 0591, K1-105).
        if action == "account-start"
            && response.ok
            && !response.confirmation
            && let (Some(session), Some(workspace)) =
                (response.session.clone(), response.workspace.clone())
        {
            match start_from.filter(|f| self.still_on(f, Some(&workspace))) {
                Some(from) => {
                    self.open_after = Some(crate::app::OpenAfter {
                        machine,
                        label: response.label.clone().unwrap_or_else(|| workspace.clone()),
                        session,
                        workspace,
                        tab: response.tab.clone(),
                        from: from.key,
                    });
                }
                None => {
                    let message = if response.message.contains('⏎') {
                        response.message.clone()
                    } else {
                        format!("{} ⏎ on its card goes in.", response.message.trim_end())
                    };
                    self.say(message, Tone::Good);
                }
            }
            return;
        }
        // A conversation resumed in a new workspace, or found open already: gone into, as ⏎ asked,
        // while you're where going in can't take keys meant for something else; otherwise said,
        // with how to go in (intent 0591, K1-03, K1-08, K3-03).
        if action == crate::app::resume::RESUME
            && response.ok
            && !response.confirmation
            && let (Some(session), Some(workspace)) =
                (response.session.clone(), response.workspace.clone())
        {
            match start_from.filter(|_| self.enter_can_land_here()) {
                Some(from) => {
                    self.open_after = Some(crate::app::OpenAfter {
                        machine,
                        label: response.label.clone().unwrap_or_else(|| workspace.clone()),
                        session,
                        workspace,
                        tab: response.tab.clone(),
                        from: from.key,
                    });
                }
                None => {
                    let message = if response.message.contains('⏎') {
                        response.message.clone()
                    } else {
                        format!("{} ⏎ on its card goes in.", response.message.trim_end())
                    };
                    self.say(message, Tone::Good);
                }
            }
            return;
        }
        // A move whose agent turned out to run nowhere on that machine now, picked on a bare
        // workspace whose conversation is known: offered to resume it in a new workspace there on
        // the account picked, to a backend that can (intent 0591, K1-08).
        if action == "account-move"
            && !response.ok
            && response.not_running
            && response.why.as_deref() == Some("gone")
            && let Some(resume) = move_target
                .as_deref()
                .and_then(|t| self.bare_resume_of_target(t))
        {
            if visible && let Mode::Accounts { back, .. } = self.mode.clone() {
                self.mode = *back;
            }
            // Over typing, it's held with what it would resume, and offered once he's back: never
            // lost for not being in sight, even once that card has gone (Codex's fourth look).
            let now = visible || self.enter_can_land_here();
            if self.offer_resume_when(&resume, request["to"].as_str(), &response.message, now) {
                return;
            }
        }
        // Another account action's question (a list saved with agents left elsewhere, an account
        // that exists as the other kind, a machine's own sign-in to remove): asked as a move's is,
        // its yes the same request with its token (intent 0591). Over the screen it was asked
        // from, or once he's back where it can't take typing.
        if action != "account-move"
            && response.confirmation
            && let Some(token) = response.confirm.clone()
            && request["confirm"].as_str() != Some(token.as_str())
        {
            // The list is saved: its screen is done with.
            if action == "account-models"
                && visible
                && let Mode::Accounts { back, .. } = self.mode.clone()
            {
                self.mode = *back;
            }
            let request = reasked(&request, &token);
            // What follows its yes, kept with it: the card a fresh agent's start goes into, and
            // the waiting sessions a saved list moves.
            self.keep_asked(machine, &request, None, start_from, then_waiting);
            let question = AccountScreen::Confirm {
                machine,
                request,
                message: response.message.clone(),
                yes: response.yes.clone().unwrap_or_else(|| "yes".into()),
                no: response.no.clone().unwrap_or_else(|| "no".into()),
                target: None,
                ready_at: Instant::now() + Duration::from_millis(500),
                choices: response.choices.clone(),
            };
            if visible || self.enter_can_land_here() {
                self.push_account_screen(question);
            } else {
                self.hold_question(question);
                self.say(
                    "An account change asks you something first; it's asked once you're back on the board.",
                    Tone::Info,
                );
            }
            return;
        }
        if action == "account-move" && response.confirmation && !visible {
            // He left the picker before its question came: it's asked once he's back on the board
            // or a list, for the agent as it is then, never over typing (intent 0591, K2-27).
            // The picker it was asked from, or the agent its last yes was for when that yes was
            // given over the board.
            if let (Some(token), Some(target)) = (response.confirm.clone(), move_target.clone())
                && request["confirm"].as_str() != Some(token.as_str())
            {
                let starts = response.id.is_some() || response.session.is_some();
                let reference = target.reference.clone();
                let request = reasked(&request, &token);
                // Kept even for a start's question, which checks no agent: a changed one is asked
                // again for it (Codex's review).
                self.keep_asked(machine, &request, Some(target.clone()), None, None);
                let question = AccountScreen::Confirm {
                    machine,
                    request,
                    message: response.message,
                    yes: response.yes.unwrap_or_else(|| "move when it stops".into()),
                    no: response.no.unwrap_or_else(|| "leave it".into()),
                    target: (!starts).then_some(target),
                    ready_at: Instant::now() + Duration::from_millis(500),
                    choices: response.choices.clone(),
                };
                if self.enter_can_land_here() {
                    self.ask_held_move(question);
                } else {
                    self.hold_question(question);
                    self.say(
                        format!(
                            "{reference}'s move asks you something first; it's asked once you're back on the board."
                        ),
                        Tone::Info,
                    );
                }
                return;
            }
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
                // The agent changed while the question came (intent 0591, K2-64): the same
                // conversation on another account is still asked about; another conversation is
                // shown in the picker for Enter again; none running ends it. Never a question
                // left on screen that can't be answered.
                let now = (!starts).then(|| self.move_now(&target));
                let target = match now {
                    Some(MoveNow::AccountChanged(agent)) => {
                        Box::new(MoveTarget { agent, ..*target })
                    }
                    Some(MoveNow::NewConversation(agent)) => {
                        self.retarget_picker(agent);
                        self.say(
                            format!(
                                "{} changed while you chose: its pane runs a new conversation now, shown here. ⏎ moves this one.",
                                target.reference
                            ),
                            Tone::Info,
                        );
                        return;
                    }
                    Some(MoveNow::Gone) => {
                        if let Mode::Accounts { back, .. } = self.mode.clone() {
                            self.mode = *back;
                        }
                        self.say(
                            format!(
                                "{}'s agent isn't running any more, so nothing moved; s on its card starts it on an account.",
                                target.reference
                            ),
                            Tone::Info,
                        );
                        return;
                    }
                    _ => target,
                };
                {
                    let request = reasked(&request, &token);
                    self.keep_asked(machine, &request, Some(target.clone()), None, None);
                    self.push_account_screen(AccountScreen::Confirm {
                        machine,
                        request,
                        message: response.message,
                        yes: response.yes.unwrap_or_else(|| "move when it stops".into()),
                        no: response.no.unwrap_or_else(|| "leave it".into()),
                        target: (!starts).then_some(target),
                        ready_at: Instant::now() + Duration::from_millis(500),
                        choices: response.choices.clone(),
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
                Some(AccountScreen::Models { .. }) if action == "account-models" => {
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
        // A list saved with "move waiting sessions" ticked: they move now, from what was saved
        // with its request, whether or not its screen is still up (Codex's third look). One
        // kept with a question it brought moves when that's settled instead.
        if action == "account-models"
            && response.ok
            && !response.confirmation
            && let Some(owner) = then_waiting.filter(|_| {
                self.asked_for(machine, &request)
                    .is_none_or(|k| k.then_waiting.is_none())
            })
        {
            self.queue_waiting_moves(&owner);
            if self.account_queue.is_empty() && !self.account_batch_messages.is_empty() {
                return;
            }
        }
        // A list saved with running agents left on other accounts: asked once whether to move them
        // to it now (intent 0591, B3-42).
        if action == "account-models" && response.ok && !response.off_list.is_empty() {
            let agents = response.off_list.clone();
            self.ask_off_list(machine, &request, &agents, visible);
        }
        // What's said last, when the box an answer brings waits until he's back.
        let mut held_said = None;
        // The account a move or the current account needs isn't signed in on that machine: the box
        // that signs it in opens, and a move goes on by itself once that's done (intent 0591,
        // B3-10, B3-40). Only where it can't take typing: over typing it's held, with the request
        // that goes again, and opens once he's back (Codex's fourth look).
        if response.sign_in
            && matches!(
                action,
                "account-move" | "account-current" | "account-start" | crate::app::resume::RESUME
            )
            && let Some(code) = response.code.clone().filter(|c| !c.is_empty())
        {
            // A move, a fresh agent's start or a resume goes again by itself once it's signed in
            // (intent 0591, B3-10, Round 2b).
            let again = action != "account-current" && !response.ok;
            let after = again.then(|| AfterSignin {
                machine,
                code: code.clone(),
                request: crate::actions::without_op(&request),
            });
            // What it was for, past the screen it may have been asked from: the agent a move's
            // question is asked for, the card a start or resume goes into (Codex's review).
            if again {
                self.keep_asked(machine, &request, move_target.clone(), start_from, None);
            }
            let label = format!("Sign {code} in on {}", self.machines[machine].name());
            let screen = AccountScreen::Where {
                code,
                copy: None,
                message: response.message.clone(),
                choices: vec![WhereChoice {
                    label,
                    machines: vec![machine],
                }],
                sel: 0,
            };
            if !self.show_or_hold(machine, &request, screen, after, visible) {
                held_said = Some("Its sign-in opens once you're back on the board.");
            }
        }
        // The test found it isn't signed in: the box that signs it in opens over the list, and
        // nothing starts until ⏎ (intent 0591). Only there: nothing he asked to change waits on it,
        // and its message says how to sign in, so it never opens over what he's gone on to.
        if action == "account-test"
            && !response.ok
            && response.sign_in
            && visible
            && let Some(code) = request["code"].as_str()
        {
            let copy = request["copy"].as_str().and_then(|id| {
                let c = self.account(machine, code)?.copy(id)?;
                Some((c.id.clone(), c.name().to_string()))
            });
            let label = format!("Sign {code} in on {}", self.machines[machine].name());
            let screen = AccountScreen::Where {
                code: code.to_string(),
                copy,
                message: response.message.clone(),
                choices: vec![WhereChoice {
                    label,
                    machines: vec![machine],
                }],
                sel: 0,
            };
            self.push_account_screen(screen);
        }
        // Done is the success colour; done already, or done with something still to come, is
        // news; not done is a warning (intent 0591). Sign-ins say `status`, moves `state`.
        let did_nothing = response.account_unchanged
            || [response.status.as_deref(), response.state.as_deref()]
                .into_iter()
                .any(|said| matches!(said, Some("stale" | "unchanged" | "already")))
            || response.message.starts_with("already on ");
        let left = response
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| response.ok && !n.is_empty() && !response.message.contains(*n))
            .map(str::to_string);
        let message = match left {
            Some(left) => format!("{} {left}", response.message),
            None => response.message,
        };
        let message = match held_said {
            Some(held) => format!("{} {held}", message.trim_end()),
            None => message,
        };
        let tone = if !response.ok {
            Tone::Warn
        } else if did_nothing || response.note.is_some() {
            Tone::Info
        } else {
            Tone::Good
        };
        self.say(message, tone);
    }
}

/// The code a sign-in page shows, out of a longer paste: pages give `code#state` and browsers add
/// text around it. Anything else is sent as it was typed (intent 0591).
fn code_from_paste(pasted: &str) -> &str {
    let is_part = |p: &str| {
        p.len() >= 16
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~')
    };
    pasted
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | ',')
        })
        .find(|word| matches!(word.split_once('#'), Some((a, b)) if is_part(a) && is_part(b)))
        .unwrap_or(pasted)
}

/// What's said as a fresh sign-in starts in place of one that stopped by itself (intent 0591).
fn fresh_signin_words(message: &str) -> String {
    if message.contains("had ended") {
        return "That sign-in had ended; here's a new link.".into();
    }
    // Its "r starts a new one" is for a Kerdr that doesn't start one by itself.
    let said = message.trim();
    let said = said
        .split_once(" r starts a new")
        .map_or(said, |(before, _)| before)
        .trim_end();
    let mut letters = said.chars();
    match letters.next() {
        Some(first) => format!(
            "{}{} Kerdr started a fresh sign-in.",
            first.to_uppercase(),
            letters.as_str()
        ),
        None => "Kerdr started a fresh sign-in.".into(),
    }
}

/// An account request's error, named by what was asked: "Moving the agent to ARK_C: no answer
/// came…". One whose result is unknown points at the accounts screen, not a card (intent 0591).
fn account_error(request: &Value, error: &str) -> String {
    let code = |field: &str| request[field].as_str().unwrap_or("the account").to_string();
    let what = match request["action"].as_str().unwrap_or_default() {
        "account-move" => match code("to").as_str() {
            "list" => "Moving the agent to its owner's list".into(),
            to => format!("Moving the agent to {to}"),
        },
        "account-move-waiting" => "Moving the waiting agents".into(),
        "account-signin" => format!("Signing {} in", code("code")),
        "account-add" => format!("Adding {}", code("code")),
        "account-remove" => format!("Removing {}", code("code")),
        "account-rename" => format!("Renaming {}", code("code")),
        "account-test" => format!("Testing {}", code("code")),
        "account-current" => "Setting the current account".into(),
        "account-models" => "Saving the model list".into(),
        crate::app::resume::RESUME => "Resuming that conversation".into(),
        "account-start" => match code("to").as_str() {
            "list" => "Starting an agent on its owner's list".into(),
            to => format!("Starting an agent on {to}"),
        },
        _ => "Changing accounts".into(),
    };
    let error = error.replace(
        "its card shows how it stands",
        "the accounts screen (A) shows how it stands",
    );
    format!("{what}: {error}")
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

    /// The backend's answer as Kerdr reads it off a real command: through the transport and its
    /// reading of the answer, not handed over already parsed.
    #[cfg(unix)]
    fn through_transport(request: &Value, reply: &Value) -> Result<Response, String> {
        let here = Config::default().machines("here", &[]).remove(0);
        let command = format!(
            "cat >/dev/null; printf '%s' {}",
            crate::transport::quote(&reply.to_string())
        );
        crate::actions::run(&here, &command, request)
    }

    #[cfg(unix)]
    #[test]
    fn an_answer_that_moved_nothing_is_news_when_it_comes_through_the_real_transport() {
        let mut a = app();
        let request = crate::actions::with_op(
            json!({"action":"account-move","pane":"w1:p1","session":"web","conversation":"c","to":"ARK_C"}),
        );
        for nothing in [
            json!({"ok":true,"state":"stale","message":"that pane runs another agent now; nothing moved"}),
            json!({"ok":true,"state":"already","message":"ARK_C was already off the account list"}),
        ] {
            let result = through_transport(&request, &nothing);
            a.finish_action(0, request.clone(), result);
            assert_eq!(
                a.message().map(|(_, t)| t),
                Some(Tone::Info),
                "{nothing}: nothing moved, so not the success colour"
            );
        }
        let moved = json!({"ok":true,"state":"moved","account":"ARK_C","message":"moved to ARK_C"});
        a.finish_action(0, request.clone(), through_transport(&request, &moved));
        assert_eq!(a.message(), Some(("moved to ARK_C", Tone::Good)));
    }

    #[test]
    fn an_account_answer_is_coloured_by_what_happened_and_an_error_names_what_was_asked() {
        let mut a = app();
        let answer = |json: Value| Ok(crate::actions::parse_response(&json.to_string()).unwrap());
        let test = crate::actions::with_op(json!({"action":"account-test","code":"ARK_C"}));
        // Done: the success colour. Done already: news, not success (intent 0591).
        a.finish_action(
            0,
            test.clone(),
            answer(json!({"ok":true,"message":"ARK_C works."})),
        );
        assert_eq!(a.message(), Some(("ARK_C works.", Tone::Good)));
        a.finish_action(
            0,
            test.clone(),
            answer(json!({"ok":true,"status":"stale","message":"ARK_C was checked a moment ago."})),
        );
        assert_eq!(a.message().map(|(_, t)| t), Some(Tone::Info));
        // The same for a backend's real answers, which say `state` or say so in words.
        for nothing in [
            json!({"ok":true,"state":"stale","message":"that pane runs another agent now; nothing moved"}),
            json!({"ok":true,"state":"already","message":"ARK_C was already off the account list"}),
            json!({"ok":true,"state":"moved","account":"ARK_C","message":"already on ARK_C"}),
        ] {
            a.finish_action(0, test.clone(), answer(nothing));
            assert_eq!(a.message().map(|(_, t)| t), Some(Tone::Info));
        }
        a.finish_action(
            0,
            test.clone(),
            answer(json!({"ok":true,"state":"moved","account":"ARK_C","message":"moved to ARK_C"})),
        );
        assert_eq!(a.message().map(|(_, t)| t), Some(Tone::Good));
        // Done, with its push to come: said, as news.
        a.finish_action(
            0,
            json!({"action":"account-rename","code":"ARK_C","op":"k-0123456789abcdef"}),
            answer(json!({"ok":true,"message":"Renamed ARK_C.","note":"It reaches your other machines once the push goes through."})),
        );
        assert_eq!(
            a.message(),
            Some((
                "Renamed ARK_C. It reaches your other machines once the push goes through.",
                Tone::Info
            ))
        );
        // No answer: what was asked, and where to look, never a card.
        a.finish_action(
            0,
            test,
            Err(format!(
                "the connection to lab dropped before its answer came; {}, and its card shows how it stands.",
                crate::actions::UNKNOWN
            )),
        );
        let (said, tone) = a.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(
            said.starts_with("Testing ARK_C: the connection to lab dropped"),
            "{said}"
        );
        assert!(
            said.contains("the accounts screen (A) shows how it stands"),
            "{said}"
        );
        // Moving waiting agents: one machine's failure makes the summary a warning.
        a.account_batch_messages.clear();
        a.account_batch_failed = false;
        a.account_queue
            .push_back((1, json!({"action":"account-move-waiting","owner":"x"})));
        a.finish_action(
            0,
            json!({"action":"account-move-waiting","owner":"x"}),
            answer(json!({"ok":false,"message":"its lock is held"})),
        );
        a.account_queue.clear();
        a.finish_action(
            1,
            json!({"action":"account-move-waiting","owner":"x"}),
            answer(json!({"ok":true,"message":"moved 2"})),
        );
        let (said, tone) = a.message().unwrap();
        assert_eq!(tone, Tone::Warn, "{said}");
        assert!(
            said.contains("its lock is held") && said.contains("moved 2"),
            "{said}"
        );
    }
    #[test]
    fn a_page_code_is_taken_out_of_a_longer_paste_and_anything_else_is_sent_as_typed() {
        let code = "AbCdEfGhIjKlMnOp1234#StAtEsTrInG5678_-";
        assert_eq!(code_from_paste(code), code);
        assert_eq!(
            code_from_paste(&format!(
                "Paste this into Claude Code: {code} (expires soon)"
            )),
            code
        );
        assert_eq!(code_from_paste(&format!("`{code}`")), code);
        // Not that shape: as it was typed, the backend's to judge.
        assert_eq!(code_from_paste("short#one"), "short#one");
        assert_eq!(code_from_paste("123456"), "123456");
    }

    #[test]
    fn a_sign_in_that_ended_by_itself_starts_a_fresh_one_in_its_place_but_only_once() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        a.on_paste("page-code");
        let action = key(&mut a, KeyCode::Enter);
        let ended = json!({"ok":false,"status":"failed","again":true,"message":"that sign-in had ended (they last 15 minutes), so nothing was saved. r starts a new one."});
        complete(&mut a, action, ended.clone());
        assert_eq!(
            a.message().unwrap().0,
            "That sign-in had ended; here's a new link."
        );
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if !s.failed && s.ticket.is_none() && s.started_fresh)
        );
        // The fresh one goes out next, and it starts like any other.
        let again = a.account_tick();
        let (_, request) = sent(again.clone());
        assert_eq!(request["step"], "start");
        complete(&mut a, again, started());
        assert!(signin_screen(&a).ticket.is_some());
        // That one ends too: it stops, for r, rather than go round for ever.
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now();
        }
        let poll = a.account_tick();
        complete(&mut a, poll, ended);
        let s = signin_screen(&a);
        assert!(s.failed && s.message.contains("had ended"), "{}", s.message);
        assert!(a.account_queue.is_empty());
    }

    fn poll_now(a: &mut App) -> Action {
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.poll_at = Instant::now();
        }
        let poll = a.account_tick();
        assert_eq!(sent(poll.clone()).1["step"], "poll");
        poll
    }

    #[test]
    fn each_sign_in_that_asks_for_another_gets_one_with_its_link_shown_not_opened() {
        // The words the backend gives for a refused code, an unused link and a program that
        // stopped.
        for (said, words) in [
            (
                "that code was refused (it may have expired), so nothing was saved. r starts a new sign-in.",
                "That code was refused (it may have expired), so nothing was saved. Kerdr started a fresh sign-in.",
            ),
            (
                "that sign-in's link went unused for 15 minutes, so it ended. r starts a new one.",
                "That sign-in's link went unused for 15 minutes, so it ended. Kerdr started a fresh sign-in.",
            ),
            (
                "the sign-in program on lab stopped without signing in or saying why, so nothing was saved. r starts a new one.",
                "The sign-in program on lab stopped without signing in or saying why, so nothing was saved. Kerdr started a fresh sign-in.",
            ),
            (
                "That sign-in isn't on lab any more. r starts a new one there.",
                "That sign-in isn't on lab any more. Kerdr started a fresh sign-in.",
            ),
        ] {
            let mut a = app();
            let action = signin(&mut a);
            complete(&mut a, action, started());
            assert!(a.signin_open.take().is_some(), "the first link opens");
            let poll = poll_now(&mut a);
            complete(
                &mut a,
                poll,
                json!({"ok":false,"status":"failed","again":true,"message":said}),
            );
            assert_eq!(a.message().unwrap().0, words);
            let start = a.account_tick();
            assert_eq!(sent(start.clone()).1["step"], "start");
            complete(&mut a, start, started());
            let s = signin_screen(&a);
            assert!(!s.failed && s.url.is_some(), "{}", s.message);
            assert!(a.signin_open.is_none(), "shown in the box, not opened");
            // That one ends too: it stops for r, and r by hand opens its link.
            let poll = poll_now(&mut a);
            complete(
                &mut a,
                poll,
                json!({"ok":false,"status":"failed","again":true,"message":said}),
            );
            assert!(signin_screen(&a).went_wrong());
            let cancel = press(&mut a, 'r');
            complete(
                &mut a,
                cancel,
                json!({"ok":true,"status":"cancelled","message":"cancelled"}),
            );
            let start = a.account_tick();
            complete(&mut a, start, started());
            assert!(a.signin_open.is_some());
        }
    }

    #[test]
    fn a_sign_in_a_newer_one_replaced_is_not_drawn_as_a_failure() {
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"cancelled","replaced":true,"message":"a newer sign-in of KBZ_C on lab replaced this one"}),
        );
        let s = signin_screen(&a);
        assert!(s.failed && s.replaced && !s.went_wrong(), "r still works");
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("replaced by a newer sign-in"), "{screen}");
        assert!(screen.contains("r take it back"), "{screen}");
        assert!(!screen.contains("sign-in stopped"), "{screen}");
        // r takes it back like any other restart, and it's a plain sign-in again.
        let cancel = press(&mut a, 'r');
        assert!(!signin_screen(&a).replaced);
        assert_eq!(sent(cancel).1["step"], "cancel");

        // Cancelled with nothing newer: as it always was.
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        assert!(signin_screen(&a).went_wrong());
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("sign-in stopped"), "{screen}");
    }

    #[test]
    fn a_sign_in_still_starting_is_waited_for_and_a_poll_brings_its_link() {
        let mut a = app();
        let action = signin(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"starting","ticket":"login-ticket","message":"Asking the sign-in program for a link…"}),
        );
        let s = signin_screen(&a);
        assert!(!s.failed && s.url.is_none(), "{}", s.message);
        assert_eq!(s.ticket.as_deref(), Some("login-ticket"));
        assert!(a.signin_open.is_none());
        // Still starting: it's kept, and goes on being asked.
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"starting","message":"Still asking…"}),
        );
        assert!(!signin_screen(&a).failed);
        assert_eq!(signin_screen(&a).ticket.as_deref(), Some("login-ticket"));
        // The link comes with a poll, without a ticket of its own, and opens as a start's does.
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"waiting","message":"Authorize in your browser.","url":"https://example.com/authorize?state=example"}),
        );
        let s = signin_screen(&a);
        assert!(!s.failed && s.url.is_some(), "{}", s.message);
        assert_eq!(s.ticket.as_deref(), Some("login-ticket"));
        assert!(a.signin_open.is_some());
        // A link Kerdr won't open is refused from a poll too, and that sign-in cancelled.
        let mut a = app();
        let action = signin(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"starting","ticket":"login-ticket","message":"…"}),
        );
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"waiting","message":"…","url":"http://example.com/"}),
        );
        assert!(signin_screen(&a).failed);
        assert_eq!(a.account_queue.len(), 1);
        // With no ticket to ask about, there's nothing to wait for.
        let mut a = app();
        let action = signin(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":true,"status":"starting","message":"…"}),
        );
        assert!(signin_screen(&a).failed);
        // A still-going sign-in isn't news for the feed either.
        let going: crate::actions::Response =
            serde_json::from_value(json!({"ok":true,"status":"starting","message":"…"})).unwrap();
        assert!(going.signin_under_way());
        let done: crate::actions::Response =
            serde_json::from_value(json!({"ok":true,"status":"done","message":"…"})).unwrap();
        assert!(!done.signin_under_way());
    }

    #[test]
    fn a_test_that_finds_it_isnt_signed_in_opens_the_box_that_signs_it_in() {
        let not_signed_in = json!({"ok":false,"status":"failed","sign_in":true,
            "message":"ARK_C isn't signed in on lab (expired): Enter on it signs it in."});
        let mut a = app();
        open_copies(&mut a, "agents");
        let test = press(&mut a, 't');
        complete(&mut a, test, not_signed_in.clone());
        let Mode::Accounts {
            screen:
                AccountScreen::Where {
                    code,
                    copy,
                    message,
                    choices,
                    ..
                },
            ..
        } = &a.mode
        else {
            panic!("{:?}", a.mode)
        };
        assert_eq!(code, "ARK_C");
        assert_eq!(copy.as_ref().map(|c| c.0.as_str()), Some("agents"));
        assert!(message.contains("isn't signed in"), "{message}");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].machines, vec![1]);
        assert!(a.account_queue.is_empty(), "nothing starts until Enter");
        assert_eq!(a.message().unwrap().1, Tone::Warn);
        // Enter is what starts it.
        let start = key(&mut a, KeyCode::Enter);
        let (machine, request) = sent(start);
        assert_eq!(machine, 1);
        assert_eq!(request["step"], "start");
        assert_eq!(request["code"], "ARK_C");
        assert_eq!(request["copy"], "agents");

        // Any other failed test only says so, as it always has.
        let mut a = app();
        open_copies(&mut a, "agents");
        let test = press(&mut a, 't');
        complete(
            &mut a,
            test,
            json!({"ok":false,"status":"failed","message":"ARK_C didn't answer."}),
        );
        assert!(matches!(
            &a.mode,
            Mode::Accounts {
                screen: AccountScreen::List { .. },
                ..
            }
        ));

        // Nor does it open over whatever you've gone on to.
        let mut a = app();
        open_copies(&mut a, "agents");
        let test = press(&mut a, 't');
        key(&mut a, KeyCode::Esc);
        complete(&mut a, test, not_signed_in);
        assert!(!matches!(
            &a.mode,
            Mode::Accounts {
                screen: AccountScreen::Where { .. },
                ..
            }
        ));
    }

    #[test]
    fn accounts_an_agent_is_held_from_are_dim_in_the_picker_and_say_why() {
        let held = |a: &mut App, programs: Vec<String>| {
            a.machines[1]
                .feed
                .as_mut()
                .unwrap()
                .agents
                .iter_mut()
                .find(|a| a.session == "api" && a.pane == "w1:p1")
                .unwrap()
                .programs = programs;
            a.rebuild();
        };
        let on_codex = |a: &mut App| {
            pick(a);
            press(a, 's');
            let Mode::Accounts {
                screen: AccountScreen::Move { choices, sel, .. },
                ..
            } = &mut a.mode
            else {
                panic!("{:?}", a.mode)
            };
            *sel = choices.iter().position(|c| c == "KBZ_X").unwrap();
        };
        let mut a = app();
        held(&mut a, vec!["claude".into()]);
        on_codex(&mut a);
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(screen.contains("has to stay on Claude Code"), "{screen}");
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        let (said, tone) = a.message().unwrap();
        assert!(said.starts_with("#44 has to stay on Claude Code"), "{said}");
        assert!(said.contains("Pick a Claude Code account"), "{said}");
        assert_eq!(tone, Tone::Warn);
        assert!(matches!(
            &a.mode,
            Mode::Accounts {
                screen: AccountScreen::Move { .. },
                ..
            }
        ));
        // One of its own program's accounts goes as ever, with no reason beside it.
        let agent = a.account_agent(a.selected().unwrap()).unwrap().clone();
        assert_eq!(a.held_to(1, &agent, "ARK_C"), None);
        assert_eq!(a.held_to(1, &agent, "list"), None);

        // A feed that doesn't say: exactly as before.
        let mut a = app();
        on_codex(&mut a);
        let screen = crate::ui::tests_render(&a, 120, 40);
        assert!(!screen.contains("has to stay on"), "{screen}");
        let (_, request) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(request["to"], "KBZ_X");
        // And a null there is no restriction either.
        let agent: crate::accounts::AccountAgent =
            serde_json::from_value(json!({"pane":"w1:p1","programs":null})).unwrap();
        assert!(agent.programs.is_empty());
    }

    #[test]
    fn a_code_that_ran_out_with_its_page_unused_is_replaced_by_itself_once() {
        let mut a = app();
        let action = signin(&mut a);
        let mut reply = started();
        reply["user_code"] = json!("K7QD-3MPWX");
        reply["expires"] = json!("2000-01-01T00:00:00Z");
        complete(&mut a, action, reply);
        assert!(signin_screen(&a).user_code.is_some());
        let cancel = a.account_tick();
        let (_, request) = sent(cancel.clone());
        assert_eq!(request["step"], "cancel");
        assert_eq!(
            a.message().unwrap().0,
            "That code ran out; here's a new one."
        );
        complete(
            &mut a,
            cancel,
            json!({"ok":true,"status":"cancelled","message":"cancelled"}),
        );
        let start = a.account_tick();
        assert_eq!(sent(start.clone()).1["step"], "start");
        let mut reply = started();
        reply["user_code"] = json!("K7QD-3MPWY");
        reply["expires"] = json!("2000-01-01T00:00:00Z");
        complete(&mut a, start, reply);
        // Once only: this one, run out too, waits for r.
        assert_eq!(a.account_tick(), Action::None);
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
        assert!(
            a.message()
                .unwrap()
                .0
                .contains("runs a new conversation now, so nothing moved")
        );
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
        // Intent 0591, K2-13: the picker under the question now shows the new conversation, and
        // Enter there moves it.
        let said = a.message().unwrap().0.to_string();
        assert!(
            said.contains("runs a new conversation now, so nothing moved. ⏎ moves this one."),
            "{said}"
        );
        let Mode::Accounts {
            screen: AccountScreen::Move { target, .. },
            ..
        } = &a.mode
        else {
            panic!("the picker isn't open: {:?}", a.mode)
        };
        assert_eq!(target.agent.conversation, "replacement");
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["conversation"], "replacement");
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
        // Intent 0591, K2-13: shown in the picker, for Enter again.
        assert!(
            a.message()
                .unwrap()
                .0
                .contains("runs a new conversation now, shown here. ⏎ moves this one.")
        );
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["conversation"], "c-two");
    }

    #[test]
    fn a_moves_question_for_a_conversation_gone_from_its_pane_is_never_left_unanswerable() {
        // Intent 0591, K2-64: the question came for the conversation he picked, but its pane
        // runs another now: the picker shows that one for Enter again, rather than a warning
        // with no y or n.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .iter_mut()
            .find(|a| a.session == "api" && a.pane == "w1:p1")
            .unwrap()
            .conversation = "replacement".into();
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Mid-step","confirmation":"Mid-step","confirm":"ticket"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Move { target, .. },
            ..
        } = &a.mode
        else {
            panic!("the picker isn't open: {:?}", a.mode)
        };
        assert_eq!(target.agent.conversation, "replacement");
        assert!(
            a.message()
                .unwrap()
                .0
                .contains("shown here. ⏎ moves this one.")
        );
    }

    #[test]
    fn a_move_chosen_as_its_agent_changed_account_still_goes_where_picked() {
        // Intent 0591, K2-13: the same conversation on another account moves where he picked.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let Mode::Accounts {
            screen: AccountScreen::Move { choices, sel, .. },
            ..
        } = &mut a.mode
        else {
            panic!("{:?}", a.mode)
        };
        *sel = choices.iter().position(|c| c == "KBZ_X").unwrap();
        let agent = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .iter_mut()
            .find(|a| a.session == "api" && a.pane == "w1:p1")
            .unwrap();
        assert_ne!(agent.account, "ARK_C");
        agent.account = "ARK_C".into();
        let (_, r) = sent(key(&mut a, KeyCode::Enter));
        assert_eq!(r["action"], "account-move");
        assert_eq!(r["to"], "KBZ_X");
        assert!(a.message().unwrap().0.contains("is on ARK_C now"));
        // Gone meanwhile: nothing to move, and the picker closes.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .agents
            .retain(|a| !(a.session == "api" && a.pane == "w1:p1"));
        let key44 = a.sel.key.clone().unwrap();
        for c in a.board.cards.iter_mut().filter(|c| c.key == key44) {
            c.agents.clear();
        }
        assert_eq!(key(&mut a, KeyCode::Enter), Action::None);
        assert!(a.message().unwrap().0.contains("isn't running any more"));
        assert!(!matches!(a.mode, Mode::Accounts { .. }));
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
    fn a_moves_question_after_its_picker_closed_is_asked_where_it_cant_take_typing() {
        // Intent 0591, K2-27: on the board it's asked at once; while he types it waits until he's
        // back, and is asked then.
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
        let Mode::Accounts {
            screen: AccountScreen::Confirm { request, .. },
            back,
        } = &a.mode
        else {
            panic!("the question wasn't asked: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "ticket");
        assert!(matches!(**back, Mode::Board));
        // While typing a new intent: held, and asked once he's back.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'c');
        assert!(matches!(a.mode, Mode::Compose { .. }));
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Mid-step","confirmation":"Mid-step","confirm":"ticket"}),
        );
        assert!(matches!(a.mode, Mode::Compose { .. }));
        assert_eq!(a.held_questions().count(), 1);
        assert!(a.message().unwrap().0.contains("asks you something first"));
        assert_eq!(a.account_owed(), Action::None);
        assert!(matches!(a.mode, Mode::Compose { .. }), "asked over typing");
        key(&mut a, KeyCode::Esc);
        a.account_owed();
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Confirm { .. },
                ..
            }
        ));
    }
    #[test]
    fn a_late_moves_question_that_changes_after_its_yes_is_asked_again_at_once() {
        // Codex's review: its yes given over the board, with no picker under it, and its backend
        // asking again, changed: the new question is asked at once, for the same agent, never
        // "open the move picker again".
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
        settle(&mut a);
        let yes = press(&mut a, 'y');
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        let (_, r) = sent(yes.clone());
        assert_eq!(r["confirm"], "ticket");
        complete(
            &mut a,
            yes,
            json!({"ok":false,"message":"Now mid-job too","confirmation":"Now mid-job too","confirm":"ticket-2"}),
        );
        let Mode::Accounts {
            screen:
                AccountScreen::Confirm {
                    request,
                    message,
                    target,
                    ..
                },
            ..
        } = &a.mode
        else {
            panic!("not asked again: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "ticket-2");
        assert_eq!(message, "Now mid-job too");
        assert!(target.as_ref().is_some_and(|t| t.reference == "#44"));
        settle(&mut a);
        let (machine, r) = sent(press(&mut a, 'y'));
        assert_eq!(machine, 1);
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C","confirm":"ticket-2"})
        );
    }
    #[test]
    fn a_yes_to_a_changed_question_after_a_choice_sends_yes_not_that_choice() {
        // Codex's review: after-job picked, the question changed, then y: the yes alone goes,
        // with the new token, never the earlier choice.
        let mut a = app();
        let action = move_44(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"#44 is mid-job: move it now?","confirmation":"#44 is mid-job: move it now?","confirm":"t-1","choices":[{"id":"after-job","key":"j","label":"move it when the job ends"}]}),
        );
        settle(&mut a);
        let picked = press(&mut a, 'j');
        assert_eq!(sent(picked.clone()).1["choice"], "after-job");
        complete(
            &mut a,
            picked,
            json!({"ok":false,"message":"Since you were asked, it stopped saving. Move it now?","confirmation":true,"confirm":"t-2"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm { request, .. },
            ..
        } = &a.mode
        else {
            panic!("not asked again: {:?} {:?}", a.mode, a.message())
        };
        assert!(request.get("choice").is_none(), "{request}");
        settle(&mut a);
        let (_, r) = sent(press(&mut a, 'y'));
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C","confirm":"t-2"})
        );
    }
    #[test]
    fn a_changed_question_about_the_start_made_instead_is_asked_again_over_the_board() {
        // Codex's review: the agent wasn't running, so the move's question is about a start; its
        // yes given over the board, and the question changed: asked again at once, never "open
        // the move picker again".
        let mut a = app();
        let (_, id) = card_44(&a);
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Its agent isn't running. Start #44 on ARK_C?","confirmation":true,"confirm":"s-1","id":id}),
        );
        settle(&mut a);
        let yes = press(&mut a, 'y');
        assert_eq!(sent(yes.clone()).1["confirm"], "s-1");
        assert!(matches!(a.mode, Mode::Board));
        complete(
            &mut a,
            yes,
            json!({"ok":false,"message":"Since you were asked, api is full. Start #44 on ARK_C anyway?","confirmation":true,"confirm":"s-2","id":id}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm { request, .. },
            ..
        } = &a.mode
        else {
            panic!("not asked again: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "s-2");
        settle(&mut a);
        assert_eq!(sent(press(&mut a, 'y')).1["confirm"], "s-2");
    }
    #[test]
    fn a_move_that_goes_again_after_a_sign_in_over_the_board_asks_its_question_at_once() {
        // Codex's review: the picker closed, the sign-in offered over the board; once it's done
        // the move goes again, and the question it brings is asked for its agent at once.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        complete(&mut a, action, needs_signin());
        where_screen(&a);
        let start = key(&mut a, KeyCode::Enter);
        complete(&mut a, start, started());
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"ARK_C is signed in on build-box."}),
        );
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        let again = a.account_tick();
        assert_eq!(sent(again.clone()).1["action"], "account-move");
        complete(
            &mut a,
            again,
            json!({"ok":false,"message":"#44 is mid-step. Move it when it next stops?","confirmation":true,"confirm":"m-1"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                request, target, ..
            },
            ..
        } = &a.mode
        else {
            panic!("not asked: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "m-1");
        assert!(target.as_ref().is_some_and(|t| t.reference == "#44"));
    }
    #[test]
    fn a_saved_lists_question_still_moves_waiting_sessions_after_yes_or_no() {
        // Codex's review: "move waiting sessions" ticked, and the save asks a question of its
        // own: they move once it's settled, whichever the answer.
        for answer in ['y', 'n'] {
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
            complete(
                &mut a,
                action,
                json!({"ok":false,"message":"2 agents here are left off it. Save anyway?","confirmation":true,"confirm":"m-1"}),
            );
            assert!(
                a.account_queue.is_empty(),
                "{answer}: moved before it was saved"
            );
            settle(&mut a);
            let sent_answer = press(&mut a, answer);
            if answer == 'y' {
                assert_eq!(
                    a.account_tick(),
                    Action::None,
                    "moved before its yes was answered"
                );
                complete(&mut a, sent_answer, json!({"ok":true,"message":"Saved."}));
            } else {
                assert_eq!(sent_answer, Action::None);
            }
            let (machine, r) = sent(a.account_tick());
            assert_eq!(machine, 0, "{answer}");
            assert_eq!(
                r,
                json!({"action":"account-move-waiting","owner":"studio"}),
                "{answer}"
            );
        }
    }
    #[test]
    fn an_account_question_longer_than_the_screen_scrolls_and_y_shows_the_rest_first() {
        // Codex's review, as for the board's question box: on 80×24 it scrolls, and y never
        // answers what couldn't be seen.
        let mut a = app();
        let words: String = (1..=60)
            .map(|i| format!("Line {i} of what it says. "))
            .collect();
        a.push_account_screen(AccountScreen::Confirm {
            machine: 0,
            request: json!({"action":"account-models","owner":"studio","models":["KBZ_C"],"confirm":"t-9"}),
            message: format!("{words}THE-END."),
            yes: "move them".into(),
            no: "leave them".into(),
            target: None,
            ready_at: Instant::now(),
            choices: vec![crate::actions::Choice {
                id: "later".into(),
                key: "l".into(),
                label: "move them later".into(),
            }],
        });
        settle(&mut a);
        let shown = crate::ui::tests_render(&a, 80, 24);
        assert!(!shown.contains("THE-END."), "{shown}");
        assert!(shown.contains("pgdn pgup the rest of it"), "{shown}");
        key(&mut a, KeyCode::PageDown);
        assert!(!crate::ui::tests_render(&a, 80, 24).contains("Line 1 of"));
        // y shows the rest, and sends nothing.
        assert_eq!(press(&mut a, 'y'), Action::None);
        let shown = crate::ui::tests_render(&a, 80, 24);
        assert!(shown.contains("THE-END."), "{shown}");
        assert!(shown.contains("y  move them"), "{shown}");
        settle(&mut a);
        let (machine, r) = sent(press(&mut a, 'y'));
        assert_eq!(machine, 0);
        assert_eq!(r["confirm"], "t-9");
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

    /// Intent 0591, B3-42: a saved list's agents left on other accounts.
    fn save_list_leaving(a: &mut App, off_list: Value) {
        let owner = a.account_feed(0).unwrap().owners[0].clone();
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: vec!["KBZ_C".into()],
            owner,
            sel: 0,
            move_waiting: false,
        });
        let action = key(a, KeyCode::Enter);
        complete(
            a,
            action,
            json!({"ok":true,"message":"studio: KBZ_C. 3 running agents here are still on ARK_C; s on their cards moves them","off_list":off_list}),
        );
    }
    fn off_agent(pane: &str, conversation: &str, account: &str, intent: Option<&str>) -> Value {
        let mut agent =
            json!({"pane":pane,"session":"web","conversation":conversation,"account":account});
        if let Some(n) = intent {
            agent["intent"] = json!(n);
        }
        agent
    }
    fn to_list(pane: &str, conversation: &str) -> Value {
        json!({"action":"account-move","pane":pane,"session":"web","conversation":conversation,"to":"list"})
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
    #[test]
    fn a_saved_list_that_leaves_agents_elsewhere_asks_once_and_moves_each_to_the_list_in_turn() {
        let mut a = app();
        save_list_leaving(
            &mut a,
            json!([
                off_agent("w91:p1", "c-1", "ARK_C", Some("0041")),
                off_agent("w92:p1", "c-2", "ARK_C", Some("0052")),
                {"session":"web","conversation":"no-pane"},
                off_agent("w93:p2", "c-3", "KBZ_X", None),
            ]),
        );
        // Asked once, naming each, nothing sent yet.
        let (_, message) = asked(&a);
        assert_eq!(
            message,
            "Example Studio's list is saved. On laptop, #41, #52 and the agent on KBZ_X in web are still running on ARK_C and KBZ_X, which aren't on it. Move them to the list now?"
        );
        assert!(a.account_queue.is_empty());
        settle(&mut a);
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(matches!(a.mode, Mode::Board));
        // The first move, alone.
        let first = a.account_tick();
        assert_eq!(sent(first.clone()).1, to_list("w91:p1", "c-1"));
        assert!(
            a.account_queue.is_empty(),
            "sent before the first was answered"
        );
        complete(
            &mut a,
            first,
            json!({"ok":true,"message":"#41 moved to KBZ_C."}),
        );
        // The second asks: its question comes, named, and the third waits for its answer.
        let second = a.account_tick();
        assert_eq!(sent(second.clone()).1, to_list("w92:p1", "c-2"));
        complete(
            &mut a,
            second,
            json!({"ok":false,"message":"It has a background job running. Move it anyway?","confirmation":true,"confirm":"t-2","yes":"move it anyway","no":"leave it","choices":[{"id":"after-job","key":"j","label":"move it when the job ends"}]}),
        );
        let (request, message) = asked(&a);
        assert_eq!(request["confirm"], "t-2");
        assert_eq!(
            message,
            "#52: It has a background job running. Move it anyway?"
        );
        assert_eq!(
            a.account_tick(),
            Action::None,
            "the next went before this was answered"
        );
        settle(&mut a);
        let (machine, r) = sent(press(&mut a, 'j'));
        assert_eq!(machine, 0);
        let mut expected = to_list("w92:p1", "c-2");
        expected["confirm"] = json!("t-2");
        expected["choice"] = json!("after-job");
        assert_eq!(r, expected);
        complete(
            &mut a,
            Action::Mutate {
                machine,
                request: r,
            },
            json!({"ok":true,"state":"queued","message":"#52 moves to KBZ_C once its job ends."}),
        );
        // The third is refused: said with the rest once it's the last.
        let third = a.account_tick();
        assert_eq!(sent(third.clone()).1, to_list("w93:p2", "c-3"));
        complete(
            &mut a,
            third,
            json!({"ok":false,"message":"Its agent isn't running now, so nothing moved."}),
        );
        assert!(a.off_list_moves.is_none());
        assert_eq!(a.account_tick(), Action::None);
        assert_eq!(
            a.message().unwrap(),
            (
                "2 of 3 moved to the list: #41 now, and #52 as soon as it can. The agent on KBZ_X in web didn't move: its agent isn't running now, so nothing moved.",
                Tone::Warn
            )
        );
    }
    #[test]
    fn n_on_one_move_to_the_list_goes_on_to_the_next_and_esc_stops_the_rest() {
        let mut a = app();
        save_list_leaving(
            &mut a,
            json!([
                off_agent("w91:p1", "c-1", "ARK_C", Some("0041")),
                off_agent("w92:p1", "c-2", "ARK_C", Some("0052")),
                off_agent("w93:p1", "c-3", "ARK_C", Some("0060")),
            ]),
        );
        settle(&mut a);
        press(&mut a, 'y');
        let question = json!({"ok":false,"message":"#41 is mid-step. Move it when it next stops?","confirmation":true,"confirm":"t-1"});
        let first = a.account_tick();
        complete(&mut a, first, question);
        assert_eq!(asked(&a).1, "#41 is mid-step. Move it when it next stops?");
        settle(&mut a);
        // n: this one stays, and the next goes.
        assert_eq!(press(&mut a, 'n'), Action::None);
        let second = a.account_tick();
        assert_eq!(sent(second.clone()).1, to_list("w92:p1", "c-2"));
        complete(
            &mut a,
            second,
            json!({"ok":true,"message":"#52 moved to KBZ_C."}),
        );
        let third = a.account_tick();
        complete(
            &mut a,
            third,
            json!({"ok":false,"message":"#60 is mid-step. Move it when it next stops?","confirmation":true,"confirm":"t-3"}),
        );
        // Esc: nothing more is sent, and what came of them is said.
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None);
        assert!(a.off_list_moves.is_none());
        assert_eq!(
            a.message().unwrap(),
            (
                "1 of 3 moved to the list: #52. #41 stayed where it was, as you said. You stopped there, so #60 wasn't moved.",
                Tone::Info
            )
        );
    }
    #[test]
    fn a_move_to_the_list_waits_on_its_sign_in_and_leaving_that_stops_the_rest() {
        let mut a = app();
        save_list_leaving(
            &mut a,
            json!([
                off_agent("w91:p1", "c-1", "ARK_C", Some("0041")),
                off_agent("w92:p1", "c-2", "ARK_C", Some("0052")),
            ]),
        );
        settle(&mut a);
        press(&mut a, 'y');
        let first = a.account_tick();
        complete(
            &mut a,
            first,
            json!({"ok":false,"sign_in":true,"code":"KBZ_C","message":"A, then Enter on KBZ_C signs it in, and s moves it then; nothing moved"}),
        );
        // The box that signs it in, and the next move waits for it.
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Where { .. },
                ..
            }
        ));
        assert_eq!(a.account_tick(), Action::None);
        assert!(a.off_list_moves.is_some());
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.account_tick(), Action::None);
        assert!(a.off_list_moves.is_none());
        assert_eq!(
            a.message().unwrap().0,
            "0 of 2 moved to the list. You stopped there, so #41 and #52 weren't moved."
        );
    }
    #[test]
    fn a_saved_list_asks_nothing_more_without_agents_off_it_and_a_held_question_waits_its_half_second()
     {
        // An older backend, or none left elsewhere: as before.
        for off_list in [
            json!(null),
            json!([]),
            json!([{"session":"web"}]),
            json!("x"),
        ] {
            let mut a = app();
            save_list_leaving(&mut a, off_list.clone());
            assert!(matches!(a.mode, Mode::Board), "{off_list}: {:?}", a.mode);
            assert!(a.account_queue.is_empty());
            assert!(a.message().unwrap().0.starts_with("studio: KBZ_C."));
        }
        // Answered while he types: held, and asked once he's back, never answered by a key
        // pressed as it opens.
        let mut a = app();
        let owner = a.account_feed(0).unwrap().owners[0].clone();
        a.push_account_screen(AccountScreen::Models {
            machine: 0,
            models: vec!["KBZ_C".into()],
            owner,
            sel: 0,
            move_waiting: false,
        });
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'c');
        assert!(matches!(a.mode, Mode::Compose { .. }));
        complete(
            &mut a,
            action,
            json!({"ok":true,"message":"saved","off_list":[off_agent("w91:p1","c-1","ARK_C",Some("0041"))]}),
        );
        assert!(matches!(a.mode, Mode::Compose { .. }));
        assert_eq!(a.held_questions().count(), 1);
        key(&mut a, KeyCode::Esc);
        // Its half second ran out while it was held.
        if let Some(AccountScreen::Confirm { ready_at, .. }) =
            a.asked_about.iter_mut().find_map(|k| k.held.as_deref_mut())
        {
            *ready_at = Instant::now() - Duration::from_secs(1);
        }
        a.account_owed();
        assert!(asked(&a).1.ends_with("Move it to the list now?"));
        assert_eq!(press(&mut a, 'y'), Action::None);
        assert!(
            a.off_list_moves.is_none(),
            "answered by a key pressed as it opened"
        );
        // A move to the list that failed is named in words.
        assert!(
            account_error(
                &json!({"action":"account-move","to":"list"}),
                "no answer came"
            )
            .starts_with("Moving the agent to its owner's list")
        );
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
    fn removing_an_account_says_what_it_leaves_behind() {
        // Intent 0591, K2-12: the lists it comes off, that it was current, and where else a
        // sign-in of it stays saved.
        let mut a = app();
        let (code, name) = {
            let acct = &a.account_feed(0).unwrap().accounts[0];
            (acct.code.clone(), acct.name.clone())
        };
        let owner = {
            let f = a.machines[0].feed.as_mut().unwrap();
            for o in f.owners.iter_mut() {
                o.models.retain(|m| m != &code);
            }
            f.owners[0].own = true;
            f.owners[0].models.push(code.clone());
            f.current = Some(Some(crate::accounts::Current {
                account: code.clone(),
                ..Default::default()
            }));
            f.owners[0].name.clone()
        };
        let other = a.machines[1].name();
        {
            let f = a.machines[1].feed.as_mut().unwrap();
            f.accounts.retain(|x| x.code != code);
            f.accounts.push(crate::accounts::Account {
                code: code.clone(),
                copies: vec![crate::accounts::AccountCopy {
                    stored: true,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        for m in 2..a.machines.len() {
            if let Some(f) = a.machines[m].feed.as_mut() {
                f.accounts.retain(|x| x.code != code);
            }
        }
        assert_eq!(
            a.remove_question(0, &code, &name),
            format!(
                "Remove {code} ({name})? It comes off {owner}'s model list. It's the current account, so each owner's list chooses again. Only this machine's saved sign-in goes: it stays signed in on {other}."
            )
        );
        // With none of those, only the question.
        {
            let f = a.machines[0].feed.as_mut().unwrap();
            f.owners[0].models.retain(|m| m != &code);
            f.current = None;
        }
        a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .retain(|x| x.code != code);
        assert_eq!(
            a.remove_question(0, &code, &name),
            format!("Remove {code} ({name})?")
        );
    }

    #[test]
    fn following_the_organization_shows_the_list_it_drops() {
        // Intent 0591, K2-14: the tenant's own list is in the question, so it could be typed back.
        let a = app();
        let feed = a.account_feed(0).unwrap();
        let mut owner = feed
            .owners
            .iter()
            .find(|o| o.parent.is_some())
            .expect("a tenant in the demo")
            .clone();
        owner.own = true;
        owner.models = vec!["ARK_C".into(), "KBZ_C".into()];
        let org = feed
            .owners
            .iter()
            .find(|o| Some(&o.id) == owner.parent.as_ref())
            .unwrap();
        let said = a.follow_question(0, &owner);
        assert!(
            said.starts_with(&format!(
                "Make {} follow its organization's model list? Its own list goes: ARK_C → KBZ_C.",
                owner.name
            )),
            "{said}"
        );
        if !org.models.is_empty() {
            assert!(
                said.ends_with(&format!(
                    " It would follow {}'s: {}.",
                    org.name,
                    org.models.join(" → ")
                )),
                "{said}"
            );
        }
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
    fn identity_mismatch_does_not_advance_signin_and_r_goes_on_even_if_its_cancel_is_refused() {
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
        // A start replaces whatever is still running, so r goes on anyway (intent 0591).
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if !s.failed && s.ticket.is_none())
        );
        let (_, again) = a.account_queue.pop_front().unwrap();
        assert_eq!(again["step"], "start");
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
    fn r_goes_on_to_start_even_when_its_cancel_got_no_answer() {
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
        let (mi, cancel) = sent(press(&mut a, 'r'));
        assert_eq!(cancel["step"], "cancel");
        // The connection dropped before the cancel's answer came (intent 0591).
        a.finish_action(
            mi,
            crate::actions::with_op(cancel),
            Err(format!(
                "the connection to arknet-one dropped before its answer came; {}, and its card shows how it stands.",
                crate::actions::UNKNOWN
            )),
        );
        assert!(
            matches!(&a.mode, Mode::Accounts { screen: AccountScreen::Signin(s), .. } if !s.failed && !s.restart && s.ticket.is_none()),
            "{:?}",
            a.mode
        );
        let (to, again) = a.account_queue.pop_front().expect("a fresh start");
        assert_eq!((to, again["step"].as_str()), (mi, Some("start")));
        // A cancel nobody restarts after says what went wrong, as before.
        let mut a = app();
        let action = signin(&mut a);
        complete(&mut a, action, started());
        key(&mut a, KeyCode::Esc);
        let (mi, cancel) = sent(a.account_tick());
        assert_eq!(cancel["step"], "cancel");
        a.finish_action(mi, crate::actions::with_op(cancel), Err("no answer".into()));
        assert!(a.account_queue.is_empty());
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
        assert!(s.message.contains("can't show safely"), "{}", s.message);
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
        assert_eq!(
            a.message().unwrap().0,
            "Local needs address: type it in the address row."
        );
        // The cursor is on that row already (intent 0591).
        assert_eq!(add_screen(&a).field, AddAccount::FIELDS_AT);
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

    /// Intent 0591: the other answers a move's backend offers are shown beside yes and no, and
    /// the one picked is sent as the move again with its token and that choice, never an override.
    #[test]
    fn an_account_question_offers_its_backends_other_answers_and_sends_the_one_picked() {
        let mut a = app();
        let action = move_44(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"#44 is mid-job: move it now?","confirmation":"#44 is mid-job: move it now?","confirm":"t-1","yes":"move now","no":"leave it","choices":[{"id":"after-job","key":"j","label":"move it when the job ends"}]}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm { choices, .. },
            ..
        } = &a.mode
        else {
            panic!("{:?}", a.mode)
        };
        assert_eq!(choices.len(), 1);
        let shown = crate::ui::tests_render(&a, 120, 40);
        assert!(shown.contains("j  move it when the job ends"), "{shown}");
        assert!(shown.contains("y  move now"), "{shown}");
        assert!(shown.contains("n  leave it"), "{shown}");
        // Not before the question has been up long enough to be read, as with y.
        assert_eq!(press(&mut a, 'j'), Action::None);
        settle(&mut a);
        let (machine, r) = sent(press(&mut a, 'j'));
        assert_eq!(machine, 1);
        assert_eq!(
            r,
            json!({"action":"account-move","pane":"w1:p1","session":"api","conversation":"demo-api-w1:p1","to":"ARK_C","confirm":"t-1","choice":"after-job"})
        );
        // A key the question doesn't offer does nothing.
        let action = move_44(&mut a);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"move?","confirmation":"move?","confirm":"t-2"}),
        );
        settle(&mut a);
        assert_eq!(press(&mut a, 'j'), Action::None);
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Confirm { .. },
                ..
            }
        ));
    }

    /// Intent 0591, B3-42 and B3-46: another account action's question is asked in the same box,
    /// its yes the same request with its token; a saved list is taken as saved first, and a
    /// question that comes while he types waits until he's back.
    #[test]
    fn another_account_actions_question_is_asked_and_its_yes_is_the_same_request_with_its_token() {
        let question = json!({"ok":false,"confirmation":"Saved. Two agents run on accounts off the list: move them now?","confirm":"t-1","yes":"move them","no":"leave them","message":"Saved. Two agents run on accounts off the list: move them now?"});
        let models = |a: &mut App| {
            let owner = a.account_feed(0).unwrap().owners[0].clone();
            a.push_account_screen(AccountScreen::Models {
                machine: 0,
                models: vec!["KBZ_C".into()],
                owner,
                sel: 0,
                move_waiting: false,
            });
            key(a, KeyCode::Enter)
        };
        let mut a = app();
        let action = models(&mut a);
        let (_, asked) = sent(action.clone());
        complete(&mut a, action, question.clone());
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                message, yes, no, ..
            },
            back,
        } = &a.mode
        else {
            panic!("not asked: {:?}", a.mode)
        };
        assert!(message.ends_with("move them now?"), "{message}");
        assert_eq!((yes.as_str(), no.as_str()), ("move them", "leave them"));
        assert!(
            matches!(**back, Mode::Board),
            "the list's screen is done with"
        );
        let owner = &a.account_feed(0).unwrap().owners[0];
        assert_eq!(owner.models, vec!["KBZ_C".to_string()], "saved either way");
        settle(&mut a);
        let (machine, r) = sent(press(&mut a, 'y'));
        assert_eq!(machine, 0);
        let mut yes = asked.clone();
        yes["confirm"] = json!("t-1");
        assert_eq!(r, yes);
        // n leaves it as it is.
        let mut a = app();
        let action = models(&mut a);
        complete(&mut a, action, question.clone());
        assert_eq!(press(&mut a, 'n'), Action::None);
        assert!(matches!(a.mode, Mode::Board));
        // While he types, it waits, and is asked once he's back.
        let mut a = app();
        let action = models(&mut a);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'c');
        assert!(matches!(a.mode, Mode::Compose { .. }));
        complete(&mut a, action, question);
        assert!(matches!(a.mode, Mode::Compose { .. }));
        assert_eq!(a.held_questions().count(), 1);
        key(&mut a, KeyCode::Esc);
        a.account_owed();
        assert!(matches!(
            a.mode,
            Mode::Accounts {
                screen: AccountScreen::Confirm { .. },
                ..
            }
        ));
    }

    /// The answer a move gets when its account isn't signed in on that machine (intent 0591,
    /// B3-10).
    fn needs_signin() -> Value {
        json!({"ok":false,"sign_in":true,"code":"ARK_C","message":"ARK_C isn't signed in on build-box: sign it in, and the move goes on once you have."})
    }

    /// Intent 0591, B3-10: a move to an account not signed in on its machine opens the box that
    /// signs it in there; once that's done, the move goes again by itself, from its picker.
    #[test]
    fn a_move_to_an_account_not_signed_in_there_signs_it_in_then_moves() {
        let mut a = app();
        let action = move_44(&mut a);
        let (_, asked) = sent(action.clone());
        complete(&mut a, action, needs_signin());
        let (message, choices) = where_screen(&a);
        assert!(message.contains("isn't signed in"), "{message}");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].machines, vec![1]);
        let start = key(&mut a, KeyCode::Enter);
        let (machine, r) = sent(start.clone());
        assert_eq!((machine, r["step"].as_str()), (1, Some("start")));
        assert_eq!(r["code"], "ARK_C");
        complete(&mut a, start, started());
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":true,"status":"done","message":"ARK_C is signed in on build-box."}),
        );
        assert!(a.after_signin.is_none());
        assert!(
            matches!(
                a.mode,
                Mode::Accounts {
                    screen: AccountScreen::Move { .. },
                    ..
                }
            ),
            "back on the picker it was asked in: {:?}",
            a.mode
        );
        assert!(a.message().unwrap().0.contains("the move goes on"));
        let (machine, again) = sent(a.account_tick());
        assert_eq!(machine, 1);
        assert_eq!(again, asked, "the same move, as a new request");
        complete(
            &mut a,
            Action::Mutate {
                machine,
                request: again,
            },
            json!({"ok":true,"message":"Moved #44's agent to ARK_C."}),
        );
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
    }

    /// Intent 0591, B3-10: a sign-in that fails keeps the move for r; leaving the box drops it
    /// and says so; and one that came while he was elsewhere opens nothing over his typing.
    #[test]
    fn a_move_waiting_on_a_sign_in_is_dropped_once_he_leaves_it() {
        let mut a = app();
        let action = move_44(&mut a);
        complete(&mut a, action, needs_signin());
        let start = key(&mut a, KeyCode::Enter);
        complete(&mut a, start, started());
        let poll = poll_now(&mut a);
        complete(
            &mut a,
            poll,
            json!({"ok":false,"status":"failed","message":"The browser sign-in was refused."}),
        );
        a.account_tick();
        assert!(a.after_signin.is_some(), "kept while r can try again");
        key(&mut a, KeyCode::Esc);
        a.account_tick();
        assert!(a.after_signin.is_none());
        assert_eq!(
            a.message().unwrap().0,
            format!(
                "Nothing moved: ARK_C isn't signed in on {}.",
                a.machines[1].name()
            )
        );
        assert!(a.account_queue.is_empty());
        // Esc on the box asking where to sign it in drops it the same way.
        let mut a = app();
        let action = move_44(&mut a);
        complete(&mut a, action, needs_signin());
        key(&mut a, KeyCode::Esc);
        a.account_tick();
        assert!(a.after_signin.is_none());
        assert!(a.message().unwrap().0.starts_with("Nothing moved"));
        // Typing: only the backend's words, nothing opened over it and nothing kept.
        let mut a = app();
        let action = move_44(&mut a);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'c');
        complete(&mut a, action, needs_signin());
        assert!(matches!(a.mode, Mode::Compose { .. }));
        assert!(a.after_signin.is_none());
        assert!(a.message().unwrap().0.contains("isn't signed in"));
    }

    /// Intent 0591, B3-40: the current account set where it isn't signed in opens the box that
    /// signs it in there, and nothing is sent again after.
    #[test]
    fn a_current_account_not_signed_in_here_opens_the_box_that_signs_it_in() {
        let mut a = app();
        knows_current(&mut a);
        a.open_accounts();
        let action = press(&mut a, 'c');
        complete(
            &mut a,
            action,
            json!({"ok":true,"sign_in":true,"code":"KBZ_C","message":"KBZ_C is the current account now, but it isn't signed in here.","current":{"account":"KBZ_C","status":"sign_in"}}),
        );
        assert_eq!(
            a.machines[0]
                .feed
                .as_ref()
                .unwrap()
                .current_account()
                .map(|c| c.account.as_str()),
            Some("KBZ_C")
        );
        let (_, choices) = where_screen(&a);
        assert_eq!(choices[0].machines, vec![0]);
        assert!(a.after_signin.is_none());
    }

    // Codex's third look at piece K: the account side's questions, each kept as its own record.

    fn question(token: &str, words: &str) -> Response {
        crate::actions::parse_response(
            &json!({"ok":false,"confirmation":true,"confirm":token,"message":words}).to_string(),
        )
        .unwrap()
    }

    #[test]
    fn two_account_questions_that_come_while_he_types_are_each_asked_in_turn() {
        // A second question held while he types never replaces the first, nor is dropped.
        let mut a = app();
        press(&mut a, 'c');
        assert!(matches!(a.mode, Mode::Compose { .. }));
        let save = json!({"action":"account-models","owner":"studio","models":["KBZ_C"]});
        let remove = json!({"action":"account-remove","code":"ARK_C"});
        a.finish_action(
            0,
            crate::actions::with_op(save.clone()),
            Ok(question(
                "q-1",
                "2 agents here are left off it. Save anyway?",
            )),
        );
        a.finish_action(
            0,
            crate::actions::with_op(remove.clone()),
            Ok(question(
                "q-2",
                "That's this machine's own sign-in. Remove it?",
            )),
        );
        assert!(matches!(a.mode, Mode::Compose { .. }), "asked over typing");
        key(&mut a, KeyCode::Esc);
        // The first, then once that's answered, the second.
        a.account_owed();
        let (request, message) = asked(&a);
        assert_eq!(message, "2 agents here are left off it. Save anyway?");
        assert_eq!(request["confirm"], "q-1");
        settle(&mut a);
        assert_eq!(press(&mut a, 'n'), Action::None);
        assert!(matches!(a.mode, Mode::Board), "{:?}", a.mode);
        a.account_owed();
        let (request, message) = asked(&a);
        assert_eq!(message, "That's this machine's own sign-in. Remove it?");
        assert_eq!(request["confirm"], "q-2");
        settle(&mut a);
        let (_, yes) = sent(press(&mut a, 'y'));
        let mut expected = remove;
        expected["confirm"] = json!("q-2");
        assert_eq!(yes, expected);
    }

    #[test]
    fn another_requests_answer_coming_first_leaves_a_move_what_it_was_asked_from() {
        // A reply to something else, before the move's own: the move's picker is still found
        // for its question, never "open the move picker again".
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        a.busy = true;
        key(&mut a, KeyCode::Esc);
        a.finish_action(
            0,
            crate::actions::with_op(json!({"action":"account-test","code":"KBZ_C"})),
            Ok(Response {
                ok: true,
                message: "KBZ_C works here.".into(),
                ..Default::default()
            }),
        );
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Mid-step","confirmation":"Mid-step","confirm":"ticket"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                request, target, ..
            },
            ..
        } = &a.mode
        else {
            panic!("not asked: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "ticket");
        assert!(target.as_ref().is_some_and(|t| t.reference == "#44"));
    }

    #[test]
    fn another_requests_answer_leaves_the_one_on_its_way_and_what_comes_next_waits_its_turn() {
        // Codex's fourth look: a reply to something else, then a send, before the move's own
        // reply. The move stays on its way (busy, "trying again", what it was asked from), what
        // comes next waits its turn, and the move's question is still about its agent.
        let mut a = app();
        pick(&mut a);
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        // As start_action sends it.
        a.busy = true;
        a.account_retrying = a.account_card_in_flight();
        assert!(a.account_retrying.is_some());
        key(&mut a, KeyCode::Esc);
        a.finish_action(
            0,
            crate::actions::with_op(json!({"action":"account-test","code":"KBZ_C"})),
            Ok(Response {
                ok: true,
                message: "KBZ_C works here.".into(),
                ..Default::default()
            }),
        );
        assert!(a.busy, "the move is still on its way");
        assert!(
            a.account_retrying.is_some(),
            "and still said to be tried again"
        );
        // What comes next waits: a follow-up the tick would send, and a request made now.
        a.account_queue
            .push_back((0, json!({"action":"account-current","to":"KBZ_C"})));
        assert_eq!(a.account_tick(), Action::None);
        assert_eq!(
            a.send_account(0, json!({"action":"account-test","code":"ARK_C"})),
            Action::None
        );
        assert_eq!(a.account_later.len(), 1);
        complete(
            &mut a,
            action,
            json!({"ok":false,"message":"Mid-step","confirmation":"Mid-step","confirm":"ticket"}),
        );
        let Mode::Accounts {
            screen: AccountScreen::Confirm {
                request, target, ..
            },
            ..
        } = &a.mode
        else {
            panic!("not asked: {:?} {:?}", a.mode, a.message())
        };
        assert_eq!(request["confirm"], "ticket");
        assert!(target.as_ref().is_some_and(|t| t.reference == "#44"));
        assert!(!a.busy && a.account_retrying.is_none());
        // Then each goes in its turn, the one made while it waited with what it was asked from.
        let (_, next) = sent(a.account_tick());
        assert_eq!(next["action"], "account-current");
        a.finish_action(
            0,
            crate::actions::with_op(next),
            Ok(Response {
                ok: true,
                message: "KBZ_C is current.".into(),
                ..Default::default()
            }),
        );
        let (_, last) = sent(a.account_tick());
        assert_eq!(last, json!({"action":"account-test","code":"ARK_C"}));
        assert!(a.account_later.is_empty());
    }

    #[test]
    fn a_ticked_save_moves_waiting_sessions_even_when_he_left_before_its_answer() {
        // "Move waiting sessions" ticked, Esc before the save's ok: they still move, with or
        // without agents it left elsewhere.
        for off_list in [
            json!(null),
            json!([off_agent("w91:p1", "c-1", "ARK_C", Some("0041"))]),
        ] {
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
            a.busy = true;
            key(&mut a, KeyCode::Esc);
            assert!(a.account_queue.is_empty());
            complete(
                &mut a,
                action,
                json!({"ok":true,"message":"saved","off_list":off_list}),
            );
            assert_eq!(
                a.account_queue.front(),
                Some(&(0, json!({"action":"account-move-waiting","owner":"studio"}))),
                "{off_list}"
            );
        }
    }

    #[test]
    fn a_move_to_the_list_whose_answer_was_lost_is_said_as_not_known_never_as_not_moved() {
        let mut a = app();
        save_list_leaving(
            &mut a,
            json!([
                off_agent("w91:p1", "c-1", "ARK_C", Some("0041")),
                off_agent("w92:p1", "c-2", "ARK_C", Some("0052")),
            ]),
        );
        settle(&mut a);
        press(&mut a, 'y');
        let (machine, first) = sent(a.account_tick());
        a.finish_action(
            machine,
            crate::actions::with_op(first),
            Err("the connection to laptop dropped before its answer came; Kerdr can't tell whether it happened, and its card shows how it stands.".into()),
        );
        let second = a.account_tick();
        complete(
            &mut a,
            second,
            json!({"ok":true,"message":"#52 moved to KBZ_C."}),
        );
        assert!(a.off_list_moves.is_none());
        assert_eq!(
            a.message().unwrap(),
            (
                "1 of 2 moved to the list: #52. Kerdr can't tell whether #41 moved: the connection to laptop dropped before its answer came. The accounts screen (A) shows where it is.",
                Tone::Warn
            )
        );
    }

    #[test]
    fn a_move_sent_again_by_itself_says_so_on_its_card_until_its_answer() {
        // A move's answer lost, or refused with `retry`: sent again by itself with its op (the
        // transport's own), its card saying so quietly meanwhile (intent 0591).
        let mut a = app();
        pick(&mut a);
        let card = a.selected_for_control().unwrap().key.clone();
        press(&mut a, 's');
        let action = key(&mut a, KeyCode::Enter);
        let (machine, request) = sent(action.clone());
        a.busy = true;
        a.busy_label = a.working_on(&request);
        a.resending(
            None,
            machine,
            &crate::actions::Resend::Lost {
                wait: Duration::from_secs(5),
                error: "no answer came".into(),
                attempt: 1,
            },
        );
        assert_eq!(a.account_retrying.as_deref(), Some(card.as_str()));
        complete(
            &mut a,
            action,
            json!({"ok":true,"state":"queued","account":"ARK_C","message":"moving to ARK_C"}),
        );
        assert_eq!(a.account_retrying, None);
    }
}
