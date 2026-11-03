//! Editing a backlog item's text in your own editor (intent 0414): the private file it opens in,
//! which editor opens it, and what comes back. The board's side (which cards, the answers) is in
//! `app/edit_controls.rs`; handing the terminal over is `jump::hand_over`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The block that holds an item's newer text, below his own version, when the text changed while
/// he had it open: its first and closing lines, each carrying a marker Kerdr made for this one edit
/// and keeps in memory (`new_marker`). Only lines with that marker are taken out when he saves, so a
/// line of his own that looks like one (an example, in a code block or not) is never touched.
fn newer_start(marker: &str) -> String {
    format!("<!-- kerdr {marker}: newer text below.")
}

fn newer_end(marker: &str) -> String {
    format!("<!-- kerdr {marker}: end of the newer text -->")
}

/// A marker for one edit's newer-text block: unlikely to be in anyone's text by chance, and never
/// the same twice in one Kerdr.
pub fn new_marker() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    format!("{:016x}", hasher.finish())
}

/// An item's text as Kerdr sends and compares it: line endings as `\n`, and no whitespace or
/// newlines at the very end (an editor adds a final newline).
pub fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_string()
}

/// What the file holds when it opens: his text, and when the item's text changed underneath him,
/// a clearly marked block below it with the newer text (marked with `marker`), so he keeps what he
/// wants of each. One final newline, as an editor would leave it.
pub fn file_text(reference: &str, mine: &str, newer: Option<&str>, marker: &str) -> String {
    match newer {
        None => format!("{mine}\n"),
        Some(newer) => format!(
            "{mine}\n\n{} {reference}'s text changed while you had it open; this is it as it is now. Keep what you want above this line: from here to the closing line is left out when you save. -->\n{newer}\n{}\n",
            newer_start(marker),
            newer_end(marker),
        ),
    }
}

/// The file's text with the newer-text block marked with `marker` taken out: from its first line
/// to its closing line, or to the end when the closing line is gone. A closing line left on its own
/// goes too, and so do blank lines left at the very start once a block's lines were there, so
/// taking the newer text whole reads as that text. Lines that only look like a block's, without
/// this edit's marker, are his own words and stay.
fn without_newer(text: &str, marker: &str) -> String {
    let (start, end) = (newer_start(marker), newer_end(marker));
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    let mut marked = false;
    for line in text.split('\n') {
        if !inside && line.trim_start().starts_with(&start) {
            inside = true;
            marked = true;
            continue;
        }
        if line.trim() == end {
            inside = false;
            marked = true;
            continue;
        }
        if !inside {
            out.push(line);
        }
    }
    if marked {
        let blank = out.iter().take_while(|l| l.trim().is_empty()).count();
        out.drain(..blank);
    }
    out.join("\n")
}

/// What an editor left in the file, as the item's text: the newer-text block this edit put there
/// (when it put one, with `marker`) taken out, then normalised.
pub fn read_back(file: &str, marker: Option<&str>) -> String {
    match marker {
        Some(marker) => normalise(&without_newer(&normalise(file), marker)),
        None => normalise(file),
    }
}

/// The file's name, from the item's id: `0414-text.md`, so an editor treats it as markdown. Only
/// letters, digits, `-` and `_` from the id are kept; anything else becomes `_`.
pub fn file_name(id: &str) -> String {
    let id: String = id
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let id = if id.is_empty() { "item".into() } else { id };
    format!("{id}-text.md")
}

/// The words of an editor setting, split as a shell would split them but never run by one:
/// `code --wait` is `code` and `--wait`. Single and double quotes keep spaces in a word; on Unix
/// a backslash keeps the next character as it is, while on Windows it's part of a path.
pub fn editor_words(value: &str) -> Vec<String> {
    split_words(value, cfg!(unix))
}

fn split_words(value: &str, escapes: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                started = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if escapes => match chars.next() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => word.push(n),
                            Some(n) => {
                                word.push('\\');
                                word.push(n);
                            }
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' if escapes => {
                started = true;
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// The editors to try, in order: Kerdr's `editor` setting when it's set, else `$VISUAL`, else
/// `$EDITOR`, else nvim, else vim. Each is its words; the file's path goes after them.
pub fn candidates(
    configured: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for value in [configured, visual, editor, Some("nvim"), Some("vim")]
        .into_iter()
        .flatten()
    {
        let words = editor_words(value);
        if !words.is_empty() && !out.contains(&words) {
            out.push(words);
        }
    }
    out
}

/// The editors to try on this machine: Kerdr's setting, then the environment's.
pub fn candidates_here(configured: Option<&str>) -> Vec<Vec<String>> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    candidates(
        configured.filter(|c| !c.trim().is_empty()),
        var("VISUAL").as_deref(),
        var("EDITOR").as_deref(),
    )
}

/// How an editor's run went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// The editor that ran, as it was written; None when none could start.
    pub editor: Option<String>,
    /// Whether it exited cleanly (`:wq`, `:q`); false for `:cq`, or when it died.
    pub ok: bool,
    /// Whether it ended with no exit code at all: killed by a signal, or crashed. Unix only; on
    /// Windows every end has a code.
    pub died: bool,
    /// The editors tried before it that couldn't start, each with why.
    pub skipped: Vec<String>,
}

/// The flag an editor that opens a window of its own needs before Kerdr can wait for it, when
/// `words` name one without it: `code` without `--wait`, say. Kerdr would come back at once and
/// never see what you write there, so it doesn't start one.
pub fn missing_wait(words: &[String]) -> Option<&'static str> {
    let (program, args) = words.split_first()?;
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".cmd"))
        .unwrap_or(&name);
    let has = |flags: &[&str]| args.iter().any(|a| flags.contains(&a.as_str()));
    match name {
        "code" | "code-insiders" | "codium" => (!has(&["--wait", "-w"])).then_some("--wait"),
        "subl" | "sublime_text" => (!has(&["-w", "--wait"])).then_some("-w"),
        "gvim" | "mvim" => (!has(&["-f", "--nofork"])).then_some("-f"),
        _ => None,
    }
}

/// Runs the first editor that starts on `path`, on the terminal as it is (the caller hands it
/// over), and waits for it. An editor that can't start is skipped for the next. `intro` is
/// printed first, for an editor that opens a window of its own and leaves the terminal as it is.
pub fn launch(candidates: &[Vec<String>], path: &Path, intro: &str) -> Run {
    let mut skipped = Vec::new();
    for words in candidates {
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        let name = words.join(" ");
        println!("\n{intro} It opens in {name}; Kerdr comes back when it closes.\n");
        let _ = std::io::stdout().flush();
        let status = Command::new(crate::jump::expand_home(program))
            .args(args)
            .arg(path)
            .status();
        match status {
            Ok(status) => {
                return Run {
                    editor: Some(name),
                    ok: status.success(),
                    died: status.code().is_none(),
                    skipped,
                };
            }
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
    }
    Run {
        editor: None,
        ok: false,
        died: false,
        skipped,
    }
}

/// How an edit ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The editor exited cleanly: what the file holds, read back as the item's text, and whether
    /// the file was written at all (`:wq`, even with nothing changed) or left alone (`:q`).
    Wrote {
        text: String,
        run: Run,
        touched: bool,
    },
    /// It exited with an error (`:cq`): nothing is saved.
    Cancelled { run: Run },
    /// It died (killed by a signal, or crashed) without exiting: what the file held then, read
    /// back as `Wrote` reads it, which is kept for you and sent nowhere.
    Died {
        text: String,
        run: Run,
        touched: bool,
    },
    /// The file couldn't be read back once the editor had gone: why, and where the file is kept,
    /// left in place so nothing written there is lost.
    Unreadable {
        why: String,
        kept: PathBuf,
        run: Run,
    },
    /// No editor could start: each tried, with why.
    NoEditor { skipped: Vec<String> },
    /// The file couldn't be written.
    Failed(String),
}

/// The start of each private folder's name: `kerdr-edit-<Kerdr's process id>-`, so a folder a
/// Kerdr that was killed left behind can be told from one still open.
const FOLDER_PREFIX: &str = "kerdr-edit-";

/// A folder kept because its file couldn't be read back: never swept.
const KEPT_PREFIX: &str = "kerdr-kept-";

fn file_name_of(path: &Path) -> std::ffi::OsString {
    path.file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default()
}

/// The private file an item's text opens in: `NNNN-text.md`, in a folder of its own in the system
/// temp folder that only you can open (0700 on Unix), the file readable only by you (0600). The
/// folder and file are removed when it's dropped, and by the next Kerdr to start when a killed
/// one couldn't (`sweep`).
pub struct EditFile {
    dir: tempfile::TempDir,
    path: PathBuf,
}

impl EditFile {
    pub fn create(id: &str, text: &str) -> std::io::Result<EditFile> {
        let mut builder = tempfile::Builder::new();
        let prefix = format!("{FOLDER_PREFIX}{}-", std::process::id());
        builder.prefix(&prefix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.path().join(file_name(id));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(EditFile { dir, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    fn read(&self) -> std::io::Result<String> {
        let bytes = std::fs::read(&self.path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Keeps the folder and its file when something went wrong reading it back: renamed out of
    /// the sweep's reach (`kerdr-kept-…`), so a later Kerdr doesn't remove it. Where the file is.
    fn keep(self) -> PathBuf {
        let name = file_name_of(&self.path);
        let dir = self.dir.keep();
        let kept = dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix(FOLDER_PREFIX))
            .map(|rest| dir.with_file_name(format!("{KEPT_PREFIX}{rest}")))
            .filter(|to| std::fs::rename(&dir, to).is_ok())
            .unwrap_or(dir);
        kept.join(name)
    }

    /// When the file was last written, as well as the file system can say.
    fn written_at(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }
}

/// Removes the private folders that Kerdrs no longer running left in `temp` (killed while an
/// editor was open, so their own clean-up never ran): only this account's, and only those whose
/// Kerdr has gone. Returns how many it removed. Unix only; elsewhere it leaves them.
pub fn sweep(temp: &Path) -> usize {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(entries) = std::fs::read_dir(temp) else {
            return 0;
        };
        // SAFETY: getuid has no preconditions and can't fail.
        let me = unsafe { libc::getuid() };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|n| n.strip_prefix(FOLDER_PREFIX))
                .and_then(|rest| rest.split_once('-'))
                .and_then(|(pid, _)| pid.parse::<libc::pid_t>().ok())
                .filter(|pid| *pid > 0)
            else {
                continue;
            };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.is_dir() || meta.uid() != me || running(pid) {
                continue;
            }
            if std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
    #[cfg(not(unix))]
    {
        let _ = temp;
        0
    }
}

/// Whether a process with this id is running (or can't be told apart from one that is).
#[cfg(unix)]
fn running(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 checks only that the process exists; nothing is sent.
    let gone = unsafe { libc::kill(pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    !gone
}

/// While an editor has the terminal: a hang-up (the terminal closed, SSH dropped) or a request to
/// stop is held until the editor has closed and its private file is gone, rather than ending
/// Kerdr with the file left behind; `held` then says it came, and Kerdr ends as it would have.
/// Dropping it puts the signals' handling back as it was.
pub struct HoldStops {
    #[cfg(unix)]
    previous: Vec<(libc::c_int, libc::sighandler_t)>,
}

#[cfg(unix)]
static HELD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn hold(signal: libc::c_int) {
    HELD.store(signal, std::sync::atomic::Ordering::SeqCst);
}

impl HoldStops {
    pub fn new() -> HoldStops {
        #[cfg(unix)]
        {
            HELD.store(0, std::sync::atomic::Ordering::SeqCst);
            let handler = hold as extern "C" fn(libc::c_int) as libc::sighandler_t;
            let previous = [libc::SIGHUP, libc::SIGTERM]
                .into_iter()
                // SAFETY: the handler only stores the signal's number in an atomic.
                .map(|s| (s, unsafe { libc::signal(s, handler) }))
                .collect();
            HoldStops { previous }
        }
        #[cfg(not(unix))]
        HoldStops {}
    }

    /// The signal held, if one came.
    pub fn held(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            Some(HELD.load(std::sync::atomic::Ordering::SeqCst)).filter(|s| *s != 0)
        }
        #[cfg(not(unix))]
        None
    }
}

impl Default for HoldStops {
    fn default() -> Self {
        HoldStops::new()
    }
}

impl Drop for HoldStops {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, previous) in self.previous.drain(..) {
            // SAFETY: restores the handler that was there before.
            unsafe {
                libc::signal(signal, previous);
            }
        }
    }
}

/// Opens `file` (what it holds when it opens) for item `id` in a private file, has `run` edit it
/// (an editor, handed the terminal), and reads back what it left, taking out the newer-text block
/// this edit put there when `marker` names one. The file and its folder are gone when this
/// returns, unless the file couldn't be read back: then they're kept, and `Unreadable` says where.
pub fn edit_by(
    id: &str,
    file: &str,
    marker: Option<&str>,
    run: impl FnOnce(&Path) -> Run,
) -> Ended {
    let edit = match EditFile::create(id, file) {
        Ok(f) => f,
        Err(e) => return Ended::Failed(format!("couldn't write the file to edit ({e})")),
    };
    let before = edit.written_at();
    let run = run(edit.path());
    if run.editor.is_none() {
        return Ended::NoEditor {
            skipped: run.skipped,
        };
    }
    // :cq: an exit of its own choosing, with an error. Nothing it left is wanted.
    if !run.ok && !run.died {
        return Ended::Cancelled { run };
    }
    match edit.read() {
        Ok(text) => {
            let touched = text != file || edit.written_at() != before;
            let text = read_back(&text, marker);
            if run.died {
                Ended::Died { text, run, touched }
            } else {
                Ended::Wrote { text, run, touched }
            }
        }
        Err(e) => Ended::Unreadable {
            why: e.to_string(),
            kept: edit.keep(),
            run,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalising_unifies_line_endings_and_drops_only_the_trailing_space() {
        assert_eq!(normalise("a\r\nb\rc\n\n  \t\n"), "a\nb\nc");
        assert_eq!(
            normalise("  indented\n\nkept  \nend"),
            "  indented\n\nkept  \nend"
        );
        assert_eq!(normalise("\n\n \n"), "");
        assert_eq!(normalise("one line\n"), "one line");
    }

    #[test]
    fn the_file_is_named_for_the_item_and_never_leaves_its_folder() {
        assert_eq!(file_name("0414"), "0414-text.md");
        assert_eq!(file_name("../../etc/passwd"), "______etc_passwd-text.md");
        assert_eq!(file_name(""), "item-text.md");
        assert_eq!(file_name("A_b-9"), "A_b-9-text.md");
    }

    #[test]
    fn the_newer_text_sits_below_his_and_is_left_out_when_he_saves() {
        let m = "0123abcd";
        let back = |file: &str| read_back(file, Some(m));
        let file = file_text(
            "#414",
            "My version.\n\nSecond line.",
            Some("Their newer text."),
            m,
        );
        assert!(file.starts_with("My version.\n\nSecond line.\n\n"));
        assert!(file.contains("#414's text changed while you had it open"));
        assert!(file.contains("\nTheir newer text.\n"));
        assert!(file.ends_with(&format!("{}\n", newer_end(m))));
        // Saved as it opened: his version, the block left out.
        assert_eq!(back(&file), "My version.\n\nSecond line.");
        // He takes a line from the newer text above the block.
        let edited = file.replacen("Second line.", "Their newer text.", 1);
        assert_eq!(back(&edited), "My version.\n\nTheir newer text.");
        // Lines he writes below the block are his too.
        assert_eq!(
            back(&format!("{file}After it.\n")),
            "My version.\n\nSecond line.\n\nAfter it."
        );
        // The closing line gone: everything from the block's first line is left out.
        let cut = file.replace(&format!("{}\n", newer_end(m)), "");
        assert_eq!(back(&cut), "My version.\n\nSecond line.");
        // An editor's own line endings change nothing.
        assert_eq!(
            back(&file.replace('\n', "\r\n")),
            "My version.\n\nSecond line."
        );
        // The block's first line taken out with his own, its closing line left: the closing line
        // goes too, and so does the blank line that stood between them, so taking the newer text
        // whole reads as exactly that text.
        let theirs_only = file
            .lines()
            .filter(|l| {
                !l.starts_with("My version.")
                    && !l.starts_with("Second line.")
                    && !l.starts_with(&newer_start(m))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(theirs_only.contains(&newer_end(m)), "{theirs_only}");
        assert_eq!(back(&theirs_only), "Their newer text.");
        // This edit's closing line on its own is never part of his words.
        assert_eq!(
            back(&format!("Mine.\n{}\nMore.\n", newer_end(m))),
            "Mine.\nMore."
        );
        // Blank lines at the start of a text with no block are his own.
        assert_eq!(back("\nMine.\n"), "\nMine.");
        // With nothing newer, the file is his text and one final newline.
        assert_eq!(file_text("#414", "Words.", None, m), "Words.\n");
        assert_eq!(read_back("Words.\n", None), "Words.");
    }

    #[test]
    fn only_this_edits_markers_are_taken_out_and_his_own_lines_like_them_stay() {
        // His text quotes a block's lines, plainly and in a fenced code block: an example of what
        // Kerdr writes, or another edit's markers.
        let other = newer_start("ffffffffffffffff");
        let his = format!(
            "Explains the conflict layout.\n\n<!-- kerdr: newer text below. old style -->\nkept\n<!-- kerdr: end of the newer text -->\n\n```\n{other} -->\ninside a fence\n{}\n```\nAfter.",
            newer_end("ffffffffffffffff")
        );
        // Opened with no newer text, no marker: read back as it is, every line.
        assert_eq!(read_back(&format!("{his}\n"), None), his);
        // Opened with a newer block of this edit's own: only that block goes.
        let m = new_marker();
        assert_ne!(m, new_marker(), "a marker is never the same twice");
        assert!(!his.contains(&m));
        let file = file_text("#414", &his, Some("Theirs."), &m);
        assert_eq!(read_back(&file, Some(&m)), his);
        // And saved with a line of his changed, every other line is as he had it.
        let edited = file.replacen("After.", "After, changed.", 1);
        assert_eq!(
            read_back(&edited, Some(&m)),
            his.replacen("After.", "After, changed.", 1)
        );
    }

    #[test]
    fn an_editor_setting_is_split_into_words_without_a_shell() {
        assert_eq!(split_words("code --wait", true), ["code", "--wait"]);
        assert_eq!(split_words("  nvim  ", true), ["nvim"]);
        assert_eq!(
            split_words("'/Applications/Sub Text/subl' -w", true),
            ["/Applications/Sub Text/subl", "-w"]
        );
        assert_eq!(
            split_words(r#""my editor" -c "set ft=md""#, true),
            ["my editor", "-c", "set ft=md"]
        );
        assert_eq!(
            split_words(r"emacs\ client -t; rm", true),
            ["emacs client", "-t;", "rm"]
        );
        assert_eq!(split_words(r#""a\"b""#, true), [r#"a"b"#]);
        assert_eq!(split_words("''", true), [""]);
        assert!(split_words("   ", true).is_empty());
        // On Windows a backslash is part of a path.
        assert_eq!(
            split_words(r#""C:\Program Files\Neovim\bin\nvim.exe" -p"#, false),
            [r"C:\Program Files\Neovim\bin\nvim.exe", "-p"]
        );
        assert_eq!(
            split_words(r"C:\tools\vim.exe", false),
            [r"C:\tools\vim.exe"]
        );
    }

    #[test]
    fn the_editor_is_the_setting_then_visual_then_editor_then_nvim_then_vim() {
        let words = |v: &[&[&str]]| -> Vec<Vec<String>> {
            v.iter()
                .map(|w| w.iter().map(|s| s.to_string()).collect())
                .collect()
        };
        assert_eq!(
            candidates(Some("code --wait"), Some("hx"), Some("nano")),
            words(&[&["code", "--wait"], &["hx"], &["nano"], &["nvim"], &["vim"]])
        );
        assert_eq!(
            candidates(None, None, Some("nvim")),
            words(&[&["nvim"], &["vim"]])
        );
        assert_eq!(
            candidates(Some("  "), None, None),
            words(&[&["nvim"], &["vim"]])
        );
    }

    #[test]
    fn the_file_is_private_named_for_the_item_and_removed_afterwards() {
        let mut seen = None;
        let ended = edit_by("0414", "The words.\n", None, |path| {
            assert_eq!(path.file_name().unwrap(), "0414-text.md");
            assert_eq!(std::fs::read_to_string(path).unwrap(), "The words.\n");
            let dir = path.parent().unwrap().to_path_buf();
            assert!(dir.starts_with(std::env::temp_dir()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode(&dir), 0o700);
                assert_eq!(mode(path), 0o600);
            }
            std::fs::write(path, "New words.\r\n\r\n").unwrap();
            seen = Some(dir);
            Run {
                editor: Some("fake".into()),
                ok: true,
                ..Default::default()
            }
        });
        assert_eq!(
            ended,
            Ended::Wrote {
                text: "New words.".into(),
                run: Run {
                    editor: Some("fake".into()),
                    ok: true,
                    ..Default::default()
                },
                touched: true,
            }
        );
        let dir = seen.unwrap();
        assert!(!dir.exists(), "the folder is removed");
        // Removed whatever happened: a cancel, and no editor at all.
        for ok in [false, true] {
            let mut seen = None;
            let ended = edit_by("0414", "x\n", None, |path| {
                seen = Some(path.parent().unwrap().to_path_buf());
                Run {
                    editor: ok.then(|| "fake".into()),
                    ok: false,
                    skipped: vec!["nvim: not found".into()],
                    ..Default::default()
                }
            });
            assert!(!seen.unwrap().exists());
            if ok {
                assert!(matches!(ended, Ended::Cancelled { .. }), "{ended:?}");
            } else {
                assert_eq!(
                    ended,
                    Ended::NoEditor {
                        skipped: vec!["nvim: not found".into()]
                    }
                );
            }
        }
    }

    #[test]
    fn an_editor_that_cannot_start_is_skipped_and_none_starting_is_said() {
        let file = EditFile::create("0414", "x\n").unwrap();
        let run = launch(
            &[
                vec!["kerdr-no-such-editor-1".into()],
                vec!["kerdr-no-such-editor-2".into(), "--wait".into()],
            ],
            file.path(),
            "Kerdr:",
        );
        assert_eq!(run.editor, None);
        assert_eq!(run.skipped.len(), 2);
        assert!(run.skipped[0].starts_with("kerdr-no-such-editor-1: "));
        assert!(run.skipped[1].starts_with("kerdr-no-such-editor-2 --wait: "));
        let dir = file.dir().to_path_buf();
        drop(file);
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn folders_a_killed_kerdr_left_are_swept_and_open_ones_kept() {
        let temp = tempfile::tempdir().unwrap();
        // A process that has finished, so its id names nobody running.
        let mut child = Command::new("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        let left = temp.path().join(format!("{FOLDER_PREFIX}{gone}-abc"));
        std::fs::create_dir(&left).unwrap();
        std::fs::write(left.join("0414-text.md"), "a token").unwrap();
        let open = temp
            .path()
            .join(format!("{FOLDER_PREFIX}{}-def", std::process::id()));
        std::fs::create_dir(&open).unwrap();
        let other = temp.path().join("kerdr-edit-notapid");
        std::fs::create_dir(&other).unwrap();
        let file = temp.path().join(format!("{FOLDER_PREFIX}{gone}-file"));
        std::fs::write(&file, "x").unwrap();
        assert_eq!(sweep(temp.path()), 1);
        assert!(!left.exists(), "a folder whose Kerdr is gone is removed");
        assert!(open.exists(), "a running Kerdr's folder is kept");
        assert!(
            other.exists() && file.exists(),
            "anything else is left alone"
        );
        // Kerdr's own folders carry its process id, so the sweep can tell.
        let edit = EditFile::create("0414", "x\n").unwrap();
        let name = edit
            .dir()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            name.starts_with(&format!("{FOLDER_PREFIX}{}-", std::process::id())),
            "{name}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hang_up_while_the_editor_is_open_is_held_until_its_file_is_gone() {
        let stops = HoldStops::new();
        assert_eq!(stops.held(), None);
        // SAFETY: SIGHUP's handler is HoldStops' own, which only records it.
        unsafe {
            libc::raise(libc::SIGHUP);
        }
        assert_eq!(stops.held(), Some(libc::SIGHUP));
        drop(stops);
        // SAFETY: reads the handler back without changing it.
        let now = unsafe {
            let now = libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, now);
            now
        };
        assert_eq!(now, libc::SIG_DFL, "its handling is put back");
    }

    /// A fake editor: a shell script given the file's path, as an editor would be.
    #[cfg(unix)]
    fn fake(dir: &Path, body: &str) -> Vec<String> {
        let path = dir.join("editor.sh");
        std::fs::write(&path, body).unwrap();
        vec!["sh".into(), path.to_str().unwrap().into()]
    }

    #[cfg(unix)]
    #[test]
    fn a_real_editor_saving_nothing_cancelling_or_writing_then_failing() {
        let scripts = tempfile::tempdir().unwrap();
        let run = |body: &str, start: &str| {
            let editor = fake(scripts.path(), body);
            edit_by("0414", start, None, |path| {
                launch(
                    &[vec!["kerdr-no-such-editor".into()], editor],
                    path,
                    "Kerdr:",
                )
            })
        };
        // :wq with a change.
        let Ended::Wrote {
            text,
            run: r,
            touched,
        } = run("printf 'Changed.\\n' > \"$1\"\n", "Was.\n")
        else {
            panic!()
        };
        assert_eq!(text, "Changed.");
        assert!(touched);
        assert!(r.ok && !r.died && r.editor.as_deref().is_some_and(|e| e.starts_with("sh ")));
        assert_eq!(r.skipped.len(), 1, "the missing editor was skipped");
        // :q at once with nothing changed: the file is left alone, and that's all.
        let Ended::Wrote { text, touched, .. } = run("exit 0\n", "Was.\n") else {
            panic!()
        };
        assert_eq!(text, "Was.");
        assert!(!touched);
        // :wq with nothing changed: written all the same, which says he chose to keep it.
        let Ended::Wrote { text, touched, .. } =
            run("sleep 0.05\nprintf 'Was.\\n' > \"$1\"\n", "Was.\n")
        else {
            panic!()
        };
        assert_eq!(text, "Was.");
        assert!(touched, "a write of the same text counts as written");
        // :cq.
        assert!(matches!(run("exit 1\n", "Was.\n"), Ended::Cancelled { .. }));
        // :w then :cq: still nothing saved.
        assert!(matches!(
            run("printf 'Half done' > \"$1\"\nexit 3\n", "Was.\n"),
            Ended::Cancelled { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_editor_that_writes_then_dies_leaves_what_it_wrote() {
        let scripts = tempfile::tempdir().unwrap();
        for signal in ["KILL", "SEGV", "HUP"] {
            let editor = fake(
                scripts.path(),
                &format!("printf 'Half a rewrite.\\n' > \"$1\"\nkill -{signal} $$\n"),
            );
            let mut dir = None;
            let ended = edit_by("0414", "Was.\n", None, |path| {
                dir = Some(path.parent().unwrap().to_path_buf());
                launch(&[editor], path, "Kerdr:")
            });
            let Ended::Died { text, run, touched } = ended else {
                panic!("{signal}: {ended:?}")
            };
            assert_eq!(text, "Half a rewrite.", "{signal}");
            assert!(touched && run.died && !run.ok, "{signal}");
            assert!(!dir.unwrap().exists(), "read, so its file is gone");
        }
        // Died before writing anything: what it held is what it opened with.
        let editor = fake(scripts.path(), "kill -KILL $$\n");
        let ended = edit_by("0414", "Was.\n", None, |path| launch(&[editor], path, ""));
        assert!(
            matches!(&ended, Ended::Died { text, touched: false, .. } if text == "Was."),
            "{ended:?}"
        );
    }

    #[test]
    fn a_file_that_cant_be_read_back_is_kept_where_it_is() {
        let mut opened = None;
        let ended = edit_by("0414", "Was.\n", None, |path| {
            // The editor left something Kerdr can't read as the file: a folder in its place,
            // with what he wrote inside.
            std::fs::remove_file(path).unwrap();
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("his words"), "Mine.").unwrap();
            opened = Some(path.to_path_buf());
            Run {
                editor: Some("fake".into()),
                ok: true,
                ..Default::default()
            }
        });
        let Ended::Unreadable { why, kept, .. } = ended else {
            panic!("{ended:?}")
        };
        assert!(!why.is_empty());
        assert!(!opened.unwrap().exists(), "moved out of the sweep's reach");
        assert_eq!(kept.file_name().unwrap(), "0414-text.md");
        let folder = kept.parent().unwrap();
        let name = folder.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(KEPT_PREFIX), "{name}");
        assert_eq!(
            std::fs::read_to_string(kept.join("his words")).unwrap(),
            "Mine."
        );
        // The sweep leaves a kept folder alone.
        #[cfg(unix)]
        {
            sweep(folder.parent().unwrap());
            assert!(kept.exists());
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn an_editor_that_opens_a_window_without_waiting_is_named_with_its_flag() {
        let words = |s: &str| editor_words(s);
        assert_eq!(missing_wait(&words("code")), Some("--wait"));
        assert_eq!(missing_wait(&words("/usr/bin/codium -n")), Some("--wait"));
        assert_eq!(missing_wait(&words("code --wait")), None);
        assert_eq!(missing_wait(&words("code -w")), None);
        assert_eq!(missing_wait(&words("subl")), Some("-w"));
        assert_eq!(missing_wait(&words("subl --wait")), None);
        assert_eq!(missing_wait(&words("gvim")), Some("-f"));
        assert_eq!(missing_wait(&words("mvim -f")), None);
        assert_eq!(
            missing_wait(&[r"C:\Program Files\Microsoft VS Code\bin\code.cmd".into()]),
            Some("--wait")
        );
        for terminal in ["nvim", "vim", "hx", "nano", "emacs -nw"] {
            assert_eq!(missing_wait(&words(terminal)), None, "{terminal}");
        }
    }
}
