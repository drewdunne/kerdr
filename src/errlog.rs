//! The error log: every error Kerdr shows, kept where it can be looked up after it has left the
//! screen. One JSON object a line in `errors.log` in Kerdr's state folder, readable only by its
//! owner, the older half in `errors.log.1`. What you typed never goes in, nor a token it can
//! recognise, and a log that can't be written changes nothing else: Kerdr carries on without it.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::config::state_dir;

/// A file this big starts again, its lines kept in `errors.log.1` until the next time.
const MOST_BYTES: u64 = 1024 * 1024;
const MOST_MESSAGE: usize = 8 * 1024;
pub const MOST_STDERR: usize = 16 * 1024;
/// A run of this many characters that's also in something you typed is taken for a quote of it.
const QUOTE_RUN: usize = 12;
/// Shorter text than this is too common to take for a quote of what you typed, and a shorter word
/// than this is left in an answer to the request it was typed into.
const SHORTEST_TYPED: usize = 4;
/// How few characters in a row from a credential count as quoting it.
const CREDENTIAL_RUN: usize = 8;
/// A run of letters and digits this long, with both in it, is taken for a token.
const TOKEN_RUN: usize = 32;
const TYPED: &str = "[typed]";
#[cfg(unix)]
const LOCK_TRIES: u32 = 5;
/// How far past its limit the log may go while its lock can't be had.
const UNLOCKED_SLACK: u64 = 256 * 1024;
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

    /// Adds a line, taking out what was typed and anything that looks like a token. `typed` is
    /// everything Kerdr holds that was typed, out wherever it's quoted; `sent` is what was typed
    /// into the request this answers, whose every word goes, as a backend may build a message or
    /// a path from them. A failure to write it is dropped: the log never stops Kerdr or changes
    /// what it shows.
    pub fn write(&self, entry: &Entry, typed: &[&str], sent: &[&str]) {
        if let Some(path) = &self.path {
            let _ = append(path, &line(entry, typed, sent));
        }
    }
}

/// Where this machine's log is.
pub fn path() -> PathBuf {
    state_dir().join("errors.log")
}

/// One line of the log, ending in its newline.
fn line(entry: &Entry, typed: &[&str], sent: &[&str]) -> String {
    let mut fields = Map::new();
    fields.insert(
        "time".into(),
        json!(chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)),
    );
    fields.insert("kerdr".into(), json!(env!("CARGO_PKG_VERSION")));
    fields.insert("source".into(), json!(entry.source));
    fields.insert("shown".into(), json!(entry.shown));
    // Kerdr's own names for things: a word you also typed stays in these.
    for (name, value) in [
        ("machine", &entry.machine),
        ("card", &entry.card),
        ("action", &entry.action),
    ] {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            fields.insert(name.into(), json!(tokens_out(first_part(value, 256))));
        }
    }
    let clean = |text: &str| scrub(first_part(text, MOST_MESSAGE), typed, sent);
    fields.insert("message".into(), json!(clean(&entry.message)));
    if let Some(answer) = entry.answer.as_deref().filter(|a| !a.trim().is_empty())
        && answer != entry.message
    {
        fields.insert("answer".into(), json!(clean(answer)));
    }
    if let Some(stderr) = entry.stderr.as_deref().filter(|s| !s.trim().is_empty()) {
        fields.insert(
            "stderr".into(),
            json!(scrub(last_part(stderr.trim(), MOST_STDERR), typed, sent)),
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
pub fn last_part(text: &str, most: usize) -> &str {
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

/// Takes out what was typed and what looks like a token: `typed` wherever it's quoted, every word
/// of `sent`.
pub fn scrub(text: &str, typed: &[&str], sent: &[&str]) -> String {
    // Tokens first: a secret's name you also typed still says what follows it is one.
    words_out(&typed_out(&tokens_out(text), typed), sent)
}

/// Takes out whatever is in `typed`, whole or quoted in part: a backend may give back only a line
/// or the start of it.
fn typed_out(text: &str, typed: &[&str]) -> String {
    let typed: Vec<&str> = typed
        .iter()
        .map(|t| t.trim())
        .filter(|t| t.chars().count() >= SHORTEST_TYPED)
        .collect();
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
    quotes_out(&out, &long, QUOTE_RUN, TYPED)
}

/// Replaces every run of at least `run` characters that's also in one of `from`.
fn quotes_out(text: &str, from: &[&str], run: usize, instead: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map_or(text.len(), |(byte, _)| *byte);
    let mut kept = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        // How far from here matches.
        let mut end = i + run;
        let matches = |end: usize| from.iter().any(|t| t.contains(&text[at(i)..at(end)]));
        if end <= chars.len() && matches(end) {
            while end < chars.len() && matches(end + 1) {
                end += 1;
            }
            if !kept.ends_with(instead) {
                kept.push_str(instead);
            }
            i = end;
        } else {
            kept.push(chars[i].1);
            i += 1;
        }
    }
    kept
}

/// Takes a credential out however much of it is quoted, for a message that's also shown.
pub fn credential_out(text: &str, credential: &str) -> String {
    quotes_out(text, &[credential.trim()], CREDENTIAL_RUN, SECRET)
}

/// Takes out every word of `sent`, in any case, however it was joined up: a backend may make a
/// path of them, as `intents/0099-make-boards-new-intent-box`. No match on the characters of a
/// quote could find a name said once on its own. Words too short to say much stay.
fn words_out(text: &str, sent: &[&str]) -> String {
    let words: HashSet<String> = sent
        .iter()
        .flat_map(|s| s.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| w.chars().count() >= SHORTEST_TYPED)
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let end_word = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            if words.contains(&word.to_lowercase()) {
                out.push_str(TYPED);
            } else {
                out.push_str(word);
            }
            word.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            end_word(&mut word, &mut out);
            out.push(c);
        }
    }
    end_word(&mut word, &mut out);
    out
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

/// Takes out what looks like a token: the value of a name like `token=` or `"password": `, what
/// follows `Bearer`, a word that starts as a token does, and any very long run of letters and digits.
fn tokens_out(text: &str) -> String {
    let text = named_values_out(text);
    let mut out = String::with_capacity(text.len());
    let mut after_bearer = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        let gap = &piece[word.len()..];
        if word.is_empty() {
            out.push_str(piece);
        } else if after_bearer {
            after_bearer = false;
            out.push_str(SECRET);
            out.push_str(gap);
        } else {
            after_bearer = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .eq_ignore_ascii_case("bearer");
            out.push_str(&word_out(word));
            out.push_str(gap);
        }
    }
    out
}

/// Whether a character can be part of a name, as in `api_key` or `x.token-id`.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|s| name.contains(s))
}

/// Takes the value from after each secret's name, however it's written: `token=x`, `password: x`,
/// `api_key = x`, `{"token":"x y"}`, `{\\"token\\": \\"x\\"}`.
fn named_values_out(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(name_char) {
        let name_len = rest[start..]
            .find(|c: char| !name_char(c))
            .unwrap_or(rest.len() - start);
        let end = start + name_len;
        let name = &rest[start..end];
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        if !secret_name(name) {
            continue;
        }
        // Past any closing quote and spaces, its separator, and spaces.
        let after_quote = rest.trim_start_matches(['\\', '"', '\'']);
        let after_gap = after_quote.trim_start();
        let Some(value) = after_gap.strip_prefix(['=', ':']) else {
            continue;
        };
        let value = value.trim_start();
        out.push_str(&rest[..rest.len() - value.len()]);
        let mut taken = value_len(value);
        // `Authorization: Bearer abc`: the word after Bearer is the one.
        if value[..taken].eq_ignore_ascii_case("bearer") {
            let next = value[taken..].trim_start();
            taken = value.len() - next.len() + value_len(next);
        }
        if taken == 0 {
            rest = value;
            continue;
        }
        out.push_str(SECRET);
        rest = &value[taken..];
    }
    out.push_str(rest);
    out
}

/// How much of `text` is the value it starts with: a quoted one to its closing quote, which a
/// backslash can hide, or one word up to the next punctuation that would end it.
fn value_len(text: &str) -> usize {
    // Quoted JSON inside a string: it ends at the next quote, which has its own backslash.
    if let Some(inner) = text.strip_prefix("\\\"") {
        return inner.find('"').map_or(text.len(), |at| 2 + at + 1);
    }
    let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return text
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '}' | ')'))
            .unwrap_or(text.len());
    };
    let open = quote.len_utf8();
    let mut escaped = false;
    for (at, c) in text[open..].char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return open + at + c.len_utf8();
        }
    }
    text.len()
}

fn word_out(word: &str) -> String {
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
    // Held until the line is written, so two writers can't both move the log and lose its history.
    // Taken only for a moment: without it the line still goes in unless the log is well over, so
    // a writer stuck holding it can't make the log grow without limit.
    let lock = rotation_lock(path);
    let size = fs::symlink_metadata(path).map_or(0, |m| m.len()) + line.len() as u64;
    if lock.is_some() {
        if size > MOST_BYTES {
            let mut old = path.as_os_str().to_owned();
            old.push(".1");
            fs::rename(path, &old)?;
        }
    } else if size > MOST_BYTES + UNLOCKED_SLACK {
        return Ok(());
    }
    let mut file = open_private(OpenOptions::new().create(true).append(true), path)?;
    file.write_all(line.as_bytes())
}

/// Opens a file for its owner alone: never through a link, and never anything but a plain file,
/// since opening a named pipe nobody reads would otherwise wait for ever.
fn open_private(options: &mut OpenOptions, path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("not a plain file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(file)
}

#[cfg(unix)]
fn rotation_lock(path: &Path) -> Option<File> {
    use std::os::fd::AsRawFd;
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let file = open_private(
        OpenOptions::new().create(true).write(true),
        Path::new(&name),
    )
    .ok()?;
    for _ in 0..LOCK_TRIES {
        // SAFETY: flock only reads the descriptor `file` holds open, and is released with it.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Some(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}

/// Windows has no flock: its one writer at a time is whichever Kerdr is open.
#[cfg(not(unix))]
fn rotation_lock(path: &Path) -> Option<()> {
    let _ = path;
    Some(())
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
            let got = scrub(&text, &[words], &[]);
            assert!(!got.contains(gone), "{got}");
            assert!(got.contains(TYPED), "{got}");
        }
        // A short answer goes when whole, and text too small to be a quote stays.
        assert_eq!(
            scrub("the laptop said no", &["laptop"], &[]),
            format!("the {TYPED} said no")
        );
        assert_eq!(scrub("no such card", &["no"], &[]), "no such card");
        // What wasn't typed stays.
        assert_eq!(
            scrub("the push failed: rejected", &[words], &[words]),
            "the push failed: rejected"
        );
        // Letters beyond ASCII don't trip the cutting.
        assert_eq!(
            scrub(
                "é café: déjà vu ünïcödé tèxt here",
                &["déjà vu ünïcödé tèxt"],
                &[]
            ),
            format!("é café: {TYPED} here")
        );
    }

    #[test]
    fn every_word_sent_stays_out_however_it_came_back() {
        let words = "Ask Zephyr's team to move the Board's new-intent box, don't \"wait\"";
        for (text, gone) in [
            // A short quote, a name said once.
            ("no such person: Zephyr", "Zephyr"),
            // A path made of them.
            (
                "intents/0099-move-boards-new-intent-box is already there",
                "intent-box",
            ),
            // Another case, or escaped as JSON.
            ("said ZEPHYR", "ZEPHYR"),
            ("refused \\\"wait\\\"", "wait"),
        ] {
            let got = scrub(text, &[], &[words]);
            assert!(!got.contains(gone), "{got}");
            assert!(got.contains(TYPED), "{got}");
        }
        assert_eq!(
            scrub(
                "intents/0099-move-boards-new-intent-box is already there",
                &[],
                &[words]
            ),
            format!("intents/0099-{TYPED}-boards-new-{TYPED}-box is already there")
        );
        // What isn't theirs stays readable beside it.
        assert_eq!(
            scrub("the push failed: Zephyr", &[], &[words]),
            format!("the push failed: {TYPED}")
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
            ("password: dummy-private-value", "dummy-private"),
            ("{\"token\": \"dummy-private-value\"}", "dummy-private"),
            ("{\"token\":\"dummy-private-value\"}", "dummy-private"),
            ("{\"password\": \"two private words\"}", "private"),
            ("api_key = dummy-private-value", "dummy-private"),
            (
                "{\"authorization\":\"Bearer dummy-private-value\"}",
                "dummy-private",
            ),
            ("{\"password\":\"two private words\"}", "private"),
            ("{\"password\":\"esc\\\" private\"}", "private"),
            ("{\\\"token\\\": \\\"two private words\\\"}", "private"),
        ] {
            let got = scrub(text, &[], &[]);
            assert!(!got.contains(secret), "{text} -> {got}");
            assert!(got.contains(SECRET), "{text} -> {got}");
        }
        let got = scrub(&format!("key was {long}."), &[], &[]);
        assert!(!got.contains(long), "{got}");
        // What follows a quoted secret carries on as usual.
        assert_eq!(
            scrub("{\"password\": \"two words\", \"who\": \"me\"}", &[], &[]),
            "{\"password\": [redacted], \"who\": \"me\"}"
        );
        // Ordinary words and paths come through.
        for text in [
            "/home/someone/repos/ark-sdlc/intents/0500-kerdr-error-log/intent.md is there",
            "the push failed: ! [rejected] main -> main (fetch first)",
            "ssh: connect to host lab port 22: Connection timed out",
            "error: tokens are fine",
        ] {
            assert_eq!(scrub(text, &[], &[]), text);
        }
    }

    #[test]
    fn a_secrets_name_you_also_typed_still_marks_its_value() {
        let got = scrub(
            "token: dummy-private-value",
            &["Debug token parsing"],
            &["Debug token parsing"],
        );
        assert!(!got.contains("dummy-private"), "{got}");
    }

    #[test]
    fn a_credential_quoted_in_part_goes_too() {
        assert_eq!(
            credential_out(
                "bad: private-credential-value!",
                "abc-private-credential-value"
            ),
            "bad: [redacted]!"
        );
        assert_eq!(
            credential_out("no match here", "abc-private"),
            "no match here"
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_its_owner_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, log, path) = log();
        log.write(&entry("one"), &[], &[]);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        // One made open before is closed by the next line.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        log.write(&entry("two"), &[], &[]);
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
        log.write(&entry("one"), &[], &[]);
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "");
    }

    #[test]
    fn it_never_grows_past_two_files_of_its_limit() {
        let (_dir, log, path) = log();
        let long = "word ".repeat(1500);
        for i in 0..1500 {
            log.write(&entry(&format!("{i} {long}")), &[], &[]);
        }
        let old = path.with_extension("log.1");
        let size = |p: &Path| fs::metadata(p).unwrap().len();
        assert!(size(&path) <= MOST_BYTES, "{}", size(&path));
        assert!(size(&old) <= MOST_BYTES, "{}", size(&old));
        // The newest is in the newer file, and whole.
        let last = lines(&path).pop().unwrap();
        assert!(last["message"].as_str().unwrap().starts_with("1499 "));
    }

    // Windows has no flock, and only one Kerdr at a time writes there.
    #[cfg(unix)]
    #[test]
    fn writers_at_once_keep_the_older_file_whole() {
        let (_dir, log, path) = log();
        let long = "word ".repeat(400);
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let (log, long) = (&log, &long);
                scope.spawn(move || {
                    for i in 0..500 {
                        log.write(&entry(&format!("{writer} {i} {long}")), &[], &[]);
                    }
                });
            }
        });
        let old = path.with_extension("log.1");
        let size = |p: &Path| fs::metadata(p).unwrap().len();
        // Never moved over itself: the older file is a full one, and each line is whole.
        assert!(size(&old) > MOST_BYTES / 2, "{}", size(&old));
        // A writer that can't get the lock in a moment adds its line anyway: a little over is fine.
        let most = MOST_BYTES + 256 * 1024;
        assert!(size(&old) <= most, "{}", size(&old));
        assert!(size(&path) <= most, "{}", size(&path));
        lines(&path);
        lines(&old);
    }

    #[cfg(unix)]
    #[test]
    fn a_stuck_lock_cant_make_it_grow_without_limit() {
        use std::os::fd::AsRawFd;
        let (_dir, log, path) = log();
        log.write(&entry("first"), &[], &[]);
        let mut name = path.as_os_str().to_owned();
        name.push(".lock");
        let holder = fs::File::open(&name).unwrap();
        // A child another test is starting can hold it for a moment on a busy machine: keep trying.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            // SAFETY: flock only reads the descriptor `holder` keeps open.
            let got = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if got == 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "the lock never came free"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let long = "word ".repeat(1500);
        for _ in 0..200 {
            log.write(&entry(&long), &[], &[]);
        }
        let size = fs::metadata(&path).unwrap().len();
        assert!(size > 0 && size <= MOST_BYTES + UNLOCKED_SLACK, "{size}");
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_in_its_place_never_holds_kerdr_up() {
        let (_dir, log, path) = log();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a valid path for the length of the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            log.write(&entry("one"), &[], &[]);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the write should have given up at once");
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
        ErrorLog::at(in_the_way.join("errors.log")).write(&entry("one"), &[], &[]);
        ErrorLog::at(PathBuf::from("/proc/no/such/errors.log")).write(&entry("two"), &[], &[]);
        ErrorLog::off().write(&entry("three"), &[], &[]);
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
