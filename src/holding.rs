//! Holding and routing a change (intent 0591, release 4): a change for a machine that isn't
//! answering is never refused when it can still happen. Sent through another machine whose
//! backend forwards it (`"forward"` in its feed's `supports`), else kept in Kerdr's hold file
//! (crate::kept) for a backend that checks a held request again (`"hold"`), and sent once that
//! machine answers. What a backend holds for a card, and what Kerdr does, shows on the card, and
//! m lists it with a key to drop it. q waits the moment a hand-over takes, then quits by itself,
//! and keeps your words for the next Kerdr.

use std::time::Instant;

use serde_json::{Value, json};

use super::{App, Tone};
use crate::actions::Response;
use crate::kept::{HeldRequest, KeptEdit, KeptImage, KeptWords, Words};
use crate::model::Card;
use crate::poll::Conn;

/// A backend that holds a request, checks it again by `rev` when it's sent, and takes `drop`.
pub const HOLD: &str = "hold";
/// A backend that sends a request on to another machine it can reach (`forward_to`).
pub const FORWARD: &str = "forward";

/// How often a request held for a machine Kerdr doesn't have is tried again through one that
/// forwards.
const FORWARD_AGAIN: std::time::Duration = std::time::Duration::from_secs(120);

/// One thing held for a card, as its card and its peek say it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldLine {
    /// "d waits for LAB-Server", or the backend's own words: "moves to ARK_C at 02:00".
    pub words: String,
    /// When it was asked for, in seconds since 1970, where that's known.
    pub since: Option<i64>,
    /// How m drops it; None for a note that it was dropped already.
    pub drop: Option<DropOf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DropOf {
    /// One in Kerdr's hold file, by its op: dropping it only takes it out.
    Kerdr(String),
    /// One its backend holds, by its op: dropping it asks that backend.
    Backend(String),
    /// An account request its backend holds (a move for later, or one queued behind another), by
    /// its op: dropping it asks with `account-drop`, on the machine the agent runs on (r4-b's
    /// shape).
    AccountBackend(String),
    /// An account request waiting in Kerdr for its machine to answer, by the request: dropping it
    /// only takes it out.
    Account(Value),
    /// A change waiting behind its card's start, save or write-up, by Kerdr's number for it
    /// (crate::app::behind): dropping it only takes it out.
    Job(u64),
}

/// What an answer said a backend holds for a card, shown until its feed says so itself, or two
/// reads of that feed after the answer don't list it (it ran, or the backend lists no `held`).
#[derive(Clone, Debug)]
pub struct Pending {
    pub key: String,
    pub machine: usize,
    pub op: String,
    pub words: String,
    pub reads: u8,
    /// An account request's (dropped with `account-drop`), not a card change's.
    pub account: bool,
}

/// How a held change names the key that asked for it: "d", or the change in words.
pub fn held_what(request: &Value) -> String {
    match request["action"].as_str().unwrap_or_default() {
        "dispatch" => "d",
        "reopen" => "⏎",
        "refresh" => "y",
        "dismiss" => "x",
        "close" => "C",
        "cancel" => "X",
        "restore" => "u",
        "move" => "the move",
        "edit" => "your new text",
        "answer" => "your answers",
        _ => "the change",
    }
    .to_string()
}

/// A short span of time: "40 s", "3 min", "2 h".
pub fn span(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{} h", secs / 3600)
    }
}

impl App {
    fn epoch_now(&self) -> i64 {
        self.now().timestamp()
    }

    /// Whether this machine's backend holds requests: its feed says so, or, before Kerdr has read
    /// it, the hold file remembers that it did.
    pub fn holds_on(&self, machine: usize) -> bool {
        let Some(m) = self.machines.get(machine) else {
            return false;
        };
        match &m.feed {
            Some(feed) => feed.supports(HOLD),
            None => self.kept.holds_for(&m.name()),
        }
    }

    /// A machine that's answering and forwards requests, other than `home`.
    pub fn forwarder(&self, home: usize) -> Option<usize> {
        let name = self.machines.get(home).map(|m| m.name());
        self.forwarder_for(name.as_deref())
    }

    /// A machine that's answering and forwards requests, other than the one named.
    pub fn forwarder_for(&self, not: Option<&str>) -> Option<usize> {
        if !self.actions_on() {
            return None;
        }
        self.machines
            .iter()
            .enumerate()
            .find(|(_, m)| {
                m.conn.is_live()
                    && not.is_none_or(|n| !m.answers_to(n))
                    && m.feed.as_ref().is_some_and(|f| f.supports(FORWARD))
            })
            .map(|(i, _)| i)
    }

    /// As can_write, for a change to a card: a machine that isn't answering takes it too when
    /// Kerdr can send it through another machine, or hold it for this one.
    pub(crate) fn can_change(&mut self, machine: usize) -> bool {
        let away = self
            .machines
            .get(machine)
            .is_some_and(|m| !m.conn.is_live());
        if self.actions_on()
            && away
            && (self.holds_on(machine) || self.forwarder(machine).is_some())
        {
            return true;
        }
        self.can_write(machine)
    }

    /// Where a change goes when its machine isn't answering: through a machine that forwards, or
    /// into the hold file. Some(the machine to send it to, and the request) to send it now; None
    /// when it's held.
    pub(crate) fn route(
        &mut self,
        machine: usize,
        mut request: Value,
        key: Option<&str>,
    ) -> Option<(usize, Value)> {
        let Some(m) = self.machines.get(machine) else {
            return Some((machine, request));
        };
        // Its card's home isn't one of Kerdr's machines: home_machine picked one that forwards,
        // and this names where it goes.
        let home = key
            .and_then(|k| self.card(k))
            .and_then(|c| c.detail.as_ref())
            .and_then(|d| d.home.as_ref())
            .and_then(|h| h.machine.clone())
            .filter(|h| self.machine_index(h).is_none());
        if let Some(home) = home
            && request.get("forward_to").is_none()
            && m.conn.is_live()
            && m.feed.as_ref().is_some_and(|f| f.supports(FORWARD))
        {
            request["forward_to"] = Value::String(home);
            return Some((machine, request));
        }
        // A read isn't held: only a change waits for its machine.
        if m.conn.is_live() || !self.actions_on() || crate::resolve::reads(&request) {
            return Some((machine, request));
        }
        let name = m.name();
        if let Some(via) = self.forwarder(machine) {
            request["forward_to"] = Value::String(name);
            return Some((via, request));
        }
        if self.holds_on(machine) {
            let request = crate::actions::with_op(request);
            let rev = key.and_then(|k| self.card_rev(k));
            self.hold_request(name, request, key.map(str::to_string), rev, None);
            return None;
        }
        Some((machine, request))
    }

    /// A card's `rev`, as its feed gave it.
    pub(crate) fn card_rev(&self, key: &str) -> Option<String> {
        self.card(key)
            .and_then(|c| c.detail.as_ref())
            .and_then(|d| d.rev.clone())
            .filter(|r| !r.is_empty())
    }

    /// Keeps a change in the hold file for the machine named, and says so.
    pub(crate) fn hold_request(
        &mut self,
        machine: String,
        request: Value,
        key: Option<String>,
        rev: Option<String>,
        after: Option<String>,
    ) {
        let reference = key
            .as_deref()
            .and_then(|k| self.card(k))
            .map(|c| c.reference.clone())
            .or_else(|| request["id"].as_str().map(|id| self.reference_for(id)))
            .unwrap_or_else(|| "its card".into());
        let what = held_what(&request);
        let entry = HeldRequest {
            machine: machine.clone(),
            request,
            rev,
            key,
            reference: reference.clone(),
            what: what.clone(),
            pressed: self.epoch_now(),
            after,
        };
        self.kept.hold(entry);
        let dropped = self
            .machine_index(&machine)
            .and_then(|i| self.dropped_ago(i))
            .map(|ago| format!(" ({ago})"))
            .unwrap_or_default();
        self.say(
            format!("Held: {what} on {reference} goes when {machine} answers{dropped}."),
            Tone::Info,
        );
        self.rebuild();
    }

    /// How long ago a machine stopped answering, in words: "it dropped 40 s ago".
    pub(crate) fn dropped_ago(&self, machine: usize) -> Option<String> {
        let since = match &self.machines.get(machine)?.conn {
            Conn::Reconnecting { since, .. } | Conn::Unreachable { since, .. } => *since,
            _ => return None,
        };
        let secs = (self.now() - since).num_seconds();
        Some(format!("it dropped {} ago", span(secs)))
    }

    /// A machine's feed was just read: whether its backend holds is noted, what an answer said it
    /// holds is counted down, and what's held for it in Kerdr's hold file goes now. Over a day
    /// old, it's dropped instead, and its card says so.
    pub(crate) fn feed_read(&mut self, machine: usize) {
        let Some(m) = self.machines.get(machine) else {
            return;
        };
        let name = m.name();
        let holds = m.feed.as_ref().is_some_and(|f| f.supports(HOLD));
        let live = m.conn.is_live();
        if !self.demo {
            self.kept.note_holds(&name, holds);
        }
        self.count_pending(machine);
        let dropped = self.kept.expire(self.epoch_now());
        if let Some(first) = dropped.first() {
            self.say(format!("{}: {}.", first.reference, first.said), Tone::Warn);
        }
        if live {
            self.release_held(machine);
        }
        // A start or a save the feed now shows: what waited behind it is decided again.
        self.release_behind();
    }

    /// Sends what's held for this machine, now that it answers. Each goes as it was asked, with
    /// its op, the card's `rev` then, and `held: true`, so its backend checks it again; a start
    /// that waited for its write-up waits for it still.
    fn release_held(&mut self, machine: usize) {
        let entries: Vec<HeldRequest> = self
            .kept
            .held
            .iter()
            .filter(|h| {
                self.machines[machine].answers_to(&h.machine)
                    && !self.held_sending.values().any(|op| op == h.op())
            })
            .cloned()
            .collect();
        for entry in entries {
            self.send_held(machine, entry);
        }
        // Held for a machine that isn't one of Kerdr's, after a forwarder couldn't reach it: tried
        // through this one, if it forwards, at most every two minutes.
        let forwards = self.machines[machine]
            .feed
            .as_ref()
            .is_some_and(|f| f.supports(FORWARD));
        if !forwards {
            return;
        }
        let entries: Vec<HeldRequest> = self
            .kept
            .held
            .iter()
            .filter(|h| {
                self.machine_index(&h.machine).is_none()
                    && !self.held_sending.values().any(|op| op == h.op())
                    && self
                        .held_tried
                        .get(h.op())
                        .is_none_or(|at| at.elapsed() >= FORWARD_AGAIN)
            })
            .cloned()
            .collect();
        for mut entry in entries {
            self.held_tried
                .insert(entry.op().to_string(), Instant::now());
            entry.request["forward_to"] = Value::String(entry.machine.clone());
            self.send_held(machine, entry);
        }
    }

    fn send_held(&mut self, machine: usize, entry: HeldRequest) {
        let op = entry.op().to_string();
        let mut request = entry.request.clone();
        if self.holds_on(machine) {
            request["held"] = Value::Bool(true);
            if let Some(rev) = entry
                .rev
                .as_deref()
                .filter(|_| request.get("rev").is_none())
            {
                request["rev"] = Value::String(rev.to_string());
            }
        }
        // A key that waited behind a start or a save when Kerdr quit: decided again now, as it
        // would have been (crate::app::behind).
        let behind = match entry.after.as_deref() {
            Some("start") => Some(crate::resolve::Behind::Start),
            Some(after) => after
                .strip_prefix("save:")
                .map(|s| crate::resolve::Behind::Save(s.to_string())),
            None => None,
        };
        if let Some(behind) = behind {
            self.kept.release(&op);
            let from = entry
                .key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(crate::model::CardSnapshot::of);
            self.resolver.add_behind(
                machine,
                entry.request.clone(),
                from,
                entry.key.clone(),
                behind,
            );
            self.release_behind();
            return;
        }
        // A change that waited for its write-up as well: only while its card still waits, which
        // Kerdr checks itself, since an older backend checks no `rev`.
        if entry.after.as_deref() == Some("write-up") {
            let card = entry.key.as_deref().and_then(|k| self.card(k)).cloned();
            let restores = request["action"] == "restore";
            let Some(card) = card.filter(|c| {
                crate::app::waiting_work(c) && !c.dismissed() || (restores && c.dismissed())
            }) else {
                self.kept.release(&op);
                let said = if request["action"] == "dispatch" {
                    format!(
                        "{} wasn't started: it changed since you pressed {}, so Kerdr dropped that start.",
                        entry.reference, entry.what
                    )
                } else {
                    format!(
                        "{} changed since you pressed {}, so Kerdr dropped that change.",
                        entry.reference, entry.what
                    )
                };
                self.say(said, Tone::Warn);
                return;
            };
            let effect = self.effect_for(&card.key, &request);
            let n = self.resolver.add_gated(
                machine,
                request,
                Some(crate::model::CardSnapshot::of(&card)),
                Some(card.key.clone()),
                effect,
            );
            self.resolver.keep_op(n, &op);
            self.held_sending.insert(n, op);
            return;
        }
        let before = self.resolver.last_added();
        // Its card as it is now: a question its backend asks (a held stop always asks again)
        // waits on that card unless you're on it.
        self.action_from = entry
            .key
            .as_deref()
            .and_then(|k| self.card(k))
            .map(crate::model::CardSnapshot::of);
        self.action_key = entry.key.clone();
        self.queue_action(machine, request);
        let n = self.resolver.last_added();
        if n == before {
            // Not sent, and said why (its backend no longer takes it, say): it isn't kept.
            self.kept.release(&op);
            return;
        }
        self.resolver.keep_op(n, &op);
        if self.holds_on(machine) {
            self.resolver.set_rev(n, entry.rev.clone());
        }
        self.held_sending.insert(n, op);
    }

    /// Before a change's answer is taken: one sent from the hold file leaves it once answered;
    /// one a forwarding machine couldn't get through is held for the machine it was for; and one
    /// its backend holds says so on its card. Whether the answer is dealt with here.
    pub(crate) fn held_answer(&mut self, n: u64, result: &Result<Response, String>) -> bool {
        let from_file = self.held_sending.remove(&n);
        let response = result.as_ref().ok();
        if let Some(op) = &from_file
            && response.is_some_and(|r| r.unreachable.is_none())
        {
            self.kept.release(op);
        }
        if let Some(to) = response.and_then(|r| r.unreachable.clone()) {
            let Some(job) = self.resolver.take(n) else {
                return false;
            };
            let mut request = job.request.clone();
            if let Some(fields) = request.as_object_mut() {
                fields.remove("forward_to");
                fields.remove("held");
            }
            if from_file.is_none() {
                self.hold_request(to, request, job.key.clone(), job.rev.clone(), None);
            } else {
                self.rebuild();
            }
            return true;
        }
        if let Some(r) = response
            && r.ok
            && let Some(words) = r.pending.clone().filter(|w| !w.is_empty())
            && let Some(job) = self.resolver.job(n)
            && let Some(key) = job.key.clone()
        {
            let op = job.request["op"].as_str().unwrap_or_default().to_string();
            let machine = job.machine;
            self.pendings.retain(|p| !(p.key == key && p.op == op));
            self.pendings.push(Pending {
                key,
                machine,
                op,
                words,
                reads: 0,
                account: false,
            });
        }
        false
    }

    /// An account request its backend accepted and holds (a move for later, or one queued
    /// behind a move under way): its `pending` words show on the card of the agent it moves.
    pub(crate) fn account_pending(
        &mut self,
        machine: usize,
        request: &Value,
        result: &Result<Response, String>,
    ) {
        let Some(words) = result
            .as_ref()
            .ok()
            .filter(|r| r.ok && !r.confirmation)
            .and_then(|r| r.pending.clone())
            .filter(|w| !w.is_empty())
        else {
            return;
        };
        let pane = request["pane"].as_str();
        let session = request["session"].as_str();
        let workspace = request["workspace"].as_str();
        let Some(key) = self
            .board
            .cards
            .iter()
            .find(|c| {
                c.machine_index == machine
                    && session.is_none_or(|s| c.session.as_deref() == Some(s))
                    && (pane.is_some_and(|p| c.agents.iter().any(|a| a.pane_id == p))
                        || (pane.is_none()
                            && workspace.is_some_and(|w| c.workspace_id.as_deref() == Some(w))))
            })
            .map(|c| c.key.clone())
        else {
            return;
        };
        let op = request["op"].as_str().unwrap_or_default().to_string();
        self.pendings.retain(|p| !(p.key == key && p.op == op));
        self.pendings.push(Pending {
            key,
            machine,
            op,
            words,
            reads: 0,
            account: true,
        });
        self.rebuild();
    }

    /// After an answer: one that went through another machine says so.
    pub(crate) fn note_via(&mut self, to: Option<String>, via: Option<String>) {
        let (Some(to), Some(via)) = (to, via) else {
            return;
        };
        let line = format!("Sent through {via}, since {to} isn't answering here.");
        match &self.message {
            Some((text, Tone::Fail, _)) => {
                let text = format!("{text}\n{line}");
                self.message = Some((text, Tone::Fail, Instant::now()));
            }
            _ => self.say(line, Tone::Info),
        }
    }

    /// Counts a read against what answers said each backend holds.
    fn count_pending(&mut self, machine: usize) {
        let listed: Vec<(String, String)> = self
            .board
            .cards
            .iter()
            .filter(|c| c.machine_index == machine)
            .flat_map(|c| {
                let agent = self
                    .account_agent(c)
                    .into_iter()
                    .flat_map(|a| a.held.iter());
                c.detail
                    .iter()
                    .flat_map(|d| d.held.iter())
                    .chain(agent)
                    .map(|h| (c.key.clone(), h.op.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        // A save still finishing in the background has no card yet: its feed lists it at the top.
        let saving: Vec<String> = self
            .machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .map(|f| f.held.iter().map(|h| h.op.clone()).collect())
            .unwrap_or_default();
        for p in self.pendings.iter_mut().filter(|p| p.machine == machine) {
            if listed.iter().any(|(k, op)| *k == p.key && *op == p.op) || saving.contains(&p.op) {
                p.reads = 0;
            } else {
                p.reads = p.reads.saturating_add(1);
            }
        }
        let gone: Vec<Pending> = self
            .pendings
            .iter()
            .filter(|p| p.reads >= 3)
            .cloned()
            .collect();
        self.pendings.retain(|p| p.reads < 3);
        // A background save its feed stopped listing with no card come for it: its backend gave
        // up on it, so its card goes too, said, rather than saving forever.
        for p in gone {
            if !self.holds_on(machine) {
                continue;
            }
            if let Some(request) = self.resolver.drop_saving(&p.op) {
                let title = crate::resolve::Placeholder::title_for(
                    request["words"].as_str().unwrap_or_default(),
                );
                let name = self.machines[machine].name();
                self.say(
                    format!(
                        "Your new intent \"{title}\" stopped saving on {name} without getting its number; {name} keeps its words."
                    ),
                    Tone::Warn,
                );
                self.rebuild();
            }
        }
    }

    /// Everything held for a card, Kerdr's first, then its backend's, then what was dropped.
    pub fn held_lines(&self, c: &Card) -> Vec<HeldLine> {
        let mut lines = vec![];
        for h in self.kept.for_card(&c.key) {
            lines.push(HeldLine {
                words: format!("{} waits for {}", h.what, h.machine),
                since: Some(h.pressed),
                drop: Some(DropOf::Kerdr(h.op().to_string())),
            });
        }
        for (words, n) in self.behind_lines(c) {
            lines.push(HeldLine {
                words,
                since: None,
                drop: Some(DropOf::Job(n)),
            });
        }
        for (words, since, request) in self.account_waits_for(c) {
            lines.push(HeldLine {
                words,
                since,
                drop: Some(DropOf::Account(request)),
            });
        }
        let mut ops: Vec<String> = vec![];
        // An account request's is dropped with `account-drop`: one the card's own `held` lists as
        // one, and every one the accounts feed lists for its agent (r4-b's shape).
        let feed_held = c
            .detail
            .iter()
            .flat_map(|d| d.held.iter())
            .map(|h| (h, h.action.starts_with("account-")));
        let agent_held = self
            .account_agent(c)
            .into_iter()
            .flat_map(|a| a.held.iter())
            .map(|h| (h, true));
        // A new intent's save its backend finishes, shown again after a restart from the feed's
        // own list: this Kerdr doesn't have its words, so it isn't dropped from here, which would
        // lose them.
        let saved_elsewhere = c.intake() == Some(crate::resolve::SAVING)
            && crate::app::behind::saving_submission(&c.key)
                .is_some_and(|s| self.resolver.saving(s).is_none());
        for (h, account) in feed_held.chain(agent_held) {
            if ops.contains(&h.op) {
                continue;
            }
            ops.push(h.op.clone());
            let drop = if saved_elsewhere {
                None
            } else if account {
                Some(DropOf::AccountBackend(h.op.clone()))
            } else {
                Some(DropOf::Backend(h.op.clone()))
            };
            lines.push(HeldLine {
                words: h.pending.clone(),
                since: h.since,
                drop,
            });
        }
        for p in self.pendings.iter().filter(|p| p.key == c.key) {
            if ops.contains(&p.op) {
                continue;
            }
            ops.push(p.op.clone());
            lines.push(HeldLine {
                words: p.words.clone(),
                since: None,
                drop: (!p.op.is_empty()).then(|| {
                    if p.account {
                        DropOf::AccountBackend(p.op.clone())
                    } else {
                        DropOf::Backend(p.op.clone())
                    }
                }),
            });
        }
        for d in self
            .kept
            .dropped
            .iter()
            .filter(|d| d.key.as_deref() == Some(c.key.as_str()))
        {
            lines.push(HeldLine {
                words: d.said.clone(),
                since: None,
                drop: None,
            });
        }
        lines
    }

    /// The card's line for what's held for it: "⧗ d waits for LAB-Server", or for a new intent
    /// its backend is still saving, "✎ saving: its number comes once the central copy answers".
    pub fn held_line_for(&self, c: &Card) -> Option<String> {
        let lines = self.held_lines(c);
        let first = lines.first()?;
        let more = match lines.len() {
            1 => String::new(),
            n => format!(" (+{})", n - 1),
        };
        let mark = if c.intake() == Some(crate::resolve::SAVING) {
            "✎"
        } else {
            "⧗"
        };
        Some(format!("{mark} {}{more}", first.words))
    }

    /// The peek's key to drop what's held: D for the only one (None), or with a count the one of
    /// that number (2D, the second).
    pub(crate) fn drop_held(&mut self, index: Option<usize>) -> super::Action {
        let Some(card) = self.selected().cloned() else {
            return super::Action::None;
        };
        let droppable: Vec<HeldLine> = self
            .held_lines(&card)
            .into_iter()
            .filter(|l| l.drop.is_some())
            .collect();
        if index.is_none() && droppable.len() > 1 {
            self.say(
                format!(
                    "{} things are held for {}: D drops the first, 2D the second.",
                    droppable.len(),
                    card.reference,
                ),
                Tone::Info,
            );
            return super::Action::None;
        }
        let Some(line) = droppable.get(index.unwrap_or(0)) else {
            // A save an earlier Kerdr sent: dropping it here would lose words this one doesn't
            // have.
            if droppable.is_empty()
                && card.intake() == Some(crate::resolve::SAVING)
                && !self.held_lines(&card).is_empty()
            {
                self.say(
                    "This new intent was sent before Kerdr last opened, so its words aren't here to give back: it isn't dropped from here. It gets its number once it's saved, or its backend keeps its words if it gives up.",
                    Tone::Info,
                );
            }
            return super::Action::None;
        };
        match line.drop.clone() {
            Some(DropOf::Kerdr(op)) => {
                if let Some(h) = self.kept.release(&op) {
                    self.say(
                        format!("Dropped: {} on {} won't be sent.", h.what, card.reference),
                        Tone::Info,
                    );
                }
                self.rebuild();
                super::Action::None
            }
            Some(DropOf::Backend(op)) => self.drop_backend(&card, &op, &line.words),
            Some(DropOf::AccountBackend(op)) => self.drop_account_backend(&card, &op, &line.words),
            Some(DropOf::Account(request)) => {
                self.drop_account_wait(&request);
                super::Action::None
            }
            Some(DropOf::Job(n)) => {
                self.drop_behind(n, &card);
                super::Action::None
            }
            None => super::Action::None,
        }
    }

    /// Asks a card's backend to drop a request it holds.
    fn drop_backend(&mut self, card: &Card, op: &str, words: &str) -> super::Action {
        let machine = match card.detail.as_ref() {
            Some(_) => match self.home_machine(card) {
                Some(m) => m,
                None => return super::Action::None,
            },
            None => card.machine_index,
        };
        if !self.holds_on(machine) {
            self.say(
                format!(
                    "{}'s backend can't drop what it holds, so \"{words}\" stays.",
                    self.machines[machine].name()
                ),
                Tone::Info,
            );
            return super::Action::None;
        }
        if !self.can_write(machine) {
            return super::Action::None;
        }
        let mut request = json!({"action": "drop", "held": op});
        match (&card.detail, &card.session, &card.workspace_id) {
            // A new intent still saving has no number yet: its request's op alone names it.
            _ if card.intake() == Some(crate::resolve::SAVING) => {}
            (Some(d), _, _) => request["id"] = Value::String(d.id.clone()),
            (None, Some(session), Some(workspace)) => {
                request["session"] = Value::String(session.clone());
                request["workspace"] = Value::String(workspace.clone());
            }
            _ => return super::Action::None,
        }
        self.action_key = Some(card.key.clone());
        super::Action::Mutate { machine, request }
    }

    /// Asks the machine an agent runs on to drop an account request its backend holds for it:
    /// `{"action": "account-drop", "held": op}`, which goes through its actions command as every
    /// account request does (r4-b's shape).
    fn drop_account_backend(&mut self, card: &Card, op: &str, words: &str) -> super::Action {
        let machine = card.machine_index;
        if !self.holds_on(machine) {
            self.say(
                format!(
                    "{}'s backend can't drop what it holds, so \"{words}\" stays.",
                    self.machines[machine].name()
                ),
                Tone::Info,
            );
            return super::Action::None;
        }
        if !self.can_write(machine) {
            return super::Action::None;
        }
        self.action_key = Some(card.key.clone());
        super::Action::Mutate {
            machine,
            request: json!({"action": "account-drop", "held": op}),
        }
    }

    /// What a drop's answer says, and the card stops showing what it dropped. A backend says
    /// whether it was dropped with `dropped`, or with `state`: `dropped`, or `running`, `ran`,
    /// `ended`, `replaced` or `unknown` when it wasn't (r4-a's shape).
    pub(crate) fn drop_answered(&mut self, request: &Value, response: &Response) -> bool {
        if !matches!(request["action"].as_str(), Some("drop" | "account-drop")) {
            return false;
        }
        let op = request["held"].as_str().unwrap_or_default();
        if response.ok {
            self.pendings.retain(|p| p.op != op);
            let dropped = match (response.dropped, response.state.as_deref()) {
                (Some(dropped), _) => dropped,
                (None, Some(state)) => state == "dropped",
                (None, None) => true,
            };
            // A save its feed still lists until the next read doesn't come back as a card.
            if dropped {
                for feed in self.machines.iter_mut().filter_map(|m| m.feed.as_mut()) {
                    feed.held.retain(|h| h.op != op);
                }
            }
            // A new intent's save dropped before it got its number: its card goes, and his words
            // come back as a failed save's do.
            let mut back = None;
            if dropped
                && request["action"] == "drop"
                && let Some(create) = self.resolver.drop_saving(op)
            {
                let words = create["words"].as_str().unwrap_or_default().to_string();
                let images = create["images"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| {
                        crate::kept::KeptImage {
                            tag: u32::try_from(i["tag"].as_u64()?).ok()?,
                            data: i["data"].as_str()?.to_string(),
                        }
                        .image()
                    })
                    .collect();
                back = Some(if self.give_back(words, images, None) {
                    "c opens it again with your words."
                } else {
                    "Its words are kept, and c brings them back once the box is empty."
                });
            }
            let mut said = if response.message.is_empty() {
                if dropped {
                    "Dropped: it won't happen."
                } else {
                    "It had already gone ahead, so there was nothing to drop."
                }
                .to_string()
            } else {
                response.message.clone()
            };
            if let Some(back) = back {
                said = format!("{said} {back}");
            }
            self.say(said, Tone::Info);
            self.rebuild();
            return true;
        }
        false
    }

    /// u on a machine's row: an intent to update herdr there, made by a machine whose backend is
    /// ark-sdlc's, this one first. It's done through that machine's infrastructure code, or
    /// omarchy-config on an Omarchy machine: Kerdr installs nothing itself. None when no such
    /// backend answers.
    pub(crate) fn herdr_intent(&mut self, i: usize) -> Option<super::Action> {
        if !self.actions_on() {
            return None;
        }
        let at = self.machines.iter().position(|m| {
            m.conn.is_live()
                && m.feed
                    .as_ref()
                    .is_some_and(|f| f.source.as_deref() == Some("ark-sdlc"))
        })?;
        let m = self.machines.get(i)?;
        let name = m.name();
        let why = match &m.conn {
            Conn::HerdrMissing => "herdr isn't installed there".to_string(),
            Conn::HerdrTooOld { version } => {
                format!("its herdr is {version}, older than Kerdr needs")
            }
            _ => "Andrew asked for it from Kerdr's list of machines".to_string(),
        };
        let words = format!(
            "Update herdr on {name}: {why}. Do it through {name}'s infrastructure code, or omarchy-config on an Omarchy machine, and run it there; nothing installed by hand."
        );
        let draft = super::Draft {
            words,
            machine: at,
            session: "default".into(),
            images: vec![],
            came_back: None,
            filed: super::Filed::default(),
            unfiled: None,
        };
        let action = self.send_draft(draft);
        if matches!(action, super::Action::Mutate { .. }) {
            self.say(
                format!(
                    "Making an intent to update herdr on {name}, through its infrastructure code: Kerdr installs nothing itself."
                ),
                Tone::Info,
            );
        }
        Some(action)
    }

    /// What q waits for before quitting, in words: "your new intent is saved". None when nothing
    /// would be lost, and q quits at once: a change its command has whole carries on without
    /// Kerdr, and your words are kept for the next one.
    pub fn quit_waits_for(&self) -> Option<String> {
        let held = self.resolver.holding_quit();
        if held.iter().any(|j| j.request["action"] == "create") {
            return Some("your new intent is saved".into());
        }
        if self.send_after_load.is_some() {
            return Some("your new intent's image is in and it's sent".into());
        }
        if let Some(first) = held.first() {
            let reference = first
                .id()
                .map(|id| self.reference_for(id))
                .unwrap_or_else(|| "a card".into());
            return Some(format!("{reference}'s change is handed over"));
        }
        if self.busy || !self.account_queue.is_empty() {
            return Some("the account change answers".into());
        }
        // An account request waiting for its machine, which only this Kerdr keeps (intent 0591,
        // release 4): a second q drops it.
        self.account_away_wait()
    }

    /// q: whether Kerdr quits now. Otherwise it says what it waits for, and quits by itself
    /// once that's done (quit_due); Esc stays. A second q quits at once when what's left can be
    /// kept: an account change, or a change whose backend holds requests, which goes into the
    /// hold file with its op.
    pub fn quit_key(&mut self) -> bool {
        let Some(what) = self.quit_waits_for() else {
            return true;
        };
        if self.quitting.is_some() {
            self.hold_unsent();
            match self.quit_waits_for() {
                None => return true,
                Some(w)
                    if w == "the account change answers"
                        || self.account_away_wait().as_deref() == Some(w.as_str()) =>
                {
                    return true;
                }
                Some(w) => {
                    self.say(
                        format!("Quitting once {w}: quitting before would lose it. Esc stays."),
                        Tone::Info,
                    );
                    return false;
                }
            }
        }
        self.quitting = Some(Instant::now());
        let again = if what == "the account change answers" {
            "q again quits now, "
        } else if self.account_away_wait().as_deref() == Some(what.as_str()) {
            "q again quits now and drops it, "
        } else {
            ""
        };
        self.say(
            format!("Quitting once {what}… {again}Esc stays."),
            Tone::Info,
        );
        false
    }

    /// Whether q is waiting and nothing holds it any more.
    pub fn quit_due(&self) -> bool {
        self.quitting.is_some() && self.quit_waits_for().is_none()
    }

    /// Changes not yet in their command's hands, for a machine whose backend holds requests,
    /// go into the hold file with their op, for the next Kerdr to send.
    fn hold_unsent(&mut self) {
        let unsent: Vec<u64> = self
            .resolver
            .holding_quit()
            .iter()
            .filter(|j| j.request["action"] != "create" && self.holds_on(j.machine))
            .map(|j| j.n)
            .collect();
        for n in unsent {
            if let Some(job) = self.resolver.take(n) {
                let machine = self.machines[job.machine].name();
                self.kept.hold(HeldRequest {
                    machine,
                    reference: job
                        .id()
                        .map(|id| self.reference_for(id))
                        .unwrap_or_default(),
                    what: held_what(&job.request),
                    request: job.request,
                    rev: job.rev,
                    key: job.key,
                    pressed: self.epoch_now(),
                    after: None,
                });
            }
        }
    }

    /// Esc while q waits: Kerdr stays open.
    pub(crate) fn stay(&mut self) -> bool {
        if self.quitting.take().is_none() {
            return false;
        }
        self.say("Staying open.", Tone::Info);
        true
    }

    /// As Kerdr quits: starts waiting for their write-up go into the hold file, for the next
    /// Kerdr to send once that's done, and your words into the words file.
    pub fn keep_on_quit(&mut self) {
        let awaited: Vec<(String, Value, Option<String>, Option<String>)> = self
            .resolver
            .write_ups_awaited()
            .iter()
            .map(|j| {
                (
                    self.machines
                        .get(j.machine)
                        .map(|m| m.name())
                        .unwrap_or_default(),
                    j.request.clone(),
                    j.key.clone(),
                    j.rev.clone(),
                )
            })
            .collect();
        for (machine, request, key, rev) in awaited {
            let reference = key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(|c| c.reference.clone())
                .unwrap_or_default();
            self.kept.hold(HeldRequest {
                machine,
                what: held_what(&request),
                request,
                rev,
                key,
                reference,
                pressed: self.epoch_now(),
                after: Some("write-up".into()),
            });
        }
        // Keys held behind a start or a save: the next Kerdr decides them again (intent 0591,
        // release 4).
        let behind: Vec<(String, Value, Option<String>, String)> = self
            .resolver
            .behind_jobs()
            .into_iter()
            .filter_map(|(n, behind)| {
                let j = self.resolver.job(n)?;
                let after = match behind {
                    crate::resolve::Behind::Start => "start".to_string(),
                    crate::resolve::Behind::Save(submission) => format!("save:{submission}"),
                };
                let machine = self.machines.get(j.machine).map(|m| m.name())?;
                Some((machine, j.request.clone(), j.key.clone(), after))
            })
            .collect();
        for (machine, request, key, after) in behind {
            let reference = key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(|c| c.reference.clone())
                .unwrap_or_default();
            self.kept.hold(HeldRequest {
                machine,
                what: held_what(&request),
                request,
                rev: None,
                key,
                reference,
                pressed: self.epoch_now(),
                after: Some(after),
            });
        }
        if let Some(dir) = self.words_dir.clone() {
            let _ = self.words().keep(&dir);
        }
    }

    /// Your words, as the words file keeps them.
    pub fn words(&self) -> Words {
        let kept = |words: &str, images: &[crate::images::Image]| KeptWords {
            words: words.to_string(),
            images: images.iter().map(KeptImage::of).collect(),
        };
        let mut words = Words::default();
        if !self.draft.trim().is_empty() {
            words.draft = Some(kept(&self.draft, &self.draft_images));
        }
        for r in &self.returned {
            words.returned.push(kept(&r.words, &r.images));
        }
        // Words sent once their image is in, which quitting would drop: given back next time.
        if let Some((_, draft)) = &self.send_after_load {
            words.returned.push(kept(&draft.words, &draft.images));
        }
        let mut edits: Vec<KeptEdit> = self
            .edits
            .iter()
            .filter_map(|(key, held)| {
                let mine = held.mine.as_ref()?;
                Some(KeptEdit {
                    key: key.clone(),
                    id: held.id.clone(),
                    machine: self.machines.get(held.machine)?.name(),
                    current: held.current.clone(),
                    was: held.was.clone(),
                    text: mine.text.clone(),
                    from: mine.from.clone(),
                })
            })
            .collect();
        edits.sort_by(|a, b| a.key.cmp(&b.key));
        words.edits = edits;
        words
    }

    /// Words the last Kerdr kept: the box's draft back in the box (c), words given back after it,
    /// and your versions of item texts, which e opens, reading the text first to see whether a
    /// save of them landed meanwhile.
    pub fn restore_words(&mut self, words: Words) {
        let images = |kept: &[KeptImage]| kept.iter().filter_map(KeptImage::image).collect();
        if let Some(draft) = words.draft {
            if self.draft.trim().is_empty() {
                self.draft = draft.words;
                self.draft_images = images(&draft.images);
            } else {
                self.returned.push_back(super::Returned {
                    words: draft.words,
                    images: images(&draft.images),
                    from: None,
                });
            }
        }
        for r in words.returned {
            self.returned.push_back(super::Returned {
                words: r.words,
                images: images(&r.images),
                from: None,
            });
        }
        for e in words.edits {
            let Some(machine) = self.machine_index(&e.machine) else {
                continue;
            };
            self.edits.insert(
                e.key,
                super::edit_controls::Held {
                    id: e.id,
                    machine,
                    current: e.current,
                    was: e.was,
                    read_at: Instant::now(),
                    mine: Some(super::edit_controls::Mine {
                        text: e.text,
                        from: e.from,
                    }),
                    check_first: true,
                    read_only: None,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::app::tests::{press, press_code};
    use crate::app::waiting_work;
    use crate::config::Config;
    use crate::kept::Kept;
    use ratatui::crossterm::event::KeyCode;

    fn app() -> App {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.cfg.actions.command = Some("test-adapter".into());
        app
    }

    /// A card waiting in the backlog, selected, with `rev` "r1" in its feed: its key, id,
    /// reference and machine.
    fn waiting(app: &mut App) -> (String, String, String, usize) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| waiting_work(c) && c.intake().is_none() && !c.dismissed())
            .expect("a waiting backlog card in the demo")
            .clone();
        let id = card.detail.as_ref().unwrap().id.clone();
        let m = card.machine_index;
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.rev = Some("r1".into());
            }
        }
        app.rebuild();
        select(app, &card.key);
        (card.key, id, card.reference, m)
    }

    fn select(app: &mut App, key: &str) {
        let col = app.card(key).unwrap().stage.index();
        app.mode = super::super::Mode::Board;
        app.sel = super::super::Selection {
            col,
            key: Some(key.to_string()),
        };
    }

    fn supports(app: &mut App, machine: usize, what: &str) {
        app.machines[machine]
            .feed
            .as_mut()
            .unwrap()
            .supports
            .push(what.into());
    }

    fn away(app: &mut App, machine: usize, secs: i64) {
        let since = app.now() - chrono::Duration::seconds(secs);
        app.machines[machine].conn = Conn::Reconnecting {
            since,
            attempt: 1,
            retry_at: since,
            detail: "no answer".into(),
        };
    }

    fn back(app: &mut App, machine: usize) {
        app.machines[machine].conn = Conn::Live {
            latency: Default::default(),
        };
        app.feed_read(machine);
    }

    fn said(app: &App) -> String {
        app.message()
            .map(|(t, _)| t.to_string())
            .unwrap_or_default()
    }

    fn ok(extra: Value) -> Result<Response, String> {
        let mut reply = json!({"ok": true, "message": ""});
        if let (Some(fields), Some(more)) = (reply.as_object_mut(), extra.as_object()) {
            fields.extend(more.clone());
        }
        Ok(crate::actions::parse_response(&reply.to_string()).unwrap())
    }

    /// d, as the board's loop takes it.
    fn d(app: &mut App) -> Action {
        let action = press(app, 'd');
        if let Action::Mutate { machine, request } = action.clone() {
            app.queue_action(machine, request);
        }
        action
    }

    #[test]
    fn a_change_for_a_machine_that_isnt_answering_is_held_and_goes_when_it_answers() {
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 40);
        let name = app.machines[m].name();
        assert!(
            matches!(d(&mut app), Action::Mutate { .. }),
            "{}",
            said(&app)
        );
        // Nothing goes yet: it's in the hold file, and its card says so.
        assert!(app.resolver.ready().is_empty());
        assert_eq!(
            said(&app),
            format!("Held: d on {reference} goes when {name} answers (it dropped 40 s ago).")
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("⧗ d waits for {name}").as_str())
        );
        let held = app.kept.held[0].clone();
        assert_eq!(held.rev.as_deref(), Some("r1"));
        assert_eq!(held.request["action"], "dispatch");
        assert_eq!(held.request["id"], id.as_str());
        assert!(!held.op().is_empty());
        // Its machine answers: it goes with the op it had, the card's rev then, and held: true.
        back(&mut app, m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{ready:?}");
        let (n, machine, request, _) = ready[0].clone();
        assert_eq!(machine, m);
        assert_eq!(request["op"], held.op());
        assert_eq!(request["rev"], "r1");
        assert_eq!(request["held"], true);
        // A second read meanwhile doesn't send it twice.
        app.feed_read(m);
        assert!(app.resolver.ready().is_empty());
        // Answered: it leaves the hold file, and its card stops saying it waits.
        assert_eq!(app.kept.held.len(), 1);
        app.finish_job(n, request, ok(json!({})));
        assert!(app.kept.held.is_empty());
    }

    #[test]
    fn a_held_change_dropped_with_m_is_never_sent() {
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        d(&mut app);
        assert_eq!(app.kept.held.len(), 1);
        // m peeks, and the peek's D drops it.
        press(&mut app, 'm');
        assert!(matches!(app.mode, super::super::Mode::Peek { .. }));
        assert_eq!(press(&mut app, 'D'), Action::None);
        assert!(app.kept.held.is_empty());
        assert_eq!(
            said(&app),
            format!("Dropped: d on {reference} won't be sent.")
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(app.held_line_for(&card), None);
        back(&mut app, m);
        assert!(app.resolver.ready().is_empty());
    }

    #[test]
    fn without_hold_or_forward_a_machine_that_isnt_answering_refuses_as_before() {
        let mut app = app();
        let (_, _, _, m) = waiting(&mut app);
        away(&mut app, m, 5);
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(said(&app).contains("not answering"), "{}", said(&app));
        assert!(app.kept.held.is_empty());
    }

    /// A card waiting on a machine that isn't answering, and another machine that forwards: the
    /// forwarder, and both machines' names.
    fn forwarding(app: &mut App) -> (usize, usize, String, String, String) {
        let (_, _, reference, m) = waiting(app);
        let via = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .expect("a second machine with a feed in the demo");
        supports(app, via, FORWARD);
        supports(app, m, HOLD);
        away(app, m, 5);
        let name = app.machines[m].name();
        let via_name = app.machines[via].name();
        (m, via, reference, name, via_name)
    }

    #[test]
    fn a_change_goes_through_a_machine_that_forwards_and_says_so() {
        let mut app = app();
        let (_, via, _, name, via_name) = forwarding(&mut app);
        d(&mut app);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1);
        let (n, machine, request, _) = ready[0].clone();
        assert_eq!(machine, via);
        assert_eq!(request["forward_to"], name.as_str());
        assert!(app.kept.held.is_empty());
        app.finish_job(n, request, ok(json!({"via": via_name})));
        assert!(
            said(&app).contains(&format!(
                "Sent through {via_name}, since {name} isn't answering"
            )),
            "{}",
            said(&app)
        );
    }

    #[test]
    fn a_change_a_forwarder_cant_get_through_is_held_for_its_machine() {
        let mut app = app();
        let (m, _, reference, name, _) = forwarding(&mut app);
        d(&mut app);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request.clone(), ok(json!({"unreachable": name})));
        assert_eq!(app.kept.held.len(), 1);
        let held = app.kept.held[0].clone();
        assert_eq!(held.machine, name);
        assert_eq!(held.op(), request["op"].as_str().unwrap());
        assert_eq!(held.rev.as_deref(), Some("r1"));
        assert!(held.request.get("forward_to").is_none());
        assert_eq!(
            said(&app),
            format!("Held: d on {reference} goes when {name} answers (it dropped 5 s ago).")
        );
        // Its machine answers: it goes there, with the same op.
        back(&mut app, m);
        let (_, machine, again, _) = app.resolver.ready()[0].clone();
        assert_eq!(machine, m);
        assert_eq!(again["op"], held.op());
        assert!(again.get("forward_to").is_none());
    }

    #[test]
    fn what_a_backend_holds_shows_on_the_card_and_m_drops_it_there() {
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        // The answer's pending words, until the feed says so itself.
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request.clone(),
            ok(json!({"pending": "queued in ARK: starts when a place frees"})),
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("⧗ queued in ARK: starts when a place frees")
        );
        // Its feed lists it: shown from there, once.
        let op = request["op"].as_str().unwrap().to_string();
        let since = app.now().timestamp() - 120;
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.held = vec![crate::feed::Held {
                    op: op.clone(),
                    action: "dispatch".into(),
                    pending: "queued in ARK: starts when a place frees".into(),
                    since: Some(since),
                    until: None,
                    submission: None,
                    title: None,
                }];
            }
        }
        app.rebuild();
        for _ in 0..4 {
            app.feed_read(m);
        }
        let card = app.card(&key).unwrap().clone();
        assert_eq!(app.held_lines(&card).len(), 1);
        // m, then D: a drop to its backend, naming the record and the op.
        select(&mut app, &key);
        press(&mut app, 'm');
        let Action::Mutate { machine, request } = press(&mut app, 'D') else {
            panic!("no drop: {}", said(&app))
        };
        assert_eq!(machine, m);
        assert_eq!(request, json!({"action":"drop","held":op,"id":id}));
        // Its answer says what was dropped.
        app.queue_action(machine, request);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request,
            ok(json!({"message": "Dropped the start of #41: it won't start.", "dropped": true})),
        );
        assert_eq!(said(&app), "Dropped the start of #41: it won't start.");
    }

    #[test]
    fn pending_words_go_once_two_reads_of_the_feed_dont_list_them() {
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request,
            ok(json!({"pending": "moves to ARK_C at 02:00"})),
        );
        assert!(app.held_line_for(&app_card(&app, &key)).is_some());
        app.feed_read(m);
        app.feed_read(m);
        assert!(app.held_line_for(&app_card(&app, &key)).is_some());
        app.feed_read(m);
        assert_eq!(app.held_line_for(&app_card(&app, &key)), None);
    }

    fn app_card(app: &App, key: &str) -> Card {
        app.card(key).unwrap().clone()
    }

    #[test]
    fn q_waits_for_a_hand_over_then_quits_by_itself_and_esc_stays() {
        let mut app = app();
        let (_, id, reference, m) = waiting(&mut app);
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        assert!(!app.quit_key());
        assert_eq!(
            said(&app),
            format!("Quitting once {reference}'s change is handed over… Esc stays.")
        );
        assert!(!app.quit_due());
        // Esc stays.
        press_code(&mut app, KeyCode::Esc);
        assert_eq!(app.quitting, None);
        assert_eq!(said(&app), "Staying open.");
        // q again, and the hand-over finishes: Kerdr quits by itself.
        assert!(!app.quit_key());
        app.job_started(n);
        app.resolver.handed(n);
        assert!(app.quit_due());
        // Nothing waiting: q quits at once, though the start still runs without Kerdr.
        let mut app2 = self::app();
        assert!(app2.quit_key());
        drop(request);
    }

    #[test]
    fn q_waits_for_an_account_change_and_a_second_q_quits_now() {
        let mut app = app();
        app.busy = true;
        assert!(!app.quit_key());
        assert_eq!(
            said(&app),
            "Quitting once the account change answers… q again quits now, Esc stays."
        );
        assert!(app.quit_key());
    }

    #[test]
    fn a_second_q_holds_a_change_not_yet_sent_for_a_backend_that_holds() {
        let mut app = app();
        let (_, id, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (_, _, request, _) = app.resolver.ready()[0].clone();
        assert!(!app.quit_key());
        assert!(app.quit_key());
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].op(), request["op"].as_str().unwrap());
    }

    #[test]
    fn a_new_intent_holds_q_until_it_is_saved_and_its_words_coming_back_stay() {
        let mut app = app();
        app.queue_action(
            0,
            json!({"action":"create","words":"Fix it","submission":"s-1","session":"ark"}),
        );
        app.resolver.ready();
        assert!(!app.quit_key());
        assert_eq!(
            said(&app),
            "Quitting once your new intent is saved… Esc stays."
        );
        // A second q doesn't drop it.
        assert!(!app.quit_key());
        assert!(
            said(&app).contains("quitting before would lose it"),
            "{}",
            said(&app)
        );
    }

    #[test]
    fn q_keeps_words_and_a_start_waiting_for_its_write_up_for_the_next_kerdr() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        app.words_dir = Some(dir.path().to_path_buf());
        let (key, id, _, m) = waiting(&mut app);
        app.resolver.add_gated(
            m,
            json!({"action":"dispatch","id":id}),
            None,
            Some(key.clone()),
            None,
        );
        app.draft = "Words not yet sent".into();
        app.returned.push_back(super::super::Returned {
            words: "Words a save gave back".into(),
            images: vec![],
            from: None,
        });
        // Nothing is lost by quitting, so q quits at once.
        assert!(app.quit_key());
        app.keep_on_quit();
        let next = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(next.held.len(), 1);
        assert_eq!(next.held[0].after.as_deref(), Some("write-up"));
        assert_eq!(next.held[0].key.as_deref(), Some(key.as_str()));
        // The next Kerdr has the words back where they were.
        let mut later = self::app();
        later.restore_words(Words::take(dir.path()));
        assert_eq!(later.draft, "Words not yet sent");
        assert_eq!(later.returned.len(), 1);
        assert_eq!(later.returned[0].words, "Words a save gave back");
    }

    #[test]
    fn a_held_start_waiting_for_its_write_up_is_dropped_if_its_card_changed() {
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        app.kept.hold(HeldRequest {
            machine: app.machines[m].name(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: None,
            key: Some(key.clone()),
            reference: reference.clone(),
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: Some("write-up".into()),
        });
        // Still waiting: it goes back to waiting for its write-up, with its op.
        let op = app.kept.held[0].op().to_string();
        back(&mut app, m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1);
        assert!(ready[0].3, "gated");
        assert_eq!(ready[0].2["op"], op.as_str());
        // Set aside meanwhile: dropped, and said.
        let mut app = self::app();
        let (key, id, reference, m) = waiting(&mut app);
        app.kept.hold(HeldRequest {
            machine: app.machines[m].name(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: None,
            key: Some(key.clone()),
            reference: reference.clone(),
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: Some("write-up".into()),
        });
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.disposition = Some("wont_do".into());
            }
        }
        app.rebuild();
        back(&mut app, m);
        assert!(app.resolver.ready().is_empty());
        assert!(app.kept.held.is_empty());
        assert!(said(&app).contains("wasn't started"), "{}", said(&app));
    }

    fn home(app: &mut App, id: &str, m: usize, home: &str) {
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.home = Some(crate::feed::Home {
                    machine: Some(home.into()),
                    session: Some("web".into()),
                });
            }
        }
        app.rebuild();
    }

    #[test]
    fn a_card_homed_on_a_machine_kerdr_doesnt_know_goes_through_one_that_forwards() {
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        home(&mut app, &id, m, "elsewhere");
        select(&mut app, &key);
        // No machine forwards: said, as before.
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(said(&app).contains("not configured"), "{}", said(&app));
        supports(&mut app, m, FORWARD);
        let Action::Mutate { machine, request } = press(&mut app, 'd') else {
            panic!("nothing sent: {}", said(&app))
        };
        assert_eq!(machine, m);
        app.queue_action(machine, request);
        let (_, machine, request, _) = app.resolver.ready()[0].clone();
        assert_eq!(machine, m);
        assert_eq!(request["forward_to"], "elsewhere");
    }

    #[test]
    fn a_refusal_naming_a_home_kerdr_cant_reach_goes_through_a_forwarder_or_is_held() {
        let refused = |home: &str| {
            Ok(Response {
                ok: false,
                message: format!("It lives on {home} now; ⏎ again opens it there."),
                home: Some(home.into()),
                ..Default::default()
            })
        };
        // A home Kerdr doesn't know, and a machine that forwards: sent on through it, once.
        let mut app = app();
        let (_, id, _, m) = waiting(&mut app);
        supports(&mut app, m, FORWARD);
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request.clone(), refused("elsewhere/ARK"));
        // The same change, its op too, goes to the runner again, now through the forwarder.
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{ready:?}");
        let (again_n, via, again, _) = ready[0].clone();
        assert_eq!((again_n, via), (n, m));
        assert_eq!(again["forward_to"], "elsewhere");
        assert_eq!(again["op"], request["op"]);
        // A home Kerdr knows that isn't answering, whose backend holds: held for it.
        let mut app = self::app();
        let (key, id, reference, m) = waiting(&mut app);
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, other, HOLD);
        away(&mut app, other, 5);
        let name = app.machines[other].name();
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request.clone(), refused(&format!("{name}/ARK")));
        assert!(app.resend.is_empty());
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].machine, name);
        assert_eq!(app.kept.held[0].key.as_deref(), Some(key.as_str()));
        assert_eq!(app.kept.held[0].op(), request["op"].as_str().unwrap());
        assert_eq!(
            said(&app),
            format!("Held: d on {reference} goes when {name} answers (it dropped 5 s ago).")
        );
    }

    #[test]
    fn u_on_a_machine_makes_an_intent_to_update_its_herdr_and_installs_nothing() {
        let mut app = app();
        let i = app.machines.len() - 1;
        app.machines[i].conn = Conn::HerdrTooOld {
            version: "0.4.1".into(),
        };
        // No backend that makes intents: herdr's own setup, as before.
        app.mode = super::super::Mode::Machines { sel: 0 };
        let row = app.machine_rows().iter().position(|&r| r == i).unwrap();
        app.mode = super::super::Mode::Machines { sel: row };
        let before = press(&mut app, 'u');
        assert!(
            matches!(before, Action::Setup(_)) || said(&app).contains("herdr update"),
            "{before:?}"
        );
        // ark-sdlc's backend answers here: an intent, made there.
        app.machines[0].feed.as_mut().unwrap().source = Some("ark-sdlc".into());
        app.mode = super::super::Mode::Machines { sel: row };
        let Action::Mutate { machine, request } = press(&mut app, 'u') else {
            panic!("no intent: {}", said(&app))
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "create");
        let name = app.machines[i].name();
        let words = request["words"].as_str().unwrap();
        assert!(
            words.starts_with(&format!(
                "Update herdr on {name}: its herdr is 0.4.1, older than Kerdr needs."
            )),
            "{words}"
        );
        assert!(words.contains("infrastructure code"), "{words}");
        assert!(
            said(&app).contains("Kerdr installs nothing itself"),
            "{}",
            said(&app)
        );
    }

    #[test]
    fn an_account_move_its_backend_holds_says_so_on_the_agents_card() {
        let mut app = app();
        let card = app
            .board
            .cards
            .iter()
            .find(|c| !c.agents.is_empty() && c.session.is_some())
            .expect("a card with an agent in the demo")
            .clone();
        let pane = card.agents[0].pane_id.clone();
        let request = json!({"action":"account-move","pane":pane,"session":card.session,"to":"ARK_C","op":"k-1"});
        app.busy = true;
        app.finish_action(
            card.machine_index,
            request,
            ok(json!({"message": "ARK_C is out until 02:00; it moves then.", "pending": "moves to ARK_C at 02:00"})),
        );
        let card = app.card(&card.key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("⧗ moves to ARK_C at 02:00")
        );
    }

    #[test]
    fn a_held_stop_that_asks_again_waits_on_its_card() {
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        supports(&mut app, m, "cancel-backlog");
        supports(&mut app, m, "steps");
        app.kept.hold(HeldRequest {
            machine: app.machines[m].name(),
            request: crate::actions::with_op(json!({"action":"cancel","id":id})),
            rev: Some("r1".into()),
            key: Some(key.clone()),
            reference: reference.clone(),
            what: "X".into(),
            pressed: app.now().timestamp(),
            after: None,
        });
        // Andrew is elsewhere meanwhile.
        app.sel.key = None;
        back(&mut app, m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{} {:?}", said(&app), app.kept.held);
        let (n, _, request, _) = ready[0].clone();
        assert_eq!(request["held"], true);
        app.finish_job(
            n,
            request,
            Ok(Response {
                ok: false,
                confirmation: true,
                confirm: Some("tok".into()),
                message: format!("Stop {reference}'s agent? It changed since you asked."),
                ..Default::default()
            }),
        );
        assert!(app.waiting_line_for(&key).is_some(), "{}", said(&app));
        assert!(app.kept.held.is_empty());
        // His yes, pressed now, is a fresh press: neither `held` (which would only get the
        // question back) nor the old `rev` goes with it (r4-a's shape).
        select(&mut app, &key);
        assert!(app.open_waiting(&key), "{}", said(&app));
        if let super::super::Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("no yes sent: {}", said(&app))
        };
        assert_eq!(request["action"], "cancel");
        assert_eq!(request["confirm"], "tok");
        assert!(request.get("held").is_none(), "{request}");
        assert!(request.get("rev").is_none(), "{request}");
    }

    const SAVING_WORDS: &str = "saving: its number comes once the central copy answers";

    /// A new intent sent to machine 0, whose backend holds, answered as one it goes on saving in
    /// the background: its card's key and its request's op.
    fn saved_in_the_background(app: &mut App) -> (String, String) {
        supports(app, 0, HOLD);
        app.queue_action(
            0,
            json!({"action":"create","words":"Fix the feed","submission":"s-1","session":"ark"}),
        );
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        let op = request["op"].as_str().unwrap().to_string();
        app.finish_job(
            n,
            request,
            ok(json!({
                "pending": SAVING_WORDS,
                "message": "Saving it in the background.",
                "submission": "s-1",
            })),
        );
        (app.placeholder_key(0, "s-1"), op)
    }

    /// Machine 0's feed lists what its backend holds with no card yet: this save, or nothing.
    fn feed_saving(app: &mut App, op: Option<&str>) {
        app.machines[0].feed.as_mut().unwrap().held = op
            .map(|op| crate::feed::Held {
                op: op.into(),
                action: "create".into(),
                pending: SAVING_WORDS.into(),
                since: Some(app.now().timestamp()),
                until: None,
                submission: Some("s-1".into()),
                title: Some("Fix the feed".into()),
            })
            .into_iter()
            .collect();
    }

    #[test]
    fn a_new_intent_its_backend_saves_in_the_background_keeps_its_card_saying_so() {
        let mut app = app();
        let (key, op) = saved_in_the_background(&mut app);
        // No number yet: its card stays, with its backend's words.
        let card = app
            .card(&key)
            .expect("its card stays while it saves")
            .clone();
        assert_eq!(card.intake(), Some(crate::resolve::SAVING));
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("✎ {SAVING_WORDS}").as_str())
        );
        // However many reads its feed lists it in.
        feed_saving(&mut app, Some(&op));
        for _ in 0..5 {
            app.feed_read(0);
        }
        let card = app.card(&key).expect("still saving").clone();
        assert!(app.held_line_for(&card).is_some());
        // It lands: the feed has its card, and the one shown while it saved goes.
        feed_saving(&mut app, None);
        let mut landed = app.machines[0].feed.as_ref().unwrap().cards[0].clone();
        landed.id = "0700".into();
        landed.title = "Fix the feed".into();
        landed.submission = Some("s-1".into());
        landed.held.clear();
        app.machines[0].feed.as_mut().unwrap().cards.push(landed);
        app.resolver.read(0, app.machines[0].feed.as_ref());
        app.rebuild();
        assert!(app.card(&key).is_none());
    }

    #[test]
    fn a_new_intent_saving_in_the_background_dropped_with_m_gives_its_words_back() {
        let mut app = app();
        let (key, op) = saved_in_the_background(&mut app);
        select(&mut app, &key);
        press(&mut app, 'm');
        // No number to name: the drop names its op alone.
        let Action::Mutate { machine, request } = press(&mut app, 'D') else {
            panic!("no drop: {}", said(&app))
        };
        assert_eq!(machine, 0);
        assert_eq!(request, json!({"action":"drop","held":op}));
        app.queue_action(machine, request);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request,
            ok(json!({"message": "Dropped the save of \"Fix the feed\".", "dropped": true})),
        );
        assert!(app.card(&key).is_none());
        assert_eq!(app.draft, "Fix the feed");
        assert_eq!(
            said(&app),
            "Dropped the save of \"Fix the feed\". c opens it again with your words."
        );
    }

    #[test]
    fn a_new_intent_its_backend_stops_saving_goes_from_the_board_and_says_so() {
        let mut app = app();
        let (key, op) = saved_in_the_background(&mut app);
        feed_saving(&mut app, Some(&op));
        app.feed_read(0);
        // Its feed stops listing it, and no card comes for it: after three reads it goes.
        feed_saving(&mut app, None);
        app.feed_read(0);
        app.feed_read(0);
        assert!(app.card(&key).is_some());
        app.feed_read(0);
        assert!(app.card(&key).is_none());
        let name = app.machines[0].name();
        assert_eq!(
            said(&app),
            format!(
                "Your new intent \"Fix the feed\" stopped saving on {name} without getting its number; {name} keeps its words."
            )
        );
    }

    #[test]
    fn a_save_an_earlier_kerdr_sent_shows_from_its_feed_with_its_title_until_it_lands() {
        // r4-a's shape: the feed's own list names the save, with a title from his words.
        let mut app = app();
        supports(&mut app, 0, HOLD);
        app.machines[0].feed.as_mut().unwrap().held = vec![crate::feed::Held {
            op: "k-0000000000000009".into(),
            action: "create".into(),
            pending: SAVING_WORDS.into(),
            since: Some(app.now().timestamp()),
            until: None,
            submission: Some("s-9".into()),
            title: Some("Fix the feed".into()),
        }];
        app.rebuild();
        let key = app.placeholder_key(0, "s-9");
        let card = app.card(&key).expect("its card shows").clone();
        assert_eq!(card.title, "Fix the feed");
        assert_eq!(card.intake(), Some(crate::resolve::SAVING));
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("✎ {SAVING_WORDS}").as_str())
        );
        // Its words aren't here, so m lists it but D doesn't drop it, and says why.
        select(&mut app, &key);
        press(&mut app, 'm');
        assert_eq!(press(&mut app, 'D'), Action::None);
        assert!(
            said(&app).contains("isn't dropped from here"),
            "{}",
            said(&app)
        );
        // One card only, however many feeds list it.
        app.machines[1].feed.as_mut().unwrap().held =
            app.machines[0].feed.as_ref().unwrap().held.clone();
        app.rebuild();
        let saving: Vec<_> = app
            .board
            .cards
            .iter()
            .filter(|c| c.key.ends_with(":saving-s-9"))
            .collect();
        assert_eq!(saving.len(), 1);
        // It lands: the record's own card takes its place.
        let mut landed = app.machines[0].feed.as_ref().unwrap().cards[0].clone();
        landed.id = "0701".into();
        landed.submission = Some("s-9".into());
        landed.held.clear();
        app.machines[0].feed.as_mut().unwrap().cards.push(landed);
        app.rebuild();
        assert!(app.card(&key).is_none());
    }

    #[test]
    fn a_drop_its_backend_says_came_too_late_leaves_the_save_and_its_card() {
        // r4-a's shape: `state` says whether it was dropped.
        let mut app = app();
        let (key, op) = saved_in_the_background(&mut app);
        feed_saving(&mut app, Some(&op));
        let drop = json!({"action":"drop","held":op,"op":"k-2"});
        app.queue_action(0, drop);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request, ok(json!({"state": "running"})));
        assert!(app.card(&key).is_some(), "still saving");
        assert!(app.draft.is_empty(), "no words given back");
        assert_eq!(
            said(&app),
            "It had already gone ahead, so there was nothing to drop."
        );
        // `state: dropped` alone is a drop: the card goes, its words come back.
        let drop = json!({"action":"drop","held":op,"op":"k-3"});
        app.queue_action(0, drop);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request, ok(json!({"state": "dropped"})));
        assert!(app.card(&key).is_none());
        assert_eq!(app.draft, "Fix the feed");
    }

    #[test]
    fn an_account_move_its_backend_holds_is_dropped_with_account_drop_on_its_agents_machine() {
        // r4-b's shape.
        let mut app = app();
        let card = app
            .board
            .cards
            .iter()
            .find(|c| !c.agents.is_empty() && c.session.is_some())
            .expect("a card with an agent in the demo")
            .clone();
        let m = card.machine_index;
        supports(&mut app, m, HOLD);
        let pane = card.agents[0].pane_id.clone();
        let request = json!({"action":"account-move","pane":pane,"session":card.session,"to":"ARK_C","op":"k-1"});
        app.busy = true;
        app.finish_action(
            m,
            request,
            ok(json!({"state": "queued", "held": "k-1", "pending": "moves to ARK_C at 02:00"})),
        );
        select(&mut app, &card.key);
        press(&mut app, 'm');
        let Action::Mutate { machine, request } = press(&mut app, 'D') else {
            panic!("no drop: {}", said(&app))
        };
        assert_eq!(machine, m);
        assert_eq!(request, json!({"action":"account-drop","held":"k-1"}));
        app.busy = true;
        app.finish_action(
            m,
            json!({"action":"account-drop","held":"k-1","op":"k-2"}),
            ok(json!({"message": "Dropped the move to ARK_C.", "state": "dropped", "dropped": true})),
        );
        assert_eq!(said(&app), "Dropped the move to ARK_C.");
        let now = app.card(&card.key).unwrap().clone();
        assert_eq!(app.held_line_for(&now), None);
        assert!(!app.busy);
        // One a card's own `held` lists as an account request goes the same way; any other is
        // a card change's, dropped with `drop`.
        let (key, id, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        for (action, op, dropped_with) in [
            ("account-move", "k-0000000000000007", "account-drop"),
            ("dispatch", "k-0000000000000008", "drop"),
        ] {
            for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
                if c.id == id {
                    c.held = vec![crate::feed::Held {
                        op: op.into(),
                        action: action.into(),
                        pending: "moves to ARK_C at 02:00".into(),
                        ..Default::default()
                    }];
                }
            }
            app.rebuild();
            select(&mut app, &key);
            press(&mut app, 'm');
            let Action::Mutate { request, .. } = press(&mut app, 'D') else {
                panic!("no drop: {}", said(&app))
            };
            assert_eq!(request["action"], dropped_with);
            assert_eq!(request["held"], op);
        }
    }

    #[test]
    fn only_a_backend_that_holds_gets_rev_on_a_resend() {
        for holds in [false, true] {
            let mut app = app();
            let (_, _, _, m) = waiting(&mut app);
            if holds {
                supports(&mut app, m, HOLD);
            }
            d(&mut app);
            let (n, _, request, _) = app.resolver.ready()[0].clone();
            // Sent at once, it carries none; its resends carry the card's rev only to a backend
            // that holds.
            assert!(request.get("rev").is_none());
            assert!(request.get("held").is_none());
            assert_eq!(app.resolver.rev_of(n).is_some(), holds);
        }
    }

    #[test]
    fn a_change_held_for_a_machine_kerdr_doesnt_have_is_tried_through_a_forwarder_every_two_minutes()
     {
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        home(&mut app, &id, m, "elsewhere");
        supports(&mut app, m, FORWARD);
        select(&mut app, &key);
        d(&mut app);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request.clone(), ok(json!({"unreachable": "elsewhere"})));
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].machine, "elsewhere");
        // The forwarder's next read: tried again through it, same op.
        app.feed_read(m);
        let (n, via, again, _) = app.resolver.ready()[0].clone();
        assert_eq!(via, m);
        assert_eq!(again["forward_to"], "elsewhere");
        assert_eq!(again["op"], request["op"]);
        // Still unreachable: kept, and not tried again for two minutes.
        app.finish_job(n, again, ok(json!({"unreachable": "elsewhere"})));
        assert_eq!(app.kept.held.len(), 1);
        app.feed_read(m);
        assert!(app.resolver.ready().is_empty());
        let op = request["op"].as_str().unwrap().to_string();
        app.held_tried.insert(op, Instant::now() - FORWARD_AGAIN);
        app.feed_read(m);
        assert_eq!(app.resolver.ready().len(), 1);
    }
}
