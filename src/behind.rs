//! A key pressed on a card that's on its way somewhere waits rather than being refused (intent
//! 0591, release 4): behind its start or reopen until that's answered, behind a new intent's save
//! until it has its number, or behind its write-up, which the runner waits for. Once that's done,
//! Kerdr decides again with the card as it is then: what still makes sense goes, and what doesn't
//! is said and dropped. Each shows on its card meanwhile, and m then D drops it.

use std::time::Instant;

use serde_json::{Value, json};

use super::holding::held_what;
use super::{App, Mode, Tone, waiting_work};
use crate::model::{Card, CardSnapshot};
use crate::resolve::{Behind, Dropping};

/// What a held key comes to once what it waited for is done.
enum Decided {
    /// It goes, to this card.
    Go(Box<Card>),
    /// It no longer makes sense, said in these words.
    Drop(String),
}

/// e pressed on a card being written up or saved: its text opens once that's done, if Andrew is
/// still on the card (intent 0591, K1-78, K1-42).
#[derive(Clone, Debug, PartialEq)]
pub struct EditAfter {
    /// The card it was pressed on: a new intent's saving card, until it has its number.
    pub key: String,
    /// The start that waits for this edit, if d was pressed before (K1-82): the runner keeps it
    /// waiting until the editor closes.
    pub paused: Option<u64>,
    pub since: Instant,
    /// The trips into herdr by then: a trip since means he's elsewhere, and it doesn't open.
    pub away: u64,
}

/// The changes a key can be held for: each changes a card's record, and none reads.
/// Closable as C closes it, or would be once its write-up lands: C's yes on work still being
/// written up waits for that (K1-23).
pub(crate) fn closable_once_written(c: &Card) -> bool {
    c.closable()
        || (c.intake() == Some("pending") && {
            let mut written = c.clone();
            if let Some(detail) = written.detail.as_mut() {
                detail.intake = None;
            }
            written.closable()
        })
}

/// Whether a request is the yes to "Cancel #N once it opens?": a stop by number alone, asked by
/// Kerdr while the card's start is on its way.
pub(crate) fn is_cancel_once_open(request: &Value) -> bool {
    request["action"] == "cancel"
        && request.get("id").is_some()
        && [
            "session",
            "workspace",
            "tab",
            "outside_since",
            "confirm",
            "to",
        ]
        .iter()
        .all(|f| request.get(*f).is_none())
}

fn holdable(request: &Value) -> bool {
    matches!(
        request["action"].as_str(),
        Some("dispatch" | "close" | "dismiss" | "cancel" | "restore" | "move" | "refresh")
    ) && request["id"].as_str().is_some()
}

/// The submission a new intent's saving card stands for, by its key.
pub fn saving_submission(key: &str) -> Option<&str> {
    key.split_once(":saving-").map(|(_, s)| s)
}

impl App {
    /// Holds a change to a card that's on its way somewhere: behind its start or its save, in
    /// Kerdr, or behind its write-up, in the runner. Whether it's held (said, and shown on the
    /// card).
    pub(crate) fn hold_behind(
        &mut self,
        machine: usize,
        request: &Value,
        from: Option<&CardSnapshot>,
        key: Option<&str>,
    ) -> bool {
        if !holdable(request) {
            return false;
        }
        let Some(card) = key.and_then(|k| self.card(k)).cloned() else {
            return false;
        };
        let id = request["id"].as_str().unwrap_or_default();
        let what = held_what(request);
        let starts = matches!(request["action"].as_str(), Some("dispatch" | "reopen"));
        let from = from.cloned().or_else(|| Some(CardSnapshot::of(&card)));
        // A new intent still being saved: no number to send it with yet.
        if card.intake() == Some(crate::resolve::SAVING)
            && let Some(submission) = saving_submission(&card.key)
        {
            let behind = Behind::Save(submission.to_string());
            self.resolver.add_behind(
                machine,
                request.clone(),
                from,
                Some(card.key.clone()),
                behind,
            );
            self.say(
                format!("Held: {what} on your new intent goes once it's saved and has its number."),
                Tone::Info,
            );
            self.rebuild();
            return true;
        }
        // Its start or reopen is on its way, unanswered: decided again once it's answered and
        // shown. One sent once the start has answered (a stop Esc asked for) goes as it is.
        let unanswered = self
            .resolver
            .starting(&card.key, id)
            .is_some_and(|j| j.settled.is_none());
        if !starts && unanswered {
            self.resolver.add_behind(
                machine,
                request.clone(),
                from,
                Some(card.key.clone()),
                Behind::Start,
            );
            self.say(
                format!(
                    "Held: {what} on {} goes once its start is answered.",
                    card.reference
                ),
                Tone::Info,
            );
            self.rebuild();
            return true;
        }
        // Still being written up: the runner holds it until its machine's feed shows that done,
        // as it does a start d asked for then.
        if card.intake() == Some("pending") {
            let effect = self.effect_for(&card.key, request);
            let n = self.resolver.add_gated(
                machine,
                request.clone(),
                from,
                Some(card.key.clone()),
                effect,
            );
            if starts {
                self.note_workspaces_for(n, machine);
                self.say(
                    format!(
                        "{} is still being written up from your words; it starts as soon as that's done. Esc on the board drops the start.",
                        card.reference
                    ),
                    Tone::Info,
                );
            } else {
                self.say(
                    format!(
                        "Held: {what} on {} goes once it's written up.",
                        card.reference
                    ),
                    Tone::Info,
                );
            }
            self.rebuild();
            return true;
        }
        false
    }

    /// Decides again each change held behind a start or a save whose wait is over: what still
    /// makes sense for its card as it is now goes; anything else is said and dropped.
    pub(crate) fn release_behind(&mut self) {
        for (n, behind) in self.resolver.behind_jobs() {
            let Some(job) = self.resolver.job(n) else {
                continue;
            };
            let request = job.request.clone();
            let key = job.key.clone();
            let reference = key
                .as_deref()
                .and_then(|k| self.card(k))
                .map(|c| c.reference.clone())
                .unwrap_or_else(|| self.reference_for(request["id"].as_str().unwrap_or_default()));
            let id = request["id"].as_str().unwrap_or_default().to_string();
            let card = match &behind {
                Behind::Start => {
                    if key
                        .as_deref()
                        .is_some_and(|k| self.resolver.starting(k, &id).is_some())
                    {
                        continue;
                    }
                    key.as_deref().and_then(|k| self.card(k)).cloned()
                }
                Behind::Save(submission) => {
                    match self.saved_card(submission) {
                        Some(card) => Some(card),
                        // Still saving, here or (after a restart) in its backend: it waits on.
                        None if key.as_deref().is_some_and(|k| self.card(k).is_some())
                            || self.saving_in_a_backend(submission) =>
                        {
                            continue;
                        }
                        None => None,
                    }
                }
            };
            match self.decide_held(&request, card, &behind, &reference) {
                // It goes the way any key's change does, from its card as it is now: through
                // a machine that forwards, held for one that isn't answering, or behind its
                // write-up.
                Decided::Go(card) => {
                    let card = *card;
                    let mut request = request;
                    if let Some(detail) = &card.detail {
                        request["id"] = Value::String(detail.id.clone());
                    }
                    // A record from the hold file stays that record, with its op, its markers
                    // and its `rev`, until it's answered (Codex's review 60); any other key gets
                    // an op of its own as it goes.
                    let carried = self.held_sending.remove(&n);
                    let rev = self.resolver.rev_of(n);
                    if carried.is_none()
                        && let Some(fields) = request.as_object_mut()
                    {
                        fields.remove("op");
                    }
                    // A stop asked for before its workspace opened names it now, for a backend
                    // that doesn't ask its own question first: his yes was to stopping it there.
                    if is_cancel_once_open(&request) && !self.does_steps(&card) {
                        if let Some(session) = &card.session {
                            request["session"] = Value::String(session.clone());
                        }
                        if let Some(workspace) = &card.workspace_id {
                            request["workspace"] = Value::String(workspace.clone());
                        }
                        if let Some(tab) = &card.tab_id {
                            request["tab"] = Value::String(tab.clone());
                        }
                    }
                    let machine = self.home_machine(&card).unwrap_or(card.machine_index);
                    self.resolver.drop_held_job(n);
                    self.action_from = Some(CardSnapshot::of(&card));
                    self.action_key = Some(card.key.clone());
                    let before = self.resolver.last_added();
                    self.carrying = carried.clone();
                    self.queue_action(machine, request);
                    self.carrying = None;
                    if let Some(op) = carried {
                        self.carry_on(before, op, rev);
                    }
                }
                Decided::Drop(said) => {
                    self.resolver.drop_held_job(n);
                    if let Some(op) = self.held_sending.remove(&n) {
                        self.kept.release(&op);
                    }
                    self.say_also(said);
                }
            }
            self.rebuild();
        }
    }

    /// After a hold-file record went the way any key's change does: the change it became carries
    /// its op and its `rev` on, and its answer takes it out of the file. Held for its machine
    /// again, it waits in the file still (route); not sent, and said why, it leaves the file.
    pub(crate) fn carry_on(&mut self, before: u64, op: String, rev: Option<String>) {
        let n = self.resolver.last_added();
        if n != before {
            self.resolver.keep_op(n, &op);
            if rev.is_some() {
                self.resolver.set_rev(n, rev);
            }
            self.held_sending.insert(n, op);
        } else if self.kept.find(&op).is_some_and(|h| h.by.is_some()) {
            self.kept.release(&op);
        }
    }

    /// Whether a feed lists this new intent's save as one its backend is still finishing.
    fn saving_in_a_backend(&self, submission: &str) -> bool {
        self.machines
            .iter()
            .filter_map(|m| m.feed.as_ref())
            .any(|f| {
                f.held
                    .iter()
                    .any(|h| h.submission.as_deref() == Some(submission))
            })
    }

    /// The card a new intent's save made, by its submission, once the feed has it.
    fn saved_card(&self, submission: &str) -> Option<Card> {
        self.board
            .cards
            .iter()
            .find(|c| {
                c.intake() != Some(crate::resolve::SAVING)
                    && c.detail
                        .as_ref()
                        .is_some_and(|d| d.submission.as_deref() == Some(submission))
            })
            .cloned()
    }

    /// Whether a held change still makes sense for its card as it is now.
    fn decide_held(
        &self,
        request: &Value,
        card: Option<Card>,
        behind: &Behind,
        reference: &str,
    ) -> Decided {
        let what = held_what(request);
        let Some(card) = card else {
            return Decided::Drop(match behind {
                Behind::Save(_) => {
                    format!("{what} on your new intent was dropped: it wasn't saved.")
                }
                Behind::Start => {
                    format!("{reference} is gone from the board, so {what} wasn't done.")
                }
            });
        };
        let reference = card.reference.clone();
        let waiting = waiting_work(&card);
        let open = card.is_open_workspace();
        let finished = card.done() || card.state.as_deref() == Some("closed");
        let changed = || {
            Decided::Drop(format!(
                "{reference} changed meanwhile, so {what} wasn't done."
            ))
        };
        match request["action"].as_str().unwrap_or_default() {
            "dispatch" | "reopen" if waiting => Decided::Go(Box::new(card)),
            "dispatch" | "reopen" => Decided::Drop(format!(
                "{reference} is under way already, so {what} wasn't needed."
            )),
            "close" | "dismiss" if waiting => Decided::Go(Box::new(card)),
            // Started meanwhile: a backend that does each key's step first asks, naming what
            // stops (K1-22); with any other, Kerdr doesn't set aside work an agent may be at.
            "close" | "dismiss" if open && !finished && self.does_steps(&card) => {
                Decided::Go(Box::new(card))
            }
            "close" | "dismiss" if open => Decided::Drop(format!(
                "{reference} started meanwhile and has a workspace open, so {what} wasn't done: an agent may be at it. ⏎ goes in, and X stops it."
            )),
            "close" | "dismiss" => changed(),
            "cancel" if open => Decided::Go(Box::new(card)),
            "cancel" if waiting => Decided::Drop(format!(
                "{reference} didn't open a workspace, so there was nothing to stop: x sets it aside."
            )),
            "cancel" => changed(),
            "restore" if card.dismissed() => Decided::Go(Box::new(card)),
            "restore" => Decided::Drop(format!(
                "{reference} isn't set aside any more, so {what} wasn't needed."
            )),
            "move" | "refresh" if waiting => Decided::Go(Box::new(card)),
            "move" | "refresh" => Decided::Drop(format!(
                "{reference} started meanwhile, so {what} wasn't done: it has no place in the backlog order now."
            )),
            _ => Decided::Go(Box::new(card)),
        }
    }

    /// Says a line, after a failure already up rather than in its place: a held key a failed save
    /// drops is said with that failure (K1-42).
    pub(crate) fn say_also(&mut self, line: String) {
        match &self.message {
            Some((text, tone @ (Tone::Fail | Tone::Warn), _)) => {
                let text = format!("{text}\n{line}");
                self.message = Some((text, *tone, Instant::now()));
            }
            _ => self.say(line, Tone::Info),
        }
    }

    /// x, C or X on a card whose start is on its way with nothing open yet (K1-41): asked as what
    /// it will be, a stop once its workspace opens, and held until then.
    pub(crate) fn cancel_once_open(&mut self, card: &Card) -> bool {
        let Some(id) = card.detail.as_ref().map(|d| d.id.clone()) else {
            return false;
        };
        let asking = match self.resolver.starting(&card.key, &id) {
            Some(job) => job.asking,
            None => return false,
        };
        if asking || card.is_open_workspace() {
            return false;
        }
        let Some(machine) = self.home_machine(card) else {
            return true;
        };
        if !self.can_change(machine) {
            return true;
        }
        let reference = &card.reference;
        self.mode = Mode::Confirm {
            machine,
            request: json!({"action": "cancel", "id": id}),
            title: format!("Cancel {reference} once it opens?"),
            message: format!(
                "{reference}'s start is on its way. Yes stops its agent once its workspace opens and sets it aside in Won't do; no lets it start."
            ),
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
            queue: false,
            ready_at: None,
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
        true
    }

    /// The lines a card shows for changes held behind its start, its save or its write-up, each
    /// with the job m's D drops.
    pub(crate) fn behind_lines(&self, c: &Card) -> Vec<(String, u64)> {
        let mut lines = vec![];
        for (n, behind) in self.resolver.behind_jobs() {
            let Some(job) = self.resolver.job(n) else {
                continue;
            };
            if job.key.as_deref() != Some(c.key.as_str()) {
                continue;
            }
            let what = held_what(&job.request);
            let words = match behind {
                Behind::Start => format!("{what} goes once its start is answered"),
                Behind::Save(_) => format!("{what} goes once it's saved"),
            };
            lines.push((words, n));
        }
        for job in self.resolver.gated_changes() {
            if job.key.as_deref() == Some(c.key.as_str()) {
                let what = held_what(&job.request);
                lines.push((format!("{what} goes once it's written up"), job.n));
            }
        }
        lines
    }

    /// m's D on a change on its way, by where it is (Codex's review 60): one still in Kerdr is
    /// taken out, and said; one the runner has is asked of it, and its answer says whether it
    /// was in time (job_drop_answered); one whose command has started is past dropping, and that's
    /// said. A hold-file record it was leaves the file once it's dropped.
    pub(crate) fn drop_behind(&mut self, n: u64, card: &Card) {
        let Some(job) = self.resolver.job(n) else {
            return;
        };
        let what = held_what(&job.request);
        let name = self
            .machines
            .get(job.machine)
            .map(|m| m.name())
            .unwrap_or_default();
        let reference = card.reference.clone();
        match self.resolver.drop_job(n) {
            Dropping::Taken(_) => {
                if let Some(op) = self.held_sending.remove(&n) {
                    self.kept.release(&op);
                }
                self.say(
                    format!("Dropped: {what} on {reference} won't be sent."),
                    Tone::Info,
                );
            }
            Dropping::Asked => {
                self.drops_asked.insert(n, (what, reference));
            }
            Dropping::Started => self.say(
                format!(
                    "{what} on {reference} is already with {name}, so it can't be dropped now: its card shows what it does."
                ),
                Tone::Info,
            ),
            Dropping::Gone => {}
        }
        self.rebuild();
    }

    /// The footing of the card a change waiting for its write-up is for, as it is now: the runner
    /// holds it to that (crate::resolve::Footing).
    pub fn gate_footing(&self, n: u64) -> Option<crate::resolve::Footing> {
        let key = self.resolver.key_of(n)?;
        let detail = self.card(&key)?.detail.as_ref()?;
        Some(crate::resolve::Footing::of(detail))
    }

    /// The runner took out a change that waited for its write-up, unstarted: its card was set
    /// aside, finished or started meanwhile. It goes, a hold-file record it was with it, and
    /// that's said (Codex's review 60).
    pub fn gate_changed(&mut self, n: u64) {
        self.drops_asked.remove(&n);
        let Some(job) = self.resolver.take(n) else {
            return;
        };
        if let Some(op) = self.held_sending.remove(&n) {
            self.kept.release(&op);
        }
        let what = held_what(&job.request);
        let reference = job
            .key
            .as_deref()
            .and_then(|k| self.card(k))
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| self.reference_for(job.request["id"].as_str().unwrap_or_default()));
        let said = if job.starts() {
            format!(
                "{reference} wasn't started: it changed since you pressed {what}, so Kerdr dropped that start."
            )
        } else {
            format!("{reference} changed since you pressed {what}, so Kerdr dropped that change.")
        };
        self.say_also(said);
        self.rebuild();
    }

    /// The runner's answer to a drop, Esc's on a start waiting for its write-up or m's D:
    /// taken out before it started, it goes, and a hold-file record it was leaves the file;
    /// otherwise its answer still comes. m's D says which (Codex's review 60).
    pub fn job_drop_answered(&mut self, n: u64, dropped: bool) {
        self.resolver.drop_answered(n, dropped);
        if dropped && let Some(op) = self.held_sending.remove(&n) {
            self.kept.release(&op);
        }
        let Some((what, reference)) = self.drops_asked.remove(&n) else {
            return;
        };
        let said = if dropped {
            format!("Dropped: {what} on {reference} won't be sent.")
        } else {
            format!(
                "{what} on {reference} had already gone when D came, so it carries on: its card shows what it does."
            )
        };
        self.say(said, Tone::Info);
        self.rebuild();
    }

    /// e on a card still being written up, or a new intent still being saved: its text opens once
    /// that's done (K1-78, K1-42), and a start d asked for waits for the edit (K1-82).
    pub(crate) fn edit_once_written(&mut self, card: &Card) {
        let paused = self
            .resolver
            .start_waiting_for_write_up(&card.key)
            .map(|j| j.n);
        if let Some(n) = paused {
            self.resolver.pause(n, true);
        }
        if let Some(old) = self.edit_after.take()
            && old.key != card.key
            && let Some(n) = old.paused
        {
            self.resolver.pause(n, false);
        }
        self.edit_after = Some(EditAfter {
            key: card.key.clone(),
            paused,
            since: Instant::now(),
            away: self.away,
        });
        let saving = card.intake() == Some(crate::resolve::SAVING);
        let what = if saving {
            "Your new intent is still being saved: its text opens once it's saved and written up"
                .to_string()
        } else {
            format!(
                "{} is still being written up: its text opens once that's done",
                card.reference
            )
        };
        let start = if paused.is_some() {
            ", and its start waits for your edit"
        } else {
            ""
        };
        self.say(
            format!("{what}{start}. Esc on the board stops waiting."),
            Tone::Info,
        );
    }

    /// After a read: the text e waited for opens if Andrew is still on its card; otherwise it's
    /// said to be ready, and a start it held goes. The action to take, if e goes now.
    pub(crate) fn edit_after_due(&mut self) -> Option<super::Action> {
        let wait = self.edit_after.clone()?;
        let card = match saving_submission(&wait.key) {
            Some(submission) => match self.saved_card(submission) {
                Some(card) => card,
                None if self.card(&wait.key).is_some() => return None,
                None => {
                    self.edit_after = None;
                    return None;
                }
            },
            None => self.card(&wait.key)?.clone(),
        };
        let id = card
            .detail
            .as_ref()
            .map(|d| d.id.clone())
            .unwrap_or_default();
        if card.intake() == Some("pending") || self.edit_on_its_way(&card.key, &id, false) {
            return None;
        }
        self.edit_after = None;
        let here = matches!(
            self.mode,
            Mode::Board | Mode::Backlog { .. } | Mode::Peek { .. } | Mode::Card { .. }
        ) && [wait.key.as_str(), card.key.as_str()]
            .contains(&self.sel.key.as_deref().unwrap_or_default());
        if here && self.away == wait.away {
            if self.sel.key.as_deref() == Some(wait.key.as_str()) {
                self.sel.key = Some(card.key.clone());
            }
            // The start that waited goes on waiting while the editor is open, and goes once it
            // closes (edit_pauses_due).
            if let Some(n) = wait.paused {
                self.edit_paused.insert(card.key.clone(), n);
            }
            return Some(self.edit_action_on(card));
        }
        if let Some(n) = wait.paused {
            self.resolver.pause(n, false);
        }
        self.say(
            format!("{} is written up: e opens its text.", card.reference),
            Tone::Info,
        );
        None
    }

    /// Esc on the board: e stops waiting for a write-up, and a start it held goes.
    pub(crate) fn stop_edit_after(&mut self) -> Option<String> {
        let wait = self.edit_after.take()?;
        if let Some(n) = wait.paused {
            self.resolver.pause(n, false);
        }
        let reference = self
            .card(&wait.key)
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| "Its card".into());
        Some(format!("{reference}'s text won't open by itself."))
    }

    /// The editor closed on a card whose start waited for the edit (K1-82): the start goes after
    /// the save, if there was one, which the runner puts it behind, else as it would have.
    pub(crate) fn edit_closed_for(&mut self, key: &str) {
        let Some(n) = self.edit_paused.remove(key) else {
            return;
        };
        self.resolver.pause(n, false);
    }

    /// Starts kept waiting for an edit whose editor won't open after all (its read failed, or you
    /// moved off the card first): they go.
    pub(crate) fn edit_pauses_due(&mut self) {
        let done: Vec<String> = self
            .edit_paused
            .keys()
            .filter(|k| {
                self.edit_open.as_deref() != Some(k.as_str())
                    && !self.edit_flights.iter().any(|f| &f.key == *k)
            })
            .cloned()
            .collect();
        for key in done {
            self.edit_closed_for(&key);
        }
    }

    /// The start that waits for e's edit on this card, kept waiting while the editor is open.
    pub(crate) fn hold_start_for_edit(&mut self, key: &str) {
        if self.edit_paused.contains_key(key) {
            return;
        }
        if let Some(n) = self.resolver.start_waiting_for_write_up(key).map(|j| j.n) {
            self.resolver.pause(n, true);
            self.edit_paused.insert(key.to_string(), n);
        }
    }

    /// Notes the workspaces a start's machine has now, as queue_action does.
    fn note_workspaces_for(&mut self, n: u64, machine: usize) {
        if let Some(name) = self.machines.get(machine).map(|m| m.name()) {
            let before = self
                .board
                .workspaces
                .iter()
                .filter(|w| w.machine == name)
                .map(|w| (w.session.clone(), w.id.clone()))
                .collect();
            self.resolver.note_workspaces(n, before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Response;
    use crate::app::Action;
    use crate::app::holding::HOLD;
    use crate::app::tests::{press, room_in_every_session};
    use crate::config::Config;
    use crate::kept::Kept;

    fn app() -> App {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.cfg.actions.command = Some("test-adapter".into());
        room_in_every_session(&mut app);
        app
    }

    /// A card waiting in the backlog, selected: its key, id, reference and machine.
    fn waiting(app: &mut App) -> (String, String, String, usize) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| waiting_work(c) && c.intake().is_none() && !c.dismissed())
            .expect("a waiting backlog card in the demo")
            .clone();
        select(app, &card.key);
        let id = card.detail.as_ref().unwrap().id.clone();
        (card.key, id, card.reference, card.machine_index)
    }

    fn select(app: &mut App, key: &str) {
        let col = app.card(key).unwrap().stage.index();
        app.mode = Mode::Board;
        app.sel = crate::app::Selection {
            col,
            key: Some(key.to_string()),
        };
    }

    fn said(app: &App) -> String {
        app.message()
            .map(|(t, _)| t.to_string())
            .unwrap_or_default()
    }

    fn ok() -> Result<Response, String> {
        Ok(Response {
            ok: true,
            ..Default::default()
        })
    }

    /// The yes to the question up now, as the board's loop takes it.
    fn yes(app: &mut App) {
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
        let action = press(app, 'y');
        let Action::Mutate { machine, request } = action else {
            panic!("no yes sent: {action:?} {:?} {}", app.mode, said(app))
        };
        app.queue_action(machine, request);
    }

    /// A key, as the board's loop takes it.
    fn key(app: &mut App, c: char) {
        if let Action::Mutate { machine, request } = press(app, c) {
            app.queue_action(machine, request);
        }
    }

    /// d on the selected card, its start handed to the runner: its job's number and request.
    fn start(app: &mut App) -> (u64, Value) {
        key(app, 'd');
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(app));
        (ready[0].0, ready[0].2.clone())
    }

    #[test]
    fn x_while_a_start_is_on_its_way_asks_to_stop_it_once_it_opens_and_waits_for_its_answer() {
        let mut app = app();
        let (key, _, reference, _) = waiting(&mut app);
        let (n, request) = start(&mut app);
        // x asks for what it will be: a stop once its workspace opens (K1-41).
        select(&mut app, &key);
        press(&mut app, 'x');
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("not asked: {}", said(&app))
        };
        assert_eq!(title, &format!("Cancel {reference} once it opens?"));
        yes(&mut app);
        assert_eq!(
            said(&app),
            format!("Held: X on {reference} goes once its start is answered.")
        );
        assert!(app.resolver.ready().is_empty(), "nothing goes yet");
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("⧗ X goes once its start is answered")
        );
        // Its start is refused, so nothing opened: there's nothing to stop, and it's said.
        app.finish_job(n, request, Err("no room".into()));
        assert!(app.resolver.behind_jobs().is_empty());
        assert!(app.resolver.ready().is_empty());
        assert!(
            said(&app).contains(&format!(
                "{reference} didn't open a workspace, so there was nothing to stop: x sets it aside."
            )),
            "{}",
            said(&app)
        );
    }

    #[test]
    fn x_while_a_start_waits_for_its_question_asks_as_ever_and_goes_once_nothing_started() {
        // K1-39: the start is waiting for its backend's question, so nothing will open: x is
        // the set-aside question it always was, and its yes waits for that answer.
        let mut app = app();
        let (key, _, reference, _) = waiting(&mut app);
        let (n, request) = start(&mut app);
        app.resolver.mark_asking(n);
        select(&mut app, &key);
        press(&mut app, 'x');
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("not asked: {}", said(&app))
        };
        assert_ne!(title, &format!("Cancel {reference} once it opens?"));
        yes(&mut app);
        assert_eq!(
            said(&app),
            format!("Held: x on {reference} goes once its start is answered.")
        );
        assert!(app.resolver.ready().is_empty());
        // Its answer started nothing: the set-aside goes.
        app.finish_job(n, request, Err("no room".into()));
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "dismiss");
    }

    #[test]
    fn a_backlog_key_while_a_start_is_on_its_way_goes_once_it_is_answered_if_it_still_fits() {
        // K1-39, K1-41: J waits behind the start, and goes only while the card still waits.
        let mut app = app();
        let (key, _, reference, _) = waiting(&mut app);
        let (n, request) = start(&mut app);
        select(&mut app, &key);
        key_move(&mut app);
        assert!(
            said(&app).starts_with("Held: the move on"),
            "{}",
            said(&app)
        );
        assert!(app.resolver.ready().is_empty());
        // Refused, the card still waits to start: the move goes now.
        app.finish_job(n, request, Err("no room".into()));
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "move");
        // Started meanwhile: it has no place in the backlog order, and that's said.
        let mut app = self::app();
        let (key, id, reference2, m) = waiting(&mut app);
        let (n, request) = start(&mut app);
        select(&mut app, &key);
        key_move(&mut app);
        app.finish_job(n, request, ok());
        for c in app.machines[m].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.state = Some("active".into());
                c.stage = Some("build".into());
            }
        }
        app.rebuild();
        app.resolver.read(m, app.machines[m].feed.as_ref());
        app.feed_read(m);
        assert!(app.resolver.behind_jobs().is_empty(), "{}", said(&app));
        assert!(
            said(&app).contains(&format!(
                "{reference2} started meanwhile, so the move wasn't done"
            )),
            "{}",
            said(&app)
        );
        assert!(!reference.is_empty());
    }

    /// J on the selected card: a move down, as the board's loop takes it.
    fn key_move(app: &mut App) {
        key(app, 'J');
    }

    #[test]
    fn c_on_work_still_being_written_up_asks_then_waits_for_the_write_up_and_m_drops_it() {
        // K1-23: asked first, then held in the runner until its write-up lands.
        let mut app = app();
        let (key, _, reference, _) = waiting(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        press(&mut app, 'C');
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("not asked: {}", said(&app))
        };
        assert_eq!(title, &format!("Mark {reference} done?"));
        yes(&mut app);
        assert_eq!(
            said(&app),
            format!("Held: C on {reference} goes once it's written up.")
        );
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1);
        let (n, _, request, gated) = ready[0].clone();
        assert_eq!(request["action"], "close");
        assert!(gated, "the runner holds it for the write-up");
        let card = app.card(&key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("⧗ C goes once it's written up")
        );
        // m, then D: the runner is asked, and its answer says it was dropped before it went.
        select(&mut app, &key);
        press(&mut app, 'm');
        assert_eq!(press(&mut app, 'D'), Action::None);
        assert_eq!(app.resolver.take_dropped(), vec![n]);
        app.job_drop_answered(n, true);
        assert_eq!(
            said(&app),
            format!("Dropped: C on {reference} won't be sent.")
        );
        assert!(app.resolver.job(n).is_none());
        // X on it: set aside after its question, held the same way.
        let mut app = self::app();
        let (key, _, reference, _) = waiting(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        press(&mut app, 'X');
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{}", said(&app));
        yes(&mut app);
        let ready = app.resolver.ready();
        assert_eq!(ready[0].2["action"], "dismiss");
        assert!(ready[0].3);
        assert_eq!(
            said(&app),
            format!("Held: x on {reference} goes once it's written up.")
        );
    }

    /// A card whose workspace is open while its start's command is still unanswered: its key, id,
    /// machine, and the start's job and request, handed to the runner.
    fn answering(app: &mut App) -> (String, String, usize, u64, Value) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.is_open_workspace() && c.detail.is_some() && !c.done() && !c.dismissed())
            .cloned()
            .expect("a card with its workspace open in the demo");
        let id = card.detail.as_ref().unwrap().id.clone();
        let request = json!({"action":"dispatch","id":id});
        let n = app.resolver.add(
            card.machine_index,
            request.clone(),
            Some(CardSnapshot::of(&card)),
            Some(card.key.clone()),
            None,
        );
        app.resolver.ready();
        select(app, &card.key);
        (card.key, id, card.machine_index, n, request)
    }

    /// Its start answered and the feed read again, its workspace still open.
    fn answered(app: &mut App, machine: usize, n: u64, request: Value) {
        app.finish_job(n, request, ok());
        app.resolver
            .read(machine, app.machines[machine].feed.as_ref());
        app.feed_read(machine);
    }

    fn steps(app: &mut App) {
        for machine in &mut app.machines {
            if let Some(feed) = machine.feed.as_mut() {
                feed.supports.push("steps".into());
            }
        }
    }

    #[test]
    fn c_while_an_open_workspace_takes_its_brief_waits_for_the_start_then_asks_as_k1_22_does() {
        // K1-40, where its backend does each key's step first: the close goes once the start is
        // answered, and that backend asks what to stop.
        let mut app = app();
        steps(&mut app);
        let (key, id, m, n, request) = answering(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        key_c(&mut app);
        assert_eq!(
            said(&app),
            format!("Held: C on {reference} goes once its start is answered.")
        );
        assert!(app.resolver.ready().is_empty());
        answered(&mut app, m, n, request);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "close");
        assert_eq!(ready[0].2["id"], id.as_str());
        assert!(
            ready[0].2.get("workspace").is_none(),
            "its backend asks where"
        );
        // Where it doesn't: dropped and said, never a set-aside under a running agent.
        let mut app = self::app();
        let (key, _, m, n, request) = answering(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let card = app.card(&key).unwrap().clone();
        app.queue_action(m, json!({"action":"close","id":card.detail.unwrap().id}));
        assert!(app.resolver.ready().is_empty());
        answered(&mut app, m, n, request);
        assert!(app.resolver.ready().is_empty());
        assert!(app.resolver.behind_jobs().is_empty());
        assert!(
            said(&app).contains(&format!(
                "{reference} started meanwhile and has a workspace open, so C wasn't done: an agent may be at it. ⏎ goes in, and X stops it."
            )),
            "{}",
            said(&app)
        );
    }

    /// C on the selected card, as the board's loop takes it.
    fn key_c(app: &mut App) {
        key(app, 'C');
    }

    /// A new intent sent to machine 0: its saving card, selected, and its request.
    fn saving(app: &mut App) -> (String, Value) {
        app.queue_action(
            0,
            json!({"action":"create","words":"Fix the feed","submission":"s-1","session":"web"}),
        );
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        assert!(n > 0);
        let key = app.placeholder_key(0, "s-1");
        select(app, &key);
        (key, request)
    }

    #[test]
    fn d_on_a_new_intent_still_saving_starts_it_once_it_has_its_number() {
        // K1-42.
        let mut app = app();
        let (card_key, create) = saving(&mut app);
        let create_n = app.resolver.saving("s-1").unwrap().n;
        key(&mut app, 'd');
        assert_eq!(
            said(&app),
            "Held: d on your new intent goes once it's saved and has its number."
        );
        let card = app.card(&card_key).unwrap().clone();
        assert_eq!(
            app.held_line_for(&card).as_deref(),
            Some("✎ d goes once it's saved")
        );
        assert!(app.resolver.ready().is_empty());
        // Saved as #0700, and the feed has it: the start goes, to #0700.
        app.finish_job(
            create_n,
            create,
            Ok(Response {
                ok: true,
                id: Some("0700".into()),
                ..Default::default()
            }),
        );
        let mut landed = app.machines[0].feed.as_ref().unwrap().cards[0].clone();
        landed.id = "0700".into();
        landed.title = "Fix the feed".into();
        landed.submission = Some("s-1".into());
        landed.stage = Some("backlog".into());
        landed.state = Some("queued".into());
        landed.intake = None;
        landed.disposition = None;
        landed.workspace = None;
        landed.held.clear();
        app.machines[0].feed.as_mut().unwrap().cards.push(landed);
        app.rebuild();
        app.feed_read(0);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "dispatch");
        assert_eq!(ready[0].2["id"], "0700");
    }

    #[test]
    fn a_key_held_for_a_new_intent_that_fails_to_save_is_dropped_and_said_with_the_failure() {
        let mut app = app();
        let (_, create) = saving(&mut app);
        let create_n = app.resolver.saving("s-1").unwrap().n;
        key(&mut app, 'd');
        app.finish_job(create_n, create, Err("the store is locked".into()));
        assert!(app.resolver.behind_jobs().is_empty());
        let said = said(&app);
        assert!(said.contains("the store is locked"), "{said}");
        assert!(
            said.ends_with("d on your new intent was dropped: it wasn't saved."),
            "{said}"
        );
    }

    #[test]
    fn a_key_held_behind_a_start_when_kerdr_quits_is_decided_by_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let (key, _, _, m) = waiting(&mut app);
        app.machines[m]
            .feed
            .as_mut()
            .unwrap()
            .supports
            .push(HOLD.into());
        start(&mut app);
        select(&mut app, &key);
        key_move(&mut app);
        assert_eq!(app.resolver.behind_jobs().len(), 1);
        // Quit before its start is answered: the move is kept, to be decided again, and stays this
        // Kerdr's until it has gone (Codex's review 62, 6).
        app.keep_on_quit().unwrap();
        drop(app);
        // The next Kerdr: no start is on its way there, and the card still waits, so it goes.
        let mut next = self::app();
        next.kept = Kept::at(dir.path(), next.now().timestamp());
        assert_eq!(next.kept.held[0].after.as_deref(), Some("start"));
        next.machines[m]
            .feed
            .as_mut()
            .unwrap()
            .supports
            .push(HOLD.into());
        next.feed_read(m);
        let ready = next.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&next));
        assert_eq!(ready[0].2["action"], "move");
        // Still the same record, with its op, until it's answered (Codex's review 60).
        assert_eq!(ready[0].2["op"], next.kept.held[0].op());
        next.finish_job(ready[0].0, ready[0].2.clone(), ok());
        assert!(next.kept.held.is_empty());
    }

    #[test]
    fn a_stop_held_behind_a_start_keeps_its_record_and_its_held_mark_after_a_restart() {
        // Codex's review 60, 6: a stop that waited behind a start when Kerdr quit goes from the
        // next one as the same record, with `held: true`, so its backend asks again before it
        // stops anything; and it stays in the file until it's answered, so a crash meanwhile
        // loses nothing.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.is_open_workspace() && c.detail.is_some() && !c.done() && !c.dismissed())
            .cloned()
            .expect("a card with its workspace open in the demo");
        let (m, id) = (card.machine_index, card.detail.as_ref().unwrap().id.clone());
        let holds = |app: &mut App| {
            let feed = app.machines[m].feed.as_mut().unwrap();
            feed.supports.push(HOLD.into());
        };
        holds(&mut app);
        let request = crate::actions::with_op(json!({"action":"cancel","id":id}));
        let op = request["op"].as_str().unwrap().to_string();
        app.kept.hold(crate::kept::HeldRequest {
            machine: app.machines[m].name(),
            request,
            rev: None,
            key: Some(card.key.clone()),
            reference: card.reference.clone(),
            what: "X".into(),
            pressed: app.now().timestamp(),
            after: Some("start".into()),
            by: None,
        });
        app.feed_read(m);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "cancel");
        assert_eq!(ready[0].2["held"], true);
        assert_eq!(ready[0].2["op"], op.as_str());
        assert_eq!(app.kept.held.len(), 1, "kept until it's answered");
        // This Kerdr ends before the answer comes: the next sends it again, the same request.
        drop(app);
        let mut next = self::app();
        next.kept = Kept::at(dir.path(), next.now().timestamp());
        holds(&mut next);
        next.feed_read(m);
        let ready = next.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&next));
        assert_eq!(ready[0].2["op"], op.as_str());
        assert_eq!(ready[0].2["held"], true);
    }

    #[test]
    fn a_stop_kept_from_before_a_restart_isnt_sent_to_a_backend_that_wouldnt_ask_again() {
        // Codex's review 62, 1: "Cancel #N once it opens?" answered yes, then Kerdr closed. Its
        // machine's backend now lists neither "hold" nor "steps", so nothing there would ask
        // again before stopping it: it stays held, and that's said. A backend that asks its own
        // question first ("steps") is sent it, naming no workspace, for it to ask.
        let stop_kept = |steps: bool| {
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
            let feed = app.machines[m].feed.as_mut().unwrap();
            feed.supports.retain(|s| s != HOLD && s != "steps");
            if steps {
                feed.supports.push("steps".into());
            }
            app.kept.hold(crate::kept::HeldRequest {
                machine: app.machines[m].name(),
                request: crate::actions::with_op(json!({"action":"cancel","id":id})),
                rev: None,
                key: Some(card.key.clone()),
                reference: card.reference.clone(),
                what: "X".into(),
                pressed: app.now().timestamp(),
                after: Some("start".into()),
                by: None,
            });
            app.feed_read(m);
            (app, card, m)
        };
        let (mut app, card, m) = stop_kept(false);
        assert!(
            app.resolver.ready().is_empty(),
            "sent without being asked again"
        );
        assert_eq!(app.kept.held.len(), 1);
        let name = app.machines[m].name();
        assert_eq!(
            said(&app),
            format!(
                "X on {} stays held: {name}'s backend wouldn't ask again before stopping it, so Kerdr won't send the yes you gave before it last closed. Press X again to be asked now, or m then D drops it.",
                card.reference
            )
        );
        let (mut app, _, _) = stop_kept(true);
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1, "{}", said(&app));
        assert_eq!(ready[0].2["action"], "cancel");
        assert!(ready[0].2.get("workspace").is_none(), "{}", ready[0].2);
    }

    #[test]
    fn a_start_waiting_for_its_write_up_goes_with_its_cards_footing_and_is_dropped_when_it_moved() {
        // Codex's review 60, 5: the runner holds a start d asked for during a write-up to the
        // card's footing then (crate::resolve::Footing), and takes it out, unstarted, once the
        // card has been set aside, finished or started meanwhile. The board says so.
        let mut app = app();
        let (key, _, reference, _) = waiting(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        key_d(&mut app);
        let (n, _, _, gated) = app.resolver.ready()[0].clone();
        assert!(gated);
        let footing = app.gate_footing(n).expect("its card's footing");
        assert!(!footing.set_aside && !footing.finished && !footing.under_way);
        let set_aside = crate::resolve::Footing {
            set_aside: true,
            ..footing
        };
        assert!(footing.moved(&set_aside));
        assert!(!set_aside.moved(&footing), "brought back isn't moved");
        app.gate_changed(n);
        assert!(app.resolver.job(n).is_none());
        assert_eq!(
            said(&app),
            format!(
                "{reference} wasn't started: it changed since you pressed d, so Kerdr dropped that start."
            )
        );
    }

    /// d on the selected card, as the board's loop takes it.
    fn key_d(app: &mut App) {
        key(app, 'd');
    }
}
