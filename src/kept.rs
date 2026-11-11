//! What Kerdr keeps for you between runs (intent 0591, release 4), in its state folder, each file
//! readable by you alone:
//!
//! - `held.json`, the hold file: changes you asked for while their machine wasn't answering, each
//!   with its op, the card's `rev` when you pressed the key, when that was, and who has it now:
//!   waiting in the file, or handed on by one Kerdr (`by`) until its answer takes it out. The next
//!   Kerdr sends them once their machine answers; nothing sends them while no Kerdr is open, and
//!   one older than a day isn't sent at all: its card says it was dropped, and why.
//! - `words.json`: the new-intent box's words and images, words a save gave back (with the send
//!   they came back from), your versions of item texts not yet saved, and new intents still being
//!   saved in their backend, so q never has to drop them. Each set of words is marked with the
//!   Kerdr that took it, and stays in the file until that Kerdr keeps its words in their place.
//!
//! Every change to either file holds `kept.lock` while it reads the file, changes it and writes it
//! whole (a new file moved into place), so Kerdrs open at once never lose each other's changes,
//! and a crash never leaves half a file. Each Kerdr holds a lock file of its own while it runs,
//! `kerdr-<name>.lock`, which the system lets go however it ends: what a Kerdr that has gone had
//! handed on, or taken, is another's to send or give back (Codex's review 60). A file that can't be
//! read is never written over: it's moved aside, and that's said. A change that couldn't be written
//! stays in memory and goes with the next write that can; until then Kerdr says so, and q says that
//! quitting would lose it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a held change waits for its machine before it's dropped instead of sent.
pub const HOLD_MOST_SECS: i64 = 24 * 60 * 60;

const HELD_FILE: &str = "held.json";
const WORDS_FILE: &str = "words.json";
const LOCK_FILE: &str = "kept.lock";

/// One change held for a machine that wasn't answering.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HeldRequest {
    /// The machine it's for, by name: its card's home.
    pub machine: String,
    /// The request as it goes, with its op.
    pub request: Value,
    /// The card's `rev` when the key was pressed, which the backend checks it against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The card it's for, by key, and how it's named: "#582".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub reference: String,
    /// The key that asked for it, for its card's line: "d".
    #[serde(default)]
    pub what: String,
    /// When the key was pressed, in seconds since 1970.
    pub pressed: i64,
    /// What it waits for besides its machine: "write-up", "start", or "save:<submission>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The Kerdr that has handed it on and waits for its answer, by name; None while it waits in
    /// the file. Another Kerdr sends it only once that one has gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Why it stays in the file though its machine answers, while that's so: "build-box's backend
    /// doesn't check a held change again". What its card says if its day runs out. Its key pressed
    /// again sends it as itself, with its op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stays: Option<String>,
}

impl HeldRequest {
    pub fn op(&self) -> &str {
        self.request["op"].as_str().unwrap_or_default()
    }

    /// Whether it has waited too long to be sent.
    pub fn expired(&self, now: i64) -> bool {
        now - self.pressed > HOLD_MOST_SECS
    }
}

/// A held change Kerdr dropped instead of sending, for its card to say so: "d dropped:
/// LAB-Server didn't answer for a day".
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub key: Option<String>,
    pub reference: String,
    pub said: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct HeldFile {
    #[serde(default)]
    held: Vec<HeldRequest>,
    /// The machines whose backend last listed "hold", so a change for one that hasn't answered
    /// since Kerdr opened can still be held for it.
    #[serde(default)]
    holds: Vec<String>,
}

/// A change to the hold file, kept until it's written.
#[derive(Clone, Debug)]
enum Change {
    Hold(HeldRequest),
    Release(String),
    /// Back to waiting in the file, by its op: only while the Kerdr named still has it, so a
    /// change written late never undoes another Kerdr's later claim (Codex's review 62, 3).
    Unclaim(String, String),
    /// The machine it's for now, and the request as it goes there, by its op: its card lives
    /// there, a refusal said.
    Moved(String, String, Value),
    Holds(String, bool),
    /// Why it stays held though its machine answers, by its op, or None once it goes.
    Stays(String, Option<String>),
}

impl Change {
    fn apply(&self, file: &mut HeldFile) {
        match self {
            Change::Hold(entry) => {
                if !file.held.iter().any(|h| h.op() == entry.op()) {
                    file.held.push(entry.clone());
                }
            }
            Change::Release(op) => file.held.retain(|h| h.op() != op),
            Change::Unclaim(op, me) => {
                for h in file.held.iter_mut().filter(|h| h.op() == op) {
                    if h.by.as_deref() == Some(me.as_str()) {
                        h.by = None;
                    }
                }
            }
            Change::Moved(op, machine, request) => {
                for h in file.held.iter_mut().filter(|h| h.op() == op) {
                    h.machine = machine.clone();
                    h.request = request.clone();
                }
            }
            Change::Holds(name, yes) => {
                file.holds.retain(|m| m != name);
                if *yes {
                    file.holds.push(name.clone());
                }
            }
            Change::Stays(op, why) => {
                for h in file.held.iter_mut().filter(|h| h.op() == op) {
                    h.stays = why.clone();
                }
            }
        }
    }
}

/// Whether this Kerdr may send a held change now (Kept::claim).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// It's this Kerdr's to send: marked as handed on by it.
    Mine,
    /// Another Kerdr open now has handed it on.
    Taken,
    /// It's gone from the file: answered, or dropped, by another Kerdr.
    Gone,
    /// The hold file couldn't be read or written, so nothing was decided: it isn't sent, or
    /// dropped, until that can be recorded (Codex's review 62, 3).
    Unsure,
}

/// This Kerdr's own lock file, held while it runs.
#[derive(Debug)]
struct Alive {
    path: PathBuf,
    _file: File,
}

impl Drop for Alive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The hold file, and what Kerdr holds in it.
#[derive(Debug, Default)]
pub struct Kept {
    /// The state folder it lives in; None keeps it in memory alone (the demo, and tests).
    dir: Option<PathBuf>,
    /// This Kerdr's name among those open at once, and the lock it holds while it runs.
    me: String,
    alive: Option<Alive>,
    pub held: Vec<HeldRequest>,
    pub holds: Vec<String>,
    /// Dropped since Kerdr opened: over a day old, or dropped with m.
    pub dropped: Vec<Dropped>,
    /// Changes not yet in the hold file, a write having failed: each write tries them again.
    unwritten: Vec<Change>,
    /// Why the hold file or the words file couldn't be written, while that's so.
    held_failed: Option<String>,
    words_failed: Option<String>,
    /// What to say once: a file moved aside because it couldn't be read.
    pub notes: Vec<String>,
}

impl Kept {
    pub fn in_memory() -> Kept {
        Kept {
            me: "here".into(),
            ..Kept::default()
        }
    }

    /// The hold file in `dir`. What's over a day old is dropped from it now, and listed in
    /// `dropped`, for its card to say so.
    pub fn at(dir: &Path, now: i64) -> Kept {
        let me = crate::actions::new_op().replace("k-", "");
        let mut kept = Kept {
            dir: Some(dir.to_path_buf()),
            alive: None,
            me,
            ..Kept::default()
        };
        match kept.lock_alive(dir) {
            Ok(alive) => kept.alive = Some(alive),
            Err(e) => kept.held_failed = Some(e.to_string()),
        }
        kept.sync();
        kept.expire(now);
        kept
    }

    /// This Kerdr's lock file, created and held.
    fn lock_alive(&self, dir: &Path) -> io::Result<Alive> {
        private_folder(dir)?;
        let path = dir.join(format!("kerdr-{}.lock", self.me));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.try_lock().map_err(io::Error::other)?;
        Ok(Alive { path, _file: file })
    }

    /// This Kerdr's name, as `by` gives it.
    pub fn me(&self) -> &str {
        &self.me
    }

    /// Tries again to write what the hold file is missing. Why it can't, if it can't.
    pub fn flush(&mut self) -> Option<String> {
        if self.dir.is_some() && !self.unwritten.is_empty() {
            self.update(|_| ());
        }
        let e = self
            .held_failed
            .as_ref()
            .filter(|_| !self.unwritten.is_empty())?;
        let file = self
            .dir
            .as_ref()
            .map(|d| d.join(HELD_FILE).display().to_string())
            .unwrap_or_default();
        Some(format!("Kerdr couldn't write {file} ({e})"))
    }

    /// Why the hold file couldn't be read or written the last time Kerdr tried, while that's so.
    pub fn held_trouble(&self) -> Option<String> {
        let e = self.held_failed.as_ref()?;
        let file = self
            .dir
            .as_ref()
            .map(|d| d.join(HELD_FILE).display().to_string())
            .unwrap_or_else(|| HELD_FILE.to_string());
        Some(format!("Kerdr couldn't write {file} ({e})"))
    }

    /// Why what Kerdr keeps couldn't be written, while that's so: quitting would lose it.
    pub fn unsaved(&self) -> Option<String> {
        let file = |name: &str| {
            self.dir
                .as_ref()
                .map(|d| d.join(name).display().to_string())
                .unwrap_or_else(|| name.to_string())
        };
        match (&self.held_failed, &self.words_failed) {
            (Some(e), _) if !self.unwritten.is_empty() => {
                Some(format!("Kerdr couldn't write {} ({e})", file(HELD_FILE)))
            }
            (_, Some(e)) => Some(format!("Kerdr couldn't write {} ({e})", file(WORDS_FILE))),
            _ => None,
        }
    }

    /// Reads the hold file again, with what this Kerdr hasn't written yet laid over it: what
    /// another Kerdr held, answered or dropped meanwhile shows here too.
    pub fn sync(&mut self) {
        if let Some(file) = self.update(|f| f.clone()) {
            self.held = file.held;
            self.holds = file.holds;
        }
    }

    /// Drops what has waited over a day, from the file too: not one a Kerdr open now has handed
    /// on, whose answer is still to come. Checked and taken out under the lock, as the file is
    /// then, so one another Kerdr claimed a moment ago isn't dropped under it (Codex's review 62,
    /// 8). When the file can't be read or written, nothing is dropped now.
    pub fn expire(&mut self, now: i64) -> Vec<Dropped> {
        let dir = self.dir.clone();
        let me = self.me.clone();
        let busy = |h: &HeldRequest| {
            h.by.as_deref()
                .is_some_and(|by| by == me || dir.as_deref().is_some_and(|d| alive(d, by)))
        };
        let old: Vec<HeldRequest> = match self.dir {
            None => {
                let old: Vec<HeldRequest> = self
                    .held
                    .iter()
                    .filter(|h| h.expired(now) && !busy(h))
                    .cloned()
                    .collect();
                self.held.retain(|h| !old.iter().any(|o| o.op() == h.op()));
                old
            }
            Some(_) => {
                let taken = self.update(|f| {
                    let (old, keep): (Vec<HeldRequest>, Vec<HeldRequest>) =
                        std::mem::take(&mut f.held)
                            .into_iter()
                            .partition(|h| h.expired(now) && !busy(h));
                    f.held = keep;
                    old
                });
                let old = taken.unwrap_or_default();
                self.held.retain(|h| !old.iter().any(|o| o.op() == h.op()));
                old
            }
        };
        // Why it was never sent: its machine didn't answer, or it did, with a backend Kerdr won't
        // send it to unchecked.
        let dropped: Vec<Dropped> = old
            .iter()
            .map(|h| Dropped {
                key: h.key.clone(),
                reference: h.reference.clone(),
                said: match &h.stays {
                    Some(why) => format!("{} dropped after a day: {why}", h.what),
                    None => format!("{} dropped: {} didn't answer for a day", h.what, h.machine),
                },
            })
            .collect();
        self.dropped.extend(dropped.clone());
        dropped
    }

    /// Keeps a change for later. One already kept, by its op, isn't kept twice. Whether it's in
    /// the file: if not, it's kept in memory, and written with the next write that can be.
    pub fn hold(&mut self, entry: HeldRequest) -> bool {
        if entry.op().is_empty() || self.held.iter().any(|h| h.op() == entry.op()) {
            return true;
        }
        self.held.push(entry.clone());
        self.write(Change::Hold(entry))
    }

    /// Takes a change out, sent and answered or dropped: the one it was, by its op.
    pub fn release(&mut self, op: &str) -> Option<HeldRequest> {
        let i = self.held.iter().position(|h| h.op() == op);
        self.write(Change::Release(op.to_string()));
        i.map(|i| self.held.remove(i))
    }

    /// Before sending a held change: it's this Kerdr's to send unless another Kerdr open now has
    /// handed it on, or it's gone from the file (answered, or dropped). Marked as handed on by
    /// this one, so no other sends it too.
    pub fn claim(&mut self, op: &str) -> Claim {
        if self.dir.is_none() {
            return match self.held.iter().any(|h| h.op() == op) {
                true => Claim::Mine,
                false => Claim::Gone,
            };
        }
        let dir = self.dir.clone().unwrap_or_default();
        let me = self.me.clone();
        let found = self.update(|f| match f.held.iter_mut().find(|h| h.op() == op) {
            None => Claim::Gone,
            Some(h) => match h.by.as_deref() {
                Some(by) if by != me && alive(&dir, by) => Claim::Taken,
                _ => {
                    h.by = Some(me.clone());
                    Claim::Mine
                }
            },
        });
        let claim = match found {
            Some(claim) => claim,
            // The file can't be read or written: only one this Kerdr held that never reached it
            // is its own, and goes into it as being sent by this one. Any other may be another
            // Kerdr's to send by now, so nothing is decided (Codex's review 62, 3).
            None => match self.unwritten.iter_mut().find_map(|c| match c {
                Change::Hold(e) if e.op() == op => Some(e),
                _ => None,
            }) {
                Some(e) => {
                    e.by = Some(me.clone());
                    Claim::Mine
                }
                None => Claim::Unsure,
            },
        };
        match claim {
            Claim::Mine => {
                for h in self.held.iter_mut().filter(|h| h.op() == op) {
                    h.by = Some(me.clone());
                }
            }
            Claim::Gone => self.held.retain(|h| h.op() != op),
            Claim::Taken | Claim::Unsure => {}
        }
        claim
    }

    /// m's D on a change waiting in the file: taken out (Mine), unless another Kerdr open now has
    /// handed it on (Taken), or it's gone already (Gone). Checked and taken out under the lock, so
    /// a Kerdr sending it meanwhile isn't told it was dropped (Codex's review 60).
    pub fn drop_waiting(&mut self, op: &str) -> Claim {
        let Some(dir) = self.dir.clone() else {
            return match self.release(op) {
                Some(_) => Claim::Mine,
                None => Claim::Gone,
            };
        };
        let me = self.me.clone();
        let found = self.update(|f| match f.held.iter().position(|h| h.op() == op) {
            None => Claim::Gone,
            Some(i) => match f.held[i].by.as_deref() {
                Some(by) if by != me && alive(&dir, by) => Claim::Taken,
                _ => {
                    f.held.remove(i);
                    Claim::Mine
                }
            },
        });
        let claim = match found {
            Some(claim) => claim,
            // The file can't be read or written: only one that never reached it is dropped now;
            // any other may be on its way from another Kerdr (Codex's review 62, 3).
            None => {
                let local = |c: &Change| matches!(c, Change::Hold(e) if e.op() == op);
                if self.unwritten.iter().any(local) {
                    self.unwritten.retain(|c| !local(c));
                    Claim::Mine
                } else {
                    Claim::Unsure
                }
            }
        };
        if matches!(claim, Claim::Mine | Claim::Gone) {
            self.held.retain(|h| h.op() != op);
        }
        claim
    }

    /// A change this Kerdr handed on that waits in the file again: its machine couldn't be
    /// reached after all, or Kerdr quits before handing it over whole.
    pub fn unclaim(&mut self, op: &str) {
        let me = self.me.clone();
        for h in self.held.iter_mut().filter(|h| h.op() == op) {
            if h.by.as_deref() == Some(me.as_str()) {
                h.by = None;
            }
        }
        self.write(Change::Unclaim(op.to_string(), me));
    }

    /// Notes why a held change stays in the file though its machine answers, or, with None, that
    /// it's on its way after all. Written only when that changes.
    pub fn stays(&mut self, op: &str, why: Option<String>) {
        let mut changed = false;
        for h in self.held.iter_mut().filter(|h| h.op() == op) {
            changed |= h.stays != why;
            h.stays = why.clone();
        }
        if changed {
            self.write(Change::Stays(op.to_string(), why));
        }
    }

    /// A held change whose card, a refusal said, lives on another machine: it's for that one now,
    /// going as `request`, still the same record with its op.
    pub fn retarget(&mut self, op: &str, machine: &str, request: Value) {
        let mut changed = false;
        for h in self.held.iter_mut().filter(|h| h.op() == op) {
            changed |= h.machine != machine || h.request != request;
            h.machine = machine.to_string();
            h.request = request.clone();
        }
        if changed {
            self.write(Change::Moved(op.to_string(), machine.to_string(), request));
        }
    }

    /// Whether another Kerdr open now has handed this one on.
    pub fn taken_elsewhere(&self, h: &HeldRequest) -> bool {
        match (&self.dir, h.by.as_deref()) {
            (Some(dir), Some(by)) => by != self.me && alive(dir, by),
            _ => false,
        }
    }

    pub fn find(&self, op: &str) -> Option<&HeldRequest> {
        self.held.iter().find(|h| h.op() == op)
    }

    /// What's held for this card, by its key.
    pub fn for_card(&self, key: &str) -> Vec<&HeldRequest> {
        self.held
            .iter()
            .filter(|h| h.key.as_deref() == Some(key))
            .collect()
    }

    /// Notes whether a machine's backend holds requests, as its feed last said.
    pub fn note_holds(&mut self, machine: &str, yes: bool) {
        if self.holds.iter().any(|m| m == machine) == yes {
            return;
        }
        let name = machine.to_string();
        if yes {
            self.holds.push(name.clone());
        } else {
            self.holds.retain(|m| *m != name);
        }
        self.write(Change::Holds(name, yes));
    }

    pub fn holds_for(&self, machine: &str) -> bool {
        self.holds.iter().any(|m| m == machine)
    }

    /// Writes a change: with any not yet written, into the file as it is now. Whether it's in.
    fn write(&mut self, change: Change) -> bool {
        if self.dir.is_none() {
            return true;
        }
        self.unwritten.push(change);
        self.update(|_| ()).is_some()
    }

    /// Holds the lock, reads the hold file, lays the changes not yet written over it, then `change`,
    /// and writes it whole. None, said, when it can't: the changes wait for the next write.
    fn update<T>(&mut self, change: impl FnOnce(&mut HeldFile) -> T) -> Option<T> {
        let dir = self.dir.clone()?;
        let unwritten = self.unwritten.clone();
        let mut notes = vec![];
        let result = locked(&dir, || {
            let mut file: HeldFile = read_or_set_aside(&dir, HELD_FILE, &mut notes)?;
            let was = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
            for c in &unwritten {
                c.apply(&mut file);
            }
            let out = change(&mut file);
            let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
            // Written only when it changed: a read alone leaves the file as it is.
            if text != was {
                write_private(&dir, HELD_FILE, &text)?;
            }
            Ok(out)
        });
        self.notes.extend(notes);
        match result {
            Ok(out) => {
                self.unwritten.clear();
                self.held_failed = None;
                Some(out)
            }
            Err(e) => {
                self.held_failed = Some(e.to_string());
                None
            }
        }
    }

    /// The words the Kerdrs before kept, taken by this one: every set no Kerdr open now has
    /// taken. They stay in the file, marked as this Kerdr's, until it keeps its own words in their
    /// place: a crash meanwhile leaves them for the next Kerdr.
    pub fn take_words(&mut self) -> Words {
        let Some(dir) = self.dir.clone() else {
            return Words::default();
        };
        let me = self.me.clone();
        let mut notes = vec![];
        let mut taken = Words::default();
        let result = locked(&dir, || {
            let mut file: WordsFile = read_or_set_aside(&dir, WORDS_FILE, &mut notes)?;
            file.settle();
            for set in file.sets.iter_mut() {
                let free = set
                    .by
                    .as_deref()
                    .is_none_or(|by| by != me && !alive(&dir, by));
                if free {
                    set.by = Some(me.clone());
                    set.left_by = None;
                    taken.merge(std::mem::take(&mut set.words));
                }
            }
            file.sets
                .retain(|s| s.by.as_deref() != Some(me.as_str()) || !s.words.is_empty());
            if !taken.is_empty() {
                file.sets.push(WordSet {
                    by: Some(me.clone()),
                    left_by: None,
                    words: taken.clone(),
                });
            }
            write_words(&dir, &file)
        });
        self.notes.extend(notes);
        if let Err(e) = result {
            self.words_failed = Some(e.to_string());
        }
        taken
    }

    /// Keeps this Kerdr's words for the next one, in place of what it took, adding to what other
    /// Kerdrs keep there. The error, in words, when it can't: nothing in the file changed.
    pub fn keep_words(&mut self, words: &Words) -> Result<(), String> {
        self.put_words(words, true)
    }

    /// Keeps this Kerdr's words in the file as they are now, while it's open, in place of what it
    /// kept or took before: as its own, so no other Kerdr takes them meanwhile, and the next one
    /// does once this one has gone, however it ended (Codex's review 62, 2).
    pub fn hold_words(&mut self, words: &Words) -> Result<(), String> {
        self.put_words(words, false)
    }

    /// Whether what Kerdr keeps goes to files at all (not in the demo, or a test's memory).
    pub fn on_disk(&self) -> bool {
        self.dir.is_some()
    }

    fn put_words(&mut self, words: &Words, leaving: bool) -> Result<(), String> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let me = self.me.clone();
        let mut notes = vec![];
        let result = locked(&dir, || {
            let mut file: WordsFile = read_or_set_aside(&dir, WORDS_FILE, &mut notes)?;
            file.settle();
            let mine = |by: &Option<String>| by.as_deref() == Some(me.as_str());
            file.sets.retain(|s| !mine(&s.by) && !mine(&s.left_by));
            if !words.is_empty() {
                file.sets.push(WordSet {
                    by: (!leaving).then(|| me.clone()),
                    left_by: leaving.then(|| me.clone()),
                    words: words.clone(),
                });
            }
            write_words(&dir, &file)
        });
        self.notes.extend(notes);
        match result {
            Ok(()) => {
                self.words_failed = None;
                Ok(())
            }
            Err(e) => {
                let e = format!(
                    "Kerdr couldn't write {} ({e})",
                    dir.join(WORDS_FILE).display()
                );
                self.words_failed = Some(e.clone());
                Err(e)
            }
        }
    }
}

/// Whether the Kerdr of this name is still open: its lock file is held. One that has gone leaves
/// its lock file free, which is then taken away.
fn alive(dir: &Path, name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return false;
    }
    let path = dir.join(format!("kerdr-{name}.lock"));
    let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => true,
    }
}

/// Runs `f` holding the state folder's lock: one Kerdr at a time reads, changes and writes.
fn locked<T>(dir: &Path, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    private_folder(dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    lock.lock()?;
    let out = f();
    let _ = lock.unlock();
    out
}

/// A file as it is: empty when there's none. One that can't be read as it should be is moved
/// aside, never written over, and that's noted to be said; one that can't be opened at all is an
/// error, so nothing is written over it.
fn read_or_set_aside<T: DeserializeOwned + Default>(
    dir: &Path,
    name: &str,
    notes: &mut Vec<String>,
) -> io::Result<T> {
    let path = dir.join(name);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(e),
    };
    if let Ok(found) = serde_json::from_str(&text) {
        return Ok(found);
    }
    let stem = name.trim_end_matches(".json");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let aside = dir.join(format!("{stem}-unreadable-{stamp}.json"));
    std::fs::rename(&path, &aside)?;
    notes.push(format!(
        "Kerdr couldn't read {}, so it moved it to {} and started a new one.",
        path.display(),
        aside.display()
    ));
    Ok(T::default())
}

/// An image kept with words, in base64.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptImage {
    pub tag: u32,
    pub data: String,
}

impl KeptImage {
    pub fn of(image: &crate::images::Image) -> KeptImage {
        use base64::Engine;
        KeptImage {
            tag: image.tag,
            data: base64::engine::general_purpose::STANDARD.encode(image.bytes.as_slice()),
        }
    }

    /// The image again; None for one that isn't an image Kerdr takes.
    pub fn image(&self) -> Option<crate::images::Image> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .ok()?;
        let media_type = crate::images::media_type(&bytes)?;
        Some(crate::images::Image {
            tag: self.tag,
            media_type,
            bytes: std::sync::Arc::new(bytes),
        })
    }
}

/// The send words came back from (crate::app::Submission), kept with them: sent again unchanged,
/// they go as that send, so their backend finds what it may have made rather than making a second
/// intent (Codex's review 60).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptSubmission {
    pub id: String,
    /// The machine it went to, by name, and its session there.
    pub machine: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// An earlier send of it got no answer, so it may have been made.
    #[serde(default)]
    pub unknown: bool,
}

/// Words for a new intent, and their images.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptWords {
    pub words: String,
    #[serde(default)]
    pub images: Vec<KeptImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<KeptSubmission>,
}

/// Your version of an item's text, not yet saved.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptEdit {
    /// Its card, by key, and its record's number.
    pub key: String,
    pub id: String,
    /// The machine its save goes to, by name.
    pub machine: String,
    /// The item's text as its backend last gave it, and its fingerprint.
    pub current: String,
    pub was: String,
    /// Your version, and the text you wrote it from.
    pub text: String,
    pub from: String,
}

/// A new intent its backend accepted and is still saving, with no number yet (intent 0591,
/// release 4): its `create` request as sent, op, submission, words and images included, so a later
/// Kerdr can still drop it and give the words back (Codex's review 60).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptSaving {
    pub machine: String,
    pub request: Value,
    /// What its backend said of it, for its card: "saving: its number comes once the central
    /// copy answers".
    #[serde(default)]
    pub pending: String,
}

/// Your words, kept in `words.json` so quitting never drops them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Words {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<KeptWords>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub returned: Vec<KeptWords>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<KeptEdit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saving: Vec<KeptSaving>,
}

impl Words {
    pub fn is_empty(&self) -> bool {
        self.draft.is_none()
            && self.returned.is_empty()
            && self.edits.is_empty()
            && self.saving.is_empty()
    }

    /// Adds another set's words: a second draft comes back after this one's, an edit of the same
    /// card is the later one, and a save is listed once.
    fn merge(&mut self, other: Words) {
        match (&self.draft, other.draft) {
            (_, None) => {}
            (None, Some(d)) => self.draft = Some(d),
            (Some(_), Some(d)) => {
                if !self.returned.contains(&d) {
                    self.returned.push(d);
                }
            }
        }
        for r in other.returned {
            if !self.returned.contains(&r) {
                self.returned.push(r);
            }
        }
        for e in other.edits {
            self.edits.retain(|o| o.key != e.key);
            self.edits.push(e);
        }
        for s in other.saving {
            if !self
                .saving
                .iter()
                .any(|o| o.request["op"] == s.request["op"])
            {
                self.saving.push(s);
            }
        }
    }
}

/// The words file: each Kerdr's set of words, with the Kerdr that took it (none for words waiting
/// for the next Kerdr to open).
#[derive(Default, Deserialize, Serialize)]
struct WordsFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sets: Vec<WordSet>,
    /// One set at the top, as an earlier Kerdr of this release kept it: no Kerdr's.
    #[serde(flatten)]
    loose: Words,
}

impl WordsFile {
    /// The words at the top as a set of their own, waiting for a Kerdr.
    fn settle(&mut self) {
        let loose = std::mem::take(&mut self.loose);
        if !loose.is_empty() {
            self.sets.push(WordSet {
                by: None,
                left_by: None,
                words: loose,
            });
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WordSet {
    /// The Kerdr that took these words, while it's open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<String>,
    /// The Kerdr that left them for the next one: a second keep of its words replaces them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    left_by: Option<String>,
    #[serde(flatten)]
    words: Words,
}

/// Writes the words file, or takes it away when no words are left in it.
fn write_words(dir: &Path, file: &WordsFile) -> io::Result<()> {
    if file.sets.iter().all(|s| s.words.is_empty()) {
        return match std::fs::remove_file(dir.join(WORDS_FILE)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
    write_private(dir, WORDS_FILE, &text)
}

/// Writes `name` in `dir` whole, readable by its owner alone: a new file, moved into place.
fn write_private(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    private_folder(dir)?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .tempfile_in(dir)?;
    io::Write::write_all(&mut file, text.as_bytes())?;
    file.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.persist(dir.join(name)).map_err(|e| e.error)?;
    Ok(())
}

fn private_folder(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if !dir.is_dir() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(op: &str, pressed: i64) -> HeldRequest {
        HeldRequest {
            machine: "LAB-Server".into(),
            request: json!({"action":"dispatch","id":"0582","op":op}),
            rev: Some("r1".into()),
            key: Some("ark-sdlc:0582".into()),
            reference: "#582".into(),
            what: "d".into(),
            pressed,
            after: None,
            by: None,
            stays: None,
        }
    }

    fn ops(dir: &Path) -> Vec<String> {
        let file: HeldFile =
            serde_json::from_str(&std::fs::read_to_string(dir.join(HELD_FILE)).unwrap()).unwrap();
        file.held.iter().map(|h| h.op().to_string()).collect()
    }

    #[test]
    fn a_held_change_survives_quitting_and_goes_once_answered() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("kerdr");
        let mut kept = Kept::at(&state, 1000);
        assert!(kept.hold(entry("op-1", 1000)));
        // The same op again isn't kept twice.
        kept.hold(entry("op-1", 1000));
        kept.note_holds("LAB-Server", true);
        drop(kept);
        // The next Kerdr finds it, and the machine's backend noted as one that holds.
        let mut next = Kept::at(&state, 2000);
        assert_eq!(next.held, vec![entry("op-1", 1000)]);
        assert!(next.holds_for("LAB-Server"));
        assert!(!next.holds_for("arknet-one"));
        assert!(next.dropped.is_empty());
        // Answered: gone from the file too.
        assert!(next.release("op-1").is_some());
        assert!(Kept::at(&state, 2000).held.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state.join("held.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(&state).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn two_kerdrs_each_keep_what_the_other_held() {
        let dir = tempfile::tempdir().unwrap();
        let mut one = Kept::at(dir.path(), 1000);
        let mut two = Kept::at(dir.path(), 1000);
        one.hold(entry("op-1", 1000));
        two.hold(entry("op-2", 1000));
        assert_eq!(ops(dir.path()), ["op-1", "op-2"]);
        one.release("op-1");
        assert_eq!(ops(dir.path()), ["op-2"]);
    }

    #[test]
    fn two_kerdrs_writing_at_once_lose_nothing() {
        // Codex's review 60, 1: each read, change and write holds the folder's lock.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        std::thread::scope(|s| {
            for who in ["a", "b", "c"] {
                let path = path.clone();
                s.spawn(move || {
                    let mut kept = Kept::at(&path, 1000);
                    for i in 0..40 {
                        let _ = kept.hold(entry(&format!("{who}-{i}"), 1000));
                    }
                });
            }
        });
        assert_eq!(ops(dir.path()).len(), 120);
    }

    #[test]
    fn a_change_another_kerdr_dropped_or_is_sending_isnt_sent_here() {
        // Codex's review 60, 1: before sending, a Kerdr checks the file, not only its own list.
        let dir = tempfile::tempdir().unwrap();
        let mut one = Kept::at(dir.path(), 1000);
        let mut two = Kept::at(dir.path(), 1000);
        one.hold(entry("op-1", 1000));
        one.hold(entry("op-2", 1000));
        two.sync();
        assert_eq!(two.held.len(), 2);
        // One drops op-1: the other doesn't send it, and stops showing it.
        one.release("op-1");
        assert_eq!(two.claim("op-1"), Claim::Gone);
        assert!(two.find("op-1").is_none());
        // Two hands op-2 on: one, still open, doesn't send it too.
        assert_eq!(two.claim("op-2"), Claim::Mine);
        assert_eq!(one.claim("op-2"), Claim::Taken);
        one.sync();
        assert!(one.taken_elsewhere(one.find("op-2").unwrap()));
        // Two ends without its answer (a crash, say): op-2 is one's to send again.
        drop(two);
        assert_eq!(one.claim("op-2"), Claim::Mine);
        assert_eq!(one.find("op-2").unwrap().by.as_deref(), Some(one.me()));
        // Back in the file for its machine to answer: no one's.
        one.unclaim("op-2");
        assert_eq!(Kept::at(dir.path(), 1000).held[0].by, None);
    }

    #[test]
    fn a_change_kerdr_couldnt_write_is_said_and_goes_with_the_next_write() {
        // Codex's review 60, 9: nothing is said to be held that isn't, and nothing is lost.
        let dir = tempfile::tempdir().unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        // Something in the way of the file (a folder where it goes): it can't be written.
        std::fs::create_dir(dir.path().join(HELD_FILE)).unwrap();
        assert!(!kept.hold(entry("op-1", 1000)));
        let why = kept.unsaved().expect("said");
        assert!(why.starts_with("Kerdr couldn't write "), "{why}");
        assert!(why.contains("held.json"), "{why}");
        assert_eq!(kept.held.len(), 1, "kept in memory meanwhile");
        // Out of the way: the next write takes both.
        std::fs::remove_dir(dir.path().join(HELD_FILE)).unwrap();
        assert!(kept.hold(entry("op-2", 1000)));
        assert_eq!(kept.unsaved(), None);
        assert_eq!(ops(dir.path()), ["op-1", "op-2"]);
        // The words file likewise: its error comes back, and nothing in it changed.
        std::fs::create_dir(dir.path().join(WORDS_FILE)).unwrap();
        let words = Words {
            draft: Some(KeptWords {
                words: "Fix it".into(),
                ..KeptWords::default()
            }),
            ..Words::default()
        };
        let e = kept.keep_words(&words).unwrap_err();
        assert!(e.contains("words.json"), "{e}");
        assert!(kept.unsaved().is_some());
    }

    #[test]
    fn a_claim_or_drop_the_hold_file_cant_record_decides_nothing() {
        // Codex's review 62, 3: two Kerdrs with the same change and a hold file neither can
        // write. Neither sends it, neither says it was dropped, and nothing written late undoes a
        // later claim. One held meanwhile, which never reached the file, is this Kerdr's alone.
        let dir = tempfile::tempdir().unwrap();
        let held = dir.path().join(HELD_FILE);
        let away = dir.path().join("held-away.json");
        let mut one = Kept::at(dir.path(), 1000);
        let mut two = Kept::at(dir.path(), 1000);
        one.hold(entry("op-1", 1000));
        two.sync();
        std::fs::rename(&held, &away).unwrap();
        std::fs::create_dir(&held).unwrap();
        assert_ne!(one.claim("op-1"), Claim::Mine);
        assert_ne!(two.claim("op-1"), Claim::Mine);
        assert_ne!(one.drop_waiting("op-1"), Claim::Mine);
        assert!(one.find("op-1").is_some(), "said dropped though it isn't");
        assert!(!one.hold(entry("op-2", 1000)));
        assert_eq!(one.claim("op-2"), Claim::Mine);
        // The file back: two claims op-1, and what one writes after never undoes that.
        std::fs::remove_dir(&held).unwrap();
        std::fs::rename(&away, &held).unwrap();
        assert_eq!(two.claim("op-1"), Claim::Mine);
        assert_eq!(one.flush(), None);
        assert_eq!(one.claim("op-1"), Claim::Taken);
        let file: HeldFile =
            serde_json::from_str(&std::fs::read_to_string(&held).unwrap()).unwrap();
        let by = |op: &str| {
            file.held
                .iter()
                .find(|h| h.op() == op)
                .and_then(|h| h.by.clone())
        };
        assert_eq!(by("op-1").as_deref(), Some(two.me()));
        assert_eq!(
            by("op-2").as_deref(),
            Some(one.me()),
            "written as on its way from the Kerdr sending it"
        );
    }

    #[test]
    fn a_change_another_kerdr_claimed_a_moment_ago_isnt_dropped_as_a_day_old() {
        // Codex's review 62, 8: who has it is checked in the file, under the lock, as it's
        // dropped. Here another Kerdr claims it after this one last read the file.
        let dir = tempfile::tempdir().unwrap();
        let mut one = Kept::at(dir.path(), 1000);
        one.hold(entry("op-1", 1000));
        let mut two = Kept::at(dir.path(), 1000);
        assert_eq!(two.claim("op-1"), Claim::Mine);
        assert!(one.expire(1001 + HOLD_MOST_SECS).is_empty());
        assert!(one.dropped.is_empty());
        assert_eq!(ops(dir.path()), ["op-1"]);
        // Once that Kerdr has gone, its day counts.
        drop(two);
        assert_eq!(one.expire(1001 + HOLD_MOST_SECS).len(), 1);
        assert!(ops(dir.path()).is_empty());
    }

    #[test]
    fn why_a_change_stays_held_is_kept_with_it_for_the_next_kerdr_until_it_goes() {
        let dir = tempfile::tempdir().unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        kept.hold(entry("unchecked", 1000));
        kept.hold(entry("sent-since", 1000));
        let why = "build-box's backend doesn't check a held change again";
        kept.stays("unchecked", Some(why.into()));
        kept.stays("sent-since", Some(why.into()));
        kept.stays("sent-since", None);
        let next = Kept::at(dir.path(), 1001 + HOLD_MOST_SECS);
        let said: Vec<&str> = next.dropped.iter().map(|d| d.said.as_str()).collect();
        assert_eq!(
            said,
            [
                format!("d dropped after a day: {why}").as_str(),
                "d dropped: LAB-Server didn't answer for a day",
            ]
        );
    }

    #[test]
    fn a_change_held_over_a_day_is_dropped_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        kept.hold(entry("old", 1000));
        kept.hold(entry("new", 1000 + HOLD_MOST_SECS));
        // One a Kerdr open now has handed on isn't dropped under it.
        kept.hold(entry("sending", 1000));
        assert_eq!(kept.claim("sending"), Claim::Mine);
        let next = Kept::at(dir.path(), 1001 + HOLD_MOST_SECS);
        let ops: Vec<&str> = next.held.iter().map(|h| h.op()).collect();
        assert_eq!(ops, ["new", "sending"]);
        assert_eq!(
            next.dropped,
            vec![Dropped {
                key: Some("ark-sdlc:0582".into()),
                reference: "#582".into(),
                said: "d dropped: LAB-Server didn't answer for a day".into(),
            }]
        );
        // Gone from the file: the Kerdr after doesn't say it again.
        drop(kept);
        let after = Kept::at(dir.path(), 1001 + HOLD_MOST_SECS);
        assert_eq!(after.dropped.len(), 1, "the one whose Kerdr has gone");
        assert!(after.held.iter().all(|h| h.op() == "new"));
    }

    #[test]
    fn a_broken_hold_file_is_moved_aside_and_a_new_one_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("held.json"), "{not json").unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        assert!(kept.held.is_empty());
        assert_eq!(kept.notes.len(), 1, "{:?}", kept.notes);
        assert!(kept.notes[0].starts_with("Kerdr couldn't read "));
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("held-unreadable-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            std::fs::read_to_string(aside[0].path()).unwrap(),
            "{not json"
        );
        kept.hold(entry("op-1", 1000));
        assert_eq!(Kept::at(dir.path(), 1000).held.len(), 1);
    }

    #[test]
    fn in_memory_it_writes_nothing() {
        let mut kept = Kept::in_memory();
        kept.hold(entry("op-1", 1000));
        assert_eq!(kept.held.len(), 1);
        assert_eq!(kept.for_card("ark-sdlc:0582").len(), 1);
        assert!(kept.find("op-1").is_some());
        assert_eq!(kept.claim("op-1"), Claim::Mine);
        assert_eq!(kept.claim("op-9"), Claim::Gone);
        assert!(kept.take_words().is_empty());
        assert_eq!(kept.keep_words(&Words::default()), Ok(()));
    }

    fn some_words() -> Words {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let image = crate::images::Image {
            tag: 1,
            media_type: "image/png",
            bytes: std::sync::Arc::new(png),
        };
        Words {
            draft: Some(KeptWords {
                words: "Fix the [Image #1] banner".into(),
                images: vec![KeptImage::of(&image)],
                from: None,
            }),
            returned: vec![KeptWords {
                words: "A save that failed".into(),
                images: vec![],
                from: Some(KeptSubmission {
                    id: "s-1".into(),
                    machine: "LAB-Server".into(),
                    session: "ark".into(),
                    unknown: true,
                    ..KeptSubmission::default()
                }),
            }],
            edits: vec![KeptEdit {
                key: "ark-sdlc:0041".into(),
                id: "0041".into(),
                machine: "LAB-Server".into(),
                current: "old".into(),
                was: "f1".into(),
                text: "mine".into(),
                from: "old".into(),
            }],
            saving: vec![KeptSaving {
                machine: "LAB-Server".into(),
                request: json!({"action":"create","words":"Saving","submission":"s-2","op":"k-2"}),
                pending: "saving…".into(),
            }],
        }
    }

    #[test]
    fn words_are_kept_once_and_given_back_once() {
        let dir = tempfile::tempdir().unwrap();
        let words = some_words();
        Kept::at(dir.path(), 1000).keep_words(&words).unwrap();
        // A second Kerdr's draft, kept meanwhile, comes back as given-back words.
        let other = Words {
            draft: Some(KeptWords {
                words: "Another".into(),
                ..KeptWords::default()
            }),
            ..Words::default()
        };
        Kept::at(dir.path(), 1000).keep_words(&other).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("words.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let mut next = Kept::at(dir.path(), 1000);
        let back = next.take_words();
        assert_eq!(back.draft, words.draft);
        assert_eq!(back.returned.len(), 2);
        assert_eq!(
            back.returned[0], words.returned[0],
            "with the send it came from"
        );
        assert_eq!(back.returned[1].words, "Another");
        assert_eq!(back.edits, words.edits);
        assert_eq!(back.saving, words.saving);
        let again = back.draft.clone().unwrap().images[0].image().unwrap();
        assert_eq!(again.media_type, "image/png");
        // Taken by a Kerdr still open: another gets nothing twice.
        assert!(Kept::at(dir.path(), 1000).take_words().is_empty());
        // It keeps nothing: no words are left, and no file.
        next.keep_words(&Words::default()).unwrap();
        assert!(!dir.path().join("words.json").exists());
    }

    #[test]
    fn words_a_kerdr_took_stay_on_disk_until_it_keeps_its_own() {
        // Codex's review 60, 10: a Kerdr that ends without keeping its words (a crash) leaves
        // the words it took for the next one.
        let dir = tempfile::tempdir().unwrap();
        Kept::at(dir.path(), 1000)
            .keep_words(&some_words())
            .unwrap();
        let mut crashed = Kept::at(dir.path(), 1000);
        assert_eq!(crashed.take_words(), some_words());
        assert!(dir.path().join("words.json").exists());
        drop(crashed);
        let mut next = Kept::at(dir.path(), 1000);
        assert_eq!(next.take_words(), some_words());
    }

    #[test]
    fn an_unreadable_words_file_is_moved_aside_and_never_deleted() {
        // Codex's review 60, 10.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words.json"), "{\"draft\": broken").unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        assert!(kept.take_words().is_empty());
        assert!(
            kept.notes.iter().any(|n| n.contains("words-unreadable-")),
            "{:?}",
            kept.notes
        );
        let kept_aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t == "{\"draft\": broken"));
        assert!(kept_aside);
    }
}
