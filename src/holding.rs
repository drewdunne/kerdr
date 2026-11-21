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
use crate::kept::{HeldRequest, KeptEdit, KeptImage, KeptSaving, KeptSubmission, KeptWords, Words};
use crate::model::Card;
use crate::poll::Conn;

/// A backend that holds a request, checks it again by `rev` when it's sent, and takes `drop`.
pub const HOLD: &str = "hold";
/// A backend that sends a request on to another machine it can reach (`forward_to`).
pub const FORWARD: &str = "forward";

/// How often a request held for a machine Kerdr doesn't have is tried again through one that
/// forwards.
const FORWARD_AGAIN: std::time::Duration = std::time::Duration::from_secs(120);

/// How often the words file is tried again while a new intent waits for it.
const WORDS_AGAIN: std::time::Duration = std::time::Duration::from_secs(2);

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

/// What a drop that can't get through says: nothing was dropped, why, and how to drop it once its
/// machine answers. Never 0.34.0's refusal (the end-to-end check's P3).
pub(crate) fn not_dropped(reference: &str, words: Option<&str>, machine: &str) -> String {
    match words.filter(|w| !w.trim().is_empty()) {
        Some(words) => format!(
            "{reference}: \"{words}\" wasn't dropped: {machine} can't be reached from here. Once it answers, m then D drops it."
        ),
        None => format!(
            "Nothing held for {reference} was dropped: {machine} can't be reached from here. Once it answers, m then D drops it."
        ),
    }
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

/// What a key pressed again on a card does about the same change in the hold file
/// (App::pressed_again).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Again {
    /// None there: it goes as a new change.
    Fresh,
    /// That record, claimed by this Kerdr, by its op: it goes as that one.
    Record(String),
    /// That record is on its way already, or Kerdr can't tell: nothing is sent, said why.
    Busy,
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

    /// A machine that's answering and forwards requests, other than `home`: only while `home`
    /// can't be reached for want of a connection. A host key or sign-in SSH refused there is
    /// never gone around (the brief's forwarding rule; Codex's review 60).
    pub fn forwarder(&self, home: usize) -> Option<usize> {
        let m = self.machines.get(home)?;
        if !m.conn.is_outage() {
            return None;
        }
        self.forwarder_for(Some(&m.name()))
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
            // A record from the hold file, on its way again: it waits there again as itself,
            // with its op, rather than as a second record (Codex's review 60).
            if let Some(entry) = self
                .carrying
                .as_deref()
                .and_then(|op| self.kept.find(op))
                .cloned()
            {
                self.kept.unclaim(entry.op());
                self.say_held(&entry, true);
                self.rebuild();
                return None;
            }
            let request = crate::actions::with_op(request);
            let rev = key.and_then(|k| self.card_rev(k));
            self.hold_request(name, request, key.map(str::to_string), rev, None);
            return None;
        }
        Some((machine, request))
    }

    /// A change to a card whose machine ssh couldn't reach, so nothing was sent: that machine is
    /// away after all, though the board still read it as answering (the end-to-end check's P1).
    /// It goes as one pressed while it's away does (route): through a machine that forwards, else
    /// into the hold file when its backend holds, with its op and the card's `rev` from when the
    /// key was pressed. Whether it was taken; otherwise its failure is said as before.
    fn away_after_all(&mut self, n: u64, how: crate::actions::Unreached) -> bool {
        let Some(job) = self.resolver.job(n) else {
            return false;
        };
        let machine = job.machine;
        let card_change = job.request["action"]
            .as_str()
            .is_some_and(|a| a != "create" && !a.starts_with("account-"))
            && !crate::resolve::reads(&job.request)
            && job.request.get("forward_to").is_none();
        let Some(name) = self.machines.get(machine).map(|m| m.name()) else {
            return false;
        };
        if !card_change || !self.actions_on() {
            return false;
        }
        // A host key or a sign-in its ssh refused is never gone around: held, or refused.
        let via = (how == crate::actions::Unreached::Away)
            .then(|| self.forwarder_for(Some(&name)))
            .flatten();
        if via.is_none() && !self.holds_on(machine) {
            return false;
        }
        let Some(job) = self.resolver.take(n) else {
            return false;
        };
        let mut request = job.request.clone();
        match via {
            Some(via) => {
                request["forward_to"] = Value::String(name);
                self.resend
                    .push_back((via, request, job.from.clone(), job.key.clone()));
            }
            None => self.hold_request(name, request, job.key.clone(), job.rev.clone(), None),
        }
        true
    }

    /// A key pressed again on a card whose same change is in the file, kept there because its
    /// machine's backend doesn't check one (`stays`), or on its way from a Kerdr already. Kept
    /// held, it goes as that record, claimed, with its op, and its answer takes it out of the
    /// file, so it's never left behind as a second copy. On its way, or when Kerdr can't record
    /// that it's this one's to send, nothing is sent again, said why (Codex's review 64, 1).
    pub(crate) fn pressed_again(
        &mut self,
        machine: usize,
        request: &Value,
        key: Option<&str>,
    ) -> Again {
        if self.carrying.is_some() || request.get("op").is_some() {
            return Again::Fresh;
        }
        let (Some(key), Some(name)) = (key, self.machines.get(machine).map(|m| m.name())) else {
            return Again::Fresh;
        };
        let what = held_what(request);
        let sending_here = |op: &str| self.held_sending.values().any(|o| o == op);
        let Some(entry) = self
            .kept
            .held
            .iter()
            .find(|h| {
                h.key.as_deref() == Some(key)
                    && h.machine == name
                    && h.what == what
                    && h.request["action"] == request["action"]
                    && (h.stays.is_some() || sending_here(h.op()) || self.kept.taken_elsewhere(h))
            })
            .cloned()
        else {
            return Again::Fresh;
        };
        let op = entry.op().to_string();
        let (what, reference) = (&entry.what, &entry.reference);
        if sending_here(&op) {
            self.say(
                format!("{what} on {reference} is already on its way to {name}."),
                Tone::Info,
            );
            return Again::Busy;
        }
        match self.kept.claim(&op) {
            crate::kept::Claim::Mine => Again::Record(op),
            // Answered or dropped by another Kerdr meanwhile: this press is a new change.
            crate::kept::Claim::Gone => Again::Fresh,
            crate::kept::Claim::Taken => {
                self.say(
                    format!(
                        "{what} on {reference} is already on its way to {name} from another Kerdr open now, so it isn't sent again from here."
                    ),
                    Tone::Info,
                );
                self.rebuild();
                Again::Busy
            }
            crate::kept::Claim::Unsure => {
                let why = self
                    .kept
                    .held_trouble()
                    .unwrap_or_else(|| "Kerdr couldn't write its hold file".to_string());
                self.say(
                    format!(
                        "{what} on {reference} wasn't sent: {why}, so Kerdr can't tell whether another Kerdr is sending it. Press {what} again once it can be written."
                    ),
                    Tone::Warn,
                );
                Again::Busy
            }
        }
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
        // A drop is never kept: sent later from the file it would carry Kerdr's `held: true` in
        // place of what it drops, and what it drops may run meanwhile anyway. It wasn't dropped,
        // said plainly, with what to do once that machine answers.
        if request["action"] == "drop" {
            let card = key.as_deref().and_then(|k| self.card(k));
            let reference = card
                .map(|c| c.reference.clone())
                .or_else(|| request["id"].as_str().map(|id| self.reference_for(id)))
                .unwrap_or_else(|| "its card".into());
            let target = request["held"].as_str().unwrap_or_default();
            let words = card
                .and_then(|c| c.detail.as_ref())
                .and_then(|d| d.held.iter().find(|h| h.op == target))
                .map(|h| h.pending.clone())
                .or_else(|| {
                    self.pendings
                        .iter()
                        .find(|p| p.op == target)
                        .map(|p| p.words.clone())
                });
            self.say(
                not_dropped(&reference, words.as_deref(), &machine),
                Tone::Warn,
            );
            self.rebuild();
            return;
        }
        // A record from the hold file refused because its card lives on this machine, which
        // isn't answering: it waits for it as itself, with its op, never as a second record.
        if let Some((_, op)) = self
            .redirecting
            .take_if(|(_, op)| request["op"].as_str() == Some(op.as_str()))
            && self.kept.find(&op).is_some()
        {
            let request = self.record_request(&op, &request);
            self.kept.retarget(&op, &machine, request);
            self.kept.unclaim(&op);
            if let Some(entry) = self.kept.find(&op).cloned() {
                self.say_held(&entry, true);
            }
            self.rebuild();
            return;
        }
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
            by: None,
            stays: None,
            unanswered: false,
        };
        let written = self.kept.hold(entry.clone());
        self.say_held(&entry, written);
        self.rebuild();
    }

    /// Says a change is held for its machine: "Held: d on #41 goes when LAB-Server answers (it
    /// dropped 40 s ago)." When its file couldn't be written, it's held all the same, while this
    /// Kerdr is open, and that's said (Codex's review 60).
    fn say_held(&mut self, entry: &HeldRequest, written: bool) {
        let HeldRequest {
            machine,
            what,
            reference,
            ..
        } = entry;
        let dropped = self
            .machine_index(machine)
            .and_then(|i| self.dropped_ago(i))
            .map(|ago| format!(" ({ago})"))
            .unwrap_or_default();
        match self.kept.unsaved().filter(|_| !written) {
            Some(why) => self.say(
                format!(
                    "Held: {what} on {reference} goes when {machine} answers{dropped}. {why}, so it's held only while this Kerdr is open."
                ),
                Tone::Warn,
            ),
            None => self.say(
                format!("Held: {what} on {reference} goes when {machine} answers{dropped}."),
                Tone::Info,
            ),
        }
    }

    /// Says what Kerdr noted about its files once: one it couldn't read, moved aside.
    pub fn say_kept_notes(&mut self) {
        let notes = std::mem::take(&mut self.kept.notes);
        if !notes.is_empty() {
            self.say(notes.join(" "), Tone::Warn);
        }
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
        self.restore_saving(machine);
        self.count_pending(machine);
        self.kept.sync();
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

    /// New intents an earlier Kerdr left still being saved on this machine, now that its feed is
    /// read: each shows as it did, saving, with its backend's words, and D drops it there and gives
    /// its words back. One its feed already has, saved, isn't.
    fn restore_saving(&mut self, machine: usize) {
        let Some(name) = self.machines.get(machine).map(|m| m.name()) else {
            return;
        };
        if self.machines[machine].feed.is_none() {
            return;
        }
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.saving_restored)
            .into_iter()
            .partition(|s| self.machines[machine].answers_to(&s.machine) || s.machine == name);
        self.saving_restored = rest;
        for saving in here {
            let request = saving.request;
            let submission = request["submission"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let op = request["op"].as_str().unwrap_or_default().to_string();
            let landed = self
                .machines
                .iter()
                .filter_map(|m| m.feed.as_ref())
                .any(|f| {
                    f.cards
                        .iter()
                        .any(|c| c.submission.as_deref() == Some(submission.as_str()))
                });
            if submission.is_empty() || op.is_empty() || landed || self.resolver.has_create(&op) {
                continue;
            }
            let key = self.placeholder_key(machine, &submission);
            let effect = crate::resolve::Effect::Saving(crate::resolve::Placeholder {
                id: crate::resolve::Placeholder::id_for(&submission),
                title: crate::resolve::Placeholder::title_for(
                    request["words"].as_str().unwrap_or_default(),
                ),
                machine: name.clone(),
                session: request["session"].as_str().unwrap_or("default").to_string(),
                source: self.source_of(machine),
                made: None,
                organization: request["organization"].as_str().map(str::to_string),
                tenant: request["tenant"].as_str().map(str::to_string),
                team: request["team"].as_str().map(str::to_string),
            });
            self.resolver
                .restore_saving(machine, request, key.clone(), effect);
            let words = if saving.pending.is_empty() {
                "saving: its number comes once it's saved".to_string()
            } else {
                saving.pending
            };
            self.pendings.push(Pending {
                key,
                machine,
                op,
                words,
                reads: 0,
                account: false,
            });
            self.rebuild();
        }
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

    /// Sends one held change: only once this Kerdr has it to itself in the file, since another
    /// open now may be sending it, or have dropped it (Codex's review 60). It stays in the file,
    /// marked as this Kerdr's, until it's answered.
    pub(crate) fn send_held(&mut self, machine: usize, entry: HeldRequest) {
        let op = entry.op().to_string();
        match self.kept.claim(&op) {
            crate::kept::Claim::Mine => {}
            // Its claim couldn't be recorded: another Kerdr may be sending it, so it waits, said
            // once (Codex's review 62, 3).
            crate::kept::Claim::Unsure => {
                if let Some(why) = self.kept.held_trouble()
                    && self.held_unsure.insert(op)
                {
                    self.say(
                        format!(
                            "{} on {} waits: {why}, so it can't tell whether another Kerdr is sending it. It goes once the file can be written.",
                            entry.what, entry.reference
                        ),
                        Tone::Warn,
                    );
                }
                return;
            }
            crate::kept::Claim::Taken | crate::kept::Claim::Gone => {
                self.rebuild();
                return;
            }
        }
        self.held_unsure.remove(&op);
        let Some(request) = self.held_marked(machine, &entry) else {
            let name = self.machines[machine].name();
            self.stays_held(op, &entry, &name, false);
            return;
        };
        if entry.stays.is_some() {
            self.kept.stays(&op, None);
        }
        self.held_unchecked.remove(&op);
        // A key that waited behind a start or a save when Kerdr quit: decided again now, as it
        // would have been (crate::app::behind), still the same record, with its op and its
        // markers, until it's answered (Codex's review 60).
        let behind = match entry.after.as_deref() {
            Some("start") => Some(crate::resolve::Behind::Start),
            Some(after) => after
                .strip_prefix("save:")
                .map(|s| crate::resolve::Behind::Save(s.to_string())),
            None => None,
        };
        if let Some(behind) = behind {
            let from = entry
                .key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(crate::model::CardSnapshot::of);
            let n = self
                .resolver
                .add_behind(machine, request, from, entry.key.clone(), behind);
            self.resolver.keep_op(n, &op);
            self.resolver.set_rev(n, entry.rev.clone());
            self.held_sending.insert(n, op);
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
        // Its resends carry the card's `rev` when the key was pressed, where it carries one.
        let rev = entry.rev.clone().filter(|_| request.get("rev").is_some());
        self.carrying = Some(op.clone());
        self.queue_action(machine, request);
        self.carrying = None;
        self.carry_on(before, op, rev);
    }

    /// A held change its machine's backend wouldn't check again, or, for a stop, wouldn't ask
    /// again about, stays in the file, waiting there with why (`stays`), for its card to say if
    /// its day runs out; said once. `lives`: its machine is the one a refusal said its card lives
    /// on now.
    fn stays_held(&mut self, op: String, entry: &HeldRequest, name: &str, lives: bool) {
        self.kept.unclaim(&op);
        // Only a stop that waited behind a start isn't decided again by Kerdr (held_marked).
        let stop = entry.after.is_some();
        let why = match stop {
            true => format!("{name}'s backend wouldn't ask again before stopping it"),
            false => format!("{name}'s backend doesn't check a held change again"),
        };
        self.kept.stays(&op, Some(why));
        if !self.held_unchecked.insert(op) {
            return;
        }
        let (what, reference) = (&entry.what, &entry.reference);
        let said = match (stop, lives) {
            (true, false) => format!(
                "{what} on {reference} stays held: {name}'s backend wouldn't ask again before stopping it, so Kerdr won't send the yes you gave before it last closed. Press {what} again to be asked now, or m then D drops it."
            ),
            (true, true) => format!(
                "{what} on {reference} stays held: it lives on {name} now, whose backend wouldn't ask again before stopping it, so Kerdr won't send the yes you gave before it last closed. Press {what} again to be asked now, or m then D drops it."
            ),
            (false, false) => format!(
                "{what} on {reference} stays held: {name}'s backend no longer checks a held change again, so Kerdr won't send it unchecked. Press {what} again to send it now, or m then D drops it."
            ),
            (false, true) => format!(
                "{what} on {reference} stays held: it lives on {name} now, whose backend doesn't check a held change again, so Kerdr won't send it unchecked. Press {what} again to send it now, or m then D drops it."
            ),
        };
        self.say(said, Tone::Warn);
    }

    /// The request a held change goes as (Codex's review 60): with `held: true`, and the card's
    /// `rev` when its key was pressed, wherever it ends up checked. That's the machine it's for,
    /// when that one's backend holds; and, through a forwarder, a machine Kerdr doesn't have,
    /// always, since a forwarder sends one on only to a backend that checks them (r4-a's shape).
    /// None when its machine's backend no longer checks a held change and nothing in Kerdr
    /// decides it again: it isn't sent unchecked. One that waited behind a start, a save or a
    /// write-up too is decided again by Kerdr, so it goes all the same, and with no `rev` from
    /// before, since what it waited for changed the card. Except a stop: Kerdr deciding it still
    /// makes sense isn't the question its backend asks again, so one goes only to a backend that
    /// asks its own first (`"steps"`), naming no workspace so that it does, and otherwise stays
    /// held (Codex's review 62, 1). The same goes for the machine a refusal sends it on to (Codex's
    /// review 63, 1).
    fn held_marked(&self, machine: usize, entry: &HeldRequest) -> Option<Value> {
        let mut request = entry.request.clone();
        let through = request.get("forward_to").is_some();
        if !through && !self.holds_on(machine) {
            let stops = request["action"] == "cancel";
            let decided = entry.after.is_some() && (!stops || self.steps_on(machine));
            if decided
                && stops
                && let Some(fields) = request.as_object_mut()
            {
                for field in ["session", "workspace", "tab"] {
                    fields.remove(field);
                }
            }
            return decided.then_some(request);
        }
        request["held"] = Value::Bool(true);
        if entry.after.is_none()
            && let Some(rev) = entry
                .rev
                .as_deref()
                .filter(|_| request.get("rev").is_none())
        {
            request["rev"] = Value::String(rev.to_string());
        }
        Some(request)
    }

    /// The changes ready to go, as the runner is handed them. One from the hold file is written as
    /// one that may have gone ahead (`unanswered`) before it's handed over, so a Kerdr that ends
    /// before its answer comes leaves it saying so; an answer that says it never arrived puts back
    /// what it said before (Codex's review 65, 2).
    pub(crate) fn hand_over(&mut self) -> Vec<(u64, usize, Value, bool)> {
        let mut ready = Vec::new();
        // The last moment: each request as its machine's backend takes it, `fix_input` on it
        // only for one that fixes input, or not sent at all, said as any refusal before it ran
        // (Codex's review 85, 6).
        for (n, machine, request, gated) in self.resolver.ready() {
            let request = self.with_fix_input(machine, request);
            match self.fits_backend(machine, &request) {
                Ok(()) => ready.push((n, machine, request, gated)),
                Err(why) => {
                    self.resolver.handed(n);
                    let metadata = crate::actions::request_metadata(&request);
                    self.finish_job(n, metadata, Err(why));
                }
            }
        }
        for (n, ..) in &ready {
            let Some(op) = self.held_sending.get(n).cloned() else {
                continue;
            };
            if !self.kept.find(&op).is_some_and(|h| h.unanswered) {
                self.held_first_go.insert(op.clone());
            }
            self.kept.unanswered(&op, true);
        }
        ready
    }

    /// A change from the hold file whose answer says it never arrived, or was refused before
    /// anything was done: if this was the first time it may have reached its machine, it's marked
    /// as not sent again.
    fn held_not_arrived(&mut self, op: &str) {
        if self.held_first_go.remove(op) {
            self.kept.unanswered(op, false);
        }
    }

    /// Before a change's answer is taken: one sent from the hold file leaves it once answered;
    /// one a forwarding machine couldn't get through is held for the machine it was for; and one
    /// its backend holds says so on its card. Whether the answer is dealt with here.
    pub(crate) fn held_answer(
        &mut self,
        n: u64,
        result: &Result<Response, String>,
        unreached: Option<crate::actions::Unreached>,
    ) -> bool {
        let from_file = self.held_sending.remove(&n);
        let response = result.as_ref().ok();
        if let Some(op) = &from_file {
            match response {
                // Its forwarder couldn't get through, or no answer: it waits in the file again,
                // for any Kerdr to send with the same op (Codex's review 60).
                Some(r) if r.unreachable.is_some() => {
                    self.held_not_arrived(op);
                    self.kept.unclaim(op);
                }
                // Refused because its card lives on another machine: it stays in the file while
                // the answer is taken, which may send it on there (redirected).
                Some(r) if r.sends_on() => {
                    self.held_not_arrived(op);
                    self.redirecting = Some((n, op.clone()));
                }
                Some(_) => {
                    self.held_first_go.remove(op);
                    self.kept.release(op);
                }
                // No answer back, though it may have reached its machine: if its day runs out its
                // card says it may have gone ahead, never that it wasn't sent (Codex's reviews
                // 64, 4, and 65, 2). Nothing sent, it's as it was.
                None => {
                    match result {
                        Err(e) if crate::actions::result_unknown(e) => {
                            self.held_first_go.remove(op);
                            self.kept.unanswered(op, true);
                        }
                        _ => self.held_not_arrived(op),
                    }
                    self.kept.unclaim(op);
                }
            }
        }
        // Its machine couldn't be reached, so nothing was sent (the end-to-end check's P1): it's
        // away, though the board hadn't seen it yet, its loop standing still while an editor was
        // open, say. One from the file waits there again, as itself; any other goes as a change
        // pressed while its machine is away does (away_after_all).
        if let Some(how) = unreached.filter(|_| result.is_err()) {
            if from_file.is_some() {
                self.resolver.take(n);
                self.rebuild();
                return true;
            }
            if self.away_after_all(n, how) {
                return true;
            }
        }
        if let Some(to) = response.and_then(|r| r.unreachable.clone()) {
            let Some(job) = self.resolver.take(n) else {
                return false;
            };
            let mut request = job.request.clone();
            if let Some(fields) = request.as_object_mut() {
                fields.remove("forward_to");
                // Kerdr's own mark alone: a drop's `held` names what it drops.
                if fields.get("held").is_some_and(Value::is_boolean) {
                    fields.remove("held");
                }
            }
            if from_file.is_none() {
                self.hold_request(to, request, job.key.clone(), job.rev.clone(), None);
            } else {
                self.rebuild();
            }
            return true;
        }
        // A start or reopen its backend already holds, queued for him, answered as not done so an
        // older Kerdr says it didn't start (r4-b's shape): held, with its words on the card, not
        // a failure.
        if let Some(r) = response
            && !r.ok
            && !r.confirmation
            && r.home.is_none()
            && r.unreachable.is_none()
            && r.retry.is_none()
            && let Some(words) = r.pending.clone().filter(|w| !w.is_empty())
            && let Some(job) = self.resolver.job(n)
            && matches!(job.request["action"].as_str(), Some("dispatch" | "reopen"))
            && let Some(key) = job.key.clone()
        {
            let op = job.request["op"].as_str().unwrap_or_default().to_string();
            let machine = job.machine;
            let message = r.message.clone();
            self.resolver.take(n);
            self.pendings.retain(|p| !(p.key == key && p.op == op));
            self.pendings.push(Pending {
                key: key.clone(),
                machine,
                op,
                words: words.clone(),
                reads: 0,
                account: false,
            });
            let said = if message.trim().is_empty() {
                let reference = self
                    .card(&key)
                    .map(|c| c.reference.clone())
                    .unwrap_or_default();
                format!("{reference}: {words}.")
            } else {
                message
            };
            self.say(said, Tone::Info);
            self.rebuild();
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

    /// Once the answer is taken that refused a hold-file record because its card lives on
    /// another machine: it stays the same record, with its op, until what became of it is answered
    /// or dropped, so a crash meanwhile can't lose it. Sent on there, it's for that machine now
    /// and its change carries it; waiting for its two copies to agree, it's this Kerdr's still,
    /// for the machine the refusal named; held for that machine, it waits in the file
    /// (hold_request). Not sent on, said why, it was answered, and it leaves the file.
    pub(crate) fn redirected(&mut self) {
        let Some((n, op)) = self.redirecting.take() else {
            return;
        };
        if self.kept.find(&op).is_none() {
            return;
        }
        if let Some(job) = self.resolver.job(n) {
            let (to, sent) = (job.machine, job.request.clone());
            let through = sent.get("forward_to").is_some();
            let name = sent["forward_to"]
                .as_str()
                .map(str::to_string)
                .or_else(|| self.machines.get(to).map(|m| m.name()))
                .unwrap_or_default();
            let request = self.record_request(&op, &sent);
            self.kept.retarget(&op, &name, request);
            let Some(mut entry) = self.kept.find(&op).cloned() else {
                return;
            };
            if through {
                entry.request["forward_to"] = Value::String(name.clone());
            }
            // Checked and marked as that machine's backend takes it, as from the file: one it
            // wouldn't check again, or a stop it wouldn't ask again about, isn't sent there
            // (Codex's review 60, 11, and review 63, 1).
            match self.held_marked(to, &entry) {
                Some(request) => {
                    self.resolver.reshape(n, request);
                    self.held_sending.insert(n, op);
                }
                None => {
                    let key = job.key.clone();
                    self.resolver.take(n);
                    self.resent
                        .retain(|(m, k)| !(*m == to && Some(k) == key.as_ref()));
                    self.stays_held(op, &entry, &name, true);
                    self.rebuild();
                }
            }
            return;
        }
        let named = self
            .catch_up
            .disagreed
            .iter()
            .find(|d| d.request["op"].as_str() == Some(op.as_str()))
            .map(|d| (d.named, d.request.clone()));
        if let Some((named, sent)) = named {
            let name = self.machines[named].name();
            let request = self.record_request(&op, &sent);
            self.kept.retarget(&op, &name, request);
            return;
        }
        self.kept.release(&op);
    }

    /// What a hold-file record keeps of the request it went as: without what each send adds,
    /// `held`, `forward_to`, and a `rev` the record keeps apart.
    pub(crate) fn record_request(&self, op: &str, sent: &Value) -> Value {
        let had_rev = self
            .kept
            .find(op)
            .is_some_and(|h| h.request.get("rev").is_some());
        let mut request = sent.clone();
        if let Some(fields) = request.as_object_mut() {
            fields.remove("held");
            fields.remove("forward_to");
            if !had_rev {
                fields.remove("rev");
            }
        }
        request
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
        let behind = self.behind_lines(c);
        for h in self.kept.for_card(&c.key) {
            // One on its way again behind its start, its save or its write-up shows as that.
            let carrier = self.carrier(h.op());
            if carrier.is_some_and(|n| behind.iter().any(|(_, b)| *b == n)) {
                continue;
            }
            let words = if carrier.is_some() {
                format!("{} is on its way to {}", h.what, h.machine)
            } else if self.kept.taken_elsewhere(h) {
                format!(
                    "{} is on its way to {} from another Kerdr",
                    h.what, h.machine
                )
            } else {
                format!("{} waits for {}", h.what, h.machine)
            };
            lines.push(HeldLine {
                words,
                since: Some(h.pressed),
                drop: Some(DropOf::Kerdr(h.op().to_string())),
            });
        }
        for (words, n) in behind {
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
                .is_some_and(|s| !self.resolver.has_submission(s));
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

    /// The peek's key to drop what's held: D for the first (None), or with a count the one of
    /// that number (2D, the second), as the peek's hint says.
    pub(crate) fn drop_held(&mut self, index: Option<usize>) -> super::Action {
        let Some(card) = self.selected().cloned() else {
            return super::Action::None;
        };
        let droppable: Vec<HeldLine> = self
            .held_lines(&card)
            .into_iter()
            .filter(|l| l.drop.is_some())
            .collect();
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
            // By where it is (Codex's review 60): one this Kerdr is sending is dropped as any
            // change on its way is; one waiting in the file only if no other Kerdr open now has
            // it.
            Some(DropOf::Kerdr(op)) => {
                if let Some(n) = self.carrier(&op) {
                    self.drop_behind(n, &card);
                    return super::Action::None;
                }
                let what = self
                    .kept
                    .find(&op)
                    .map(|h| h.what.clone())
                    .unwrap_or_else(|| "the change".into());
                let reference = &card.reference;
                let said = match self.kept.drop_waiting(&op) {
                    crate::kept::Claim::Mine => {
                        // One waiting for its copies to agree isn't sent once they do.
                        self.catch_up
                            .disagreed
                            .retain(|d| d.request["op"].as_str() != Some(op.as_str()));
                        format!("Dropped: {what} on {reference} won't be sent.")
                    }
                    crate::kept::Claim::Taken => format!(
                        "{what} on {reference} is on its way from another Kerdr open now, so it can't be dropped from here."
                    ),
                    crate::kept::Claim::Gone => format!(
                        "{what} on {reference} isn't held any more: another Kerdr sent it or dropped it."
                    ),
                    // Nothing recorded, so nothing dropped (Codex's review 62, 3).
                    crate::kept::Claim::Unsure => format!(
                        "{what} on {reference} wasn't dropped: {}. Press D again once it can be written.",
                        self.kept
                            .held_trouble()
                            .unwrap_or_else(|| "Kerdr couldn't write its hold file".into())
                    ),
                };
                self.say(said, Tone::Info);
                self.rebuild();
                super::Action::None
            }
            Some(DropOf::Backend(op)) => self.drop_backend(&card, &op, &line.words),
            Some(DropOf::AccountBackend(op)) => {
                self.drop_account_backend(card.machine_index, &card.key, &op, &line.words)
            }
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

    /// The change on its way that a hold-file record is, by its number: this Kerdr sent it.
    pub(crate) fn carrier(&self, op: &str) -> Option<u64> {
        self.held_sending
            .iter()
            .find(|(_, o)| o.as_str() == op)
            .map(|(n, _)| *n)
    }

    /// Why a drop answered with `home` wasn't dropped: the machine that answer names holds what it
    /// drops, and this Kerdr can't reach it (r4-a's round 4). Said in the same shape as a drop that
    /// can't get through, never as the card's record having moved.
    pub(crate) fn drop_unreached(
        &self,
        key: &str,
        reference: &str,
        request: &Value,
        host: &str,
    ) -> String {
        let target = request["held"].as_str().unwrap_or_default();
        let held = self
            .card(key)
            .and_then(|c| c.detail.as_ref())
            .and_then(|d| d.held.iter().find(|h| h.op == target));
        let what = match held {
            Some(h) if matches!(h.action.as_str(), "dispatch" | "reopen") => {
                "queued start".to_string()
            }
            Some(h) if !h.pending.is_empty() => format!("\"{}\"", h.pending),
            _ => "held request".to_string(),
        };
        format!(
            "{reference}'s {what} is held on {host}, which this Kerdr can't reach, so it wasn't dropped. Once {host} answers, m then D drops it."
        )
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
        // A change to a card: for a machine that isn't answering it goes through one that
        // forwards, or is said as not dropped (hold_request), never refused as 0.34.0 refused it
        // (the end-to-end check's P3).
        if !self.can_change(machine) {
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
    pub(crate) fn drop_account_backend(
        &mut self,
        machine: usize,
        key: &str,
        op: &str,
        words: &str,
    ) -> super::Action {
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
        // Its machine isn't answering: an account request is never held or sent on, so it's said
        // as not dropped, as a card's drop is (the end-to-end check's P3).
        if self.actions_on() && !self.machines[machine].conn.is_live() {
            let reference = self
                .card(key)
                .map(|c| c.reference.clone())
                .unwrap_or_else(|| "its card".into());
            let name = self.machines[machine].name();
            self.say(not_dropped(&reference, Some(words), &name), Tone::Warn);
            return super::Action::None;
        }
        if !self.can_write(machine) {
            return super::Action::None;
        }
        self.action_key = Some(key.to_string());
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

    /// The machine that makes Kerdr's own intents (u's herdr update, S's owner): the first that's
    /// answering whose backend is ark-sdlc's. None without one, or with no actions backend.
    pub(crate) fn intent_maker(&self) -> Option<usize> {
        if !self.actions_on() {
            return None;
        }
        self.machines.iter().position(|m| {
            m.conn.is_live()
                && m.feed
                    .as_ref()
                    .is_some_and(|f| f.source.as_deref() == Some("ark-sdlc"))
        })
    }

    /// u on a machine's row: an intent to update herdr there, made by a machine whose backend is
    /// ark-sdlc's, this one first. It's done through that machine's infrastructure code, or
    /// omarchy-config on an Omarchy machine: Kerdr installs nothing itself. None when no such
    /// backend answers.
    pub(crate) fn herdr_intent(&mut self, i: usize) -> Option<super::Action> {
        let at = self.intent_maker()?;
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
            edit: None,
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
        // A save of an item's text not answered yet: one whose machine turned out to be away is
        // held only once its answer says so, so q waits the moment that takes (the end-to-end
        // check's P1).
        if let Some(first) = self.resolver.saving_text().first() {
            let reference = first
                .id()
                .map(|id| self.reference_for(id))
                .unwrap_or_else(|| "a card".into());
            return Some(format!("{reference}'s new text is saved or held"));
        }
        // A change whose copies disagreed, sent once more when they agree (K1-53): within two
        // minutes, it goes or is said (Codex's review 60).
        if let Some(d) = self.catch_up.disagreed.first() {
            return Some(format!(
                "{}'s copies agree and it's sent again",
                self.reference_for(&d.id)
            ));
        }
        // Account requests waiting their turn behind the one on its way go too (Codex's review
        // 60): only one waiting for a machine that isn't answering is said as that.
        if self.busy
            || !self.account_queue.is_empty()
            || self.account_later.iter().any(|(p, _)| p.away.is_none())
        {
            return Some("the account change answers".into());
        }
        // An account request waiting for its machine, which only this Kerdr keeps (intent 0591,
        // release 4): a second q drops it.
        self.account_away_wait()
    }

    /// q: whether Kerdr quits now, what it keeps for the next one kept. Otherwise it says what
    /// it waits for, and quits by itself once that's done (quit_due); Esc stays. A second q quits
    /// at once when what's left can be kept: an account change, or a change whose backend holds
    /// requests, which goes into the hold file with its op. When what it keeps can't be written,
    /// it says so and stays, and a second q quits anyway (Codex's review 60).
    pub fn quit_key(&mut self) -> bool {
        if self.quit_unsaved && self.quitting.is_some() {
            let _ = self.keep_on_quit();
            return true;
        }
        let Some(what) = self.quit_waits_for() else {
            return self.quit_now();
        };
        if self.quitting.is_some() {
            self.hold_unsent();
            // Only saves of an item's text left, for backends that don't hold: their words are kept
            // as your version, which e opens, so quitting loses nothing.
            let texts_only = self.resolver.holding_quit().is_empty()
                && self.send_after_load.is_none()
                && self.catch_up.disagreed.is_empty()
                && !self.resolver.saving_text().is_empty();
            match self.quit_waits_for() {
                None => return self.quit_now(),
                Some(_) if texts_only => return self.quit_now(),
                Some(w)
                    if w == "the account change answers"
                        || self.account_away_wait().as_deref() == Some(w.as_str()) =>
                {
                    return self.quit_now();
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
        self.quitting.is_some() && !self.quit_unsaved && self.quit_waits_for().is_none()
    }

    /// Quits once what Kerdr keeps for the next one is written: whether it may. When it can't be
    /// written, Kerdr stays open with it, says why, and a second q quits anyway (Codex's review
    /// 60).
    pub fn quit_now(&mut self) -> bool {
        let Err(why) = self.keep_on_quit() else {
            return true;
        };
        self.quitting = Some(Instant::now());
        self.quit_unsaved = true;
        self.say(
            format!(
                "{why}, so quitting now would lose what Kerdr keeps for the next one: your words, and changes it holds. q again quits anyway, Esc stays."
            ),
            Tone::Warn,
        );
        false
    }

    /// Changes not yet in their command's hands, for a machine whose backend holds requests,
    /// go into the hold file with their op, for the next Kerdr to send. So does one whose copies
    /// disagreed, for the home its last refusal named (Codex's review 60). Each is this Kerdr's
    /// until it has gone (Codex's review 62, 6).
    fn hold_unsent(&mut self) {
        let me = self.kept.me().to_string();
        let disagreed = std::mem::take(&mut self.catch_up.disagreed);
        for d in disagreed {
            if !self.holds_on(d.named) || d.request["op"].as_str().is_none_or(str::is_empty) {
                self.catch_up.disagreed.push(d);
                continue;
            }
            let machine = self.machines[d.named].name();
            // One from the hold file waits there again as itself, for that machine.
            let op = d.request["op"].as_str().unwrap_or_default().to_string();
            if self.kept.find(&op).is_some() {
                let request = self.record_request(&op, &d.request);
                self.kept.retarget(&op, &machine, request);
                continue;
            }
            self.kept.hold(HeldRequest {
                machine,
                reference: self.reference_for(&d.id),
                what: held_what(&d.request),
                request: d.request,
                rev: None,
                key: Some(d.key),
                pressed: self.epoch_now(),
                after: None,
                by: Some(me.clone()),
                stays: None,
                unanswered: false,
            });
        }
        // With saves of an item's text not answered yet (the end-to-end check's P1): held with
        // their op, so if one did land, its backend answers the next send from its record.
        let unsent: Vec<u64> = self
            .resolver
            .holding_quit()
            .into_iter()
            .chain(self.resolver.saving_text())
            .filter(|j| j.request["action"] != "create" && self.holds_on(j.machine))
            .map(|j| j.n)
            .collect();
        for n in unsent {
            if let Some(job) = self.resolver.take(n) {
                // Its command had all of it: it may have gone ahead.
                let unanswered = job.handed;
                // One from the hold file is there already, as itself (Codex's review 60).
                if let Some(op) = self.held_sending.remove(&n)
                    && self.kept.find(&op).is_some()
                {
                    continue;
                }
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
                    by: Some(me.clone()),
                    stays: None,
                    unanswered,
                });
            }
        }
    }

    /// Esc while q waits: Kerdr stays open.
    pub(crate) fn stay(&mut self) -> bool {
        self.quit_unsaved = false;
        if self.quitting.take().is_none() {
            return false;
        }
        self.say("Staying open.", Tone::Info);
        true
    }

    /// As Kerdr quits: starts waiting for their write-up go into the hold file, for the next
    /// Kerdr to send once that's done, and your words into the words file. Err, saying what
    /// couldn't be written, when either can't be: what it added to the hold file is taken out
    /// again, since Kerdr stays open with it (Codex's review 60). Each stays this Kerdr's in the
    /// file until it has really gone: its lock then lets go, and the next Kerdr takes them. So no
    /// other Kerdr sends one while this one may still (Codex's review 62, 6).
    pub fn keep_on_quit(&mut self) -> Result<(), String> {
        let before: Vec<String> = self.kept.held.iter().map(|h| h.op().to_string()).collect();
        self.hold_for_next();
        let added: Vec<String> = self
            .kept
            .held
            .iter()
            .map(|h| h.op().to_string())
            .filter(|op| !before.contains(op))
            .collect();
        let words = self.words();
        // The words only once the hold file is whole: Kerdr may yet stay open with both.
        let failed = match self.kept.flush() {
            Some(why) => Some(why),
            None => self.kept.keep_words(&words).err(),
        };
        let Some(why) = failed else {
            return Ok(());
        };
        for op in added {
            self.kept.release(&op);
        }
        Err(why)
    }

    /// What waits in Kerdr itself goes into the hold file as Kerdr quits: starts and changes
    /// waiting for their write-up, and keys held behind a start or a save, which the next Kerdr
    /// decides again (intent 0591, release 4). One that came from the hold file is there already,
    /// as itself (Codex's review 60). Each is this Kerdr's until it has gone (Codex's review 62,
    /// 6).
    fn hold_for_next(&mut self) {
        let mut waiting: Vec<(u64, String, &'static str)> = self
            .resolver
            .write_ups_awaited()
            .iter()
            .map(|j| (j.n, "write-up".to_string(), "write-up"))
            .collect();
        waiting.extend(
            self.resolver
                .behind_jobs()
                .into_iter()
                .map(|(n, behind)| match behind {
                    crate::resolve::Behind::Start => (n, "start".to_string(), "start"),
                    crate::resolve::Behind::Save(s) => (n, format!("save:{s}"), "save"),
                }),
        );
        let me = self.kept.me().to_string();
        for (n, after, kind) in waiting {
            if self
                .held_sending
                .get(&n)
                .is_some_and(|op| self.kept.find(op).is_some())
            {
                continue;
            }
            let Some(j) = self.resolver.job(n) else {
                continue;
            };
            let Some(machine) = self.machines.get(j.machine).map(|m| m.name()) else {
                continue;
            };
            let (request, key) = (j.request.clone(), j.key.clone());
            // A start waiting for its write-up keeps the rev it has; a key behind a start or a
            // save is decided again by the next Kerdr.
            let rev = j.rev.clone().filter(|_| kind == "write-up");
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
                after: Some(after),
                by: Some(me.clone()),
                stays: None,
                unanswered: false,
            });
        }
    }

    /// The send words came back from, as the words file keeps it.
    fn kept_submission(&self, s: &super::Submission) -> Option<KeptSubmission> {
        Some(KeptSubmission {
            id: s.id.clone(),
            machine: self.machines.get(s.machine)?.name(),
            session: s.session.clone(),
            organization: s.filed.organization.clone(),
            tenant: s.filed.tenant.clone(),
            team: s.filed.team.clone(),
            unknown: s.unknown,
        })
    }

    /// That send again, for these words, on a machine Kerdr still has.
    fn submission_from(
        &self,
        k: &KeptSubmission,
        words: &str,
        images: &[crate::images::Image],
    ) -> Option<super::Submission> {
        Some(super::Submission {
            id: k.id.clone(),
            words: words.to_string(),
            images: images.to_vec(),
            machine: self.machine_index(&k.machine)?,
            session: k.session.clone(),
            filed: super::Filed {
                organization: k.organization.clone(),
                tenant: k.tenant.clone(),
                team: k.team.clone(),
            },
            unknown: k.unknown,
        })
    }

    /// Your words, as the words file keeps them: each with the send it came back from, so sent
    /// again it goes as that send (Codex's review 60), and the new intents still being saved in
    /// their backend, so a later Kerdr can still drop them.
    pub fn words(&self) -> Words {
        let kept = |words: &str,
                    images: &[crate::images::Image],
                    from: Option<&super::Submission>| KeptWords {
            words: words.to_string(),
            images: images.iter().map(KeptImage::of).collect(),
            from: from.and_then(|s| self.kept_submission(s)),
        };
        let mut words = Words::default();
        if !self.draft.trim().is_empty() {
            words.draft = Some(kept(
                &self.draft,
                &self.draft_images,
                self.draft_came_back.as_ref(),
            ));
        }
        for r in &self.returned {
            words
                .returned
                .push(kept(&r.words, &r.images, r.from.as_ref()));
        }
        // Words sent once their image is in, which quitting would drop: given back next time.
        if let Some((_, draft)) = &self.send_after_load {
            words
                .returned
                .push(kept(&draft.words, &draft.images, draft.came_back.as_ref()));
        }
        // A new intent on its way, unanswered: given back as the send it is, so sent again it
        // finds what that send may have made rather than making a second (Codex's review 62, 2).
        for job in self.resolver.unanswered_creates() {
            let request = &job.request;
            let Some(submission) = request["submission"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let Some(machine) = self.machines.get(job.machine).map(|m| m.name()) else {
                continue;
            };
            let sent = self.submissions.iter().find(|s| s.id == submission);
            let field = |name: &str| request[name].as_str().map(str::to_string);
            words.returned.push(KeptWords {
                words: sent
                    .map(|s| s.words.clone())
                    .or_else(|| field("words"))
                    .unwrap_or_default(),
                images: sent
                    .map(|s| s.images.iter().map(KeptImage::of).collect())
                    .unwrap_or_default(),
                from: Some(KeptSubmission {
                    id: submission.to_string(),
                    machine,
                    session: field("session").unwrap_or_else(|| "default".into()),
                    organization: field("organization"),
                    tenant: field("tenant"),
                    team: field("team"),
                    unknown: true,
                }),
            });
        }
        for job in self.resolver.saving_in_background() {
            let Some(machine) = self.machines.get(job.machine).map(|m| m.name()) else {
                continue;
            };
            let op = job.request["op"].as_str().unwrap_or_default();
            let pending = self
                .pendings
                .iter()
                .find(|p| p.op == op)
                .map(|p| p.words.clone())
                .unwrap_or_default();
            words.saving.push(KeptSaving {
                machine,
                request: job.request.clone(),
                pending,
            });
        }
        words.saving.extend(self.saving_restored.iter().cloned());
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
                    as_written: self.as_written.contains(key),
                    in_box: false,
                })
            })
            .collect();
        // An edit open in Kerdr's own box: what the box holds now is that card's version, kept
        // with the text it was read as and its fingerprint (Codex's review 85, 3).
        if let Some((key, edit)) = self.open_edit_box()
            && let Some(held) = self.edits.get(&key)
            && let Some(machine) = self.machines.get(held.machine).map(|m| m.name())
        {
            edits.retain(|e| e.key != key);
            edits.push(KeptEdit {
                key,
                id: held.id.clone(),
                machine,
                current: held.current.clone(),
                was: held.was.clone(),
                text: edit,
                from: held
                    .mine
                    .as_ref()
                    .map_or_else(|| held.current.clone(), |m| m.from.clone()),
                as_written: false,
                in_box: true,
            });
        }
        edits.sort_by(|a, b| a.key.cmp(&b.key));
        words.edits = edits;
        words
    }

    /// Your words into the words file as they are now, while this Kerdr is open, when they've
    /// changed since it last wrote them: a new intent sent goes as the send it is, one saved or
    /// dropped goes from it, and the words this Kerdr took when it opened are replaced. So a crash
    /// never gives back words already saved as new words, which would make a second intent
    /// (Codex's review 62, 2).
    pub(crate) fn keep_words_now(&mut self) -> bool {
        if !self.kept.on_disk() {
            self.resolver.words_kept();
            return true;
        }
        let words = self.words();
        let kept = self.words_kept.as_ref() == Some(&words)
            || match self.kept.hold_words(&words) {
                Ok(()) => {
                    self.words_kept = Some(words);
                    true
                }
                Err(_) => false,
            };
        // A new intent that waited for its send to be kept goes now (Codex's review 63, 2).
        if kept {
            self.resolver.words_kept();
        }
        kept
    }

    /// The edit open in Kerdr's own box, if one is: its card's key and the text the box holds, read
    /// back as a save would read it.
    pub(crate) fn open_edit_box(&self) -> Option<(String, String)> {
        let super::Mode::Compose { draft, .. } = &self.mode else {
            return None;
        };
        let edit = draft.edit.as_ref()?;
        Some((
            edit.key.clone(),
            crate::edit::read_back(&draft.words, edit.marker.as_deref()),
        ))
    }

    /// While an edit is open in Kerdr's own box, what it holds is in the words file, written again
    /// when it has changed, every two seconds at most, so a Kerdr killed before ⏎ or Esc brings it
    /// back as that card's edit (Codex's review 85, 3).
    pub(crate) fn keep_open_edit(&mut self) {
        let Some((key, text)) = self.open_edit_box() else {
            return;
        };
        let kept = self.words_kept.as_ref().is_some_and(|w| {
            w.edits
                .iter()
                .any(|e| e.in_box && e.key == key && e.text == text)
        });
        if kept
            || self
                .words_tried
                .is_some_and(|at| at.elapsed() < WORDS_AGAIN)
        {
            return;
        }
        self.words_tried = Some(Instant::now());
        self.keep_words_now();
    }

    /// A new intent waiting for the words file to be written: tried again every two seconds.
    pub(crate) fn keep_waiting_words(&mut self) {
        if !self.resolver.waits_for_words()
            || self
                .words_tried
                .is_some_and(|at| at.elapsed() < WORDS_AGAIN)
        {
            return;
        }
        self.words_tried = Some(Instant::now());
        self.keep_words_now();
    }

    /// Words the last Kerdr kept: the box's draft back in the box (c), words given back after it,
    /// and your versions of item texts, which e opens, reading the text first to see whether a
    /// save of them landed meanwhile.
    pub fn restore_words(&mut self, words: Words) {
        let images = |kept: &[KeptImage]| -> Vec<crate::images::Image> {
            kept.iter().filter_map(KeptImage::image).collect()
        };
        if let Some(draft) = words.draft {
            let pictures = images(&draft.images);
            let from = draft
                .from
                .as_ref()
                .and_then(|k| self.submission_from(k, &draft.words, &pictures));
            if self.draft.trim().is_empty() {
                self.draft = draft.words;
                self.draft_images = pictures;
                self.draft_came_back = from;
            } else {
                self.returned.push_back(super::Returned {
                    words: draft.words,
                    images: pictures,
                    from,
                });
            }
        }
        for r in words.returned {
            let pictures = images(&r.images);
            let from = r
                .from
                .as_ref()
                .and_then(|k| self.submission_from(k, &r.words, &pictures));
            self.returned.push_back(super::Returned {
                words: r.words,
                images: pictures,
                from,
            });
        }
        // New intents still being saved in their backend: back on the board once their
        // machine's feed is read, so D can still drop them (Codex's review 60).
        self.saving_restored.extend(words.saving);
        for e in words.edits {
            let Some(machine) = self.machine_index(&e.machine) else {
                continue;
            };
            if e.as_written {
                self.as_written.insert(e.key.clone());
            }
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

    /// Every file under `dir`, with its text, however deep.
    #[cfg(unix)]
    fn every_file(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
        let mut out = vec![];
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(every_file(&path));
            } else {
                let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
                out.push((path, text));
            }
        }
        out
    }

    #[cfg(unix)]
    #[test]
    fn a_token_typed_while_offline_is_kept_only_in_the_private_words_file_and_sent_whole() {
        // Codex's review 85, secrets: a new intent, a text and a Kickoff answer, each with a line
        // like a token, made while their machine doesn't answer: the change and the answer are
        // held, the new intent's words kept in the box. Every file Kerdr writes (the hold file,
        // the words file, the error log, its locks and state) is read: the token is in the words
        // file alone, readable by him alone, and every request goes whole once it answers.
        use std::os::unix::fs::PermissionsExt;
        const TOKEN: &str = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8";
        let state = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(state.path(), app.now().timestamp());
        app.error_log = crate::errlog::ErrorLog::at(state.path().join("errors.log"));
        let (_key, id, _reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        supports(&mut app, m, crate::images::FIX_INPUT);
        away(&mut app, m, 30);
        // The text and the answer are held for it.
        let edit = json!({"action":"edit","id":id,"text":format!("Deploy it.\ntoken = {TOKEN}"),
            "was":"sha256:aa","base":"Deploy it.","from":"here"});
        app.queue_action(m, edit);
        let answer = json!({"action":"answer","id":id,"asked":"fedcba9876543210",
            "answers":[{"num":"1","text":format!("the key is {TOKEN}")}]});
        app.queue_action(m, answer);
        assert!(app.resolver.ready().is_empty());
        assert_eq!(app.kept.held.len(), 2, "both held: {:?}", app.message());
        // The new intent's words stay in the box, kept in the words file when it closes.
        press(&mut app, 'c');
        if let super::super::Mode::Compose { draft, .. } = &mut app.mode {
            draft.machine = m;
        }
        for c in format!("Use {TOKEN} to deploy").chars() {
            press(&mut app, c);
        }
        press_code(&mut app, KeyCode::Enter);
        press_code(&mut app, KeyCode::Esc);
        assert!(app.draft.contains(TOKEN));
        app.keep_words_now();
        let only_in_words = |when: &str| {
            let files = every_file(state.path());
            assert!(
                files.iter().any(|(p, _)| p.ends_with("held.json")),
                "{when}: no hold file"
            );
            for (path, text) in &files {
                let name = path.file_name().unwrap().to_string_lossy();
                if name == "words.json" {
                    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
                    assert_eq!(mode, 0o600, "{when}: words.json is {mode:o}");
                    assert!(
                        text.contains(TOKEN),
                        "{when}: the words file keeps his words"
                    );
                } else {
                    assert!(
                        !text.contains(TOKEN),
                        "{when}: the token is in {}",
                        path.display()
                    );
                }
            }
        };
        only_in_words("while offline");
        // Back: what was held goes whole, and so do the new intent's words.
        back(&mut app, m);
        let handed = app.hand_over();
        let texts: Vec<String> = handed.iter().map(|(_, _, r, _)| r.to_string()).collect();
        assert_eq!(handed.len(), 2, "{texts:?}");
        assert!(
            texts.iter().all(|t| t.contains(TOKEN)),
            "sent whole: {texts:?}"
        );
        press(&mut app, 'c');
        if let super::super::Mode::Compose { draft, .. } = &mut app.mode {
            draft.machine = m;
        }
        let Action::Mutate { machine, request } = press_code(&mut app, KeyCode::Enter) else {
            panic!("the new intent wasn't sent: {:?}", app.message())
        };
        app.queue_action(machine, request);
        let handed = app.hand_over();
        assert!(
            handed
                .iter()
                .any(|(_, _, r, _)| r["words"].as_str().is_some_and(|w| w.contains(TOKEN)))
        );
        app.keep_words_now();
        only_in_words("once sent");
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

    /// The answer to change `n` when ssh's own words, with its command never started, are
    /// `stderr`: as the runner hands the board that answer, and how its machine wasn't reached.
    fn ssh_said(app: &mut App, n: u64, request: Value, stderr: &str) {
        let output = crate::transport::Output {
            status: Some(255),
            stderr: stderr.into(),
            started: Some(false),
            ..Default::default()
        };
        let name = app.machines[app.resolver.job(n).unwrap().machine].name();
        let (result, unreached) = crate::actions::over_ssh_answer(&output, &request, &name);
        app.answer_unreached = unreached;
        app.finish_job(n, request, result);
    }

    /// The same, when ssh couldn't connect: nothing was sent.
    fn unreached(app: &mut App, n: u64, request: Value) {
        let name = app.machines[app.resolver.job(n).unwrap().machine].name();
        ssh_said(
            app,
            n,
            request,
            &format!("ssh: connect to host {name} port 22: No route to host\n"),
        );
    }

    #[test]
    fn a_change_whose_machine_couldnt_be_reached_after_all_is_held_or_forwarded() {
        // The end-to-end check's P1: the board still read the machine as answering (its loop
        // stood still while an editor was open), so the change went, and ssh couldn't connect.
        // Nothing reached it, so the machine is away: the change is held as any change for an away
        // machine is, with its op, or goes through a machine that forwards. Never "can't tell".
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        let name = app.machines[m].name();
        assert!(app.machines[m].conn.is_live());
        d(&mut app);
        let ready = app.resolver.ready();
        let [(n, machine, request, _)] = &ready[..] else {
            panic!("d didn't go once: {ready:?}")
        };
        assert_eq!(*machine, m);
        unreached(&mut app, *n, request.clone());
        assert_eq!(app.kept.held.len(), 1, "{}", said(&app));
        let held = app.kept.held[0].clone();
        assert_eq!(held.op(), request["op"].as_str().unwrap());
        assert_eq!(
            (held.machine.as_str(), held.unanswered),
            (name.as_str(), false)
        );
        assert_eq!(held.rev.as_deref(), Some("r1"));
        assert_eq!(
            said(&app),
            format!("Held: d on {reference} goes when {name} answers.")
        );
        assert!(!app.failing());
        assert_eq!(app.resolver.unanswered(), 0);
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("⧗ d waits for {name}").as_str())
        );
        // A machine that forwards: through it, once.
        let mut app = self::app();
        let (_, _, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        let via = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, via, FORWARD);
        d(&mut app);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        unreached(&mut app, n, request.clone());
        assert!(app.kept.held.is_empty());
        let Action::Mutate {
            machine,
            request: again,
        } = app.next_owed()
        else {
            panic!("not sent on: {}", said(&app))
        };
        app.queue_action(machine, again);
        let ready = app.resolver.ready();
        let [(_, to, sent, _)] = &ready[..] else {
            panic!("not sent once: {ready:?}")
        };
        assert_eq!(*to, via);
        assert_eq!(
            (&sent["action"], &sent["forward_to"], &sent["id"]),
            (&request["action"], &json!(name), &request["id"])
        );
    }

    #[test]
    fn a_host_key_or_sign_in_refused_is_never_forwarded_whatever_ssh_said_after_it() {
        // A host key or a sign-in its ssh refused is never gone around: held for it, not
        // forwarded. Codex's review 74, 1: also when another line of ssh's follows the refusal,
        // a jump host's say, which made it read as a machine that's merely away.
        for stderr in [
            "Host key verification failed.\n",
            "Host key verification failed.\nConnection closed by UNKNOWN port 65535\n",
            "drew@zznew: Permission denied (publickey,password).\nConnection closed by 10.0.0.9 port 22\n",
        ] {
            let mut app = app();
            let (_, _, reference, m) = waiting(&mut app);
            let via = (0..app.machines.len())
                .find(|&i| i != m && app.machines[i].feed.is_some())
                .unwrap();
            supports(&mut app, m, HOLD);
            supports(&mut app, via, FORWARD);
            let name = app.machines[m].name();
            d(&mut app);
            let (n, _, request, _) = app.resolver.ready()[0].clone();
            ssh_said(&mut app, n, request.clone(), stderr);
            assert_eq!(app.next_owed(), Action::None, "{stderr}");
            assert!(app.resolver.ready().is_empty(), "{stderr}");
            assert_eq!(app.kept.held.len(), 1, "{stderr}: {}", said(&app));
            assert_eq!(app.kept.held[0].op(), request["op"].as_str().unwrap());
            assert_eq!(
                said(&app),
                format!("Held: d on {reference} goes when {name} answers.")
            );
        }
    }

    #[test]
    fn a_change_from_the_hold_file_whose_machine_couldnt_be_reached_waits_there_unsaid() {
        // The same from the hold file: it waits there again as itself, unclaimed, and goes when
        // its machine answers. Nothing reached the machine, so it's not one that may have gone.
        let mut app = app();
        let (_, _, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 40);
        d(&mut app);
        let op = app.kept.held[0].op().to_string();
        back(&mut app, m);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        assert_eq!(request["held"], true);
        app.message = None;
        unreached(&mut app, n, request);
        assert_eq!(app.kept.held.len(), 1);
        let held = app.kept.held[0].clone();
        assert_eq!(held.op(), op);
        assert_eq!((held.by.as_deref(), held.unanswered), (None, false));
        assert_eq!(app.message(), None);
        assert_eq!(app.resolver.unanswered(), 0);
        back(&mut app, m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{ready:?}");
        assert_eq!(ready[0].2["op"], op.as_str());
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
    fn a_host_key_or_sign_in_refusal_is_never_gone_around_through_a_forwarder() {
        // Codex's review 60, 2: only a dropped or unreachable connection goes through another
        // machine. One SSH refused for its host key or sign-in is held for it instead, where its
        // backend holds, and refused where it doesn't.
        for conn in [
            Conn::HostKey {
                detail: "host key changed".into(),
            },
            Conn::Auth {
                detail: "permission denied".into(),
            },
        ] {
            let mut app = app();
            let (m, _, _, name, _) = forwarding(&mut app);
            app.machines[m].conn = conn.clone();
            d(&mut app);
            assert!(app.resolver.ready().is_empty(), "{conn:?}: went around");
            assert_eq!(app.kept.held.len(), 1, "{conn:?}");
            assert_eq!(app.kept.held[0].machine, name);
            assert!(app.kept.held[0].request.get("forward_to").is_none());
            let mut app = self::app();
            let (m, _, _, _, _) = forwarding(&mut app);
            app.machines[m]
                .feed
                .as_mut()
                .unwrap()
                .supports
                .retain(|s| s != HOLD);
            app.machines[m].conn = conn.clone();
            assert_eq!(d(&mut app), Action::None, "{conn:?}");
            assert!(app.resolver.ready().is_empty(), "{conn:?}: went around");
            assert!(app.kept.held.is_empty());
        }
    }

    #[test]
    fn bare_d_drops_the_first_of_several_held_and_2d_the_second() {
        // Codex's review 60, 16: the peek's hint says "D drop the first".
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        let name = app.machines[m].name();
        for action in ["dispatch", "refresh", "dismiss"] {
            let request = crate::actions::with_op(json!({"action": action, "id": id}));
            app.hold_request(name.clone(), request, Some(key.clone()), None, None);
        }
        let left =
            |app: &App| -> Vec<String> { app.kept.held.iter().map(|h| h.what.clone()).collect() };
        select(&mut app, &key);
        press(&mut app, 'm');
        press(&mut app, '2');
        assert_eq!(press(&mut app, 'D'), Action::None);
        assert_eq!(
            said(&app),
            format!("Dropped: y on {reference} won't be sent.")
        );
        assert_eq!(left(&app), ["d", "x"]);
        assert_eq!(press(&mut app, 'D'), Action::None);
        assert_eq!(
            said(&app),
            format!("Dropped: d on {reference} won't be sent.")
        );
        assert_eq!(left(&app), ["x"]);
    }

    #[test]
    fn a_start_queued_on_another_machine_shows_as_held_not_as_starting_there() {
        // r4-a's shape for B1-92: a start queued on another machine at his word answers ok, with
        // `pending`, `machine` and `session`, and no `home`. Its card shows it held, in those
        // words, never "Starting on far-box…", and it isn't taken as a start just made.
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        app.queue_action(m, json!({"action":"dispatch","id":id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request,
            ok(json!({
                "pending": "queued on far-box: starts in ARK once there's room",
                "machine": "far-box",
                "session": "ARK",
                "message": "Queued on far-box: it starts in ARK once there's room.",
            })),
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("⧗ queued on far-box: starts in ARK once there's room")
        );
        assert_ne!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("Starting on far-box…")
        );
        assert!(!app.started_lately(&key, &id));
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
                    asks: None,
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
    fn q_waits_for_every_unsent_request_not_only_the_one_on_its_way() {
        // Codex's review 60, 12: an account request waiting its turn behind the one on its way,
        // a sign-in's cancel kept for its machine, and a change whose copies disagree.
        let mut app = app();
        app.busy = true;
        app.account_later.push_back((
            crate::app::account_controls::Pending {
                machine: 0,
                request: json!({"action":"account-move","pane":"w1:p1","to":"ARK_C"}),
                screen: None,
                away: None,
                routed: None,
            },
            json!({"action":"account-move","pane":"w1:p1","to":"ARK_C"}),
        ));
        assert!(!app.quit_key());
        // The first answers: the one behind it hasn't gone yet.
        app.busy = false;
        assert!(!app.quit_due(), "{:?}", app.quit_waits_for());
        let mut app = self::app();
        let name = app.machines[1].name();
        away(&mut app, 1, 60);
        app.account_cancels.push((
            1,
            json!({"action":"account-signin","step":"cancel","ticket":"t"}),
            app.now().timestamp(),
        ));
        assert!(!app.quit_key());
        assert!(
            said(&app).starts_with(&format!("Quitting once {name} answers and ")),
            "{}",
            said(&app)
        );
        assert!(app.quit_key(), "a second q quits now and drops it");
        let mut app = self::app();
        let (key, id, reference, m) = waiting(&mut app);
        let request = crate::actions::with_op(json!({"action":"dispatch","id":id}));
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        assert!(app.wait_for_agreement(m, other, &id, &key, request.clone(), None));
        assert!(!app.quit_key());
        assert_eq!(
            said(&app),
            format!("Quitting once {reference}'s copies agree and it's sent again… Esc stays.")
        );
        // A second q keeps it for the home last named, where its backend holds.
        supports(&mut app, other, HOLD);
        assert!(app.quit_key());
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].op(), request["op"].as_str().unwrap());
        assert_eq!(app.kept.held[0].machine, app.machines[other].name());
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
        // Nothing is lost by quitting, so q quits at once, keeping what it holds.
        assert!(app.quit_key());
        let mut next = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(next.held.len(), 1);
        assert_eq!(next.held[0].after.as_deref(), Some("write-up"));
        assert_eq!(next.held[0].key.as_deref(), Some(key.as_str()));
        // The next Kerdr has the words back where they were.
        let mut later = self::app();
        later.restore_words(next.take_words());
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
            by: None,
            stays: None,
            unanswered: false,
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
            by: None,
            stays: None,
            unanswered: false,
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
    fn a_drop_that_cant_get_through_is_never_kept_and_says_it_wasnt_dropped() {
        // A drop's `held` names what it drops, and Kerdr's own mark on a change sent from its hold
        // file is `held: true` too, so a drop kept there would go later naming nothing. It's never
        // kept: through a forwarder that can't reach the card's machine, or for a machine that
        // isn't answering, it wasn't dropped, and Kerdr says so and what to do then.
        let queued = |app: &mut App, id: &str, m: usize| {
            let held: crate::feed::Held = serde_json::from_value(json!({
                "op": "k-target",
                "action": "dispatch",
                "pending": "queued in ARK: starts when a place frees",
            }))
            .unwrap();
            for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
                if c.id == id {
                    c.held = vec![held.clone()];
                }
            }
            app.rebuild();
        };
        // Through a forwarder that answers `unreachable`.
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        home(&mut app, &id, m, "elsewhere");
        supports(&mut app, m, FORWARD);
        supports(&mut app, m, HOLD);
        queued(&mut app, &id, m);
        let card = app.card(&key).unwrap().clone();
        let Action::Mutate { machine, request } = app.drop_backend(
            &card,
            "k-target",
            "queued in ARK: starts when a place frees",
        ) else {
            panic!("no drop: {}", said(&app))
        };
        app.queue_action(machine, request);
        let (n, _, sent, _) = app.resolver.ready()[0].clone();
        assert_eq!(
            (sent["held"].clone(), sent["forward_to"].clone()),
            (json!("k-target"), json!("elsewhere"))
        );
        app.finish_job(
            n,
            sent,
            Ok(Response {
                ok: false,
                message: "elsewhere can't be reached from here".into(),
                unreachable: Some("elsewhere".into()),
                ..Default::default()
            }),
        );
        assert!(app.kept.held.is_empty(), "kept: {:?}", app.kept.held);
        assert!(app.resolver.ready().is_empty());
        assert_eq!(
            said(&app),
            format!(
                "{reference}: \"queued in ARK: starts when a place frees\" wasn't dropped: elsewhere can't be reached from here. Once it answers, m then D drops it."
            )
        );
        // For its own machine, which isn't answering.
        let mut app = self::app();
        let (key, id, reference, m) = waiting(&mut app);
        let name = app.machines[m].name();
        supports(&mut app, m, HOLD);
        away(&mut app, m, 30);
        app.action_key = Some(key);
        app.queue_action(m, json!({"action":"drop","id":id,"held":"k-other"}));
        assert!(app.kept.held.is_empty(), "kept: {:?}", app.kept.held);
        assert!(app.resolver.ready().is_empty());
        assert_eq!(
            said(&app),
            format!(
                "Nothing held for {reference} was dropped: {name} can't be reached from here. Once it answers, m then D drops it."
            )
        );
    }

    #[test]
    fn a_backlog_row_shows_what_is_held_for_it_and_says_it_in_full_when_selected() {
        // The end-to-end check's P2: card boxes showed "⧗ d waits for X", but backlog rows, where
        // most d and x presses are, didn't. The row shows ⧗ in its glyph's place, staying one
        // line, and the message line says it in full while the row is selected.
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 40);
        let name = app.machines[m].name();
        assert!(
            matches!(d(&mut app), Action::Mutate { .. }),
            "{}",
            said(&app)
        );
        assert_eq!(app.kept.held.len(), 1);
        app.message = None;
        let screen = crate::ui::tests_render(&app, 160, 40);
        assert!(
            screen.contains(&format!("⧗ {reference} ")),
            "no ⧗ on its row: {screen}"
        );
        assert!(
            screen.contains(&format!("{reference} ⧗ d waits for {name}")),
            "not on the message line: {screen}"
        );
        // Another row selected: its own row still shows it, the message line doesn't.
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && c.stage == crate::model::Stage::Backlog)
            .expect("another backlog card")
            .key
            .clone();
        select(&mut app, &other);
        let screen = crate::ui::tests_render(&app, 160, 40);
        assert!(screen.contains(&format!("⧗ {reference} ")), "{screen}");
        assert!(!screen.contains("⧗ d waits for"), "{screen}");
    }

    #[test]
    fn d_for_a_machine_that_isnt_answering_says_it_wasnt_dropped_not_the_old_refusal() {
        // The end-to-end check's P3: D on what a backend holds, its machine away, said 0.34.0's
        // "That machine is not answering. Reconnect before changing its intents." It's a change to
        // a card: through a machine that forwards when there is one, else not dropped, said so,
        // with what to do once it answers. An account request's drop says the same.
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        let held: crate::feed::Held = serde_json::from_value(json!({
            "op": "k-target",
            "action": "dispatch",
            "pending": "starts once a workspace in ARK frees up",
        }))
        .unwrap();
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.held = vec![held.clone()];
            }
        }
        app.rebuild();
        select(&mut app, &key);
        away(&mut app, m, 30);
        let name = app.machines[m].name();
        if let Action::Mutate { machine, request } = app.drop_held(None) {
            app.queue_action(machine, request);
        }
        assert_eq!(
            said(&app),
            format!(
                "{reference}: \"starts once a workspace in ARK frees up\" wasn't dropped: {name} can't be reached from here. Once it answers, m then D drops it."
            )
        );
        assert!(app.kept.held.is_empty(), "kept: {:?}", app.kept.held);
        assert!(app.resolver.ready().is_empty());
        // A machine that forwards: sent through it.
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, other, FORWARD);
        let Action::Mutate { machine, request } = app.drop_held(None) else {
            panic!("not sent through the forwarder: {}", said(&app))
        };
        app.queue_action(machine, request);
        let ready = app.resolver.ready();
        let [(_, via, sent, _)] = &ready[..] else {
            panic!("not sent once: {ready:?}")
        };
        assert_eq!(*via, other);
        assert_eq!(
            (&sent["action"], &sent["held"], &sent["forward_to"]),
            (&json!("drop"), &json!("k-target"), &json!(name))
        );
        // An account request's drop, its machine away.
        app.message = None;
        assert_eq!(
            app.drop_account_backend(m, &key, "k-move", "moves to ARK_C at 02:00"),
            Action::None
        );
        assert_eq!(
            said(&app),
            format!(
                "{reference}: \"moves to ARK_C at 02:00\" wasn't dropped: {name} can't be reached from here. Once it answers, m then D drops it."
            )
        );
    }

    #[test]
    fn a_drop_answered_home_goes_to_the_machine_that_holds_it_and_its_card_stays_home() {
        // r4-a's round 4: a drop sent to the machine that queued a start for another answers
        // `home`, naming the machine that holds the start. The drop goes there once, with the same
        // op and target, and the card's home doesn't move: its feed still shows where its record
        // lives, and its next key goes there.
        let mut app = app();
        let (key, id, _, m) = waiting(&mut app);
        let holder = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, m, HOLD);
        supports(&mut app, holder, HOLD);
        let name = app.machines[holder].name();
        app.action_key = Some(key.clone());
        app.queue_action(m, json!({"action":"drop","id":id,"held":"k-target"}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request.clone(),
            Ok(Response {
                ok: false,
                message: format!("Its start is held on {name}, which decides."),
                home: Some(name.clone()),
                ..Default::default()
            }),
        );
        let ready = app.resolver.ready();
        let [(again_n, to, again, _)] = &ready[..] else {
            panic!("not sent on once: {ready:?}")
        };
        assert_eq!(*to, holder);
        assert_eq!(
            (&again["op"], &again["held"], &again["id"]),
            (&request["op"], &json!("k-target"), &json!(id))
        );
        app.finish_job(
            *again_n,
            again.clone(),
            Ok(Response {
                ok: true,
                message: "Dropped: it won't start.".into(),
                ..Default::default()
            }),
        );
        assert_eq!(said(&app), "Dropped: it won't start.");
        assert!(app.moved_home.is_empty(), "{:?}", app.moved_home);
        app.rebuild();
        select(&mut app, &key);
        let Action::Mutate { machine, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {}", said(&app))
        };
        assert_eq!(
            machine, m,
            "its next key went to the machine that held the drop"
        );
    }

    #[test]
    fn a_drop_whose_holder_cant_be_reached_says_where_it_is_held_not_that_the_card_moved() {
        // The team lead, after 971bd0d: a drop answered `home` whose holder this Kerdr can't
        // reach says what's true, in the shape of a drop that can't get through, never "#41
        // lives on X now". Nothing is sent on, kept or moved.
        let check = |holder_on_board: bool, held: Option<&str>, words: &str| {
            let mut app = self::app();
            let (key, id, reference, m) = waiting(&mut app);
            supports(&mut app, m, HOLD);
            if let Some(action) = held {
                for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
                    if c.id == id {
                        c.held = vec![crate::feed::Held {
                            op: "k-target".into(),
                            action: action.into(),
                            pending: "moves to ARK_C at 02:00".into(),
                            ..Default::default()
                        }];
                    }
                }
                app.rebuild();
                select(&mut app, &key);
            }
            // Not one of the board's machines, or one that isn't answering and whose feed doesn't
            // list "hold"; no forwarder either way.
            let host = if holder_on_board {
                let other = (0..app.machines.len())
                    .find(|&i| i != m && app.machines[i].feed.is_some())
                    .unwrap();
                away(&mut app, other, 30);
                app.machines[other].name()
            } else {
                "elsewhere".to_string()
            };
            app.action_key = Some(key.clone());
            app.queue_action(m, json!({"action":"drop","id":id,"held":"k-target"}));
            let (n, _, request, _) = app.resolver.ready()[0].clone();
            app.finish_job(
                n,
                request,
                Ok(Response {
                    ok: false,
                    message: format!("Its start is held on {host}, which decides."),
                    home: Some(format!("{host}/ARK")),
                    ..Default::default()
                }),
            );
            assert!(app.resolver.ready().is_empty());
            assert!(app.resend.is_empty(), "{:?}", app.resend);
            assert!(app.kept.held.is_empty(), "kept: {:?}", app.kept.held);
            assert!(app.moved_home.is_empty(), "{:?}", app.moved_home);
            assert!(!app.failing(), "said as a failure: {}", said(&app));
            assert_eq!(
                said(&app),
                format!(
                    "{reference}'s {words} is held on {host}, which this Kerdr can't reach, so it wasn't dropped. Once {host} answers, m then D drops it."
                )
            );
        };
        check(false, Some("dispatch"), "queued start");
        check(true, Some("reopen"), "queued start");
        check(false, Some("move"), "\"moves to ARK_C at 02:00\"");
        check(false, None, "held request");
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
            by: None,
            stays: None,
            unanswered: false,
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
                asks: None,
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
    fn a_start_its_backend_already_queued_answered_as_not_done_is_held_not_failed() {
        // r4-b's shape: d at the most on a reopen queued for him answers ok:false with `pending`,
        // so an older Kerdr says it didn't reopen. This one shows it waiting.
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        d(&mut app);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        let (n, request) = (ready[0].0, ready[0].2.clone());
        let words = "queued in web: reopens when a workspace there frees up";
        let answer = json!({"ok": false, "confirmation": false, "pending": words,
            "message": format!("{reference} is queued in web already: it reopens when a workspace there frees up.")});
        app.finish_job(
            n,
            request,
            Ok(crate::actions::parse_response(&answer.to_string()).unwrap()),
        );
        assert!(!app.failing(), "not a failure: {}", said(&app));
        assert_eq!(
            said(&app),
            format!(
                "{reference} is queued in web already: it reopens when a workspace there frees up."
            )
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("⧗ {words}").as_str())
        );
        assert!(app.resolver.job(n).is_none());
    }

    #[test]
    fn a_save_still_finishing_in_its_backend_can_be_dropped_after_a_restart() {
        // Codex's review 60, 15: its words and op go into the words file, so the next Kerdr can
        // drop it and give them back, as this one could.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (key, op) = saved_in_the_background(&mut app);
        assert!(app.quit_key());
        drop(app);
        let mut next = self::app();
        next.kept = Kept::at(dir.path(), next.now().timestamp());
        let words = next.kept.take_words();
        next.restore_words(words);
        supports(&mut next, 0, HOLD);
        feed_saving(&mut next, Some(&op));
        next.feed_read(0);
        next.rebuild();
        let card = next.card(&key).expect("its card shows").clone();
        assert_eq!(
            next.held_line_for(&card).as_deref(),
            Some(format!("✎ {SAVING_WORDS}").as_str())
        );
        select(&mut next, &key);
        press(&mut next, 'm');
        let Action::Mutate { machine, request } = press(&mut next, 'D') else {
            panic!("no drop: {}", said(&next))
        };
        assert_eq!(machine, 0);
        assert_eq!(request, json!({"action":"drop","held":op}));
        next.queue_action(machine, request);
        let (n, _, request, _) = next.resolver.ready()[0].clone();
        next.finish_job(n, request, ok(json!({"dropped": true})));
        assert!(next.card(&key).is_none());
        assert_eq!(next.draft, "Fix the feed");
    }

    #[test]
    fn words_a_lost_save_gave_back_go_again_as_that_send_after_a_restart() {
        // Codex's review 60, 4: the send they came back from is kept with them, so sending them
        // again finds what the lost save may have made rather than making a second intent.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let lost = super::super::Submission {
            id: "s-lost".into(),
            words: "Fix the banner".into(),
            images: vec![],
            machine: 0,
            session: "ark".into(),
            filed: super::super::Filed::default(),
            unknown: true,
        };
        assert!(app.give_back("Fix the banner".into(), vec![], Some(lost.clone())));
        let mut other = lost.clone();
        other.id = "s-other".into();
        other.words = "Tidy the logs".into();
        app.give_back("Tidy the logs".into(), vec![], Some(other.clone()));
        assert!(app.quit_key());
        drop(app);
        let mut next = self::app();
        next.kept = Kept::at(dir.path(), next.now().timestamp());
        let words = next.kept.take_words();
        next.restore_words(words);
        assert_eq!(next.draft, "Fix the banner");
        assert_eq!(next.draft_came_back, Some(lost));
        assert_eq!(next.returned[0].from, Some(other));
        // Sent again unchanged, from another view: it goes as that send.
        let draft = super::super::Draft {
            words: next.draft.clone(),
            machine: 1,
            session: "elsewhere".into(),
            images: vec![],
            came_back: next.draft_came_back.take(),
            filed: super::super::Filed::default(),
            unfiled: None,
            edit: None,
        };
        let Action::Mutate { machine, request } = next.send_draft(draft) else {
            panic!("not sent: {}", said(&next))
        };
        assert_eq!(machine, 0);
        assert_eq!(request["submission"], "s-lost");
        assert_eq!(request["session"], "ark");
    }

    #[test]
    fn q_stays_open_when_what_it_keeps_cant_be_written_and_a_second_q_quits_anyway() {
        // Codex's review 60, 9.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        app.words_dir = Some(dir.path().to_path_buf());
        app.draft = "Words not yet sent".into();
        // Something in the way of the words file: it can't be written.
        std::fs::create_dir(dir.path().join("words.json")).unwrap();
        assert!(!app.quit_key(), "it doesn't quit as if they were kept");
        assert!(said(&app).contains("words.json"), "{}", said(&app));
        assert!(
            said(&app).ends_with("q again quits anyway, Esc stays."),
            "{}",
            said(&app)
        );
        assert!(!app.quit_due(), "nor by itself");
        // Esc stays; out of the way, q keeps them and quits.
        press_code(&mut app, KeyCode::Esc);
        std::fs::remove_dir(dir.path().join("words.json")).unwrap();
        assert!(app.quit_key());
        assert_eq!(
            Kept::at(dir.path(), 0).take_words().draft.map(|d| d.words),
            Some("Words not yet sent".to_string())
        );
        // Still in the way: a second q quits anyway.
        let mut app = self::app();
        let dir = tempfile::tempdir().unwrap();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        app.draft = "Words not yet sent".into();
        std::fs::create_dir(dir.path().join("words.json")).unwrap();
        assert!(!app.quit_key());
        assert!(app.quit_key());
    }

    #[test]
    fn a_change_held_when_the_hold_file_cant_be_written_says_so() {
        // Codex's review 60, 9: never said to be held for the next Kerdr when it isn't.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (_, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        let name = app.machines[m].name();
        std::fs::create_dir(dir.path().join("held.json")).unwrap();
        d(&mut app);
        assert_eq!(app.kept.held.len(), 1, "held while this Kerdr is open");
        assert!(
            said(&app).starts_with(&format!(
                "Held: d on {reference} goes when {name} answers (it dropped 5 s ago). Kerdr couldn't write "
            )),
            "{}",
            said(&app)
        );
        assert!(
            said(&app).ends_with("so it's held only while this Kerdr is open."),
            "{}",
            said(&app)
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
            asks: None,
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

    /// m on a card, then D, as the board's loop takes it.
    fn peek_drop(app: &mut App, key: &str) -> Action {
        select(app, key);
        press(app, 'm');
        press(app, 'D')
    }

    #[test]
    fn a_held_change_another_kerdr_open_is_sending_isnt_sent_here_too() {
        // Codex's review 60, 1: two Kerdrs open at once share the hold file. Only the one that
        // has a held change to itself sends it, and the other can't drop it from under it.
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        a.kept = Kept::at(dir.path(), a.now().timestamp());
        let (key, _, reference, m) = waiting(&mut a);
        supports(&mut a, m, HOLD);
        away(&mut a, m, 5);
        d(&mut a);
        let mut b = app();
        b.kept = Kept::at(dir.path(), b.now().timestamp());
        waiting(&mut b);
        supports(&mut b, m, HOLD);
        assert_eq!(b.kept.held.len(), 1);
        back(&mut a, m);
        let sent = a.resolver.ready();
        assert_eq!(sent.len(), 1);
        back(&mut b, m);
        assert!(b.resolver.ready().is_empty(), "sent by both Kerdrs");
        assert_eq!(peek_drop(&mut b, &key), Action::None);
        assert_eq!(
            said(&b),
            format!(
                "d on {reference} is on its way from another Kerdr open now, so it can't be dropped from here."
            )
        );
        // Answered in the first: it leaves the file, which the second sees at its next read.
        a.finish_job(sent[0].0, sent[0].2.clone(), ok(json!({})));
        b.feed_read(m);
        assert!(b.kept.held.is_empty());
    }

    #[test]
    fn a_held_key_behind_a_held_start_keeps_its_op_and_leaves_the_file_once_answered() {
        // Codex's review 60, 7: d and y held while the machine was away. Once it answers, y
        // waits behind d's start as the same record, then goes with its own op, and its answer
        // takes it out of the file: it can't run a second time from there.
        let mut app = app();
        let (key, _, _, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        d(&mut app);
        select(&mut app, &key);
        if let Action::Mutate { machine, request } = press(&mut app, 'y') {
            app.queue_action(machine, request);
        }
        assert_eq!(app.kept.held.len(), 2, "{}", said(&app));
        let y = app.kept.held.iter().find(|h| h.what == "y").unwrap();
        let y_op = y.op().to_string();
        back(&mut app, m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "only the start goes; y waits behind it");
        let (start, _, request, _) = ready[0].clone();
        assert_eq!(request["action"], "dispatch");
        // The start is refused: y goes now, still the same record.
        let refused = Response {
            ok: false,
            message: "No slot is free.".into(),
            ..Default::default()
        };
        app.finish_job(start, request, Ok(refused));
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "refresh");
        assert_eq!(ready[0].2["op"], y_op.as_str());
        assert_eq!(ready[0].2["held"], true);
        assert_eq!(app.kept.held.len(), 1, "y stays in the file until answered");
        app.finish_job(ready[0].0, ready[0].2.clone(), ok(json!({})));
        assert!(app.kept.held.is_empty(), "{:?}", app.kept.held);
    }

    #[test]
    fn d_on_a_held_change_kerdr_is_about_to_send_takes_it_out_of_kerdr_too() {
        // Codex's review 60, 8: once its machine answers, a held change is on its way; m's D
        // takes it out of Kerdr as well as the file, so it never goes.
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        d(&mut app);
        back(&mut app, m);
        assert_eq!(peek_drop(&mut app, &key), Action::None);
        assert_eq!(
            said(&app),
            format!("Dropped: d on {reference} won't be sent.")
        );
        assert!(app.resolver.ready().is_empty(), "sent though dropped");
        assert!(app.kept.held.is_empty());
    }

    #[test]
    fn d_on_a_held_change_the_runner_has_asks_it_and_says_when_it_was_too_late() {
        // Codex's review 60, 8: with the runner, the drop is asked of it, and its answer says
        // whether it was in time; once its command has started, D says it's too late.
        let held_and_handed = || {
            let mut app = app();
            let (key, _, reference, m) = waiting(&mut app);
            supports(&mut app, m, HOLD);
            away(&mut app, m, 5);
            d(&mut app);
            back(&mut app, m);
            let (n, _, request, _) = app.resolver.ready()[0].clone();
            (app, key, reference, m, n, request)
        };
        let (mut app, key, reference, _, n, _) = held_and_handed();
        assert_eq!(peek_drop(&mut app, &key), Action::None);
        assert_eq!(app.resolver.take_dropped(), vec![n]);
        assert_eq!(app.kept.held.len(), 1, "until the runner answers");
        app.job_drop_answered(n, true);
        assert_eq!(
            said(&app),
            format!("Dropped: d on {reference} won't be sent.")
        );
        assert!(app.kept.held.is_empty());
        assert!(app.resolver.job(n).is_none());
        // Its command began a moment before the drop reached the runner.
        let (mut app, key, reference, m, n, request) = held_and_handed();
        peek_drop(&mut app, &key);
        assert_eq!(app.resolver.take_dropped(), vec![n]);
        app.job_started(n);
        app.job_drop_answered(n, false);
        assert_eq!(
            said(&app),
            format!(
                "d on {reference} had already gone when D came, so it carries on: its card shows what it does."
            )
        );
        assert_eq!(app.kept.held.len(), 1, "until it's answered");
        // Its command running: D says it's too late to drop it here.
        peek_drop(&mut app, &key);
        let name = app.machines[m].name();
        assert_eq!(
            said(&app),
            format!(
                "d on {reference} is already with {name}, so it can't be dropped now: its card shows what it does."
            )
        );
        app.finish_job(n, request, ok(json!({})));
        assert!(app.kept.held.is_empty());
    }

    #[test]
    fn a_held_change_through_a_forwarder_that_doesnt_hold_keeps_held_and_its_rev() {
        // Codex's review 60, 11: what the machine it's for checks goes with it, whatever the
        // forwarder's own backend does.
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        home(&mut app, &id, m, "elsewhere");
        supports(&mut app, m, FORWARD);
        app.kept.hold(HeldRequest {
            machine: "elsewhere".into(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: Some("r0".into()),
            key: Some(key),
            reference,
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: None,
            by: None,
            stays: None,
            unanswered: false,
        });
        app.feed_read(m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].1, m);
        assert_eq!(ready[0].2["forward_to"], "elsewhere");
        assert_eq!(ready[0].2["held"], true);
        assert_eq!(ready[0].2["rev"], "r0");
    }

    #[test]
    fn a_held_change_whose_backend_no_longer_checks_one_stays_held_and_says_so_once() {
        // Codex's review 60, 11: its machine answers with a backend that no longer lists
        // "hold": the change isn't sent unchecked.
        let mut app = app();
        let (key, id, reference, m) = waiting(&mut app);
        let name = app.machines[m].name();
        app.kept.hold(HeldRequest {
            machine: name.clone(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: Some("r1".into()),
            key: Some(key),
            reference: reference.clone(),
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: None,
            by: None,
            stays: None,
            unanswered: false,
        });
        back(&mut app, m);
        assert!(app.resolver.ready().is_empty(), "sent unchecked");
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(
            said(&app),
            format!(
                "d on {reference} stays held: {name}'s backend no longer checks a held change again, so Kerdr won't send it unchecked. Press d again to send it now, or m then D drops it."
            )
        );
        app.say("Something else.", Tone::Info);
        app.feed_read(m);
        assert_eq!(said(&app), "Something else.");
    }

    /// A d kept in a hold file in its own folder for a machine whose backend doesn't check a held
    /// change, which has just answered: the folder, the card's key, id and reference, the
    /// machine, and the record's op.
    fn kept_unchecked(app: &mut App) -> (tempfile::TempDir, String, String, String, usize, String) {
        let dir = tempfile::tempdir().unwrap();
        app.fixed_now = Some(app.now());
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (key, id, reference, m) = waiting(app);
        app.kept.hold(HeldRequest {
            machine: app.machines[m].name(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: Some("r1".into()),
            key: Some(key.clone()),
            reference: reference.clone(),
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: None,
            by: None,
            stays: None,
            unanswered: false,
        });
        let op = app.kept.held[0].op().to_string();
        back(app, m);
        assert!(app.resolver.ready().is_empty(), "sent unchecked");
        (dir, key, id, reference, m, op)
    }

    /// The hold file's records as written: each one's op and who has it.
    fn on_file(dir: &std::path::Path) -> Vec<(String, Option<String>)> {
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("held.json")).unwrap()).unwrap();
        file["held"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["request"]["op"].as_str().unwrap_or_default().to_string(),
                    h["by"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    #[test]
    fn d_pressed_again_on_a_change_kept_held_for_a_backend_that_doesnt_check_goes_as_that_record() {
        // The team lead, after round 4: pressing d again sends the change now, and uses up the
        // record kept held, with its op, rather than leaving it behind as a second copy.
        let mut app = app();
        let (dir, _, _, _, _, op) = kept_unchecked(&mut app);
        let pressed = d(&mut app);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        let (n, _, request, _) = ready[0].clone();
        assert_eq!(
            request["op"],
            op.as_str(),
            "a second copy, with an op of its own"
        );
        assert!(request.get("held").is_none(), "{request}");
        assert_eq!(
            on_file(dir.path()),
            [(op.clone(), Some(app.kept.me().to_string()))],
            "on its way from this Kerdr"
        );
        // The same change once more while it's on its way, even from below the key, which itself
        // says Kerdr is still asking: nothing more goes (Codex's review 64, 1).
        let Action::Mutate {
            machine,
            request: again,
        } = pressed
        else {
            panic!("d sent nothing")
        };
        app.queue_action(machine, again);
        let again = app.resolver.ready();
        assert!(again.is_empty(), "sent again: {again:?}");
        let (reference, name) = (
            app.kept.held[0].reference.clone(),
            app.kept.held[0].machine.clone(),
        );
        assert_eq!(
            said(&app),
            format!("d on {reference} is already on its way to {name}.")
        );
        app.finish_job(n, request, ok(json!({})));
        assert!(app.kept.held.is_empty(), "left behind once answered");
        assert!(on_file(dir.path()).is_empty());
    }

    #[test]
    fn a_change_kept_held_for_a_backend_that_doesnt_check_says_why_when_its_day_runs_out() {
        // The team lead, after round 4: not "didn't answer for a day", since its machine did.
        let mut app = app();
        let (_dir, _, _, reference, m, _) = kept_unchecked(&mut app);
        let name = app.machines[m].name();
        app.fixed_now =
            Some(app.now() + chrono::Duration::seconds(crate::kept::HOLD_MOST_SECS + 1));
        app.feed_read(m);
        assert!(app.kept.held.is_empty());
        assert_eq!(
            said(&app),
            format!(
                "{reference}: d dropped after a day: {name}'s backend doesn't check a held change again, so it wasn't sent."
            )
        );
    }

    #[test]
    fn d_pressed_again_while_another_kerdr_sends_that_record_sends_nothing_from_here() {
        // Codex's review 64, 1: another Kerdr open now has claimed the record d kept held. Pressing
        // d again here sends nothing, under its op or a new one, and says why; the record stays
        // that Kerdr's. So too once that Kerdr has cleared why it stayed held, as its send does.
        let mut app = app();
        let (dir, _, _, reference, m, op) = kept_unchecked(&mut app);
        let name = app.machines[m].name();
        let mut other = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(other.claim(&op), crate::kept::Claim::Mine);
        let theirs = [(op.clone(), Some(other.me().to_string()))];
        let busy = format!(
            "d on {reference} is already on its way to {name} from another Kerdr open now, so it isn't sent again from here."
        );
        d(&mut app);
        let ready = app.resolver.ready();
        assert!(ready.is_empty(), "sent again: {ready:?}");
        assert_eq!(said(&app), busy);
        assert_eq!(on_file(dir.path()), theirs);
        other.stays(&op, None);
        app.kept.sync();
        app.say("Something else.", Tone::Info);
        d(&mut app);
        let ready = app.resolver.ready();
        assert!(ready.is_empty(), "sent again: {ready:?}");
        assert_eq!(said(&app), busy);
        assert_eq!(on_file(dir.path()), theirs);
    }

    #[test]
    fn d_pressed_again_when_its_claim_cant_be_written_sends_nothing_until_it_can() {
        // Codex's review 64, 1: the hold file can't be written, so Kerdr can't record that the
        // record d kept held is this one's to send, and another may be sending it. Nothing is sent,
        // said why; once the file can be written, d sends it as that record.
        let mut app = app();
        let (dir, _, _, reference, _, op) = kept_unchecked(&mut app);
        let lock = dir.path().join("kept.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        d(&mut app);
        let ready = app.resolver.ready();
        assert!(ready.is_empty(), "sent unclaimed: {ready:?}");
        let held = dir.path().join("held.json").display().to_string();
        let line = said(&app);
        assert!(
            line.starts_with(&format!("d on {reference} wasn't sent: Kerdr couldn't write {held} ("))
                && line.ends_with(
                    "so Kerdr can't tell whether another Kerdr is sending it. Press d again once it can be written."
                ),
            "{line}"
        );
        std::fs::remove_dir(&lock).unwrap();
        d(&mut app);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["op"], op.as_str());
        assert_eq!(
            on_file(dir.path()),
            [(op.clone(), Some(app.kept.me().to_string()))]
        );
    }

    #[test]
    fn d_pressed_again_whose_answer_is_lost_says_it_may_have_gone_ahead_when_its_day_runs_out() {
        // Codex's reviews 64, 4, and 65, 2: d pressed again sends the record kept held. Before it's
        // handed over, the file says it may have gone ahead, and no longer why it stayed unsent.
        // Its answer is lost, or Kerdr ends before it comes and the next one opens on the same
        // folder: either way, Kerdr keeps it held again for the same backend, and if its day runs
        // out its card says it may have gone ahead, never that it wasn't sent. An answer that says
        // nothing reached its machine puts it back as it was.
        enum End {
            Lost,
            Ended,
            NotSent,
        }
        for end in [End::Lost, End::Ended, End::NotSent] {
            let mut app = app();
            let (dir, _, _, reference, m, op) = kept_unchecked(&mut app);
            let name = app.machines[m].name();
            d(&mut app);
            let ready = app.hand_over();
            assert_eq!(ready.len(), 1, "{}", said(&app));
            let (n, _, request, _) = ready[0].clone();
            let file: Value = serde_json::from_str(
                &std::fs::read_to_string(dir.path().join("held.json")).unwrap(),
            )
            .unwrap();
            assert!(
                file["held"][0].get("stays").is_none(),
                "still says why it stayed unsent: {file}"
            );
            assert_eq!(
                file["held"][0]["unanswered"], true,
                "handed over, not yet written as possibly gone: {file}"
            );
            let mut app = match end {
                End::Lost | End::NotSent => {
                    let why = match end {
                        End::Lost => crate::actions::UNKNOWN,
                        _ => crate::actions::NOT_SENT,
                    };
                    app.finish_job(n, request, Err(format!("{name} didn't answer, {why}")));
                    assert_eq!(on_file(dir.path()), [(op.clone(), None)]);
                    // Its failure, seen.
                    app.message = None;
                    app.feed_read(m);
                    app
                }
                End::Ended => {
                    let now = app.now();
                    drop(app);
                    let mut next = self::app();
                    next.fixed_now = Some(now);
                    next.kept = Kept::at(dir.path(), now.timestamp());
                    let (_, _, _, m) = waiting(&mut next);
                    back(&mut next, m);
                    assert!(next.resolver.ready().is_empty(), "sent unchecked");
                    next
                }
            };
            assert_eq!(app.kept.held.len(), 1, "{}", said(&app));
            app.fixed_now =
                Some(app.now() + chrono::Duration::seconds(crate::kept::HOLD_MOST_SECS + 1));
            app.feed_read(m);
            assert!(app.kept.held.is_empty());
            let dropped = match end {
                End::NotSent => format!(
                    "{reference}: d dropped after a day: {name}'s backend doesn't check a held change again, so it wasn't sent."
                ),
                _ => format!(
                    "{reference}: d dropped after a day: {name} never answered after it was sent, so it may have gone ahead."
                ),
            };
            assert_eq!(said(&app), dropped);
        }
    }

    fn refused_for(home: &str) -> Result<Response, String> {
        Ok(Response {
            ok: false,
            message: format!("It lives on {home} now; ⏎ again opens it there."),
            home: Some(home.into()),
            ..Default::default()
        })
    }

    /// d held while its machine was away, sent from the file once it answered, and refused there
    /// because its card lives on another machine, whose backend holds: that answer's job and
    /// request, the other machine, and the record's op.
    fn held_then_refused(app: &mut App) -> (u64, Value, usize, String) {
        let (_, _, _, m) = waiting(app);
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(app, m, HOLD);
        supports(app, other, HOLD);
        away(app, m, 5);
        d(app);
        let op = app.kept.held[0].op().to_string();
        back(app, m);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        assert_eq!(request["op"], op.as_str());
        let name = app.machines[other].name();
        app.finish_job(n, request.clone(), refused_for(&format!("{name}/ARK")));
        (n, request, other, op)
    }

    #[test]
    fn a_stop_kept_from_before_a_restart_and_sent_on_to_a_backend_that_wouldnt_ask_again_stays_held()
     {
        // Codex's review 63, 1: "Cancel #N once it opens?" answered yes before Kerdr closed, sent
        // from the file with `held: true` and its workspace named to a backend that holds, and
        // refused there because its card lives on a machine whose backend lists neither "hold"
        // nor "steps". It isn't sent there: it stays held, and that's said. A backend there that
        // asks its own question first ("steps") is sent it, naming no workspace, for it to ask.
        let sent_on = |steps: bool| {
            let mut app = app();
            let card = app
                .board
                .cards
                .iter()
                .find(|c| {
                    c.is_open_workspace() && c.detail.is_some() && !c.done() && !c.dismissed()
                })
                .cloned()
                .expect("a card with its workspace open in the demo");
            let (m, id) = (card.machine_index, card.detail.as_ref().unwrap().id.clone());
            let other = (0..app.machines.len())
                .find(|&i| i != m && app.machines[i].feed.is_some())
                .unwrap();
            supports(&mut app, m, HOLD);
            let feed = app.machines[other].feed.as_mut().unwrap();
            feed.supports.retain(|s| s != HOLD && s != "steps");
            if steps {
                feed.supports.push("steps".into());
            }
            app.kept.hold(HeldRequest {
                machine: app.machines[m].name(),
                request: crate::actions::with_op(json!({"action":"cancel","id":id})),
                rev: None,
                key: Some(card.key.clone()),
                reference: card.reference.clone(),
                what: "X".into(),
                pressed: app.now().timestamp(),
                after: Some("start".into()),
                by: None,
                stays: None,
                unanswered: false,
            });
            let op = app.kept.held[0].op().to_string();
            app.feed_read(m);
            let ready = app.resolver.ready();
            assert_eq!(ready.len(), 1, "{}", said(&app));
            let (n, _, request, _) = ready[0].clone();
            assert_eq!(request["held"], true);
            assert!(request.get("workspace").is_some(), "{request}");
            let name = app.machines[other].name();
            app.finish_job(n, request, refused_for(&format!("{name}/ARK")));
            (app, card.reference, other, op)
        };
        let (mut app, reference, other, op) = sent_on(false);
        let ready = app.resolver.ready();
        assert!(
            ready.is_empty(),
            "sent there without being asked again: {ready:?}"
        );
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].op(), op);
        let name = app.machines[other].name();
        assert_eq!(app.kept.held[0].machine, name);
        assert_eq!(
            said(&app),
            format!(
                "X on {reference} stays held: it lives on {name} now, whose backend wouldn't ask again before stopping it, so Kerdr won't send the yes you gave before it last closed. Press X again to be asked now, or m then D drops it."
            )
        );
        let (mut app, _, other, op) = sent_on(true);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(
            (ready[0].1, ready[0].2["op"].as_str()),
            (other, Some(op.as_str()))
        );
        assert!(ready[0].2.get("workspace").is_none(), "{}", ready[0].2);
        assert!(ready[0].2.get("held").is_none(), "{}", ready[0].2);
    }

    #[test]
    fn a_held_change_refused_for_another_home_stays_one_record_until_that_home_answers() {
        // After Codex's review 60: refused because its card lives on another machine, a held
        // change goes there as the same record, with its op, and leaves the file only once that
        // machine answers.
        let mut app = app();
        let (n, _, other, op) = held_then_refused(&mut app);
        let name = app.machines[other].name();
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        let (again_n, to, again, _) = ready[0].clone();
        assert_eq!((again_n, to), (n, other));
        assert_eq!(again["op"], op.as_str());
        assert_eq!(
            app.kept.held.len(),
            1,
            "it left the file before that machine answered"
        );
        assert_eq!(app.kept.held[0].op(), op);
        assert_eq!(app.kept.held[0].machine, name);
        assert!(app.kept.held[0].request.get("held").is_none());
        app.finish_job(n, again, ok(json!({})));
        assert!(app.kept.held.is_empty());

        // A Kerdr that ends before that answer leaves it in the file, for that machine: the next
        // one sends it there once, still with its op, `held` and the card's rev then.
        let dir = tempfile::tempdir().unwrap();
        let mut app = self::app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (_, _, other, op) = held_then_refused(&mut app);
        drop(app);
        let mut next = self::app();
        next.kept = Kept::at(dir.path(), next.now().timestamp());
        waiting(&mut next);
        supports(&mut next, other, HOLD);
        assert_eq!(next.kept.held.len(), 1);
        assert_eq!(next.kept.held[0].machine, next.machines[other].name());
        back(&mut next, other);
        let ready = next.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&next));
        let (n, to, request, _) = ready[0].clone();
        assert_eq!(to, other);
        assert_eq!(request["op"], op.as_str());
        assert_eq!(request["held"], true);
        assert_eq!(request["rev"], "r1");
        next.feed_read(other);
        assert!(next.resolver.ready().is_empty(), "sent twice");
        next.finish_job(n, request, ok(json!({})));
        assert!(next.kept.held.is_empty());
    }

    #[test]
    fn a_held_change_refused_for_a_home_that_isnt_answering_waits_for_it_as_itself() {
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, m, HOLD);
        supports(&mut app, other, HOLD);
        away(&mut app, m, 5);
        d(&mut app);
        let op = app.kept.held[0].op().to_string();
        back(&mut app, m);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        away(&mut app, other, 7);
        let name = app.machines[other].name();
        app.finish_job(n, request, refused_for(&format!("{name}/ARK")));
        assert!(app.resolver.ready().is_empty());
        assert_eq!(app.kept.held.len(), 1, "{:?}", app.kept.held);
        let h = app.kept.held[0].clone();
        assert_eq!((h.op(), h.machine.as_str()), (op.as_str(), name.as_str()));
        assert_eq!(h.by, None, "not waiting for a Kerdr's answer any more");
        assert!(h.request.get("held").is_none() && h.request.get("rev").is_none());
        assert_eq!(h.rev.as_deref(), Some("r1"));
        assert_eq!(
            said(&app),
            format!("Held: d on {reference} goes when {name} answers (it dropped 7 s ago).")
        );
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some(format!("⧗ d waits for {name}").as_str())
        );
    }

    #[test]
    fn a_held_change_refused_for_a_home_whose_backend_doesnt_check_one_stays_held() {
        // Codex's review 60, 11, for the machine a refusal names: a held change isn't sent there
        // unchecked.
        let mut app = app();
        let (key, _, reference, m) = waiting(&mut app);
        let other = (0..app.machines.len())
            .find(|&i| i != m && app.machines[i].feed.is_some())
            .unwrap();
        supports(&mut app, m, HOLD);
        away(&mut app, m, 5);
        d(&mut app);
        back(&mut app, m);
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        let name = app.machines[other].name();
        app.finish_job(n, request, refused_for(&format!("{name}/ARK")));
        assert!(app.resolver.ready().is_empty(), "sent unchecked");
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].machine, name);
        assert_eq!(
            said(&app),
            format!(
                "d on {reference} stays held: it lives on {name} now, whose backend doesn't check a held change again, so Kerdr won't send it unchecked. Press d again to send it now, or m then D drops it."
            )
        );
        // Its machine read again: still not sent, and not said again.
        app.say("Something else.", Tone::Info);
        app.feed_read(other);
        assert!(app.resolver.ready().is_empty());
        assert_eq!(said(&app), "Something else.");
        assert_eq!(peek_drop(&mut app, &key), Action::None);
        assert!(app.kept.held.is_empty());
    }

    #[test]
    fn a_held_change_whose_copies_disagree_stays_one_record_until_sent_said_or_dropped() {
        // Sent on to the home a refusal named, and refused there naming the first machine: it
        // waits for both copies to agree as the same record, this Kerdr's, for the machine named
        // last. Once they agree it goes from the file, checked as any held change; m's D drops
        // it; two minutes on it leaves the file once that's said; q leaves it in the file.
        let disagreeing = || {
            let dir = tempfile::tempdir().unwrap();
            let mut app = app();
            app.kept = Kept::at(dir.path(), app.now().timestamp());
            let (n, _, other, op) = held_then_refused(&mut app);
            let (_, _, again, _) = app.resolver.ready()[0].clone();
            let m = (0..app.machines.len())
                .find(|&i| i != other && app.machines[i].feed.is_some())
                .unwrap();
            let first = app.machines[m].name();
            app.finish_job(n, again, refused_for(&format!("{first}/ARK")));
            assert_eq!(app.catch_up.disagreed.len(), 1, "{}", said(&app));
            assert_eq!(app.kept.held.len(), 1);
            assert_eq!(app.kept.held[0].op(), op);
            assert_eq!(app.kept.held[0].machine, first);
            assert!(app.kept.held[0].by.is_some(), "this Kerdr's while it waits");
            (app, m, op, dir)
        };
        let (mut app, m, op, _dir) = disagreeing();
        let key = app.kept.held[0].key.clone().unwrap();
        let first = app.machines[m].name();
        crate::app::catch_up::tests::homes_say(&mut app, &key, &first);
        app.catch_up_read(0, true);
        app.catch_up_read(1, true);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        let (n, to, request, _) = ready[0].clone();
        assert_eq!(to, m);
        assert_eq!(request["op"], op.as_str());
        assert_eq!(request["held"], true);
        assert_eq!(request["rev"], "r1");
        assert_eq!(app.kept.held.len(), 1, "until that machine answers");
        app.finish_job(n, request, ok(json!({})));
        assert!(app.kept.held.is_empty());

        let (mut app, _, _, _dir) = disagreeing();
        let key = app.kept.held[0].key.clone().unwrap();
        peek_drop(&mut app, &key);
        assert!(app.kept.held.is_empty());
        assert!(app.catch_up.disagreed.is_empty(), "sent once they agree");

        let (mut app, m, _, _dir) = disagreeing();
        app.catch_up.disagreed[0].since = Instant::now() - crate::app::catch_up::AGREE_WITHIN;
        app.catch_up_read(m, false);
        assert!(app.catch_up.disagreed.is_empty());
        assert!(app.kept.held.is_empty());

        let (mut app, m, op, dir) = disagreeing();
        assert!(!app.quit_key());
        assert!(app.quit_key(), "{}", said(&app));
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].op(), op);
        assert_eq!(app.kept.held[0].machine, app.machines[m].name());
        // This Kerdr's until it has gone (Codex's review 62, 6), then the next one's.
        assert_eq!(app.kept.held[0].by.as_deref(), Some(app.kept.me()));
        drop(app);
        let mut next = Kept::at(dir.path(), 0);
        assert_eq!(next.claim(&op), crate::kept::Claim::Mine);
    }

    /// A Kerdr with its own state folder carrying two starts waiting for their write-up: one
    /// from the hold file, and one d asked for. That folder, and the first one's op.
    fn carrying_two(app: &mut App) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (key, id, reference, m) = waiting(app);
        app.kept.hold(HeldRequest {
            machine: app.machines[m].name(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":id})),
            rev: None,
            key: Some(key),
            reference,
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: Some("write-up".into()),
            by: None,
            stays: None,
            unanswered: false,
        });
        let carried = app.kept.held[0].op().to_string();
        back(app, m);
        assert!(
            app.held_sending.values().any(|op| *op == carried),
            "{}",
            said(app)
        );
        app.resolver.add_gated(
            m,
            json!({"action":"dispatch","id":"0999"}),
            None,
            None,
            None,
        );
        (dir, carried)
    }

    #[test]
    fn what_a_quitting_kerdr_keeps_stays_its_own_until_it_has_gone() {
        // Codex's review 62, 6: another Kerdr that looks while this one puts what it carries in
        // the file for the next can't take any of it, nor after a quit whose words couldn't be
        // written leaves this one open. Once this one has gone, it can.
        use crate::kept::Claim;
        let mut app = app();
        let (dir, carried) = carrying_two(&mut app);
        app.hold_for_next();
        let fresh = app
            .kept
            .held
            .iter()
            .map(|h| h.op().to_string())
            .find(|op| *op != carried)
            .expect("the d asked for, kept for the next Kerdr");
        let mut other = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(other.claim(&carried), Claim::Taken);
        assert_eq!(other.claim(&fresh), Claim::Taken);

        let mut app = self::app();
        let (dir, carried) = carrying_two(&mut app);
        app.draft = "Words not yet sent".into();
        std::fs::create_dir(dir.path().join("words.json")).unwrap();
        assert!(app.keep_on_quit().is_err());
        let mut other = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(
            other.held.len(),
            1,
            "what it added for the next is gone again"
        );
        assert_eq!(other.claim(&carried), Claim::Taken);
        assert!(app.held_sending.values().any(|op| *op == carried));
        std::fs::remove_dir(dir.path().join("words.json")).unwrap();
        app.keep_on_quit().unwrap();
        let fresh = app
            .kept
            .held
            .iter()
            .map(|h| h.op().to_string())
            .find(|op| *op != carried)
            .unwrap();
        assert_eq!(other.claim(&fresh), Claim::Taken);
        drop(app);
        assert_eq!(other.claim(&carried), Claim::Mine);
        assert_eq!(other.claim(&fresh), Claim::Mine);
    }

    #[test]
    fn words_sent_from_a_restored_draft_never_come_back_as_new_words_after_a_crash() {
        // Codex's review 62, 2: the draft the last Kerdr kept, restored and sent. A crash before
        // its answer gives the words back as that send, with its submission, so sending them
        // again finds what it made; a crash after it was saved gives nothing back.
        let restored_and_sent = || {
            let dir = tempfile::tempdir().unwrap();
            let draft = Words {
                draft: Some(KeptWords {
                    words: "Fix the banner".into(),
                    ..KeptWords::default()
                }),
                ..Words::default()
            };
            Kept::at(dir.path(), 0).keep_words(&draft).unwrap();
            let mut app = app();
            app.kept = Kept::at(dir.path(), app.now().timestamp());
            let words = app.kept.take_words();
            app.restore_words(words);
            assert_eq!(app.draft, "Fix the banner");
            press(&mut app, 'c');
            let Action::Mutate { machine, request } = press_code(&mut app, KeyCode::Enter) else {
                panic!("not sent: {}", said(&app))
            };
            app.queue_action(machine, request);
            let (n, _, request, _) = app.resolver.ready()[0].clone();
            (app, dir, n, request)
        };
        let (app, dir, _, request) = restored_and_sent();
        drop(app);
        let back = Kept::at(dir.path(), 0).take_words();
        assert!(back.draft.is_none(), "{back:?}");
        assert_eq!(back.returned.len(), 1, "{back:?}");
        assert_eq!(back.returned[0].words, "Fix the banner");
        assert_eq!(
            back.returned[0].from.as_ref().map(|f| f.id.as_str()),
            request["submission"].as_str()
        );
        let (mut app, dir, n, request) = restored_and_sent();
        app.finish_job(n, request, ok(json!({"id": "0700"})));
        drop(app);
        let back = Kept::at(dir.path(), 0).take_words();
        assert!(back.is_empty(), "{back:?}");
    }

    #[test]
    fn a_new_intent_isnt_sent_until_its_send_is_in_the_words_file() {
        // Codex's review 63, 2: a draft the last Kerdr kept, with no send of its own, restored and
        // sent while the words file can't be written. It isn't sent: it waits, said why, and the
        // file keeps the draft as it was, so a crash then sends nothing twice. Once the file can
        // be written, the send goes into it, and then goes.
        let dir = tempfile::tempdir().unwrap();
        let draft = Words {
            draft: Some(KeptWords {
                words: "Fix the banner".into(),
                ..KeptWords::default()
            }),
            ..Words::default()
        };
        Kept::at(dir.path(), 0).keep_words(&draft).unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let words = app.kept.take_words();
        app.restore_words(words);
        let words_file = dir.path().join("words.json");
        let before = std::fs::read_to_string(&words_file).unwrap();
        // Every write fails, its lock not to be had; the file itself stays as it is.
        let lock = dir.path().join("kept.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        press(&mut app, 'c');
        let Action::Mutate { machine, request } = press_code(&mut app, KeyCode::Enter) else {
            panic!("not sent: {}", said(&app))
        };
        app.queue_action(machine, request);
        assert!(
            app.resolver.ready().is_empty(),
            "sent before its words were kept"
        );
        let words_path = words_file.display().to_string();
        assert!(
            said(&app).starts_with(&format!(
                "Your new intent waits: Kerdr couldn't write {words_path} ("
            )),
            "{}",
            said(&app)
        );
        assert!(
            said(&app).ends_with(
                "It's sent once its words are kept there, so a crash can never save it twice."
            ),
            "{}",
            said(&app)
        );
        assert_eq!(std::fs::read_to_string(&words_file).unwrap(), before);
        app.keep_waiting_words();
        assert!(app.resolver.ready().is_empty(), "sent though still unkept");
        // The file can be written again: the send goes into it, then goes. A crash after that
        // gives the words back as that send.
        std::fs::remove_dir(&lock).unwrap();
        app.words_tried = None;
        app.keep_waiting_words();
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        let submission = ready[0].2["submission"].as_str().unwrap().to_string();
        drop(app);
        let back = Kept::at(dir.path(), 0).take_words();
        assert!(back.draft.is_none(), "{back:?}");
        assert_eq!(back.returned.len(), 1, "{back:?}");
        assert_eq!(
            back.returned[0].from.as_ref().map(|f| f.id.as_str()),
            Some(submission.as_str())
        );
    }
}
