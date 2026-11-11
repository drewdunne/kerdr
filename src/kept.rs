//! What Kerdr keeps for you between runs (intent 0591, release 4), in its state folder, each file
//! readable by you alone:
//!
//! - `held.json`, the hold file: changes you asked for while their machine wasn't answering, each
//!   with its op, the card's `rev` when you pressed the key, and when that was. The next Kerdr
//!   sends them once their machine answers; nothing sends them while no Kerdr is open, and one
//!   older than a day isn't sent at all: its card says it was dropped, and why.
//! - `words.json`: the new-intent box's words and images, words a save gave back, and your
//!   versions of item texts not yet saved, so q never has to drop them.
//!
//! Each change to a file reads it first and writes it whole, through a new file moved into place,
//! so two Kerdrs open at once each keep what the other added, and a crash never leaves half a file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a held change waits for its machine before it's dropped instead of sent.
pub const HOLD_MOST_SECS: i64 = 24 * 60 * 60;

const HELD_FILE: &str = "held.json";
const WORDS_FILE: &str = "words.json";

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
    /// A start that waits for its intent's write-up as well: "write-up".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
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

#[derive(Default, Deserialize, Serialize)]
struct HeldFile {
    #[serde(default)]
    held: Vec<HeldRequest>,
    /// The machines whose backend last listed "hold", so a change for one that hasn't answered
    /// since Kerdr opened can still be held for it.
    #[serde(default)]
    holds: Vec<String>,
}

/// The hold file, and what Kerdr holds in it.
#[derive(Debug, Default)]
pub struct Kept {
    /// The state folder it lives in; None keeps it in memory alone (the demo, and tests).
    dir: Option<PathBuf>,
    pub held: Vec<HeldRequest>,
    pub holds: Vec<String>,
    /// Dropped since Kerdr opened: over a day old, or dropped with m.
    pub dropped: Vec<Dropped>,
}

impl Kept {
    pub fn in_memory() -> Kept {
        Kept::default()
    }

    /// The hold file in `dir`. What's over a day old is dropped from it now, and listed in
    /// `dropped`, for its card to say so.
    pub fn at(dir: &Path, now: i64) -> Kept {
        let file = read_held(dir);
        let mut kept = Kept {
            dir: Some(dir.to_path_buf()),
            held: file.held,
            holds: file.holds,
            dropped: vec![],
        };
        kept.expire(now);
        kept
    }

    /// Drops what has waited over a day, from the file too.
    pub fn expire(&mut self, now: i64) -> Vec<Dropped> {
        let (old, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|h| h.expired(now));
        self.held = keep;
        let mut dropped = vec![];
        for h in old {
            self.write(|f| f.held.retain(|e| e.op() != h.op()));
            let said = format!("{} dropped: {} didn't answer for a day", h.what, h.machine);
            dropped.push(Dropped {
                key: h.key.clone(),
                reference: h.reference.clone(),
                said,
            });
        }
        self.dropped.extend(dropped.clone());
        dropped
    }

    /// Keeps a change for later. One already kept, by its op, isn't kept twice.
    pub fn hold(&mut self, entry: HeldRequest) {
        if entry.op().is_empty() || self.held.iter().any(|h| h.op() == entry.op()) {
            return;
        }
        let copy = entry.clone();
        self.write(|f| {
            if !f.held.iter().any(|h| h.op() == copy.op()) {
                f.held.push(copy);
            }
        });
        self.held.push(entry);
    }

    /// Takes a change out, sent and answered or dropped: the one it was, by its op.
    pub fn release(&mut self, op: &str) -> Option<HeldRequest> {
        let i = self.held.iter().position(|h| h.op() == op)?;
        self.write(|f| f.held.retain(|h| h.op() != op));
        Some(self.held.remove(i))
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
        self.write(|f| {
            f.holds.retain(|m| *m != name);
            if yes {
                f.holds.push(name.clone());
            }
        });
    }

    pub fn holds_for(&self, machine: &str) -> bool {
        self.holds.iter().any(|m| m == machine)
    }

    /// Reads the file, changes it, and writes it whole again.
    fn write(&self, change: impl FnOnce(&mut HeldFile)) {
        let Some(dir) = &self.dir else {
            return;
        };
        let mut file = read_held(dir);
        change(&mut file);
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let _ = write_private(dir, HELD_FILE, &text);
        }
    }
}

fn read_held(dir: &Path) -> HeldFile {
    std::fs::read_to_string(dir.join(HELD_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
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

/// Words for a new intent, and their images.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct KeptWords {
    pub words: String,
    #[serde(default)]
    pub images: Vec<KeptImage>,
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

/// Your words, kept in `words.json` so quitting never drops them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Words {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<KeptWords>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub returned: Vec<KeptWords>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<KeptEdit>,
}

impl Words {
    pub fn is_empty(&self) -> bool {
        self.draft.is_none() && self.returned.is_empty() && self.edits.is_empty()
    }

    /// Reads the words kept in `dir`, and takes them out: they're Kerdr's again, in memory, until
    /// it quits and keeps them once more.
    pub fn take(dir: &Path) -> Words {
        let path = dir.join(WORDS_FILE);
        let words = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        words
    }

    /// Keeps these words in `dir`, adding to what another Kerdr kept there meanwhile.
    pub fn keep(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join(WORDS_FILE);
        let mut all: Words = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        match (&all.draft, &self.draft) {
            (_, None) => {}
            (None, Some(d)) => all.draft = Some(d.clone()),
            // Another Kerdr's draft is there already: this one is given back next time, after it.
            (Some(_), Some(d)) => all.returned.push(d.clone()),
        }
        for r in &self.returned {
            if !all.returned.contains(r) {
                all.returned.push(r.clone());
            }
        }
        for e in &self.edits {
            all.edits.retain(|o| o.key != e.key);
            all.edits.push(e.clone());
        }
        if all.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&all).map_err(std::io::Error::other)?;
        write_private(dir, WORDS_FILE, &text)
    }
}

/// Writes `name` in `dir` whole, readable by its owner alone: a new file, moved into place.
fn write_private(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
    private_folder(dir)?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .tempfile_in(dir)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
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

fn private_folder(dir: &Path) -> std::io::Result<()> {
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
        }
    }

    #[test]
    fn a_held_change_survives_quitting_and_goes_once_answered() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("kerdr");
        let mut kept = Kept::at(&state, 1000);
        kept.hold(entry("op-1", 1000));
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
        let ops: Vec<String> = Kept::at(dir.path(), 1000)
            .held
            .iter()
            .map(|h| h.op().to_string())
            .collect();
        assert_eq!(ops, ["op-1", "op-2"]);
        one.release("op-1");
        let ops: Vec<String> = Kept::at(dir.path(), 1000)
            .held
            .iter()
            .map(|h| h.op().to_string())
            .collect();
        assert_eq!(ops, ["op-2"]);
    }

    #[test]
    fn a_change_held_over_a_day_is_dropped_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        kept.hold(entry("old", 1000));
        kept.hold(entry("new", 1000 + HOLD_MOST_SECS));
        let next = Kept::at(dir.path(), 1001 + HOLD_MOST_SECS);
        let ops: Vec<&str> = next.held.iter().map(|h| h.op()).collect();
        assert_eq!(ops, ["new"]);
        assert_eq!(
            next.dropped,
            vec![Dropped {
                key: Some("ark-sdlc:0582".into()),
                reference: "#582".into(),
                said: "d dropped: LAB-Server didn't answer for a day".into(),
            }]
        );
        // Gone from the file: the Kerdr after doesn't say it again.
        assert!(
            Kept::at(dir.path(), 1001 + HOLD_MOST_SECS)
                .dropped
                .is_empty()
        );
    }

    #[test]
    fn a_broken_hold_file_holds_nothing_and_is_written_whole_again() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("held.json"), "{not json").unwrap();
        let mut kept = Kept::at(dir.path(), 1000);
        assert!(kept.held.is_empty());
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
    }

    #[test]
    fn words_are_kept_once_and_given_back_once() {
        let dir = tempfile::tempdir().unwrap();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let image = crate::images::Image {
            tag: 1,
            media_type: "image/png",
            bytes: std::sync::Arc::new(png.clone()),
        };
        let words = Words {
            draft: Some(KeptWords {
                words: "Fix the [Image #1] banner".into(),
                images: vec![KeptImage::of(&image)],
            }),
            returned: vec![KeptWords {
                words: "A save that failed".into(),
                images: vec![],
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
        };
        words.keep(dir.path()).unwrap();
        // A second Kerdr's draft, kept meanwhile, comes back as given-back words.
        let other = Words {
            draft: Some(KeptWords {
                words: "Another".into(),
                images: vec![],
            }),
            ..Words::default()
        };
        other.keep(dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("words.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let back = Words::take(dir.path());
        assert_eq!(back.draft, words.draft);
        assert_eq!(back.returned.len(), 2);
        assert_eq!(back.returned[1].words, "Another");
        assert_eq!(back.edits, words.edits);
        let again = back.draft.unwrap().images[0].image().unwrap();
        assert_eq!(again.media_type, "image/png");
        assert_eq!(*again.bytes, png);
        // Taken: the next Kerdr gets nothing twice.
        assert!(Words::take(dir.path()).is_empty());
        // Nothing to keep leaves no file.
        Words::default().keep(dir.path()).unwrap();
        assert!(!dir.path().join("words.json").exists());
    }
}
