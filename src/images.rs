//! Images in the new-intent box: read from this machine's clipboard, or from the path of an
//! image file pasted or dropped into the box. Each becomes an `[Image #N]` tag in the words, as
//! Claude Code shows them, and travels with the `create` request.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The most one image may be, in bytes.
pub const MAX_IMAGE_BYTES: usize = 10_000_000;
/// The most images one new intent may carry.
pub const MAX_IMAGES: usize = 20;
/// The most all of one new intent's images may be together, in bytes.
pub const MAX_IMAGES_BYTES: usize = 20_000_000;

/// How long a clipboard reader may take before Kerdr gives up on it.
const READ_DEADLINE: Duration = Duration::from_secs(5);

/// The kinds Kerdr attaches, by the type it sends.
const KINDS: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// One attached image, numbered by its tag in the words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub tag: u32,
    pub media_type: &'static str,
    pub bytes: Arc<Vec<u8>>,
}

/// An image's bytes and type, before it has a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Where the box's next images come from: this machine's clipboard, or files pasted by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Clipboard,
    Files(Vec<PathBuf>),
}

impl Source {
    /// Reads them, in the room a draft has left. Slow readers (PowerShell, a network mount)
    /// run this away from the board's own thread.
    pub fn load(&self, room: Room) -> Loading {
        match self {
            Source::Clipboard => from_clipboard(room),
            Source::Files(paths) => from_files(paths, room),
        }
    }
}

/// What a clipboard reader found: one image, or files a file manager copied, not read yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Image(Loaded),
    Files(Vec<PathBuf>),
}

/// The images read for the box, and why nothing more was: an empty clipboard, a refused
/// image, a limit reached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loading {
    pub images: Vec<Loaded>,
    pub why: Option<String>,
}

impl Loading {
    pub fn refused(why: impl Into<String>) -> Loading {
        Loading {
            images: vec![],
            why: Some(why.into()),
        }
    }

    fn say(&mut self, why: String) {
        self.why = Some(match self.why.take() {
            Some(before) => format!("{before} {why}"),
            None => why,
        });
    }
}

/// How many more images, and bytes of them, a draft can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub images: usize,
    pub bytes: usize,
}

impl Room {
    pub fn left(images: &[Image]) -> Room {
        let used: usize = images.iter().map(|i| i.bytes.len()).sum();
        Room {
            images: MAX_IMAGES.saturating_sub(images.len()),
            bytes: MAX_IMAGES_BYTES.saturating_sub(used),
        }
    }
}

/// Why an intent can't take another image: words naming the limit it's at.
pub fn too_many() -> String {
    format!("An intent takes at most {MAX_IMAGES} images.")
}

pub fn too_much() -> String {
    format!(
        "That would take this intent's images past {} together, the most Kerdr sends.",
        size(MAX_IMAGES_BYTES)
    )
}

/// Image files read in order while they fit in the room left, each one's size checked before
/// it's read, so a folder's worth copied at once never fills memory.
pub fn from_files(paths: &[PathBuf], room: Room) -> Loading {
    let mut out = Loading::default();
    let mut used = 0;
    for path in paths {
        if out.images.len() >= room.images {
            out.say(too_many());
            break;
        }
        let len = std::fs::metadata(path).map_or(0, |m| m.len() as usize);
        if len <= MAX_IMAGE_BYTES && used + len > room.bytes {
            out.say(too_much());
            break;
        }
        match from_file(path) {
            Ok(image) => {
                used += image.bytes.len();
                out.images.push(image);
            }
            Err(why) => out.say(why),
        }
    }
    out
}

/// The tag that stands for image `n` in the words.
pub fn tag(n: u32) -> String {
    format!("[Image #{n}]")
}

/// The image type its first bytes say it is; None for anything Kerdr doesn't attach.
pub fn media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A type's short name for messages: PNG, JPEG, GIF or WebP.
pub fn kind_name(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/gif" => "GIF",
        "image/webp" => "WebP",
        _ => "image",
    }
}

/// A size in plain words: 412 KB, 3.4 MB, 10 MB.
pub fn size(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        let tenths = (bytes as f64 / 100_000.0).round() as usize;
        if tenths.is_multiple_of(10) {
            format!("{} MB", tenths / 10)
        } else {
            format!("{}.{} MB", tenths / 10, tenths % 10)
        }
    } else {
        format!("{} KB", bytes.div_ceil(1000).max(1))
    }
}

/// Checks bytes read from somewhere as one image Kerdr can attach. A reader stops just past the
/// limit, so an image that's too large is named as over it rather than by its size.
pub fn checked(bytes: Vec<u8>, from: &str) -> Result<Loaded, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{from} is over {}, the most Kerdr attaches for one image.",
            size(MAX_IMAGE_BYTES)
        ));
    }
    match media_type(&bytes) {
        Some(media_type) => Ok(Loaded { media_type, bytes }),
        None => Err(format!(
            "{from} isn't a PNG, JPEG, GIF or WebP image, the kinds Kerdr attaches."
        )),
    }
}

/// An image file, read and checked.
pub fn from_file(path: &Path) -> Result<Loaded, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let meta = std::fs::metadata(path).map_err(|e| format!("Couldn't read {name}: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{name} isn't a file."));
    }
    if meta.len() > MAX_IMAGE_BYTES as u64 {
        return Err(format!(
            "{name} is {}; Kerdr attaches images up to {}.",
            size(meta.len() as usize),
            size(MAX_IMAGE_BYTES)
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("Couldn't read {name}: {e}."))?;
    checked(bytes, &name)
}

fn image_name(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
    })
}

/// The image files a paste names, when it is nothing but one or more paths to them (as a
/// terminal pastes a dropped file: quoted, backslash-escaped, or as a file:// address).
/// None when it is anything else, so it goes in as words.
pub fn paths_in_paste(text: &str) -> Option<Vec<PathBuf>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let all = |words: Vec<String>| -> Option<Vec<PathBuf>> {
        let paths: Vec<PathBuf> = words.iter().filter_map(|w| local_path(w)).collect();
        (!paths.is_empty()
            && paths.len() == words.len()
            && paths.iter().all(|p| image_name(p) && p.is_file()))
        .then_some(paths)
    };
    shell_words(text).and_then(all).or_else(|| {
        // One path with spaces that nobody escaped.
        (!text.contains('\n'))
            .then(|| all(vec![text.to_string()]))
            .flatten()
    })
}

/// A pasted word as a path on this machine: a file:// address, ~/…, or a path as it is.
fn local_path(word: &str) -> Option<PathBuf> {
    if let Some(rest) = word.strip_prefix("file://") {
        // file:///path, or file://localhost/path; another host's file isn't here.
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        let decoded = percent_decode(path)?;
        #[cfg(windows)]
        let decoded = decoded
            .strip_prefix('/')
            .map(str::to_string)
            .unwrap_or(decoded);
        return Some(PathBuf::from(decoded));
    }
    if let Some(rest) = word.strip_prefix("~/") {
        return Some(crate::config::home().join(rest));
    }
    let path = PathBuf::from(word);
    path.is_absolute().then_some(path)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Splits pasted text into words as a POSIX shell would: whitespace between words, with single
/// quotes, double quotes and backslashes. None when a quote is never closed.
fn shell_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            // Windows paths keep their backslashes; elsewhere one escapes the next character.
            '\\' if !cfg!(windows) => {
                started = true;
                if let Some(c) = chars.next() {
                    word.push(c);
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
    Some(words)
}

/// The JSON a `create` request carries for the images whose tags are still in the words:
/// each one's tag number, type and bytes in base64.
pub fn request_images(words: &str, images: &[Image]) -> Vec<Value> {
    use base64::Engine;
    images
        .iter()
        .filter(|i| words.contains(&tag(i.tag)))
        .map(|i| {
            json!({
                "tag": i.tag,
                "type": i.media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(i.bytes.as_slice()),
            })
        })
        .collect()
}

/// The room a request needs beyond its words for the most images one intent may carry.
pub const REQUEST_ALLOWANCE: usize = MAX_IMAGES_BYTES.div_ceil(3) * 4 + MAX_IMAGES * 128;

// --- The clipboard ---------------------------------------------------------------------------

/// What a clipboard reader printed and how it ended.
#[derive(Debug, Default)]
pub struct Captured {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Runs a program as `run` does for a reader, or fails as spawning it would.
pub type Runner<'a> = &'a dyn Fn(&str, &[&str]) -> std::io::Result<Captured>;

/// What this machine's clipboard holds that Kerdr can attach, in the room a draft has left:
/// one image, or the image files copied in a file manager, or plain words saying why there's
/// nothing to attach.
pub fn from_clipboard(room: Room) -> Loading {
    if room.images == 0 {
        return Loading::refused(too_many());
    }
    #[cfg(target_os = "macos")]
    let found = macos_clipboard();
    #[cfg(windows)]
    let found = windows_clipboard();
    #[cfg(not(any(target_os = "macos", windows)))]
    let found = {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        unix_clipboard(set("WAYLAND_DISPLAY"), set("DISPLAY"), &|program, args| {
            let mut cmd = Command::new(program);
            cmd.args(args);
            capture(cmd, MAX_IMAGE_BYTES + 1, READ_DEADLINE)
        })
    };
    match found {
        Ok(Found::Image(image)) => Loading {
            images: vec![image],
            why: None,
        },
        Ok(Found::Files(paths)) => from_files(&paths, room),
        Err(why) => Loading::refused(why),
    }
}

const NO_IMAGE: &str = "There's no image on the clipboard. Copy a screenshot or an image first, or paste an image file's path.";

/// Linux and the BSDs: wl-paste under Wayland, xclip under X11.
pub fn unix_clipboard(wayland: bool, x11: bool, run: Runner) -> Result<Found, String> {
    let mut missing = None;
    if wayland {
        match clipboard_with(run, "wl-paste", &["--list-types"], |t| {
            vec!["--no-newline".into(), "--type".into(), t.into()]
        }) {
            Err(Missing) => missing = Some("wl-paste (from wl-clipboard)"),
            Ok(found) => return found,
        }
    }
    if x11 {
        let base = ["-selection", "clipboard", "-o", "-t"];
        match clipboard_with(run, "xclip", &[&base[..], &["TARGETS"]].concat(), |t| {
            [&base[..], &[t]]
                .concat()
                .into_iter()
                .map(String::from)
                .collect()
        }) {
            Err(Missing) => missing = Some(missing.map_or("xclip", |_| "wl-paste or xclip")),
            Ok(found) => return found,
        }
    }
    Err(match missing {
        Some(tool) => format!(
            "Kerdr reads images from the clipboard with {tool}, which isn't installed on this machine. Paste an image file's path instead."
        ),
        None => "Kerdr can't reach a clipboard here: there's no Wayland or X11 desktop (over SSH, say). Paste an image file's path instead.".into(),
    })
}

struct Missing;

/// Lists the clipboard's types with `list`, then reads the best image type with `read`, or the
/// image files a file manager copied.
fn clipboard_with(
    run: Runner,
    program: &str,
    list: &[&str],
    read: impl Fn(&str) -> Vec<String>,
) -> Result<Result<Found, String>, Missing> {
    let listed = match run(program, list) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Missing),
        Err(e) => {
            return Ok(Err(format!(
                "Couldn't read the clipboard with {program}: {e}."
            )));
        }
        Ok(c) => c,
    };
    let types: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if listed.status != Some(0) || types.is_empty() {
        let why = listed.stderr.trim().to_lowercase();
        if types.is_empty()
            && (why.is_empty()
                || why.contains("nothing is copied")
                || why.contains("no selection")
                || why.contains("not available"))
        {
            return Ok(Err("The clipboard is empty.".into()));
        }
        let detail = listed.stderr.lines().next().unwrap_or("").trim();
        return Ok(Err(format!(
            "Couldn't read the clipboard with {program}{}.",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    let read_type = |t: &str| -> Result<Vec<u8>, String> {
        let args = read(t);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match run(program, &args) {
            Ok(c) if c.status == Some(0) => Ok(c.stdout),
            Ok(c) if c.stdout.len() > MAX_IMAGE_BYTES => Ok(c.stdout),
            Ok(c) => Err(format!(
                "Couldn't read the clipboard's {t} with {program}: {}",
                c.stderr.lines().next().unwrap_or("it failed").trim()
            )),
            Err(e) => Err(format!("Couldn't read the clipboard with {program}: {e}.")),
        }
    };
    if let Some(t) = KINDS.iter().find(|k| types.iter().any(|t| t == *k)) {
        return Ok(read_type(t)
            .and_then(|bytes| checked(bytes, "The clipboard's image").map(Found::Image)));
    }
    if types.iter().any(|t| t == "text/uri-list") {
        let listed = match read_type("text/uri-list") {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(why) => return Ok(Err(why)),
        };
        return Ok(copied_files(&listed));
    }
    if let Some(other) = types.iter().find(|t| t.starts_with("image/")) {
        return Ok(Err(format!(
            "The clipboard's image is {other}; Kerdr attaches PNG, JPEG, GIF and WebP."
        )));
    }
    Ok(Err(NO_IMAGE.into()))
}

/// The image files a file manager copied, from its list of file:// addresses.
fn copied_files(uri_list: &str) -> Result<Found, String> {
    let paths: Vec<PathBuf> = uri_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(local_path)
        .collect();
    let images: Vec<&PathBuf> = paths.iter().filter(|p| image_name(p)).collect();
    if images.is_empty() {
        return Err(if paths.is_empty() {
            NO_IMAGE.into()
        } else {
            "The copied file isn't an image Kerdr attaches (PNG, JPEG, GIF or WebP).".into()
        });
    }
    Ok(Found::Files(images.into_iter().cloned().collect()))
}

/// A private folder for a reader to write the clipboard's image into, and the path it writes:
/// a path nobody has open, since Windows won't let another program write a file Kerdr holds.
#[cfg(any(target_os = "macos", windows))]
fn scratch() -> Result<(tempfile::TempDir, PathBuf), String> {
    let dir = tempfile::Builder::new()
        .prefix("kerdr-clipboard-")
        .tempdir()
        .map_err(|e| format!("Couldn't make a folder to read the clipboard into: {e}."))?;
    let file = dir.path().join("clipboard.png");
    Ok((dir, file))
}

#[cfg(target_os = "macos")]
fn macos_clipboard() -> Result<Found, String> {
    let (_dir, file) = scratch()?;
    let path = file.to_string_lossy().into_owned();
    // The path goes in as an argument, never as script source.
    let script = [
        "on run argv",
        "set f to open for access (POSIX file (item 1 of argv)) with write permission",
        "try",
        "write (the clipboard as «class PNGf») to f",
        "close access f",
        "on error m number n",
        "close access f",
        "error m number n",
        "end try",
        "end run",
    ];
    let mut cmd = Command::new("osascript");
    for line in script {
        cmd.arg("-e").arg(line);
    }
    cmd.arg(&path);
    let got = capture(cmd, 4096, READ_DEADLINE)
        .map_err(|e| format!("Couldn't read the clipboard with osascript: {e}."))?;
    if got.status == Some(0) {
        return from_file(&file).map(Found::Image);
    }
    // A file copied in Finder.
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", "POSIX path of (the clipboard as «class furl»)"]);
    if let Ok(c) = capture(cmd, 64 * 1024, READ_DEADLINE)
        && c.status == Some(0)
    {
        let path = String::from_utf8_lossy(&c.stdout).trim().to_string();
        if !path.is_empty() {
            return copied_files(&format!("file://{}", path.replace('%', "%25")));
        }
    }
    Err(NO_IMAGE.into())
}

#[cfg(windows)]
fn windows_clipboard() -> Result<Found, String> {
    use base64::Engine;
    let (_dir, file) = scratch()?;
    let path = file.to_string_lossy().into_owned();
    // Exit 0: the image is saved; 4: copied files, one per line; 3: text only; 2: nothing; 5: an
    // error, its message on standard error as plain text rather than PowerShell's CLIXML.
    let script = format!(
        "$ProgressPreference = 'SilentlyContinue'; $ErrorActionPreference = 'Stop'; \
         try {{ \
         [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; \
         Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
         if ([System.Windows.Forms.Clipboard]::ContainsImage()) {{ \
           [System.Windows.Forms.Clipboard]::GetImage().Save({}, [System.Drawing.Imaging.ImageFormat]::Png); exit 0 }}; \
         $files = [System.Windows.Forms.Clipboard]::GetFileDropList(); \
         if ($files.Count -gt 0) {{ [Console]::Out.Write(($files -join \"`n\")); exit 4 }}; \
         if ([System.Windows.Forms.Clipboard]::ContainsText()) {{ exit 3 }}; exit 2 \
         }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 5 }}",
        crate::transport::powershell_quote(&path)
    );
    let encoded: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut cmd = Command::new("powershell.exe");
    // The clipboard needs a single-threaded apartment.
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        &base64::engine::general_purpose::STANDARD.encode(encoded),
    ]);
    // PowerShell takes a few seconds to start on a cold machine.
    let got = capture(cmd, 1024 * 1024, READ_DEADLINE * 3)
        .map_err(|e| format!("Couldn't read the clipboard with PowerShell: {e}."))?;
    match got.status {
        Some(0) => from_file(&file).map(Found::Image),
        Some(4) => {
            let listed: String = String::from_utf8_lossy(&got.stdout)
                .lines()
                .map(|l| format!("file:///{}\n", l.trim().replace('%', "%25")))
                .collect();
            copied_files(&listed)
        }
        Some(2) => Err("The clipboard is empty.".into()),
        Some(3) => Err(NO_IMAGE.into()),
        _ => Err(format!(
            "Couldn't read the clipboard with PowerShell: {}",
            got.stderr.lines().next().unwrap_or("it failed").trim()
        )),
    }
}

/// Runs a clipboard reader with nothing on its standard input, keeping at most `cap` bytes of
/// what it prints, and stops it once `deadline` passes. A reader that prints more than `cap`
/// is stopped as soon as it has.
pub fn capture(mut cmd: Command, cap: usize, deadline: Duration) -> std::io::Result<Captured> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut out).take(cap as u64).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let (etx, erx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut err).take(4096).read_to_end(&mut text);
        let _ = etx.send(String::from_utf8_lossy(&text).into_owned());
    });
    let until = Instant::now() + deadline;
    let stdout = match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "it didn't answer in time",
            ));
        }
    };
    if stdout.len() >= cap {
        let _ = child.kill();
    }
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status.code(),
            None if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "it didn't finish in time",
                ));
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let stderr = erx
        .recv_timeout(until.saturating_duration_since(Instant::now()))
        .unwrap_or_default();
    Ok(Captured {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    pub(crate) const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn knows_the_kinds_it_attaches_by_their_bytes() {
        assert_eq!(media_type(PNG), Some("image/png"));
        assert_eq!(media_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(media_type(b"GIF89a...."), Some("image/gif"));
        assert_eq!(media_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(media_type(b"BM...."), None);
        assert_eq!(media_type(b"hello"), None);
        assert_eq!(size(412_300), "413 KB");
        assert_eq!(size(3_400_000), "3.4 MB");
        assert_eq!(size(MAX_IMAGE_BYTES), "10 MB");
        assert_eq!(size(10), "1 KB");
    }

    #[test]
    fn an_oversized_or_unknown_image_is_refused_with_its_limit_named() {
        let mut big = PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        let why = checked(big.clone(), "The clipboard's image").unwrap_err();
        assert_eq!(
            why,
            "The clipboard's image is over 10 MB, the most Kerdr attaches for one image."
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("huge.png");
        big.resize(14_200_000, 0);
        std::fs::write(&file, &big).unwrap();
        assert_eq!(
            from_file(&file).unwrap_err(),
            "huge.png is 14.2 MB; Kerdr attaches images up to 10 MB."
        );
        let why = checked(b"BM....".to_vec(), "shot.bmp").unwrap_err();
        assert!(why.contains("shot.bmp isn't a PNG"), "{why}");
    }

    #[test]
    fn a_pasted_image_path_is_found_however_the_terminal_quoted_it() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my shot.png");
        let plain = dir.path().join("plain.PNG");
        std::fs::write(&spaced, PNG).unwrap();
        std::fs::write(&plain, PNG).unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hi").unwrap();
        let s = spaced.to_str().unwrap();
        let p = plain.to_str().unwrap();
        assert_eq!(paths_in_paste(p), Some(vec![plain.clone()]));
        assert_eq!(
            paths_in_paste(&format!("'{s}'\n")),
            Some(vec![spaced.clone()])
        );
        assert_eq!(
            paths_in_paste(&format!("\"{s}\"")),
            Some(vec![spaced.clone()])
        );
        assert_eq!(paths_in_paste(s), Some(vec![spaced.clone()]));
        if !cfg!(windows) {
            assert_eq!(
                paths_in_paste(&s.replace(' ', "\\ ")),
                Some(vec![spaced.clone()])
            );
            let uri = format!("file://{}", s.replace(' ', "%20"));
            assert_eq!(paths_in_paste(&uri), Some(vec![spaced.clone()]));
        }
        assert_eq!(
            paths_in_paste(&format!("'{s}' '{p}'")),
            Some(vec![spaced.clone(), plain.clone()])
        );
        // Words, a missing file, a file that isn't an image, a relative name: all stay words.
        assert_eq!(paths_in_paste(&format!("look at {p}")), None);
        assert_eq!(paths_in_paste(&format!("{p}.missing.png")), None);
        assert_eq!(paths_in_paste(text.to_str().unwrap()), None);
        assert_eq!(paths_in_paste("plain.png"), None);
        assert_eq!(paths_in_paste("   "), None);
    }

    #[test]
    fn only_images_whose_tags_are_still_in_the_words_are_sent() {
        let image = |tag| Image {
            tag,
            media_type: "image/png",
            bytes: Arc::new(PNG.to_vec()),
        };
        let sent = request_images("see [Image #2] only", &[image(1), image(2)]);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["tag"], 2);
        assert_eq!(sent[0]["type"], "image/png");
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(sent[0]["data"].as_str().unwrap())
                .unwrap(),
            PNG
        );
    }

    #[test]
    fn copied_files_past_the_room_left_are_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..5)
            .map(|n| {
                let p = dir.path().join(format!("{n}.png"));
                std::fs::write(&p, PNG).unwrap();
                p
            })
            .collect();
        // Room for two: the third is named as the limit, and nothing past it is opened.
        let got = from_files(
            &paths,
            Room {
                images: 2,
                bytes: MAX_IMAGES_BYTES,
            },
        );
        assert_eq!(got.images.len(), 2);
        assert_eq!(
            got.why.as_deref(),
            Some("An intent takes at most 20 images.")
        );
        let big = dir.path().join("big.png");
        std::fs::write(&big, [PNG, &vec![0; 2000]].concat()).unwrap();
        let got = from_files(
            &[paths[0].clone(), big],
            Room {
                images: 9,
                bytes: 1000,
            },
        );
        assert_eq!(got.images.len(), 1);
        assert!(got.why.unwrap().contains("past 20 MB together"));
        // A file that can't attach is named, and the rest still come.
        let notes = dir.path().join("notes.png");
        std::fs::write(&notes, "hi").unwrap();
        let got = from_files(&[notes, paths[1].clone()], Room::left(&[]));
        assert_eq!(got.images.len(), 1);
        assert!(got.why.unwrap().contains("notes.png isn't a PNG"));
        assert_eq!(
            from_clipboard(Room {
                images: 0,
                bytes: 1
            })
            .why,
            Some(too_many())
        );
    }

    /// A stand-in for wl-paste or xclip: answers by the arguments it's given.
    fn fake<'a>(
        calls: &'a RefCell<Vec<String>>,
        answer: impl Fn(&str, &[&str]) -> std::io::Result<Captured> + 'a,
    ) -> impl Fn(&str, &[&str]) -> std::io::Result<Captured> + 'a {
        move |program, args| {
            calls
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            answer(program, args)
        }
    }

    fn ok(stdout: &[u8]) -> std::io::Result<Captured> {
        Ok(Captured {
            status: Some(0),
            stdout: stdout.to_vec(),
            stderr: String::new(),
        })
    }

    #[test]
    fn wayland_reads_the_best_image_type_the_clipboard_offers() {
        let calls = RefCell::new(vec![]);
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"text/plain\nimage/jpeg\nimage/png\n")
            } else {
                ok(PNG)
            }
        });
        let Ok(Found::Image(got)) = unix_clipboard(true, false, &run) else {
            panic!("no image")
        };
        assert_eq!(got.media_type, "image/png");
        assert_eq!(
            calls.borrow().as_slice(),
            [
                "wl-paste --list-types",
                "wl-paste --no-newline --type image/png"
            ]
        );
    }

    #[test]
    fn x11_reads_the_clipboard_with_xclip() {
        let calls = RefCell::new(vec![]);
        let run = fake(&calls, |_, args| {
            if args.last() == Some(&"TARGETS") {
                ok(b"TARGETS\nimage/png\nSTRING\n")
            } else {
                ok(PNG)
            }
        });
        let Ok(Found::Image(got)) = unix_clipboard(false, true, &run) else {
            panic!("no image")
        };
        assert_eq!(got.bytes, PNG);
        assert_eq!(
            calls.borrow()[1],
            "xclip -selection clipboard -o -t image/png"
        );
    }

    #[test]
    fn says_in_plain_words_when_there_is_nothing_to_attach() {
        let calls = RefCell::new(vec![]);
        let text_only = fake(&calls, |_, _| {
            ok(b"text/plain;charset=utf-8\nUTF8_STRING\n")
        });
        assert!(
            unix_clipboard(true, false, &text_only)
                .unwrap_err()
                .contains("no image on the clipboard")
        );
        let empty = fake(&calls, |_, _| {
            Ok(Captured {
                status: Some(1),
                stdout: vec![],
                stderr: "Nothing is copied\n".into(),
            })
        });
        assert_eq!(
            unix_clipboard(true, false, &empty).unwrap_err(),
            "The clipboard is empty."
        );
        let bmp = fake(&calls, |_, _| ok(b"image/bmp\n"));
        assert!(
            unix_clipboard(true, false, &bmp)
                .unwrap_err()
                .contains("image/bmp")
        );
        let missing = fake(&calls, |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let why = unix_clipboard(true, false, &missing).unwrap_err();
        assert!(
            why.contains("wl-paste") && why.contains("isn't installed"),
            "{why}"
        );
        let why = unix_clipboard(true, true, &missing).unwrap_err();
        assert!(why.contains("wl-paste or xclip"), "{why}");
        let why = unix_clipboard(false, false, &missing).unwrap_err();
        assert!(why.contains("no Wayland or X11"), "{why}");
    }

    #[test]
    fn a_too_large_clipboard_image_is_refused() {
        let calls = RefCell::new(vec![]);
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"image/png\n")
            } else {
                let mut big = PNG.to_vec();
                big.resize(MAX_IMAGE_BYTES + 1, 0);
                Ok(Captured {
                    status: None,
                    stdout: big,
                    stderr: String::new(),
                })
            }
        });
        let why = unix_clipboard(true, false, &run).unwrap_err();
        assert!(why.contains("over 10 MB"), "{why}");
    }

    #[test]
    fn files_copied_in_a_file_manager_attach_when_they_are_images() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("a shot.png");
        std::fs::write(&shot, PNG).unwrap();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "hi").unwrap();
        let listed = format!("file://{}\r\n", shot.to_str().unwrap().replace(' ', "%20"));
        let calls = RefCell::new(vec![]);
        let run = fake(&calls, move |_, args| {
            if args == ["--list-types"] {
                ok(b"text/uri-list\nx-special/gnome-copied-files\n")
            } else {
                ok(listed.as_bytes())
            }
        });
        if !cfg!(windows) {
            assert_eq!(
                unix_clipboard(true, false, &run),
                Ok(Found::Files(vec![shot.clone()]))
            );
            assert!(
                copied_files(&format!("file://{}", notes.display()))
                    .unwrap_err()
                    .contains("isn't an image")
            );
        }
    }

    /// Reads this machine's real clipboard, which the caller has filled with the image file
    /// KERDR_LIVE_CLIPBOARD_IMAGE names (CI does it on Linux under X11 and Wayland, macOS and
    /// Windows): `cargo test -- --ignored live_clipboard`.
    #[test]
    #[ignore]
    fn live_clipboard() {
        let path = std::env::var("KERDR_LIVE_CLIPBOARD_IMAGE")
            .expect("KERDR_LIVE_CLIPBOARD_IMAGE names the image put on the clipboard");
        let got = from_clipboard(Room::left(&[]));
        assert_eq!(got.why, None);
        let got = got.images;
        assert_eq!(got.len(), 1);
        let want = std::fs::read(&path).unwrap();
        // Windows hands images over as bitmaps, so it gives back a PNG of the same picture.
        if cfg!(windows) {
            assert_eq!(got[0].media_type, "image/png");
        } else {
            assert_eq!(got[0].media_type, media_type(&want).unwrap());
            assert_eq!(got[0].bytes, want);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_reader_that_hangs_or_floods_is_stopped() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 5"]);
        let started = Instant::now();
        let err = capture(cmd, 10, Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes"]);
        let got = capture(cmd, 1000, Duration::from_secs(5)).unwrap();
        assert_eq!(got.stdout.len(), 1000);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf hi; echo oops >&2; exit 3"]);
        let got = capture(cmd, 1000, Duration::from_secs(5)).unwrap();
        assert_eq!((got.status, &got.stdout[..]), (Some(3), &b"hi"[..]));
        assert_eq!(got.stderr.trim(), "oops");
    }
}
