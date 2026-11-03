//! The error log: every error Kerdr shows, kept where it can be looked up after it has left the
//! screen. One JSON object a line in `errors.log` in Kerdr's state folder, readable only by its
//! owner, the older half in `errors.log.1`. What you typed never goes in, nor a token it can
//! recognise, and a log that can't be written changes nothing else: Kerdr carries on without it.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::config::state_dir;

/// A file this big starts again, its lines kept in `errors.log.1` until the next time.
const MOST_BYTES: u64 = 1024 * 1024;
const MOST_MESSAGE: usize = 8 * 1024;
const MOST_STDERR: usize = 16 * 1024;
/// A run of this many characters that's also in something you typed is taken for a quote of it.
const QUOTE_RUN: usize = 12;
/// Shorter words than this are too common to take for a quote of what you typed.
const SHORTEST_TYPED: usize = 4;
/// A run of letters and digits this long, with both in it, is taken for a token.
const TOKEN_RUN: usize = 32;
const TYPED: &str = "[what you typed]";
const SECRET: &str = "[redacted]";

/// What one line says. Anything it doesn't know is left out.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// Who said it: `action` (an answer from an action's command), `machine` (a machine or its
    /// feed stopped answering) or `kerdr` (Kerdr itself).
    pub source: &'static str,
    /// How it was shown: `fail`, `warn`, `board` for a machine's state on the board, or `quiet`
    /// for a failure Kerdr had no reason to put on screen.
    pub shown: &'static str,
    pub machine: Option<String>,
    pub card: Option<String>,
    pub action: Option<String>,
    /// What Kerdr said.
    pub message: String,
    /// The backend's answer whole, as a message on screen may be shortened.
    pub answer: Option<String>,
    /// What the backend printed on standard error.
    pub stderr: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ErrorLog {
    path: Option<PathBuf>,
}

impl ErrorLog {
    /// This machine's log. Tests write only to one they name.
    pub fn on_this_machine() -> ErrorLog {
        if cfg!(test) {
            return ErrorLog::off();
        }
        ErrorLog { path: Some(path()) }
    }

    pub fn at(path: PathBuf) -> ErrorLog {
        ErrorLog { path: Some(path) }
    }

    pub fn off() -> ErrorLog {
        ErrorLog { path: None }
    }

    /// Adds a line, taking out what `typed` holds and anything that looks like a token. A failure
    /// to write it is dropped: the log never stops Kerdr or changes what it shows.
    pub fn write(&self, entry: &Entry, typed: &[&str]) {
        if let Some(path) = &self.path {
            let _ = append(path, &line(entry, typed));
        }
    }
}

/// Where this machine's log is.
pub fn path() -> PathBuf {
    state_dir().join("errors.log")
}

/// One line of the log, ending in its newline.
fn line(entry: &Entry, typed: &[&str]) -> String {
    let clean = |text: &str, most: usize, keep_end: bool| -> String {
        let cut = if keep_end {
            last_part(text, most)
        } else {
            first_part(text, most)
        };
        scrub(cut, typed)
    };
    let mut fields = Map::new();
    fields.insert(
        "time".into(),
        json!(chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)),
    );
    fields.insert("kerdr".into(), json!(env!("CARGO_PKG_VERSION")));
    fields.insert("source".into(), json!(entry.source));
    fields.insert("shown".into(), json!(entry.shown));
    for (name, value) in [
        ("machine", &entry.machine),
        ("card", &entry.card),
        ("action", &entry.action),
    ] {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            fields.insert(name.into(), json!(clean(value, 256, false)));
        }
    }
    fields.insert(
        "message".into(),
        json!(clean(&entry.message, MOST_MESSAGE, false)),
    );
    if let Some(answer) = entry.answer.as_deref().filter(|a| !a.trim().is_empty())
        && answer != entry.message
    {
        fields.insert("answer".into(), json!(clean(answer, MOST_MESSAGE, false)));
    }
    if let Some(stderr) = entry.stderr.as_deref().filter(|s| !s.trim().is_empty()) {
        fields.insert(
            "stderr".into(),
            json!(clean(stderr.trim(), MOST_STDERR, true)),
        );
    }
    let mut text = Value::Object(fields).to_string();
    text.push('\n');
    text
}

/// Text up to `most` bytes, cut where a word ends so no token is left half there.
fn first_part(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text;
    }
    let mut end = most;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = &text[..end];
    cut.rfind(char::is_whitespace).map_or("", |at| &cut[..at])
}

/// The last `most` bytes, which is where a command says what finally went wrong, cut likewise.
fn last_part(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text;
    }
    let mut start = text.len() - most;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    cut.find(char::is_whitespace).map_or("", |at| &cut[at..])
}

/// Takes out what was typed and what looks like a token.
pub fn scrub(text: &str, typed: &[&str]) -> String {
    tokens_out(&typed_out(text, typed))
}

/// Takes out whatever is typed in `typed`, whole or quoted in part: a backend may give back only
/// a line or the start of it.
fn typed_out(text: &str, typed: &[&str]) -> String {
    let typed: Vec<&str> = typed
        .iter()
        .map(|t| t.trim())
        .filter(|t| t.chars().count() >= SHORTEST_TYPED)
        .collect();
    if typed.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    // Short ones can only be matched whole.
    for t in typed.iter().filter(|t| t.chars().count() < QUOTE_RUN) {
        out = out.replace(t, TYPED);
    }
    let long: Vec<&str> = typed
        .iter()
        .copied()
        .filter(|t| t.chars().count() >= QUOTE_RUN)
        .collect();
    if long.is_empty() {
        return out;
    }
    let chars: Vec<(usize, char)> = out.char_indices().collect();
    let at = |i: usize| chars.get(i).map_or(out.len(), |(byte, _)| *byte);
    let mut kept = String::with_capacity(out.len());
    let mut i = 0;
    while i < chars.len() {
        // How far from here matches something typed.
        let mut end = i + QUOTE_RUN;
        let run = |end: usize| long.iter().any(|t| t.contains(&out[at(i)..at(end)]));
        if end <= chars.len() && run(end) {
            while end < chars.len() && run(end + 1) {
                end += 1;
            }
            if !kept.ends_with(TYPED) {
                kept.push_str(TYPED);
            }
            i = end;
        } else {
            kept.push(chars[i].1);
            i += 1;
        }
    }
    kept
}

/// Whether a character can be part of a token's long run.
fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=')
}

const TOKEN_STARTS: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "gldt-",
    "glrt-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
    "eyJ",
];
const SECRET_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "api-key",
    "authorization",
    "credential",
];

/// Takes out what looks like a token: a word that starts as one does, the value of a name like
/// `token=`, what follows `Bearer`, and any very long run of letters and digits.
fn tokens_out(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_bearer = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let gap = &piece[word.len()..];
        if word.is_empty() {
            out.push_str(piece);
            continue;
        }
        if after_bearer {
            after_bearer = false;
            out.push_str(SECRET);
            out.push_str(gap);
            continue;
        }
        after_bearer = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("token:");
        out.push_str(&word_out(word));
        out.push_str(gap);
    }
    out
}

fn word_out(word: &str) -> String {
    // A value given a secret's name: everything after the first = or : goes.
    if let Some(at) = word.find(['=', ':']) {
        let name = word[..at].to_ascii_lowercase();
        let value = word[at + 1..].trim_matches(|c: char| !c.is_alphanumeric());
        if !value.is_empty() && SECRET_NAMES.iter().any(|s| name.contains(s)) {
            return format!("{}{SECRET}", &word[..=at]);
        }
    }
    // Quotes, brackets and the like may hold a token on either side.
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    while !rest.is_empty() {
        let start = rest
            .find(|c: char| c.is_alphanumeric())
            .unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .find(|c: char| {
                !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | '-' | '.'))
            })
            .unwrap_or(rest.len());
        let part = &rest[..end];
        if !part.is_empty() {
            out.push_str(if looks_like_token(part) { SECRET } else { part });
        } else if !rest.is_empty() {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        rest = &rest[end..];
    }
    out
}

fn looks_like_token(part: &str) -> bool {
    if TOKEN_STARTS
        .iter()
        .any(|s| part.starts_with(s) && part.len() >= s.len() + 8)
    {
        return true;
    }
    part.split(|c: char| !token_char(c)).any(|run| {
        run.len() >= TOKEN_RUN
            && run.chars().any(|c| c.is_ascii_digit())
            && run.chars().any(|c| c.is_ascii_alphabetic())
    })
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        private_folder(dir)?;
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.len() + line.len() as u64 > MOST_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        fs::rename(path, &old)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    file.write_all(line.as_bytes())
}

/// Makes the log's folder, closed to everyone but its owner.
fn private_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.is_dir() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        if fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

/// What a panic says, for the log: its place and first line. Rust's own panics quote the values
/// they tripped on between backticks or after `value:`, which could be anything, so those stay out.
pub fn panic_text(message: &str, place: &str) -> String {
    let first = message.lines().next().unwrap_or_default();
    let first = first.split(" value:").next().unwrap_or_default();
    let mut said = String::new();
    let mut quoted = false;
    for piece in first.split('`') {
        if !quoted {
            said.push_str(piece);
        }
        quoted = !quoted;
    }
    format!("Kerdr panicked at {place}: {}", said.trim())
}

/// Writes panics to the log, then does what happens to them anyway. A panic in a worker thread
/// otherwise leaves no trace on the board.
pub fn keep_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        ErrorLog::on_this_machine().write(
            &Entry {
                source: "kerdr",
                shown: "fail",
                message: panic_text(&message, &place),
                ..Default::default()
            },
            &[],
        );
        before(info);
    }));
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A log in a folder of its own, and how to read it back.
    pub fn log() -> (tempfile::TempDir, ErrorLog, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("errors.log");
        (dir, ErrorLog::at(path.clone()), path)
    }

    pub fn lines(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn entry(message: &str) -> Entry {
        Entry {
            source: "action",
            shown: "fail",
            message: message.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_line_says_when_where_what_and_the_backends_stderr() {
        let (_dir, log, path) = log();
        log.write(
            &Entry {
                machine: Some("LAB-Server".into()),
                card: Some("#500".into()),
                action: Some("dispatch".into()),
                answer: Some("the whole answer".into()),
                stderr: Some("  the push failed: rejected\n".into()),
                ..entry("#500 didn't start: the who…")
            },
            &[],
        );
        let got = lines(&path);
        assert_eq!(got.len(), 1);
        let l = &got[0];
        assert!(
            chrono::DateTime::parse_from_rfc3339(l["time"].as_str().unwrap()).is_ok(),
            "{l}"
        );
        assert_eq!(l["kerdr"], env!("CARGO_PKG_VERSION"));
        assert_eq!(l["source"], "action");
        assert_eq!(l["machine"], "LAB-Server");
        assert_eq!(l["card"], "#500");
        assert_eq!(l["action"], "dispatch");
        assert_eq!(l["message"], "#500 didn't start: the who…");
        assert_eq!(l["answer"], "the whole answer");
        assert_eq!(l["stderr"], "the push failed: rejected");
    }

    #[test]
    fn what_was_typed_stays_out_whole_or_quoted_in_part() {
        let words = "please rebuild the marketing site\nand tell nobody about the launch date";
        for (text, gone) in [
            (format!("refused: {words}"), "marketing"),
            (
                "refused: and tell nobody about the launch".to_string(),
                "nobody",
            ),
            ("saved 'please rebuild the mark…'".to_string(), "rebuild"),
        ] {
            let got = scrub(&text, &[words]);
            assert!(!got.contains(gone), "{got}");
            assert!(got.contains(TYPED), "{got}");
        }
        // A short answer goes when whole, and a word too small to be a quote stays.
        assert_eq!(
            scrub("the laptop said no", &["laptop"]),
            format!("the {TYPED} said no")
        );
        assert_eq!(scrub("no such card", &["no"]), "no such card");
        // What wasn't typed stays.
        assert_eq!(
            scrub("the push failed: rejected", &[words]),
            "the push failed: rejected"
        );
        // Letters beyond ASCII don't trip the cutting.
        assert_eq!(
            scrub(
                "é café: déjà vu ünïcödé tèxt here",
                &["déjà vu ünïcödé tèxt"]
            ),
            format!("é café: {TYPED} here")
        );
    }

    #[test]
    fn a_quoted_token_is_not_kept_in_the_clear() {
        let long = "aB3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF3hJ6";
        for (text, secret) in [
            ("git said glpat-abcDEF123456xyz", "abcDEF123456"),
            ("sk-ant-api03-Abc123Def456Ghi789 refused", "Abc123Def456"),
            ("\"ghp_abcdefghij0123456789\"", "abcdefghij"),
            ("Authorization: Bearer abc.def.ghi", "abc.def.ghi"),
            ("url?token=hunter2hunter2", "hunter2"),
            ("PASSWORD=swordfish", "swordfish"),
            ("token: notsecretlooking", "notsecretlooking"),
        ] {
            let got = scrub(text, &[]);
            assert!(!got.contains(secret), "{text} -> {got}");
            assert!(got.contains(SECRET), "{text} -> {got}");
        }
        let got = scrub(&format!("key was {long}."), &[]);
        assert!(!got.contains(long), "{got}");
        // Ordinary words and paths come through.
        for text in [
            "/home/someone/repos/ark-sdlc/intents/0500-kerdr-error-log/intent.md is there",
            "the push failed: ! [rejected] main -> main (fetch first)",
            "ssh: connect to host lab port 22: Connection timed out",
            "error: tokens are fine",
        ] {
            assert_eq!(scrub(text, &[]), text);
        }
    }

    #[cfg(unix)]
    #[test]
    fn only_its_owner_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, log, path) = log();
        log.write(&entry("one"), &[]);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        // One made open before is closed by the next line.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        log.write(&entry("two"), &[]);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(lines(&path).len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn it_writes_through_no_link() {
        let (dir, log, path) = log();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, "").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        log.write(&entry("one"), &[]);
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "");
    }

    #[test]
    fn it_never_grows_past_two_files_of_its_limit() {
        let (_dir, log, path) = log();
        let long = "word ".repeat(1500);
        for i in 0..1500 {
            log.write(&entry(&format!("{i} {long}")), &[]);
        }
        let old = path.with_extension("log.1");
        let size = |p: &Path| fs::metadata(p).unwrap().len();
        assert!(size(&path) <= MOST_BYTES, "{}", size(&path));
        assert!(size(&old) <= MOST_BYTES, "{}", size(&old));
        // The newest is in the newer file, and whole.
        let last = lines(&path).pop().unwrap();
        assert!(last["message"].as_str().unwrap().starts_with("1499 "));
    }

    #[test]
    fn a_long_message_or_stderr_is_cut_where_a_word_ends() {
        let (_dir, log, path) = log();
        log.write(
            &Entry {
                stderr: Some(format!("{}the real cause", "noise ".repeat(5000))),
                ..entry(&format!("the first thing {}", "more ".repeat(5000)))
            },
            &[],
        );
        let l = &lines(&path)[0];
        let message = l["message"].as_str().unwrap();
        let stderr = l["stderr"].as_str().unwrap();
        assert!(message.starts_with("the first thing"));
        assert!(message.len() <= MOST_MESSAGE);
        assert!(stderr.ends_with("the real cause"));
        assert!(stderr.len() <= MOST_STDERR);
        // A cut inside a letter of several bytes doesn't panic.
        assert!(!first_part(&"é".repeat(MOST_MESSAGE), MOST_MESSAGE).contains('\u{fffd}'));
        assert!(!last_part(&"é".repeat(MOST_STDERR), MOST_STDERR).contains('\u{fffd}'));
    }

    #[test]
    fn a_log_that_cant_be_written_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let in_the_way = dir.path().join("a file");
        fs::write(&in_the_way, "").unwrap();
        ErrorLog::at(in_the_way.join("errors.log")).write(&entry("one"), &[]);
        ErrorLog::at(PathBuf::from("/proc/no/such/errors.log")).write(&entry("two"), &[]);
        ErrorLog::off().write(&entry("three"), &[]);
    }

    #[test]
    fn a_panics_quoted_values_stay_out() {
        let said = panic_text(
            "byte index 3 is not a char boundary; it is inside 'é' of `my private words`[...]",
            "src/ui.rs:12",
        );
        assert!(said.contains("src/ui.rs:12"));
        assert!(said.contains("byte index 3"));
        assert!(!said.contains("private"), "{said}");
        let said = panic_text(
            "called `Result::unwrap()` on an `Err` value: \"my private words\"",
            "src/a.rs:1",
        );
        assert!(!said.contains("private"), "{said}");
        let said = panic_text("assertion failed\n left: my private words", "src/a.rs:1");
        assert!(!said.contains("private"), "{said}");
    }

    #[test]
    fn the_log_is_off_under_test_unless_named() {
        assert!(ErrorLog::on_this_machine().path.is_none());
    }
}
