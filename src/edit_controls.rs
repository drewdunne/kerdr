//! `e` on a backlog card (intent 0414): the item's text, the words that started it, opened full
//! screen in your own editor, and saved through the action backend, which then has the card
//! written up again while you carry on.
//!
//! Both requests go through the action resolution tool (intent 0416). The save is a change like
//! any other: the card shows the new words and ✎ the moment the editor closes, its command runs
//! in the background, and when it lands nothing more is said. A feed that carries the item's text
//! opens the editor at once (intent 0491), and the save checks everything. Otherwise a read comes
//! first, which isn't a change: it says only that it's reading, and nothing waits for it. Unlike a
//! read, a text from the feed isn't refused for an agent the feed doesn't show yet: the save still
//! is, with what you wrote kept. Kerdr keeps your version until a save of it
//! succeeds, so a refusal, a change underneath or a lost answer never loses what you typed, and
//! says so in the red box that stays until it's read: a save that works says nothing, so one that
//! doesn't can't be missed.

use super::controls::short;
use super::*;
use crate::actions::Response;
use crate::edit::{self, Ended, Run};
use crate::model::CardSnapshot;
use serde_json::{Value, json};

/// How soon after `e` the text must come back for the editor to open by itself; later, Kerdr says
/// it's ready and `e` opens it, so an editor never springs up on you.
pub(crate) const OPEN_WITHIN: Duration = Duration::from_secs(15);

/// How long a text read but not opened yet is opened as it was read; after that `e` reads it
/// again.
const READY_FOR: Duration = Duration::from_secs(300);

/// Your version of an item's text, not saved yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mine {
    pub text: String,
    /// The item's text you wrote it from.
    pub from: String,
}

/// One item's text, as Kerdr holds it for editing.
#[derive(Clone, Debug)]
pub struct Held {
    pub id: String,
    /// The machine that answered for it, where a save goes.
    pub machine: usize,
    /// The item's text as the backend last gave it, and its fingerprint (`was`).
    pub current: String,
    pub was: String,
    pub read_at: Instant,
    /// Your version, kept until a save of it succeeds.
    pub mine: Option<Mine>,
    /// A save of `mine` got no clear answer: `e` reads the text first, to see whether it landed.
    pub check_first: bool,
    /// Opened only so you can take your words with you: the item can't be edited any more (it
    /// started, say), and this says why. Nothing is sent when the editor closes.
    pub read_only: Option<String>,
}

/// A read `e` sent: which item and card it's for, and when, counted in trips away from the board
/// (into herdr, say), so its answer opens the editor only while you're still where you pressed it.
#[derive(Clone, Debug)]
pub struct EditFlight {
    pub id: String,
    pub key: String,
    pub at: Instant,
    pub away: u64,
    /// What it said when it was sent, taken back when the answer comes.
    pub said: String,
}

/// When and where the editor was asked for: the `e` press it answers, the trips away from the board
/// by then, and the card you were on. It opens only while all three still hold.
#[derive(Clone, Debug)]
pub struct OpenWhen {
    pub asked: Instant,
    pub away: u64,
    pub from: CardSnapshot,
}

/// The editors that couldn't start before the one that opened the text, and that one.
fn skipped(run: &Run) -> Option<(String, &str)> {
    let editor = run.editor.as_deref()?;
    if run.skipped.is_empty() {
        return None;
    }
    let names: Vec<&str> = run
        .skipped
        .iter()
        .map(|s| s.split_once(": ").map_or(s.as_str(), |(name, _)| name))
        .collect();
    Some((names.join(", "), editor))
}

/// The editor's words for a message: what couldn't start before the one that opened it.
fn skipped_note(run: &Run) -> String {
    skipped(run)
        .map(|(names, editor)| format!(" ({names} couldn't start, so {editor} opened it.)"))
        .unwrap_or_default()
}

/// The file couldn't be read back once the editor closed: nothing was sent, and where the file is
/// kept for you.
fn unreadable(
    reference: &str,
    why: &str,
    kept: &std::path::Path,
    run: &Run,
    yours: &str,
) -> String {
    let editor = run.editor.as_deref().unwrap_or("your editor");
    format!(
        "Couldn't read {reference}'s text back after {editor} closed ({why}), so nothing was sent and it's as it was. The file is kept at {}: copy what you want from it, then delete it. e opens the text again{yours}.",
        kept.display()
    )
}

/// Whether a save's answer says it only partly happened: the text saved, something after it not.
fn partial(r: &Response) -> bool {
    r.status.as_deref() == Some("partial")
}

/// What a partly happened save says: the backend's words, which name what didn't happen.
fn partly_saved(reference: &str, r: Response) -> String {
    if r.message.trim().is_empty() {
        format!(
            "{reference}'s text is saved, but something after the save didn't happen; its action backend didn't say what."
        )
    } else {
        short(r.message)
    }
}

/// An action's error to its first clause: "the action timed out" rather than its advice on
/// retrying, which a save's own message replaces.
pub(super) fn first_clause(error: &str) -> &str {
    let end = [error.find("; "), error.find(". ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(error.len());
    error[..end].trim_end_matches('.')
}

impl App {
    /// `e`: the selected card's text, if it's backlog work.
    pub(super) fn edit_action(&mut self) -> Action {
        let Some(card) = self.selected_for_control().cloned() else {
            self.say("Select an intent first.", Tone::Info);
            return Action::None;
        };
        self.edit_action_on(card)
    }

    /// Whether a request for this item's text is on its way: its read (`reads`), or a save.
    fn edit_on_its_way(&self, id: &str, reads: bool) -> bool {
        self.resolver
            .on_its_way()
            .any(|j| j.request["action"] == "edit" && j.request["id"] == id && j.reads() == reads)
    }

    /// `e` on this card: refused plainly unless it's backlog work that's written up; else its
    /// text opens straight away when Kerdr holds it (your unsaved version, or a text just read),
    /// or it's read first and opens when the answer comes.
    pub fn edit_action_on(&mut self, card: Card) -> Action {
        let Some(detail) = card.detail.clone() else {
            self.say(
                "This workspace has no intent record: e edits a backlog item's text.",
                Tone::Info,
            );
            return Action::None;
        };
        // As the other card keys: nothing on a new intent's card while its words are saved, nor
        // while a start or a reopen of it is on its way.
        if self.still_saving(&card) || self.still_changing(&card, "edit") {
            return Action::None;
        }
        self.backlog_focus = Some(card.key.clone());
        let reference = card.reference.clone();
        // Its new text on its way, then written up again: the card shows ✎ meanwhile.
        if self.edit_on_its_way(&detail.id, false) {
            self.say(
                format!("{reference} is still being written up; edit it once that's done."),
                Tone::Info,
            );
            return Action::None;
        }
        // Asked already: it opens when it comes, counted from now.
        if self.edit_on_its_way(&detail.id, true) {
            if let Some(f) = self.edit_flights.iter_mut().find(|f| f.key == card.key) {
                f.at = Instant::now();
                f.away = self.away;
            }
            self.say(
                format!("{reference}'s text is on its way; it opens when it comes."),
                Tone::Info,
            );
            return Action::None;
        }
        let refusal = if card.dismissed() {
            Some(format!(
                "{reference} is in won't do; u brings it back before its text can be edited."
            ))
        } else if card.done() || card.state.as_deref() == Some("closed") {
            Some(format!(
                "{reference} is done, so its text can't be edited here."
            ))
        } else if !waiting_work(&card) {
            Some(format!(
                "{reference} has started, so its text can't be edited here."
            ))
        } else if card.intake() == Some("pending") {
            Some(format!(
                "{reference} is still being written up; edit it once that's done."
            ))
        } else if self.starts_when_written(&card.key) {
            // d was pressed while it was written up: its start goes in a moment (intent 0445).
            Some(format!(
                "{reference} starts as soon as its write-up is read, so its text can't be edited here. Esc on the board drops the start."
            ))
        } else if self.demo {
            Some(
                "This is the demo: on your own board, e opens this item's text in your editor."
                    .to_string(),
            )
        } else {
            None
        };
        if let Some(said) = refusal {
            // Your unsaved version still opens, so you can take your words with you.
            if !self.demo
                && let Some(held) = self
                    .edits
                    .get_mut(&card.key)
                    .filter(|h| h.id == detail.id && h.mine.is_some())
            {
                held.read_only = Some(said);
                self.open_now(&card);
                return Action::None;
            }
            self.say(said, Tone::Info);
            return Action::None;
        }
        // Your version opens without asking its machine, so it's yours to get at even while that
        // machine isn't answering: what you write is kept, and sent only once it can be. After a
        // save with no clear answer, Kerdr checks first; when it can't, your version still opens,
        // and nothing is sent until it can check.
        if let Some(held) = self
            .edits
            .get(&card.key)
            .filter(|h| h.id == detail.id && h.mine.is_some())
        {
            let machine = self.edit_machine(&card).unwrap_or(held.machine);
            let check_first = held.check_first;
            let cant = self.cant_send(machine);
            if !check_first || cant.is_some() {
                let held = self.edits.get_mut(&card.key).expect("held");
                held.machine = machine;
                held.read_only = cant.filter(|_| check_first).map(|why| {
                    format!(
                        "Kerdr can't check whether your last save of {reference}'s text landed: {why}"
                    )
                });
                self.open_now(&card);
                return Action::None;
            }
        }
        let Some(machine) = self.home_machine(&card) else {
            return Action::None;
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        // Its feed holds the text: it opens now, as `d` opens a card's questions (intent 0491).
        if let Some((text, was)) = self.text_from_feed(&card, machine) {
            self.edits.insert(
                card.key.clone(),
                Held {
                    id: detail.id.clone(),
                    machine,
                    current: edit::normalise(&text),
                    was,
                    read_at: Instant::now(),
                    mine: None,
                    check_first: false,
                    read_only: None,
                },
            );
            self.open_now(&card);
            return Action::None;
        }
        // Read moments ago: it opens as it was read, with no read.
        if let Some(held) = self.edits.get_mut(&card.key).filter(|h| {
            h.id == detail.id
                && h.mine.is_none()
                && !h.check_first
                && h.read_at.elapsed() < READY_FOR
        }) {
            held.machine = machine;
            held.read_only = None;
            self.open_now(&card);
            return Action::None;
        }
        // The read: through the action resolution tool, which runs it once the changes made to
        // this card before it have gone, and holds nothing up meanwhile.
        self.action_from = Some(CardSnapshot::of(&card));
        self.edit_flights
            .retain(|f| f.key != card.key && f.at.elapsed() < 4 * OPEN_WITHIN);
        // This can take a second or two, and the card shows nothing: say so at once (intent 0491).
        let said = format!("Reading {reference}'s text…");
        self.say(said.clone(), Tone::Info);
        self.edit_flights.push(EditFlight {
            id: detail.id.clone(),
            key: card.key.clone(),
            at: Instant::now(),
            away: self.away,
            said,
        });
        Action::Mutate {
            machine,
            request: json!({"action":"edit","id":detail.id,"from":crate::config::hostname()}),
        }
    }

    /// The item's text and fingerprint as the card's feed read them, when `e` can open from those:
    /// Kerdr holds nothing of yours to check first, the feed is from the machine a save goes to,
    /// and it's been read lately. Otherwise `e` reads it, as it does from a feed without them.
    fn text_from_feed(&self, card: &Card, machine: usize) -> Option<(String, String)> {
        if card.machine_index != machine {
            return None;
        }
        if self
            .edits
            .get(&card.key)
            .is_some_and(|h| h.mine.is_some() || h.check_first)
        {
            return None;
        }
        let taken = self.machines.get(machine)?.feed_taken?;
        let age = (self.now() - taken).to_std().unwrap_or_default();
        if age >= READY_FOR {
            return None;
        }
        let detail = card.detail.as_ref()?;
        Some((detail.item_text.clone()?, detail.item_was.clone()?))
    }

    /// The machine an item's save goes to, without saying anything: where a refusal said it lives
    /// now, else its home, else the machine its card came from. None when its home isn't one of
    /// Kerdr's machines.
    fn edit_machine(&self, card: &Card) -> Option<usize> {
        if let Some(to) = self.moved_target(card) {
            return Some(to);
        }
        match card
            .detail
            .as_ref()
            .and_then(|d| d.home.as_ref())
            .and_then(|h| h.machine.as_deref())
        {
            Some(name) => self.machine_index(name),
            None => Some(card.machine_index),
        }
    }

    /// The editor opens on this card's text at the next turn of the board, for the `e` pressed
    /// just now.
    fn open_now(&mut self, card: &Card) {
        let when = OpenWhen {
            asked: Instant::now(),
            away: self.away,
            from: CardSnapshot::of(card),
        };
        self.open_editor(&card.key, when);
    }

    /// The editor opens on this card's text at the next turn of the board, if what `when` says
    /// still holds then.
    fn open_editor(&mut self, key: &str, when: OpenWhen) {
        self.edit_open = Some(key.to_string());
        self.edit_open_when = Some(when);
    }

    /// The card whose text the editor opens now, checked at the moment it would open: within 15
    /// seconds of the e it answers, with no trip into herdr since, and you still on that card. Not
    /// while a failure is up, which the editor would hide before it's read: it waits for that, but
    /// never past those 15 seconds. When it can't open, Kerdr says the text is ready and e opens it,
    /// so an editor never springs up on you.
    pub fn editor_due(&mut self) -> Option<String> {
        let key = self.edit_open.clone()?;
        let fresh = self
            .edit_open_when
            .as_ref()
            .is_some_and(|w| w.asked.elapsed() < OPEN_WITHIN);
        if fresh && self.failing() {
            return None;
        }
        self.edit_open = None;
        let when = self.edit_open_when.take();
        let here = fresh
            && when
                .as_ref()
                .is_some_and(|w| w.away == self.away && self.still_on(&w.from, None));
        if here {
            return Some(key);
        }
        if let Some(held) = self.edits.get_mut(&key) {
            held.read_only = None;
            let id = held.id.clone();
            let mine = held.mine.is_some();
            let reference = self.reference_for(&id);
            let said = if mine {
                format!("Your version of {reference}'s text is ready: e opens it.")
            } else {
                format!("{reference}'s text is ready: e opens it.")
            };
            self.say(said, Tone::Info);
        }
        None
    }

    /// What the editor opens for this card: its id (for the file's name), what the file holds,
    /// the line shown while it's open, and the marker of the newer-text block when there is one.
    /// Your version when there is one, with the newer text below it when the item's text has
    /// changed since you wrote it; else the text as read.
    pub fn edit_file(&self, key: &str) -> Option<(String, String, String, Option<String>)> {
        let held = self.edits.get(key)?;
        let reference = self.reference_for(&held.id);
        let (file, marker) = match &held.mine {
            Some(mine) => {
                let newer = (held.current != mine.from && held.current != mine.text)
                    .then_some(held.current.as_str());
                let marker = newer.map(|_| edit::new_marker());
                let file = edit::file_text(
                    &reference,
                    &mine.text,
                    newer,
                    marker.as_deref().unwrap_or_default(),
                );
                (file, marker)
            }
            None => (edit::file_text(&reference, &held.current, None, ""), None),
        };
        let intro = match &held.read_only {
            Some(why) => format!(
                "Kerdr: your unsaved version of {reference}'s text. {why} Copy what you want to keep; nothing is sent when it closes."
            ),
            None => format!("Kerdr: editing {reference}'s text."),
        };
        Some((held.id.clone(), file, intro, marker))
    }

    /// Before the editor starts: one that opens a window of its own and wouldn't be waited for
    /// (`code` without `--wait`) isn't started, since Kerdr would never see what you write there.
    /// Says which flag to add, keeps the text, and whether it stopped it.
    pub fn editor_wont_wait(&mut self, key: &str, candidates: &[Vec<String>]) -> bool {
        let Some(words) = candidates.first() else {
            return false;
        };
        let Some(flag) = edit::missing_wait(words) else {
            return false;
        };
        let Some(held) = self.edits.get_mut(key) else {
            return false;
        };
        held.read_only = None;
        let (id, mine) = (held.id.clone(), held.mine.is_some());
        let reference = self.reference_for(&id);
        let named = words.join(" ");
        let kept = if mine {
            "Your version is kept: e opens it once that's set."
        } else {
            "Nothing changed; e opens it once that's set."
        };
        self.say(
            format!(
                "Didn't open {reference}'s text: {named} opens a window of its own, and without {flag} Kerdr can't wait for you to close it. Add {flag} to editor in Kerdr's config (editor = \"{named} {flag}\"). {kept}"
            ),
            Tone::Warn,
        );
        true
    }

    /// Says that the editor you named couldn't start and which one opened instead, when so: your
    /// setting needs fixing even when nothing else is said.
    fn editor_trouble(&mut self, run: &Run, reference: &str) {
        if let Some((names, editor)) = skipped(run) {
            self.say(
                format!("{names} couldn't start, so {editor} opened {reference}'s text."),
                Tone::Warn,
            );
        }
    }

    /// Why a save can't go now, after the editor: the action backend is off, or the item's
    /// machine stopped answering while it was open.
    fn cant_send(&self, machine: usize) -> Option<String> {
        if !self.actions_on() {
            return Some("intent controls are not configured for this board.".into());
        }
        match self.machines.get(machine) {
            Some(m) if m.conn.is_live() => None,
            Some(m) => Some(format!("{} isn't answering.", m.name())),
            None => Some("its machine isn't on this board.".into()),
        }
    }

    /// The editor has closed on this card's text. A changed text is sent as a save, which the
    /// card shows at once; closing with nothing changed, or with `:cq`, says nothing. What you
    /// wrote that can't be sent says so where it can't be missed, and your version is kept until a
    /// save of it succeeds.
    pub fn edit_ended(&mut self, key: &str, ended: Ended) -> Action {
        let Some(mut held) = self.edits.get(key).cloned() else {
            return Action::None;
        };
        let reference = self.reference_for(&held.id);
        let yours = if held.mine.is_some() {
            " with your version"
        } else {
            ""
        };
        if let Some(why) = held.read_only.take() {
            return self.edit_ended_read_only(key, &reference, &why, held, ended);
        }
        match ended {
            Ended::Failed(why) => {
                self.say(
                    format!(
                        "Couldn't open {reference}'s text: {why}. It's as it was; e tries again{yours}."
                    ),
                    Tone::Warn,
                );
                Action::None
            }
            Ended::NoEditor { skipped } => {
                self.say(
                    format!(
                        "No editor could start ({}). Name one as editor in Kerdr's config, or set $EDITOR. {reference} is as it was; e tries again{yours}.",
                        skipped.join("; ")
                    ),
                    Tone::Warn,
                );
                Action::None
            }
            // :cq: left as it was, and any version of yours let go. Nothing to say.
            Ended::Cancelled { run } => {
                self.edits.remove(key);
                self.editor_trouble(&run, &reference);
                Action::None
            }
            // It died without closing: what it had written is kept as your version, and nothing is
            // sent, since you never chose to save it.
            Ended::Died { text, run, touched } => {
                let editor = run.editor.as_deref().unwrap_or("Your editor");
                let note = skipped_note(&run);
                let mine = held.mine.as_ref().map(|m| m.text.as_str());
                if touched
                    && !text.is_empty()
                    && text != held.current
                    && mine != Some(text.as_str())
                {
                    held.mine = Some(Mine {
                        text,
                        from: held.current.clone(),
                    });
                    self.edits.insert(key.to_string(), held);
                    self.fail(format!(
                        "{editor} stopped without closing (it was killed or crashed), so {reference}'s text wasn't saved. What it had written is kept as your version, sent nowhere: e opens it.{note}"
                    ));
                } else {
                    let kept = if mine.is_some() {
                        " Your version is kept: e opens it."
                    } else {
                        ""
                    };
                    self.fail(format!(
                        "{editor} stopped without closing (it was killed or crashed); nothing was sent, and {reference} is as it was.{kept}{note}"
                    ));
                }
                Action::None
            }
            Ended::Unreadable { why, kept, run } => {
                self.fail(unreadable(&reference, &why, &kept, &run, yours));
                Action::None
            }
            Ended::Wrote { text, run, touched } => {
                let note = skipped_note(&run);
                // :q: nothing written, so nothing changed, whatever the file holds; any version of
                // yours is still kept. Nothing to say.
                if !touched {
                    if held.mine.is_none() {
                        self.edits.remove(key);
                    }
                    self.editor_trouble(&run, &reference);
                    return Action::None;
                }
                if text.is_empty() {
                    let kept = if held.mine.is_some() {
                        " e opens your last version."
                    } else {
                        ""
                    };
                    self.fail(format!(
                        "An item's text can't be empty; {reference} is as it was.{kept}{note}"
                    ));
                    return Action::None;
                }
                // Nothing changed: as it was, and nothing to say.
                if text == held.current {
                    self.edits.remove(key);
                    self.editor_trouble(&run, &reference);
                    return Action::None;
                }
                held.mine = Some(Mine {
                    text: text.clone(),
                    from: held.current.clone(),
                });
                held.check_first = false;
                let (machine, request) = (
                    held.machine,
                    json!({"action":"edit","id":held.id,"text":text,"was":held.was,"from":crate::config::hostname()}),
                );
                self.edits.insert(key.to_string(), held);
                if crate::actions::too_large(&request) {
                    self.fail(format!(
                        "{reference}'s text is too large to send: a request is at most 64 KiB. Your version is kept: e opens it to make it shorter.{note}"
                    ));
                    return Action::None;
                }
                if let Some(why) = self.cant_send(machine) {
                    self.fail(format!(
                        "{reference}'s text wasn't saved: {why} Your version is kept: e opens it.{note}"
                    ));
                    return Action::None;
                }
                self.editor_trouble(&run, &reference);
                Action::Mutate { machine, request }
            }
        }
    }

    /// The editor has closed on your unsaved version of an item that can't be edited any more, or
    /// whose last save Kerdr couldn't check: nothing is sent. What you wrote stays yours until
    /// Kerdr quits, unless you cancelled.
    fn edit_ended_read_only(
        &mut self,
        key: &str,
        reference: &str,
        why: &str,
        mut held: Held,
        ended: Ended,
    ) -> Action {
        if let Ended::Cancelled { .. } = ended {
            // :cq lets it go, saying nothing.
            self.edits.remove(key);
            return Action::None;
        }
        let mut wrote = false;
        if let Ended::Wrote {
            text,
            touched: true,
            ..
        }
        | Ended::Died {
            text,
            touched: true,
            ..
        } = &ended
            && let Some(mine) = held.mine.as_mut()
            && *text != mine.text
        {
            wrote = true;
            if !text.is_empty() {
                mine.text = text.clone();
            }
        }
        let trouble = match &ended {
            Ended::Failed(e) => format!(" Couldn't open it: {e}."),
            Ended::NoEditor { skipped } => {
                format!(" No editor could start ({}).", skipped.join("; "))
            }
            Ended::Died { run, .. } => format!(
                " {} stopped without closing (it was killed or crashed).",
                run.editor.as_deref().unwrap_or("Your editor")
            ),
            Ended::Unreadable { why, kept, .. } => {
                wrote = true;
                format!(
                    " Couldn't read it back ({why}); the file is kept at {}: copy what you want from it, then delete it.",
                    kept.display()
                )
            }
            _ => String::new(),
        };
        let opens = if held.check_first {
            "e checks whether your last save landed, then opens it"
        } else {
            "e opens it"
        };
        self.edits.insert(key.to_string(), held);
        let said = format!(
            "{why}{trouble} Nothing was sent. Your version is kept until Kerdr quits: {opens}, and :cq there lets it go."
        );
        if wrote {
            // What you wrote there went nowhere: said where it can't be missed.
            self.fail(format!("{reference}'s text wasn't saved: {said}"));
        } else if !trouble.is_empty() {
            self.say(said, Tone::Warn);
        }
        Action::None
    }

    /// What quitting would lose of the texts you edited, for q to say first (a second q quits):
    /// your versions Kerdr keeps until a save of them lands, which live only in its memory. None
    /// when it holds none.
    pub(super) fn edits_lost(&self) -> Option<String> {
        let (mut saving, mut unsaved): (Vec<String>, Vec<String>) = (vec![], vec![]);
        for held in self.edits.values().filter(|h| h.mine.is_some()) {
            let reference = self.reference_for(&held.id);
            if self.edit_on_its_way(&held.id, false) {
                saving.push(reference);
            } else {
                unsaved.push(reference);
            }
        }
        saving.sort();
        unsaved.sort();
        let mut said = vec![];
        match unsaved.as_slice() {
            [] => {}
            [one] => said.push(format!(
                "Your unsaved version of {one}'s text is kept only while Kerdr is open (e on its card opens it): quitting drops it."
            )),
            many => said.push(format!(
                "Your unsaved versions of the texts of {} are kept only while Kerdr is open (e on each card opens it): quitting drops them.",
                many.join(", ")
            )),
        }
        match saving.as_slice() {
            [] => {}
            [one] => said.push(format!(
                "{one}'s new text is still being saved, and if that fails your version is kept only while Kerdr is open: quitting drops it."
            )),
            many => said.push(format!(
                "The new texts of {} are still being saved, and if one fails your version is kept only while Kerdr is open: quitting drops them.",
                many.join(", ")
            )),
        }
        (!said.is_empty()).then(|| said.join(" "))
    }

    /// A save sent on to the machine a refusal named: its text, which came back only counted,
    /// goes again as you wrote it.
    pub(super) fn edit_resend(&self, request: &mut Value) {
        if request.get("text").is_none() {
            return;
        }
        let id = request["id"].as_str().unwrap_or_default();
        match self
            .edits
            .values()
            .find(|h| h.id == id)
            .and_then(|h| h.mine.as_ref())
        {
            Some(mine) => request["text"] = json!(mine.text),
            // Nothing to send again: ask for the text instead.
            None => {
                if let Some(fields) = request.as_object_mut() {
                    fields.remove("text");
                    fields.remove("was");
                }
            }
        }
    }

    /// For a save of an item's text whose request can't go on: that your version is kept.
    pub(super) fn edit_kept(&self, request: &Value) -> String {
        if request["action"] != "edit" || request.get("text").is_none() {
            return String::new();
        }
        let reference = self.reference_for(request["id"].as_str().unwrap_or_default());
        format!(" Your version of {reference}'s text is kept: e opens it.")
    }

    /// The card whose text Kerdr holds for this item, on this machine: where its save shows.
    pub(super) fn edit_card(&self, id: &str, machine: usize) -> Option<String> {
        let mut held: Vec<(&String, &Held)> =
            self.edits.iter().filter(|(_, h)| h.id == id).collect();
        held.sort_by_key(|(_, h)| h.machine != machine);
        held.first().map(|(k, _)| (*k).clone())
    }

    /// The card an edit's answer is for: the one it was sent from while that's still the same
    /// record, else whichever card has its id.
    fn edit_key(&self, id: &str, flight: Option<&EditFlight>) -> Option<String> {
        let of = |key: &str| {
            self.card(key)
                .and_then(|c| c.detail.as_ref())
                .is_some_and(|d| d.id == id)
                || self.edits.get(key).is_some_and(|h| h.id == id)
        };
        flight
            .map(|f| f.key.clone())
            .filter(|k| of(k))
            .or_else(|| {
                self.edits
                    .iter()
                    .find(|(_, h)| h.id == id)
                    .map(|(k, _)| k.clone())
            })
            .or_else(|| {
                self.board
                    .cards
                    .iter()
                    .find(|c| c.detail.as_ref().is_some_and(|d| d.id == id))
                    .map(|c| c.key.clone())
            })
    }

    /// An edit's answer: a read opens the editor while you're still where you pressed `e`, and a
    /// save's answer settles your version, or keeps it.
    pub(super) fn finish_edit(
        &mut self,
        machine: usize,
        request: Value,
        result: Result<Response, String>,
        from: Option<CardSnapshot>,
    ) {
        let id = request["id"].as_str().unwrap_or_default().to_string();
        let reference = self.reference_for(&id);
        let reads = request.get("text").is_none();
        let flight = if reads {
            self.edit_flights
                .iter()
                .position(|f| f.id == id)
                .map(|i| self.edit_flights.remove(i))
        } else {
            None
        };
        if let Some(flight) = &flight {
            self.unsay(&flight.said);
        }
        let Some(key) = self.edit_key(&id, flight.as_ref()) else {
            // Its card is gone from the board, and Kerdr holds nothing of it: only a problem is
            // worth saying.
            let problem = match result {
                Ok(r) if r.ok && partial(&r) => return self.fail(partly_saved(&reference, r)),
                Ok(r) if r.ok => return,
                Ok(r) => short(r.message),
                Err(e) => short(e),
            };
            if reads {
                self.say(problem, Tone::Warn);
            } else {
                self.fail(format!("{reference}'s text wasn't saved: {problem}"));
            }
            return;
        };
        if reads {
            self.finish_edit_read(&key, &id, &reference, machine, result, flight, from);
        } else {
            self.finish_edit_save(&key, &reference, machine, result);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_edit_read(
        &mut self,
        key: &str,
        id: &str,
        reference: &str,
        machine: usize,
        result: Result<Response, String>,
        flight: Option<EditFlight>,
        from: Option<CardSnapshot>,
    ) {
        // You're still where you pressed e, within its 15 seconds.
        let here = |app: &App| {
            flight
                .as_ref()
                .is_some_and(|f| f.key == key && f.at.elapsed() < OPEN_WITHIN && f.away == app.away)
                && from
                    .as_ref()
                    .is_some_and(|f| f.key == key && app.still_on(f, None))
        };
        let response = match result {
            Err(error) => Err(short(error)),
            Ok(r) if !r.ok => Err(short(r.message)),
            Ok(r) => Ok(r),
        };
        let response = match response {
            Ok(r) => r,
            // A check after a save with no clear answer that couldn't be made: your version opens
            // all the same, and nothing is sent until Kerdr can check.
            Err(problem)
                if self
                    .edits
                    .get(key)
                    .is_some_and(|h| h.id == id && h.check_first && h.mine.is_some()) =>
            {
                if here(self)
                    && let (Some(flight), Some(from)) = (flight.as_ref(), from.clone())
                {
                    let held = self.edits.get_mut(key).expect("held");
                    held.read_only = Some(format!(
                        "Kerdr couldn't check whether your last save of {reference}'s text landed ({problem})."
                    ));
                    let when = OpenWhen {
                        asked: flight.at,
                        away: flight.away,
                        from,
                    };
                    self.open_editor(key, when);
                } else {
                    self.say(
                        format!(
                            "Couldn't check whether {reference}'s save landed ({problem}). Nothing is sent until it can be: e checks again, and opens your version either way."
                        ),
                        Tone::Warn,
                    );
                }
                return;
            }
            Err(problem) => return self.say(problem, Tone::Warn),
        };
        let (Some(text), Some(was)) = (response.text, response.was) else {
            return self.say(
                format!(
                    "{reference}'s text didn't come with the answer, so there's nothing to open: its action backend may not know edit yet."
                ),
                Tone::Warn,
            );
        };
        let held = self.edits.entry(key.to_string()).or_insert_with(|| Held {
            id: id.to_string(),
            machine,
            current: String::new(),
            was: String::new(),
            read_at: Instant::now(),
            mine: None,
            check_first: false,
            read_only: None,
        });
        held.id = id.to_string();
        held.machine = machine;
        held.current = edit::normalise(&text);
        held.was = was;
        held.read_at = Instant::now();
        // After a save that got no answer: did it land?
        let checked = std::mem::take(&mut held.check_first);
        if checked && held.mine.as_ref().is_some_and(|m| m.text == held.current) {
            self.edits.remove(key);
            self.say(
                format!("✓ {reference}'s text was saved after all: it reads as your version."),
                Tone::Good,
            );
            return;
        }
        let mine = held.mine.is_some();
        if here(self)
            && let (Some(flight), Some(from)) = (flight, from)
        {
            // Counted from e, not from now: checked again the moment it would open.
            let when = OpenWhen {
                asked: flight.at,
                away: flight.away,
                from,
            };
            self.open_editor(key, when);
        } else if checked {
            self.say(
                format!("{reference}'s text wasn't saved; e opens your version."),
                Tone::Warn,
            );
        } else if mine {
            self.say(
                format!("Your version of {reference}'s text is ready: e opens it."),
                Tone::Info,
            );
        } else {
            self.say(
                format!("{reference}'s text is ready: e opens it."),
                Tone::Info,
            );
        }
    }

    /// A save's answer. Saved (or unchanged): your version is let go and nothing is said, since
    /// the card already shows it. Anything else: the card is back as it is, your version is kept
    /// for the next `e`, and the red box says so.
    fn finish_edit_save(
        &mut self,
        key: &str,
        reference: &str,
        machine: usize,
        result: Result<Response, String>,
    ) {
        match result {
            // Saved: the card shows it, and nothing is said. Saved, but something after it didn't
            // happen (its write-up couldn't start, or the push failed): saved all the same, and
            // that part is said where it can't be missed.
            Ok(r) if r.ok => {
                self.edits.remove(key);
                if partial(&r) {
                    self.fail(partly_saved(reference, r));
                } else if let Some(left) =
                    r.note.as_deref().map(str::trim).filter(|n| !n.is_empty())
                {
                    // Saved, with something still to come by itself (its push): said, not red
                    // (intent 0591).
                    self.say(format!("{reference}'s text is saved; {left}"), Tone::Info);
                }
            }
            // Only part of it happened, and the backend's words say which: his version stays, and
            // e checks what's there before it opens it (intent 0591).
            Ok(r) if r.partial => {
                if let Some(held) = self.edits.get_mut(key) {
                    held.machine = machine;
                    held.check_first = true;
                }
                self.say(
                    format!(
                        "{reference}'s save happened only in part: {} e checks the text, and opens your version if it isn't there.",
                        short(r.message)
                    ),
                    Tone::Warn,
                );
            }
            Ok(r) => {
                let stale = r.status.as_deref() == Some("stale");
                let Some(held) = self.edits.get_mut(key) else {
                    return self.fail(format!(
                        "{reference}'s text wasn't saved: {}",
                        short(r.message)
                    ));
                };
                held.machine = machine;
                let next = if stale {
                    match (r.text, r.was) {
                        (Some(text), Some(was)) => {
                            held.current = edit::normalise(&text);
                            held.was = was;
                            held.read_at = Instant::now();
                            held.check_first = false;
                            "Your version is kept: e opens it, with the newer text below it."
                        }
                        // Changed, but not said to what: e reads it first.
                        _ => {
                            held.check_first = true;
                            "Your version is kept: e reads the newer text, then opens yours."
                        }
                    }
                } else {
                    held.check_first = false;
                    "Your version is kept: e opens it."
                };
                self.fail(format!(
                    "{reference}'s text wasn't saved: {} {next}",
                    short(r.message)
                ));
            }
            Err(error) => {
                if let Some(held) = self.edits.get_mut(key) {
                    held.check_first = true;
                }
                self.fail(format!(
                    "No clear answer came for {reference}'s save ({}), so it may or may not have happened. e checks first, and opens your version if it didn't.",
                    first_clause(&error)
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.cfg.actions.command = Some("test-adapter".into());
        app
    }

    fn press(app: &mut App, c: char) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn esc(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    /// A card waiting in the backlog, selected on the board: its key, id and reference.
    fn waiting(app: &mut App) -> (String, String, String) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| waiting_work(c) && c.intake().is_none())
            .expect("a waiting backlog card in the demo")
            .clone();
        select(app, &card.key);
        (
            card.key.clone(),
            card.detail.as_ref().unwrap().id.clone(),
            card.reference.clone(),
        )
    }

    fn select(app: &mut App, key: &str) {
        let col = app.card(key).unwrap().stage.index();
        app.mode = Mode::Board;
        app.sel = Selection {
            col,
            key: Some(key.to_string()),
        };
    }

    fn text_answer(text: &str, was: &str) -> Result<Response, String> {
        Ok(Response {
            ok: true,
            message: String::new(),
            text: Some(text.into()),
            was: Some(was.into()),
            ..Default::default()
        })
    }

    fn refused(message: &str) -> Result<Response, String> {
        Ok(Response {
            ok: false,
            message: message.into(),
            ..Default::default()
        })
    }

    fn saved() -> Result<Response, String> {
        Ok(Response {
            ok: true,
            status: Some("saved".into()),
            message: "Saved #414's text. Its title and done-when catch up in about a minute."
                .into(),
            ..Default::default()
        })
    }

    fn nvim() -> Run {
        Run {
            editor: Some("nvim".into()),
            ok: true,
            ..Default::default()
        }
    }

    /// The editor written and closed (`:wq`), the file holding `text`.
    fn wrote(text: &str) -> Ended {
        Ended::Wrote {
            text: text.into(),
            run: nvim(),
            touched: true,
        }
    }

    /// The editor closed without writing (`:q`), the file as it opened.
    fn left(text: &str) -> Ended {
        Ended::Wrote {
            text: text.into(),
            run: nvim(),
            touched: false,
        }
    }

    fn cancelled() -> Ended {
        Ended::Cancelled {
            run: Run {
                editor: Some("nvim".into()),
                ok: false,
                ..Default::default()
            },
        }
    }

    /// A request on its way, as the board sends it: into the action resolution tool, handed to
    /// the runner, its command started with the whole request (intent 0445).
    struct Sent {
        n: u64,
        machine: usize,
        request: Value,
    }

    fn send(app: &mut App, machine: usize, request: Value) -> Sent {
        app.queue_action(machine, request.clone());
        let n = app
            .resolver
            .ready()
            .into_iter()
            .find(|(_, m, r, gated)| {
                *m == machine && crate::actions::without_op(r) == request && !gated
            })
            .map(|(n, _, r, _)| (n, r))
            .expect("it goes to the runner at once");
        let (n, request) = n;
        app.job_started(n);
        app.resolver.handed(n);
        Sent {
            n,
            machine,
            request,
        }
    }

    /// Its answer, as the board takes it: through the action resolution tool, with the request's
    /// metadata, which only counts a save's text.
    fn answer(app: &mut App, sent: &Sent, result: Result<Response, String>) {
        app.finish_job(
            sent.n,
            crate::actions::request_metadata(&sent.request),
            result,
        );
    }

    /// e on the selected card: the read it asks for, which names the item and this machine.
    fn e_reads(app: &mut App, id: &str) -> (usize, Value) {
        let Action::Mutate { machine, request } = press(app, 'e') else {
            panic!("e sent nothing: {:?}", app.message())
        };
        assert_eq!(
            request,
            json!({"action":"edit","id":id,"from":crate::config::hostname()})
        );
        (machine, request)
    }

    /// e, its read sent.
    fn read(app: &mut App, id: &str) -> Sent {
        let (machine, request) = e_reads(app, id);
        send(app, machine, request)
    }

    /// e, its read answered with `text`, and the editor due to open on it.
    fn opened(app: &mut App, key: &str, id: &str, text: &str, was: &str) -> usize {
        let sent = read(app, id);
        answer(app, &sent, text_answer(text, was));
        assert_eq!(app.edit_open.take().as_deref(), Some(key));
        sent.machine
    }

    /// The editor closing on `ended`: the save it sends.
    fn saves(app: &mut App, key: &str, ended: Ended) -> Sent {
        let Action::Mutate { machine, request } = app.edit_ended(key, ended) else {
            panic!("no save was sent: {:?}", app.message())
        };
        send(app, machine, request)
    }

    fn said(app: &App) -> String {
        app.message()
            .map(|(t, _)| t.to_string())
            .unwrap_or_default()
    }

    /// A failure on screen has been read, and whatever was said under it with it.
    fn seen(app: &mut App) {
        assert!(app.failing(), "no failure up: {:?}", app.message());
        app.message = None;
        app.held.clear();
    }

    /// Two reads of every machine: what the board showed for changes that landed gives way to the
    /// feed, as it does once their two reads have gone by.
    fn caught_up(app: &mut App) {
        for _ in 0..2 {
            for m in 0..app.machines.len() {
                let feed = app.machines[m].feed.clone();
                app.resolver.read(m, feed.as_ref());
            }
        }
        app.rebuild();
    }

    /// The card's words under "How it started", and whether it shows ✎.
    fn shown(app: &App, key: &str) -> (Option<String>, bool) {
        let card = app.card(key).unwrap();
        (
            card.detail.as_ref().unwrap().words.clone(),
            card.intake() == Some("pending"),
        )
    }

    #[test]
    fn e_reads_the_text_opens_it_and_saves_a_change_with_what_it_was_read_as() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let machine = opened(&mut app, &key, &id, "Old words.\n\nTwo lines.", "sha256:aa");
        let (file_id, file, intro, marker) = app.edit_file(&key).unwrap();
        assert_eq!(marker, None, "nothing newer, no block");
        assert_eq!(file_id, id);
        assert_eq!(file, "Old words.\n\nTwo lines.\n");
        assert!(intro.contains(&reference), "{intro}");
        let sent = saves(&mut app, &key, wrote("New words."));
        assert_eq!(sent.machine, machine);
        // A save changes something: it carries an op of its own (intent 0591).
        assert!(
            sent.request["op"]
                .as_str()
                .is_some_and(|op| op.len() == 18 && op.starts_with("k-")),
            "{}",
            sent.request
        );
        assert_eq!(
            crate::actions::without_op(&sent.request),
            json!({"action":"edit","id":id,"text":"New words.","was":"sha256:aa","from":crate::config::hostname()})
        );
        answer(&mut app, &sent, saved());
        // The card is the confirmation: the backend's message isn't shown (intent 0416).
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty(), "nothing is held once it's saved");
    }

    #[test]
    fn a_save_shows_on_the_card_at_once_and_says_nothing_when_it_lands() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let before = shown(&app, &key);
        assert!(!before.1);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("New words,\nas he wrote them."));
        // Shown before any answer: the new words under "How it started", and ✎.
        assert_eq!(
            shown(&app, &key),
            (Some("New words,\nas he wrote them.".into()), true)
        );
        assert!(!app.busy, "the board isn't held");
        assert_eq!(app.message(), None, "nothing said on the way");
        // Its peek and own screen say what's on its way (intent 0445).
        let card = app.card(&key).cloned().unwrap();
        let line = app.starting_line(&card).unwrap_or_default();
        assert!(line.starts_with("◐ Saving its new text…"), "{line}");
        // On its way: its command has the whole request, so q says it carries on, and names your
        // version, which only Kerdr keeps if it fails; e says it's being written up without
        // opening anything.
        assert_eq!(app.resolver.unanswered(), 1);
        assert_eq!(app.quit_held(), None);
        assert!(
            app.still_running()
                .unwrap()
                .starts_with(&format!("{reference} is still being saved")),
        );
        assert!(app.quit_loses().unwrap().contains(&reference));
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(
            said(&app),
            format!("{reference} is still being written up; edit it once that's done.")
        );
        assert_eq!(app.edit_open, None);
        app.message = None;
        // Saved: nothing more is said, and the card keeps showing it until the feed does.
        answer(&mut app, &sent, saved());
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty());
        assert_eq!(app.resolver.unanswered(), 0);
        app.rebuild();
        assert_eq!(
            shown(&app, &key),
            (Some("New words,\nas he wrote them.".into()), true)
        );
        // Once a read of its machine has the new words, the feed speaks for itself.
        let mut feed = app.machines[sent.machine].feed.clone().unwrap();
        let fc = feed.cards.iter_mut().find(|c| c.id == id).unwrap();
        fc.words = Some("New words, as he wrote them.".into());
        assert!(app.resolver.read(sent.machine, Some(&feed)));
        assert!(!app.resolver.shows());
    }

    #[test]
    fn a_failed_save_takes_the_card_back_speaks_and_keeps_your_version() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let before = shown(&app, &key);
        for (result, words) in [
            (
                refused("An agent is at it now; its text can't change under it."),
                "wasn't saved: An agent is at it now; its text can't change under it. Your version is kept: e opens it.",
            ),
            (
                Err("the action timed out; its result is unknown.".into()),
                "may or may not have happened. e checks first",
            ),
        ] {
            opened(&mut app, &key, &id, "Old words.", "sha256:aa");
            let sent = saves(&mut app, &key, wrote("Mine."));
            assert_eq!(shown(&app, &key), (Some("Mine.".into()), true));
            answer(&mut app, &sent, result);
            assert_eq!(shown(&app, &key), before, "back as it really is at once");
            let (text, tone) = app.message().unwrap();
            assert_eq!(tone, Tone::Fail, "{text}");
            assert!(text.contains(&reference) && text.contains(words), "{text}");
            assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine.");
            assert_eq!(app.resolver.unanswered(), 0);
            assert!(!app.resolver.shows());
            seen(&mut app);
            app.edits.clear();
        }
    }

    /// Gives a card's copy in its feed the fields `e` opens from, as a read of that feed brings
    /// them, and says when the feed was taken in.
    fn feed_holds_text(app: &mut App, key: &str, text: &str, was: &str) -> usize {
        let i = app.card(key).unwrap().machine_index;
        let id = key.rsplit_once(':').unwrap().1.to_string();
        let m = &mut app.machines[i];
        let feed = m.feed.as_mut().unwrap();
        let fc = feed.cards.iter_mut().find(|c| c.id == id).unwrap();
        fc.item_text = Some(text.into());
        fc.item_was = Some(was.into());
        m.feed_taken = Some(chrono::Local::now());
        app.rebuild();
        i
    }

    #[test]
    fn e_opens_at_once_from_the_feed_and_the_save_checks_what_it_read() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        let machine = feed_holds_text(
            &mut app,
            &key,
            "The first words.\r\nAnd more.\n",
            "sha256:a",
        );
        // No read is sent, nothing has to answer, and the editor is due this very turn.
        assert!(matches!(press(&mut app, 'e'), Action::None));
        assert_eq!(app.resolver.on_its_way().count(), 0);
        assert_eq!(app.editor_due(), Some(key.clone()));
        let (_, file, ..) = app.edit_file(&key).unwrap();
        assert!(file.contains("The first words.\nAnd more."), "{file}");
        // What's saved says what it was read as, to the machine it was read from.
        let Action::Mutate {
            machine: to,
            request,
        } = app.edit_ended(&key, wrote("New words."))
        else {
            panic!("a changed text is sent");
        };
        assert_eq!(to, machine);
        assert_eq!(request["was"], "sha256:a");
        assert_eq!(request["id"], id);
        // Changed underneath: refused by the save, with his words kept, as after any read.
        let sent = send(&mut app, to, request);
        app.finish_job(
            sent.n,
            sent.request,
            Ok(Response {
                ok: false,
                status: Some("stale".into()),
                message: "It changed.".into(),
                text: Some("Someone else's words.".into()),
                was: Some("sha256:b".into()),
                ..Default::default()
            }),
        );
        let held = app.edits.get(&key).unwrap();
        assert_eq!(held.mine.as_ref().unwrap().text, "New words.");
        assert_eq!(held.was, "sha256:b");
    }

    #[test]
    fn e_reads_first_where_the_feed_cant_be_trusted_with_it() {
        // No fields at all: read, saying so at once, and unsaid when it answers.
        let mut plain = app();
        let (key, id, reference) = waiting(&mut plain);
        let (machine, request) = e_reads(&mut plain, &id);
        assert_eq!(
            plain.message().map(|(t, _)| t.to_string()),
            Some(format!("Reading {reference}'s text…"))
        );
        let sent = send(&mut plain, machine, request);
        plain.finish_job(sent.n, sent.request, text_answer("Words.", "sha256:a"));
        assert_eq!(plain.message(), None);
        assert_eq!(plain.editor_due(), Some(key));

        let reads = |setup: &dyn Fn(&mut App, &str)| {
            let mut app = app();
            let (key, id, _) = waiting(&mut app);
            feed_holds_text(&mut app, &key, "From the feed.", "sha256:feed");
            setup(&mut app, &key);
            e_reads(&mut app, &id);
        };
        // A feed that has stopped coming is too old to open from.
        reads(&|app, key| {
            let i = app.card(key).unwrap().machine_index;
            app.machines[i].feed_taken = Some(chrono::Local::now() - chrono::Duration::hours(1));
        });
        // A save that got no clear answer is checked first, by a read.
        reads(&|app, key| {
            let (id, machine) = {
                let c = app.card(key).unwrap();
                (c.detail.as_ref().unwrap().id.clone(), c.machine_index)
            };
            app.edits.insert(
                key.to_string(),
                Held {
                    id,
                    machine,
                    current: "As it was.".into(),
                    was: "sha256:old".into(),
                    read_at: Instant::now(),
                    mine: Some(Mine {
                        text: "Mine.".into(),
                        from: "As it was.".into(),
                    }),
                    check_first: true,
                    read_only: None,
                },
            );
        });
    }

    #[test]
    fn e_from_the_feed_is_refused_as_any_other_and_your_version_comes_first() {
        // Being written up: refused, with no text opened.
        let mut a = app();
        let (key, id, _) = waiting(&mut a);
        let machine = feed_holds_text(&mut a, &key, "From the feed.", "sha256:feed");
        let m = &mut a.machines[machine];
        let fc = m
            .feed
            .as_mut()
            .unwrap()
            .cards
            .iter_mut()
            .find(|c| c.id == id)
            .unwrap();
        fc.intake = Some("pending".into());
        a.rebuild();
        assert!(matches!(press(&mut a, 'e'), Action::None));
        assert!(a.message().unwrap().0.contains("still being written up"));
        assert_eq!(a.editor_due(), None);
        // Your unsaved version wins over what the feed reads.
        let mut b = app();
        let (key, id, _) = waiting(&mut b);
        let machine = feed_holds_text(&mut b, &key, "From the feed.", "sha256:feed");
        b.edits.insert(
            key.clone(),
            Held {
                id,
                machine,
                current: "As it was.".into(),
                was: "sha256:old".into(),
                read_at: Instant::now(),
                mine: Some(Mine {
                    text: "Mine, unsaved.".into(),
                    from: "As it was.".into(),
                }),
                check_first: false,
                read_only: None,
            },
        );
        assert!(matches!(press(&mut b, 'e'), Action::None));
        assert_eq!(b.editor_due(), Some(key.clone()));
        assert_eq!(b.edits[&key].was, "sha256:old");
        assert!(b.edit_file(&key).unwrap().1.contains("Mine, unsaved."));
    }

    #[test]
    fn a_save_on_its_way_hides_the_text_the_feed_read_before_it() {
        // Effect::Text: the card shows the new words as being written up, so nothing opens the
        // words they replace, even once its answer has come and the feed hasn't caught up.
        let mut app = app();
        let (key, _, _) = waiting(&mut app);
        let machine = feed_holds_text(&mut app, &key, "Old words.", "sha256:old");
        assert!(matches!(press(&mut app, 'e'), Action::None));
        assert_eq!(app.editor_due(), Some(key.clone()));
        let Action::Mutate {
            machine: to,
            request,
        } = app.edit_ended(&key, wrote("New words."))
        else {
            panic!("a changed text is sent");
        };
        let sent = send(&mut app, to, request);
        assert_eq!(to, machine);
        app.finish_job(sent.n, sent.request, saved());
        let detail = app.card(&key).unwrap().detail.clone().unwrap();
        assert_eq!(detail.item_text, None);
        assert_eq!(detail.item_was, None);
        assert!(matches!(press(&mut app, 'e'), Action::None));
        assert_eq!(app.editor_due(), None);
    }

    #[test]
    fn e_works_in_the_backlog_list_and_in_a_peek_of_a_backlog_card() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        for back in [Mode::Board, Mode::Backlog { sel: 0 }] {
            select(&mut app, &key);
            app.mode = Mode::Peek {
                scroll: 0,
                back: Box::new(back),
            };
            e_reads(&mut app, &id);
        }
        app.enter_backlog(false);
        let sel = app
            .backlog_cards()
            .iter()
            .position(|&i| app.board.cards[i].key == key)
            .unwrap();
        app.mode = Mode::Backlog { sel };
        e_reads(&mut app, &id);
        // The menu's e is the same key.
        select(&mut app, &key);
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Palette { .. }));
        e_reads(&mut app, &id);
    }

    #[test]
    fn e_works_on_a_backlog_cards_own_screen_and_its_line_says_so() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        app.open_inside(&key);
        assert!(matches!(app.mode, Mode::Card { .. }), "{:?}", app.mode);
        let screen = crate::ui::tests_render(&app, 220, 44);
        assert!(screen.contains("d start it   e edit its text"), "{screen}");
        let sent = read(&mut app, &id);
        // Its answer opens the editor there too, still on that card.
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn e_on_a_card_it_cant_edit_says_why_and_sends_nothing() {
        let mut app = app();
        let (key, _, reference) = waiting(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        let check = |app: &mut App, change: &dyn Fn(&mut Card), words: &str| {
            let was = app.board.cards[i].clone();
            change(&mut app.board.cards[i]);
            select(app, &key);
            assert_eq!(press(app, 'e'), Action::None, "{words}");
            assert!(said(app).contains(words), "{words}: {}", said(app));
            assert!(app.edit_flights.is_empty() && app.edit_open.is_none());
            app.board.cards[i] = was;
        };
        check(
            &mut app,
            &|c| c.detail.as_mut().unwrap().intake = Some("pending".into()),
            &format!("{reference} is still being written up; edit it once that's done."),
        );
        // A new intent's card while its words are saved: no record yet (intent 0445).
        check(
            &mut app,
            &|c| c.detail.as_mut().unwrap().intake = Some(crate::resolve::SAVING.into()),
            "This new intent is still being saved",
        );
        check(
            &mut app,
            &|c| {
                c.stage = Stage::Build;
                c.state = Some("active".into());
            },
            "has started, so its text can't be edited here",
        );
        check(
            &mut app,
            &|c| c.stage = Stage::Maintain,
            "is done, so its text can't be edited here",
        );
        check(
            &mut app,
            &|c| c.detail.as_mut().unwrap().disposition = Some("wont_do".into()),
            "won't do; u brings it back",
        );
        // A card with no record behind it.
        check(&mut app, &|c| c.detail = None, "no intent record");
        // On a card in a working column, too.
        let started = app
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Build && c.detail.is_some())
            .unwrap()
            .key
            .clone();
        select(&mut app, &started);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert!(said(&app).contains("has started"), "{}", said(&app));
    }

    #[test]
    fn e_goes_at_once_while_other_changes_run_and_waits_as_the_other_card_keys_do() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        // Another card's change on its way, and an account action running: e still reads at once,
        // as d does (intent 0416).
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && waiting_work(c))
            .map(|c| (c.detail.as_ref().unwrap().id.clone(), c.machine_index))
            .unwrap();
        send(&mut app, other.1, json!({"action":"close","id":other.0}));
        app.busy = true;
        select(&mut app, &key);
        let (machine, request) = e_reads(&mut app, &id);
        app.busy = false;
        // A change made to this card before it goes first; the read holds up nothing after it.
        // The runner keeps that order, by the card each is for (intent 0445).
        let from = app.action_from.take();
        app.queue_action(machine, json!({"action":"refresh","id":id}));
        app.action_from = from;
        app.queue_action(machine, request.clone());
        app.queue_action(machine, json!({"action":"move","id":id,"delta":1}));
        let handed = app.resolver.ready();
        assert_eq!(handed.len(), 3, "every one goes to the runner: {handed:?}");
        let mut queue = crate::resolve::Queue::default();
        for (n, m, r, _) in &handed {
            assert_eq!(app.resolver.key_of(*n).as_deref(), Some(key.as_str()));
            queue.push(*n, *m, app.resolver.key_of(*n), r);
        }
        let (refresh, read, moved) = (handed[0].0, handed[1].0, handed[2].0);
        assert_eq!(handed[1].2, request);
        assert_eq!(queue.startable(), vec![refresh], "the refresh goes first");
        queue.done(refresh);
        assert_eq!(
            queue.startable(),
            vec![read, moved],
            "then the read, and a change after it doesn't wait for it"
        );
        // A read isn't a change: q neither waits for it nor says it carries on.
        for n in [refresh, read, moved] {
            app.job_started(n);
            app.resolver.handed(n);
        }
        app.finish_job(refresh, handed[0].2.clone(), saved());
        assert_eq!(app.resolver.unanswered(), 2, "the close and the move");
        assert_eq!(app.quit_held(), None);
        assert_eq!(app.resolver.running().len(), 2, "the close and the move");
        // e again while it's on its way asks nothing more.
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(
            said(&app),
            format!("{reference}'s text is on its way; it opens when it comes.")
        );
        // A start on its way: as d and ⏎, e waits for it.
        let mut app = self::app();
        crate::app::tests::room_in_every_session(&mut app);
        let (key, id, _) = waiting(&mut app);
        let machine = app.card(&key).unwrap().machine_index;
        app.queue_action(machine, json!({"action":"dispatch","id":id}));
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert!(said(&app).contains("is starting"), "{}", said(&app));
        // Without an action backend, as d.
        let mut app = self::app();
        waiting(&mut app);
        app.cfg.actions.command = None;
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert!(said(&app).contains("not configured"), "{}", said(&app));
    }

    #[test]
    fn e_on_a_card_whose_start_waits_for_its_write_up_says_so_and_queues_nothing() {
        // d on work still being written up starts it once that's done (intent 0445); e there
        // still says it's being written up, and once the write-up is read, that it's starting.
        let mut app = app();
        let (key, _, reference) = waiting(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(app.starts_when_written(&key));
        let handed = app.resolver.ready().len();
        assert_eq!(handed, 1, "the start, held for its write-up");
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(
            said(&app),
            format!("{reference} is still being written up; edit it once that's done.")
        );
        // The board has read the write-up before the runner started it.
        app.board.cards[i].detail.as_mut().unwrap().intake = None;
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert!(
            said(&app).starts_with(&format!(
                "{reference} starts as soon as its write-up is read"
            )),
            "{}",
            said(&app)
        );
        assert!(app.resolver.ready().is_empty(), "no read was queued");
        assert!(app.edit_flights.is_empty() && app.edit_open.is_none());
    }

    #[test]
    fn d_on_a_card_whose_new_text_is_being_saved_starts_it_once_its_written_up() {
        // The card shows ✎ for the save, as for any write-up: d waits for it, as 0.23.0's d on
        // work still being written up does, and the runner holds the start until a read from
        // after the save says the write-up is done (intents 0414 and 0445).
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let save = saves(&mut app, &key, wrote("Mine."));
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(
            said(&app).starts_with(&format!("{reference} is still being written up")),
            "{}",
            said(&app)
        );
        assert!(app.starts_when_written(&key));
        let handed = app.resolver.ready();
        assert_eq!(handed.len(), 1);
        let (n, m, start, gated) = handed[0].clone();
        assert!(gated);
        let mut queue = crate::resolve::Queue::default();
        queue.push(
            save.n,
            save.machine,
            app.resolver.key_of(save.n),
            &save.request,
        );
        queue.push_gated(n, m, app.resolver.key_of(n), &start);
        assert_eq!(queue.startable(), vec![save.n], "the save goes first");
        let before = std::collections::HashMap::from([(id.clone(), None)]);
        assert!(queue.written(m, &before).is_empty());
        queue.read_passed(m);
        assert!(
            queue.startable().is_empty(),
            "a read from before it says nothing"
        );
        answer(&mut app, &save, saved());
        queue.done(save.n);
        assert!(queue.written(m, &before).is_empty());
        queue.read_passed(m);
        assert!(queue.startable().is_empty());
        assert!(queue.written(m, &before).is_empty());
        queue.read_passed(m);
        assert_eq!(queue.startable(), vec![n]);
    }

    #[test]
    fn e_in_the_demo_says_its_the_demo() {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.demo = true;
        waiting(&mut app);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert!(said(&app).starts_with("This is the demo"), "{}", said(&app));
    }

    #[test]
    fn nothing_changed_or_a_cancel_says_nothing_and_an_empty_text_isnt_sent() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        assert_eq!(app.edit_ended(&key, wrote("Old words.")), Action::None);
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty());
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        assert_eq!(app.edit_ended(&key, cancelled()), Action::None);
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty());
        assert_eq!(app.resolver.unanswered(), 0, "nothing was sent");
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        assert_eq!(app.edit_ended(&key, wrote("")), Action::None);
        assert!(said(&app).contains("can't be empty"), "{}", said(&app));
        seen(&mut app);
        // A cancel forgets an unsaved version too.
        let sent = saves(&mut app, &key, wrote("Mine."));
        answer(&mut app, &sent, refused("No."));
        seen(&mut app);
        assert!(app.edits[&key].mine.is_some());
        press(&mut app, 'e');
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        // Emptied, it keeps his version, and says so where it can't be missed.
        assert_eq!(app.edit_ended(&key, wrote("")), Action::None);
        assert!(
            said(&app).contains("e opens your last version"),
            "{}",
            said(&app)
        );
        seen(&mut app);
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine.");
        app.edit_ended(&key, cancelled());
        assert!(app.edits.is_empty());
        assert_eq!(app.message(), None);
    }

    #[test]
    fn an_answer_that_comes_late_or_after_you_moved_on_never_opens_the_editor() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        // Long after e.
        let sent = read(&mut app, &id);
        app.edit_flights.last_mut().unwrap().at -= OPEN_WITHIN + Duration::from_secs(1);
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open, None);
        assert_eq!(
            said(&app),
            format!("{reference}'s text is ready: e opens it.")
        );
        // e then opens it at once, without reading it again.
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        app.edits.clear();
        // After a trip into herdr.
        let sent = read(&mut app, &id);
        app.away += 1;
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open, None);
        app.edits.clear();
        // On another card by then.
        let sent = read(&mut app, &id);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && c.detail.is_some())
            .unwrap()
            .key
            .clone();
        select(&mut app, &other);
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open, None);
        app.edits.clear();
        // Esc on the board lets it go.
        select(&mut app, &key);
        let sent = read(&mut app, &id);
        esc(&mut app);
        assert!(said(&app).starts_with("Stopped waiting"), "{}", said(&app));
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open, None);
        // Typing a new intent.
        app.edits.clear();
        let sent = read(&mut app, &id);
        press(&mut app, 'c');
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open, None);
        assert!(matches!(app.mode, Mode::Compose { .. }));
    }

    #[test]
    fn the_editor_waits_for_a_failure_to_be_read_but_never_long() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let sent = read(&mut app, &id);
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        app.fail("#9 didn't start: refused.");
        assert_eq!(app.editor_due(), None, "it would hide the failure");
        assert_eq!(app.edit_open.as_deref(), Some(key.as_str()));
        seen(&mut app);
        assert_eq!(app.editor_due().as_deref(), Some(key.as_str()));
        assert_eq!(app.editor_due(), None, "once");
        // A failure read long after e: the editor doesn't spring up; it's ready instead.
        press(&mut app, 'e');
        assert_eq!(app.edit_open.as_deref(), Some(key.as_str()));
        app.fail("#9 didn't start: refused.");
        if let Some(w) = app.edit_open_when.as_mut() {
            w.asked -= OPEN_WITHIN;
        }
        assert_eq!(app.editor_due(), None);
        assert_eq!(app.edit_open, None);
        assert_eq!(
            app.held.last().map(|(t, _)| t.clone()),
            Some(format!("{reference}'s text is ready: e opens it."))
        );
    }

    #[test]
    fn a_read_refused_or_without_its_text_says_so_without_the_failure_box() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let sent = read(&mut app, &id);
        answer(
            &mut app,
            &sent,
            refused(&format!(
                "{reference} has started, so its text can't be edited here."
            )),
        );
        assert_eq!(app.edit_open, None);
        assert!(said(&app).contains("has started"));
        // A read changes nothing, so a refused one is said, not boxed (intent 0416).
        assert!(!app.failing());
        let sent = read(&mut app, &id);
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: true,
                message: "done".into(),
                ..Default::default()
            }),
        );
        assert_eq!(app.edit_open, None);
        assert!(
            said(&app).contains("may not know edit yet"),
            "{}",
            said(&app)
        );
        let sent = read(&mut app, &id);
        answer(&mut app, &sent, Err("the action timed out; x".into()));
        assert_eq!(app.edit_open, None);
        assert!(!app.edits.contains_key(&key));
        assert!(!app.failing());
    }

    #[test]
    fn a_refusal_keeps_your_version_and_e_opens_it_with_the_same_fingerprint() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine, token ghp_x."));
        let refusal = "Line 1 looks like a token or password; take it out and save again.";
        answer(&mut app, &sent, refused(refusal));
        assert_eq!(
            app.message(),
            Some((
                format!(
                    "{reference}'s text wasn't saved: {refusal} Your version is kept: e opens it."
                )
                .as_str(),
                Tone::Fail
            ))
        );
        seen(&mut app);
        // e opens his version straight away: no read.
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        assert_eq!(app.edit_file(&key).unwrap().1, "Mine, token ghp_x.\n");
        let sent = saves(&mut app, &key, wrote("Mine."));
        assert_eq!(sent.request["was"], "sha256:aa");
        assert_eq!(sent.request["text"], "Mine.");
    }

    #[test]
    fn a_change_underneath_opens_your_version_above_the_newer_text() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        let changed = format!("{reference}'s text changed while you had it open.");
        let stale = || {
            Ok(Response {
                ok: false,
                status: Some("stale".into()),
                message: changed.clone(),
                text: Some("Theirs, newer.".into()),
                was: Some("sha256:bb".into()),
                ..Default::default()
            })
        };
        answer(&mut app, &sent, stale());
        assert_eq!(
            said(&app),
            format!(
                "{reference}'s text wasn't saved: {changed} Your version is kept: e opens it, with the newer text below it."
            )
        );
        seen(&mut app);
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        let (_, file, _, marker) = app.edit_file(&key).unwrap();
        let marker = marker.expect("the newer text's block has its marker");
        assert!(
            file.starts_with(&format!("Mine.\n\n<!-- kerdr {marker}: newer text below.")),
            "{file}"
        );
        assert!(file.contains("\nTheirs, newer.\n"), "{file}");
        // Saved as it opened: his version, against the newer fingerprint.
        let sent = saves(
            &mut app,
            &key,
            wrote(&edit::read_back(&file, Some(&marker))),
        );
        assert_eq!(sent.request["text"], "Mine.");
        assert_eq!(sent.request["was"], "sha256:bb");
        // Taking the newer text whole changes nothing, and says nothing.
        answer(&mut app, &sent, stale());
        seen(&mut app);
        press(&mut app, 'e');
        app.edit_open.take();
        assert_eq!(app.edit_ended(&key, wrote("Theirs, newer.")), Action::None);
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty());
    }

    /// A save of "Mine." that got no answer.
    fn lost_save(app: &mut App, key: &str, id: &str, reference: &str) -> usize {
        opened(app, key, id, "Old words.", "sha256:aa");
        let sent = saves(app, key, wrote("Mine."));
        answer(
            app,
            &sent,
            Err("the action timed out; its result is unknown. Check the canonical record before trying again. Do not retry automatically.".into()),
        );
        assert_eq!(
            app.message(),
            Some((
                format!(
                    "No clear answer came for {reference}'s save (the action timed out), so it may or may not have happened. e checks first, and opens your version if it didn't."
                )
                .as_str(),
                Tone::Fail
            ))
        );
        seen(app);
        assert_eq!(app.edits[key].mine.as_ref().unwrap().text, "Mine.");
        sent.machine
    }

    #[test]
    fn no_answer_to_a_save_has_e_check_first() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        // It landed: e says so and forgets it.
        lost_save(&mut app, &key, &id, &reference);
        let sent = read(&mut app, &id);
        answer(&mut app, &sent, text_answer("Mine.\n", "sha256:cc"));
        assert_eq!(app.edit_open, None);
        assert_eq!(
            app.message(),
            Some((
                format!("✓ {reference}'s text was saved after all: it reads as your version.")
                    .as_str(),
                Tone::Good
            ))
        );
        assert!(app.edits.is_empty());
        // It didn't: his version opens against the fresh fingerprint.
        lost_save(&mut app, &key, &id, &reference);
        let sent = read(&mut app, &id);
        answer(&mut app, &sent, text_answer("Old words.", "sha256:aa"));
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        assert_eq!(app.edit_file(&key).unwrap().1, "Mine.\n");
        let sent = saves(&mut app, &key, wrote("Mine."));
        assert_eq!(sent.request["was"], "sha256:aa");
        answer(&mut app, &sent, saved());
        caught_up(&mut app);
        // Someone else's text since: his version, with theirs below it.
        lost_save(&mut app, &key, &id, &reference);
        let sent = read(&mut app, &id);
        answer(&mut app, &sent, text_answer("Someone else's.", "sha256:dd"));
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        let file = app.edit_file(&key).unwrap().1;
        assert!(file.starts_with("Mine.\n\n<!--"), "{file}");
        assert!(file.contains("\nSomeone else's.\n"), "{file}");
        let sent = saves(&mut app, &key, wrote("Mine."));
        assert_eq!(sent.request["was"], "sha256:dd");
    }

    #[test]
    fn your_version_survives_a_save_that_cant_go() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let machine = opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        // Past the 64 KiB a request may be: refused before anything runs.
        let long = "x".repeat(70_000);
        assert_eq!(app.edit_ended(&key, wrote(&long)), Action::None);
        assert!(said(&app).contains("too large"), "{}", said(&app));
        seen(&mut app);
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, long);
        assert_eq!(app.resolver.unanswered(), 0, "nothing was sent");
        // Its machine stopped answering while the editor was open.
        app.machines[machine].conn = crate::poll::Conn::Connecting;
        let name = app.machines[machine].name();
        assert_eq!(app.edit_ended(&key, wrote("Mine.")), Action::None);
        assert_eq!(
            said(&app),
            format!(
                "{reference}'s text wasn't saved: {name} isn't answering. Your version is kept: e opens it."
            )
        );
        seen(&mut app);
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine.");
        // No editor could start: what was tried, and nothing lost.
        app.edits.get_mut(&key).unwrap().mine = None;
        assert_eq!(
            app.edit_ended(
                &key,
                Ended::NoEditor {
                    skipped: vec!["nvim: not found".into(), "vim: not found".into()]
                }
            ),
            Action::None
        );
        let text = said(&app);
        assert!(text.contains("nvim: not found; vim: not found"), "{text}");
        assert!(text.contains("editor in Kerdr's config"), "{text}");
        assert!(app.edits.contains_key(&key));
    }

    #[test]
    fn a_save_sent_on_to_the_home_a_refusal_named_carries_your_text() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        let machine = opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        let metadata = crate::actions::request_metadata(&sent.request);
        assert_eq!(
            metadata["text"], 5,
            "the text is only counted after sending"
        );
        assert!(!metadata.to_string().contains("Mine."));
        let other = if machine == 1 { 0 } else { 1 };
        let host = app.machines[other].name();
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: false,
                message: "It lives elsewhere.".into(),
                home: Some(format!("{host}/ARK")),
                ..Default::default()
            }),
        );
        // Sent on quietly, still shown, in its own place in the queue.
        assert_eq!(app.message(), None);
        assert_eq!(shown(&app, &key), (Some("Mine.".into()), true));
        let Some((n, to, again, _)) = app.resolver.ready().into_iter().next() else {
            panic!("nothing was sent on")
        };
        assert_eq!(to, other);
        assert_eq!(again["text"], "Mine.");
        assert_eq!(again["was"], "sha256:aa");
        // Its answer settles it there.
        app.finish_job(n, crate::actions::request_metadata(&again), saved());
        assert!(app.edits.is_empty());
        assert_eq!(app.message(), None);
    }

    #[test]
    fn a_save_whose_home_cant_be_asked_keeps_your_version() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: false,
                message: "It lives elsewhere.".into(),
                home: Some("elsewhere/ARK".into()),
                ..Default::default()
            }),
        );
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(
            text.ends_with(&format!(
                "can't change it there. Your version of {reference}'s text is kept: e opens it."
            )),
            "{text}"
        );
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine.");
        assert!(!app.resolver.shows());
    }

    #[test]
    fn an_editor_that_couldnt_start_is_named_beside_the_one_that_did() {
        let run = Run {
            editor: Some("vim".into()),
            ok: true,
            skipped: vec!["code --wait: No such file or directory (os error 2)".into()],
            ..Default::default()
        };
        assert_eq!(
            skipped_note(&run),
            " (code --wait couldn't start, so vim opened it.)"
        );
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Same.", "sha256:aa");
        // Nothing changed, so that's all that's said: the setting needs fixing.
        app.edit_ended(
            &key,
            Ended::Wrote {
                text: "Same.".into(),
                run: run.clone(),
                touched: true,
            },
        );
        assert_eq!(
            app.message(),
            Some((
                format!("code --wait couldn't start, so vim opened {reference}'s text.").as_str(),
                Tone::Warn
            ))
        );
        // Beside a save, too, which itself says nothing.
        opened(&mut app, &key, &id, "Same.", "sha256:aa");
        app.message = None;
        let Action::Mutate { .. } = app.edit_ended(
            &key,
            Ended::Wrote {
                text: "Changed.".into(),
                run,
                touched: true,
            },
        ) else {
            panic!("no save")
        };
        assert!(said(&app).starts_with("code --wait couldn't start"));
    }

    #[test]
    fn your_version_still_opens_once_the_item_has_started_but_nothing_is_sent() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("My long rewrite."));
        let started = format!("{reference} has started, so its text can't be edited here.");
        answer(&mut app, &sent, refused(&started));
        assert!(said(&app).ends_with("Your version is kept: e opens it."));
        seen(&mut app);
        // It's dispatched meanwhile.
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].state = Some("active".into());
        select(&mut app, &key);
        assert_eq!(press(&mut app, 'e'), Action::None, "no read, no save");
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        let (_, file, intro, _) = app.edit_file(&key).unwrap();
        assert_eq!(file, "My long rewrite.\n");
        assert!(intro.contains("nothing is sent"), "{intro}");
        // Closing it written sends nothing, keeps what he wrote there, and says so where it can't
        // be missed: a save that works says nothing.
        assert_eq!(
            app.edit_ended(&key, wrote("My longer rewrite.")),
            Action::None
        );
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(text.contains(&started), "{text}");
        assert!(text.contains("Nothing was sent"), "{text}");
        seen(&mut app);
        assert_eq!(
            app.edits[&key].mine.as_ref().unwrap().text,
            "My longer rewrite."
        );
        assert!(app.edits[&key].read_only.is_none());
        assert_eq!(app.resolver.unanswered(), 0);
        // Closed as it opened: nothing to say.
        press(&mut app, 'e');
        app.edit_open.take();
        assert_eq!(
            app.edit_ended(&key, left("My longer rewrite.")),
            Action::None
        );
        assert_eq!(app.message(), None);
        // An emptied file keeps the version he had.
        press(&mut app, 'e');
        app.edit_open.take();
        assert_eq!(app.edit_ended(&key, wrote("")), Action::None);
        seen(&mut app);
        assert_eq!(
            app.edits[&key].mine.as_ref().unwrap().text,
            "My longer rewrite."
        );
        // :cq lets it go, saying nothing.
        press(&mut app, 'e');
        app.edit_open.take();
        let cancelled = Ended::Cancelled {
            run: Run::default(),
        };
        assert_eq!(app.edit_ended(&key, cancelled), Action::None);
        assert!(app.edits.is_empty());
        assert_eq!(app.message(), None);
        // With nothing of his held, a started card is refused as before.
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open, None);
        assert_eq!(said(&app), started);
    }

    #[test]
    fn q_warns_before_it_loses_an_unsaved_version() {
        // One way of warning on q (intent 0445's): what only Kerdr keeps is named, and a second q
        // soon after quits.
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        assert_eq!(app.quit_loses(), None, "nothing held: q quits");
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        assert_eq!(app.quit_loses(), None, "a text only read: q quits");
        app.edits.clear();
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        // On its way: if it fails, only Kerdr has your version.
        let said = app.quit_loses().unwrap();
        assert!(
            said.contains(&format!("{reference}'s new text is still being saved"))
                && said.contains("quitting drops it"),
            "{said}"
        );
        answer(
            &mut app,
            &sent,
            refused("Line 1 looks like a token or password; take it out and save again."),
        );
        seen(&mut app);
        assert_eq!(
            app.quit_loses().as_deref(),
            Some(
                format!(
                    "Your unsaved version of {reference}'s text is kept only while Kerdr is open (e on its card opens it): quitting drops it."
                )
                .as_str()
            )
        );
        // Named beside the other things only Kerdr keeps.
        app.draft = "half a thought".into();
        let said = app.quit_loses().unwrap();
        assert!(
            said.contains("c brings them back") && said.contains(&reference),
            "{said}"
        );
        app.draft.clear();
        // Saved after all: nothing left to lose.
        let again = {
            press(&mut app, 'e');
            app.edit_open.take();
            saves(&mut app, &key, wrote("Mine, fixed."))
        };
        answer(&mut app, &again, saved());
        assert_eq!(app.quit_loses(), None);
    }

    #[test]
    fn closing_without_writing_sends_nothing_and_keeps_your_version() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: false,
                status: Some("stale".into()),
                message: "Changed.".into(),
                text: Some("Theirs, newer.".into()),
                was: Some("sha256:bb".into()),
                ..Default::default()
            }),
        );
        seen(&mut app);
        press(&mut app, 'e');
        app.edit_open.take();
        let (_, file, _, marker) = app.edit_file(&key).unwrap();
        // :q on the file as it opened: the newer text stays, his version is kept, and nothing
        // happened, so nothing is said.
        assert_eq!(
            app.edit_ended(&key, left(&edit::read_back(&file, marker.as_deref()))),
            Action::None
        );
        assert_eq!(app.message(), None);
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine.");
        assert_eq!(app.resolver.unanswered(), 0);
        // After a refusal too: :q doesn't send the refused text again.
        app.edits.get_mut(&key).unwrap().current = "Old words.".into();
        press(&mut app, 'e');
        app.edit_open.take();
        assert_eq!(app.edit_ended(&key, left("Mine.")), Action::None);
        // :wq on it, the same text written, is his choice to keep his version.
        press(&mut app, 'e');
        app.edit_open.take();
        let sent = saves(&mut app, &key, wrote("Mine."));
        assert_eq!(sent.request["text"], "Mine.");
    }

    #[test]
    fn a_quick_quit_is_quiet_and_a_window_editor_without_its_wait_flag_isnt_started() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        // Opened and :q at once: nothing changed, and nothing is said, however quick.
        assert_eq!(app.edit_ended(&key, left("Old words.")), Action::None);
        assert_eq!(app.message(), None);
        assert!(app.edits.is_empty());
        // code without --wait: not started, the flag named, and the text kept for e.
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let code = |s: &str| vec![edit::editor_words(s), vec!["nvim".to_string()]];
        assert!(app.editor_wont_wait(&key, &code("code")));
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(
            text.contains(&format!("Didn't open {reference}'s text")),
            "{text}"
        );
        assert!(
            text.contains("Add --wait to editor in Kerdr's config"),
            "{text}"
        );
        assert!(text.contains(r#"editor = "code --wait""#), "{text}");
        assert!(app.edits.contains_key(&key), "the text is kept");
        press(&mut app, 'e');
        assert_eq!(
            app.edit_open.take().as_deref(),
            Some(key.as_str()),
            "e opens it with no read"
        );
        // With its flag, or a terminal editor, it starts.
        app.message = None;
        assert!(!app.editor_wont_wait(&key, &code("code --wait")));
        assert!(!app.editor_wont_wait(&key, &code("nvim")));
        assert_eq!(app.message(), None);
    }

    #[test]
    fn a_save_that_only_partly_happened_counts_as_saved_and_says_what_didnt() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("New words."));
        let didnt = format!(
            "Saved {reference}'s text, but its write-up couldn't start, so its title and done-when won't catch up until it's rerun."
        );
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: true,
                status: Some("partial".into()),
                message: didnt.clone(),
                ..Default::default()
            }),
        );
        // Saved: his version let go, the card keeps its new words.
        assert!(app.edits.is_empty());
        assert!(app.resolver.shows());
        assert_eq!(shown(&app, &key).0.as_deref(), Some("New words."));
        // And what didn't happen is said where it can't be missed.
        assert_eq!(app.message(), Some((didnt.as_str(), Tone::Fail)));
        seen(&mut app);
        // A plain save still says nothing.
        caught_up(&mut app);
        opened(&mut app, &key, &id, "New words.", "sha256:bb");
        let sent = saves(&mut app, &key, wrote("Newer words."));
        answer(&mut app, &sent, saved());
        assert_eq!(app.message(), None);
    }

    #[test]
    fn a_save_that_stands_with_its_push_to_come_says_so_and_a_partial_one_keeps_your_version() {
        // Release 2's backends: ok means the change stands, with a note for what's left; only
        // part happening is `partial` (intent 0591).
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("New words."));
        let left = "saved here; it reaches your other machines once the push goes through, which is retried by itself.";
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: true,
                message: format!("Saved {reference}'s text: {left}"),
                note: Some(left.into()),
                ..Default::default()
            }),
        );
        assert!(app.edits.is_empty(), "saved: his version let go");
        assert_eq!(
            app.message(),
            Some((
                format!("{reference}'s text is saved; {left}").as_str(),
                Tone::Info
            ))
        );
        caught_up(&mut app);
        app.message = None;
        opened(&mut app, &key, &id, "New words.", "sha256:bb");
        let sent = saves(&mut app, &key, wrote("Newer words."));
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: false,
                partial: true,
                message: "Its text changed, but its title couldn't be.".into(),
                ..Default::default()
            }),
        );
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(
            said.starts_with(&format!(
                "{reference}'s save happened only in part: Its text changed"
            )),
            "{said}"
        );
        assert!(!said.contains("wasn't saved"), "{said}");
        assert!(
            app.edits.contains_key(&key),
            "his version stays until e checks"
        );
    }

    #[test]
    fn the_editors_window_counts_from_e_and_is_checked_as_it_opens() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let ready = format!("{reference}'s text is ready: e opens it.");
        // A slow read: back 14 seconds after e, just in time, but a failure box is up.
        let sent = read(&mut app, &id);
        app.edit_flights.last_mut().unwrap().at -= Duration::from_secs(14);
        answer(&mut app, &sent, text_answer("Words.", "sha256:aa"));
        assert_eq!(app.edit_open.as_deref(), Some(key.as_str()), "scheduled");
        app.fail("#9 didn't start: refused.");
        assert_eq!(app.editor_due(), None, "it waits for the box");
        // The box is read two seconds later: 16 seconds after e, too late to open by itself.
        if let Some(w) = app.edit_open_when.as_mut() {
            w.asked -= Duration::from_secs(2);
        }
        seen(&mut app);
        assert_eq!(app.editor_due(), None);
        assert_eq!(app.edit_open, None);
        assert_eq!(said(&app), ready);
        // e then opens it, with no read.
        press(&mut app, 'e');
        assert_eq!(app.editor_due().as_deref(), Some(key.as_str()));
        app.edits.clear();
        // Past its window with no box up at all: the same.
        opened_for_later(&mut app, &id);
        if let Some(w) = app.edit_open_when.as_mut() {
            w.asked -= OPEN_WITHIN;
        }
        assert_eq!(app.editor_due(), None);
        assert_eq!(said(&app), ready);
        app.edits.clear();
        // Scheduled, then he moved to another card before it opened.
        opened_for_later(&mut app, &id);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && c.detail.is_some())
            .unwrap()
            .key
            .clone();
        select(&mut app, &other);
        assert_eq!(app.editor_due(), None);
        assert_eq!(said(&app), ready);
        app.edits.clear();
        // Scheduled, then a trip into herdr.
        select(&mut app, &key);
        opened_for_later(&mut app, &id);
        app.away += 1;
        assert_eq!(app.editor_due(), None);
        app.edits.clear();
        // Scheduled, then typing a new intent.
        opened_for_later(&mut app, &id);
        press(&mut app, 'c');
        assert_eq!(app.editor_due(), None);
        esc(&mut app);
        // Still there, in time: it opens.
        app.edits.clear();
        select(&mut app, &key);
        opened_for_later(&mut app, &id);
        assert_eq!(app.editor_due().as_deref(), Some(key.as_str()));
    }

    /// e, its read answered at once: the editor scheduled, not opened yet.
    fn opened_for_later(app: &mut App, id: &str) {
        let sent = read(app, id);
        answer(app, &sent, text_answer("Words.", "sha256:aa"));
        assert!(app.edit_open.is_some(), "{:?}", app.message());
        app.message = None;
    }

    #[test]
    fn an_editor_that_dies_keeps_what_it_wrote_and_sends_nothing() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        let killed = |text: &str, touched: bool| Ended::Died {
            text: text.into(),
            run: Run {
                editor: Some("nvim".into()),
                died: true,
                ..Default::default()
            },
            touched,
        };
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        assert_eq!(
            app.edit_ended(&key, killed("Half a rewrite.", true)),
            Action::None
        );
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(text.starts_with("nvim stopped without closing"), "{text}");
        assert!(
            text.contains("kept as your version, sent nowhere"),
            "{text}"
        );
        seen(&mut app);
        assert_eq!(app.resolver.unanswered(), 0, "nothing was sent");
        assert_eq!(
            app.edits[&key].mine.as_ref().unwrap().text,
            "Half a rewrite."
        );
        // e opens it straight away, and :wq then saves it, with what it was read as.
        press(&mut app, 'e');
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        assert_eq!(app.edit_file(&key).unwrap().1, "Half a rewrite.\n");
        // Died again before writing: his version stays as it was.
        assert_eq!(
            app.edit_ended(&key, killed("Half a rewrite.", false)),
            Action::None
        );
        assert!(
            said(&app).contains("Your version is kept"),
            "{}",
            said(&app)
        );
        seen(&mut app);
        assert_eq!(
            app.edits[&key].mine.as_ref().unwrap().text,
            "Half a rewrite."
        );
        let sent = saves(&mut app, &key, wrote("Half a rewrite."));
        assert_eq!(sent.request["was"], "sha256:aa");
        answer(&mut app, &sent, saved());
        caught_up(&mut app);
        // A file that couldn't be read back: nothing sent, nothing of his let go, and where the
        // file is kept named.
        opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let kept = std::env::temp_dir()
            .join("kerdr-kept-1-x")
            .join("0414-text.md");
        assert_eq!(
            app.edit_ended(
                &key,
                Ended::Unreadable {
                    why: "Is a directory (os error 21)".into(),
                    kept: kept.clone(),
                    run: nvim(),
                }
            ),
            Action::None
        );
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(text.contains(&format!("{}", kept.display())), "{text}");
        assert!(text.contains("nothing was sent"), "{text}");
        assert_eq!(app.resolver.unanswered(), 0);
        assert!(app.edits.contains_key(&key));
    }

    #[test]
    fn his_own_lines_like_kerdrs_markers_survive_a_save() {
        let mut app = app();
        let (key, id, _) = waiting(&mut app);
        let his = "How the conflict layout looks:\n\n<!-- kerdr: newer text below. an example -->\nkept\n<!-- kerdr: end of the newer text -->\n\n```\n<!-- kerdr 0123456789abcdef: newer text below. -->\nin a fence\n<!-- kerdr 0123456789abcdef: end of the newer text -->\n```\nLast line.";
        opened(&mut app, &key, &id, his, "sha256:aa");
        let (_, file, _, marker) = app.edit_file(&key).unwrap();
        assert_eq!(file, format!("{his}\n"));
        // :q: the file as it opened, nothing written, so nothing changed, held version or not.
        let back = edit::read_back(&file, marker.as_deref());
        assert_eq!(app.edit_ended(&key, left(&back)), Action::None);
        assert_eq!(app.resolver.unanswered(), 0);
        // Even if what it reads back somehow differed: an unwritten file is never sent.
        opened(&mut app, &key, &id, his, "sha256:aa");
        assert_eq!(app.edit_ended(&key, left("How the conflict")), Action::None);
        assert_eq!(app.resolver.unanswered(), 0);
        // His last line changed and saved: every other line of his goes as it was.
        opened(&mut app, &key, &id, his, "sha256:aa");
        let (_, file, _, marker) = app.edit_file(&key).unwrap();
        let edited = file.replace("Last line.", "Last line, changed.");
        let sent = saves(
            &mut app,
            &key,
            wrote(&edit::read_back(&edited, marker.as_deref())),
        );
        let expected = his.replace("Last line.", "Last line, changed.");
        assert_eq!(sent.request["text"], expected.as_str());
        // Changed underneath meanwhile: only the block Kerdr adds is left out of what he saves.
        answer(
            &mut app,
            &sent,
            Ok(Response {
                ok: false,
                status: Some("stale".into()),
                message: "Changed.".into(),
                text: Some("Theirs.".into()),
                was: Some("sha256:bb".into()),
                ..Default::default()
            }),
        );
        seen(&mut app);
        press(&mut app, 'e');
        app.edit_open.take();
        let (_, file, _, marker) = app.edit_file(&key).unwrap();
        assert!(file.contains("Theirs."));
        let sent = saves(
            &mut app,
            &key,
            wrote(&edit::read_back(&file, marker.as_deref())),
        );
        assert_eq!(sent.request["text"], expected.as_str());
        assert_eq!(sent.request["was"], "sha256:bb");
    }

    #[test]
    fn your_version_opens_while_its_machine_isnt_answering() {
        let mut app = app();
        let (key, id, reference) = waiting(&mut app);
        let machine = opened(&mut app, &key, &id, "Old words.", "sha256:aa");
        let sent = saves(&mut app, &key, wrote("Mine."));
        answer(&mut app, &sent, refused("Busy."));
        seen(&mut app);
        // Its machine stops answering: e still opens his version, without asking it.
        app.machines[machine].conn = crate::poll::Conn::Connecting;
        let name = app.machines[machine].name();
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        assert_eq!(app.message(), None);
        assert_eq!(app.edit_file(&key).unwrap().1, "Mine.\n");
        // What he writes there is kept, and nothing is sent.
        assert_eq!(app.edit_ended(&key, wrote("Mine, more.")), Action::None);
        assert!(
            said(&app).contains(&format!("{name} isn't answering")),
            "{}",
            said(&app)
        );
        seen(&mut app);
        assert_eq!(app.edits[&key].mine.as_ref().unwrap().text, "Mine, more.");
        assert_eq!(app.resolver.unanswered(), 0);
        app.machines[machine].conn = crate::poll::Conn::Live {
            latency: Duration::ZERO,
        };
        app.edits.clear();
        // After a save with no clear answer, the machine gone too: nothing can be checked, so his
        // version opens to copy from, and nothing is sent.
        lost_save(&mut app, &key, &id, &reference);
        app.machines[machine].conn = crate::poll::Conn::Connecting;
        assert_eq!(press(&mut app, 'e'), Action::None);
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        let (_, file, intro, _) = app.edit_file(&key).unwrap();
        assert_eq!(file, "Mine.\n");
        assert!(
            intro.contains("can't check whether your last save"),
            "{intro}"
        );
        assert!(intro.contains("nothing is sent"), "{intro}");
        assert_eq!(app.edit_ended(&key, wrote("Mine, again.")), Action::None);
        assert!(
            said(&app).contains("e checks whether your last save landed, then opens it"),
            "{}",
            said(&app)
        );
        seen(&mut app);
        assert_eq!(app.resolver.unanswered(), 0);
        let held = &app.edits[&key];
        assert_eq!(held.mine.as_ref().unwrap().text, "Mine, again.");
        assert!(held.check_first, "the next save still checks first");
        // Back: e checks first.
        app.machines[machine].conn = crate::poll::Conn::Live {
            latency: Duration::ZERO,
        };
        let sent = read(&mut app, &id);
        // The check itself refused: his version opens all the same, and nothing is sent.
        answer(&mut app, &sent, refused("The backend is busy."));
        assert_eq!(app.edit_open.take().as_deref(), Some(key.as_str()));
        let (_, _, intro, _) = app.edit_file(&key).unwrap();
        assert!(
            intro.contains("couldn't check whether your last save"),
            "{intro}"
        );
        assert!(intro.contains("The backend is busy."), "{intro}");
        assert_eq!(app.edit_ended(&key, left("Mine, again.")), Action::None);
        assert_eq!(app.resolver.unanswered(), 0);
        assert!(app.edits[&key].check_first);
        // A check refused long after e: said, and e checks again.
        let sent = read(&mut app, &id);
        app.edit_flights.last_mut().unwrap().at -= OPEN_WITHIN;
        answer(&mut app, &sent, Err("the action timed out; x".into()));
        assert_eq!(app.edit_open, None);
        assert!(
            said(&app).starts_with("Couldn't check whether"),
            "{}",
            said(&app)
        );
        assert!(app.edits[&key].check_first);
        e_reads(&mut app, &id);
    }

    #[test]
    fn an_error_is_cut_to_its_first_clause() {
        assert_eq!(
            first_clause(
                "the action timed out; its result is unknown. Check the canonical record before trying again."
            ),
            "the action timed out"
        );
        assert_eq!(
            first_clause(
                "the action's JSON response is invalid (exit 1): x. Its result is unknown."
            ),
            "the action's JSON response is invalid (exit 1): x"
        );
        assert_eq!(first_clause("plain"), "plain");
    }

    #[test]
    fn the_keys_name_e_wherever_the_other_card_keys_show() {
        let mut app = app();
        let (key, _, _) = waiting(&mut app);
        let board = crate::ui::tests_render(&app, 220, 44);
        assert!(board.contains("d ⏎ start it   e edit its text"), "{board}");
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let peek = crate::ui::tests_render(&app, 220, 44);
        assert!(
            peek.contains("start it in its session   e edit its text"),
            "{peek}"
        );
        app.enter_backlog(false);
        let backlog = crate::ui::tests_render(&app, 220, 44);
        assert!(
            backlog.contains("d start it   e edit its text"),
            "{backlog}"
        );
        select(&mut app, &key);
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let menu = crate::ui::tests_render(&app, 220, 44);
        assert!(menu.contains("edit its text"), "{menu}");
        let menu = crate::ui::tests_render(&app, 80, 24);
        assert!(menu.contains("edit its text"), "{menu}");
        esc(&mut app);
        press(&mut app, '?');
        let keys = crate::ui::tests_render(&app, 220, 60);
        assert!(
            keys.contains("edit its text, the words that started it"),
            "{keys}"
        );
    }
}
