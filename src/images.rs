//! Images in the new-intent box: read from this machine's clipboard, or from the path of an
//! image file pasted or dropped into the box. Each becomes an `[Image #N]` tag in the words, as
//! Claude Code shows them, and travels with the `create` request.
//!
//! What it takes depends on the backend the words go to (intent 0591, release 5): one whose feed
//! lists "fix-input" in `supports` takes any kind of image (saving a PNG copy beside one an agent
//! can't show), as many as fit 20 MB together, each up to 20 MB; past that Kerdr asks before it
//! sends a smaller copy. An older one takes what Kerdr 0.35.0 sent: 20 PNG, JPEG, GIF or WebP
//! images, each up to 10 MB, 20 MB together.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// What a backend's feed lists in `supports` when it takes what release 5 of intent 0591 sends:
/// requests up to 32 MiB, words of any length, images of any kind and number within 20 MB.
pub const FIX_INPUT: &str = "fix-input";

/// The most one image may be, in bytes, for a backend without fix-input.
pub const MAX_IMAGE_BYTES: usize = 10_000_000;
/// The most images one new intent may carry, for a backend without fix-input.
pub const MAX_IMAGES: usize = 20;
/// The most all of one new intent's images may be together, in bytes.
pub const MAX_IMAGES_BYTES: usize = 20_000_000;
/// The most one image may be for a backend with fix-input: all of them may be as much together.
pub const FIX_IMAGE_BYTES: usize = 20_000_000;
/// The most Kerdr reads of one image, or of one paste's images together: past what it sends, so
/// it can offer a smaller copy of one too large, but bounded, so a runaway paste can't fill memory.
pub const MOST_READ: usize = 200_000_000;

/// How long a clipboard reader may take before Kerdr gives up on it.
const READ_DEADLINE: Duration = Duration::from_secs(5);

/// How long a paste may take to have what the clipboard holds in hand: after that, what it holds
/// may not be what he pasted, so nothing from it goes in, and he's told (Codex's review 86, 7).
pub const CLIPBOARD_BOUND: Duration = Duration::from_secs(3);

/// The most reads for the box that run at once, one Kerdr has stopped waiting for counted until it
/// really ends: past it, a paste is refused and said (Codex's review 86, 6).
pub const MOST_READERS: usize = 8;

/// The reads for the box running now, each counted from its start until its thread ends, however
/// long after the board stopped waiting for it that is (Codex's review 86, 6).
#[derive(Clone, Debug, Default)]
pub struct Workers {
    running: Arc<std::sync::atomic::AtomicUsize>,
}

/// One running read, counted until it's dropped, as its thread ends.
struct Running(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Workers {
    pub fn running(&self) -> usize {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Runs `f` on a thread of its own, counted until that thread ends, unless `most` are running
    /// already: then false, and `f` isn't run.
    pub fn start(&self, most: usize, f: impl FnOnce() + Send + 'static) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        let mut now = self.running.load(SeqCst);
        loop {
            if now >= most {
                return false;
            }
            match self.running.compare_exchange(now, now + 1, SeqCst, SeqCst) {
                Ok(_) => break,
                Err(was) => now = was,
            }
        }
        let counted = Running(self.running.clone());
        // A thread that can't be made drops `f`, and its count with it.
        std::thread::Builder::new()
            .name("kerdr-read".into())
            .spawn(move || {
                let _counted = counted;
                f();
            })
            .is_ok()
    }
}

/// Why a paste didn't go in while `running` reads still run (Codex's review 86, 6).
pub fn too_many_reads(running: usize) -> String {
    format!(
        "Kerdr is still waiting on {running} earlier reads that haven't ended (a drive or a clipboard that doesn't answer), so this one didn't go in. Paste it again once they have."
    )
}

/// Why a paste didn't go in when the clipboard didn't answer within CLIPBOARD_BOUND.
pub fn clipboard_late(bound: Duration) -> String {
    format!(
        "The clipboard didn't answer within {} seconds, so nothing from this paste went in: what it holds by now may not be what you pasted. Paste again.",
        bound.as_secs_f32()
    )
}

/// The most a read for the box may take, whatever it waits on (a clipboard program, a mount that
/// doesn't answer), before the board lets it go and says so (Codex's review 85, 8): two clipboard
/// readers' deadlines and some, or PowerShell's on a cold machine; longer for a smaller copy,
/// which decodes and encodes a picture.
pub fn read_most(what: &str) -> Duration {
    if what.starts_with("Making") {
        Duration::from_secs(60)
    } else {
        READ_DEADLINE * 4
    }
}

/// The most a pasted list of files is read for, beyond which the rest is left and named.
const FILES_DEADLINE: Duration = Duration::from_secs(15);

/// The most of a folder's entries looked at for its images, and the most images it gives.
const FOLDER_ENTRIES: usize = 1000;
const FOLDER_IMAGES: usize = 100;

/// The kinds every backend takes and an agent can show, by the type sent.
const KINDS: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// One attached image, numbered by its tag in the words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub tag: u32,
    pub media_type: &'static str,
    pub bytes: Arc<Vec<u8>>,
}

/// An image's bytes and type, before it has a tag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
    /// Where it came from, for messages: "shot.png", "The clipboard's image".
    pub from: String,
}

/// Where the box's next images come from: this machine's clipboard, files pasted by path, or
/// smaller copies of images too large to send, made once he said so, their tags going where the
/// words ended when they were pasted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Clipboard,
    Files(Vec<PathBuf>),
    Smaller(Vec<Loaded>, usize),
}

impl Source {
    /// Reads them, in the room a draft has left. Slow readers (PowerShell, a network mount)
    /// run this away from the board's own thread.
    pub fn load(&self, room: Room) -> Loading {
        match self {
            Source::Clipboard => from_clipboard(room),
            Source::Files(paths) => from_files(paths, room),
            Source::Smaller(images, _) => smaller_copies(images, room),
        }
    }
}

/// What a clipboard reader found: one image, files a file manager copied (not read yet), or
/// text, which goes into the words as any paste would (intent 0591, K3-61).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Image(Loaded),
    /// A picture Windows held as a bitmap, as BMP bytes: made a PNG once it's in hand.
    Bitmap(Vec<u8>),
    Files(Vec<PathBuf>),
    Text(String),
}

/// The images read for the box, text to go into the words with them, and why nothing more was:
/// an empty clipboard, a refused image, a limit reached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loading {
    pub images: Vec<Loaded>,
    pub text: Option<String>,
    /// What to say of what was read, beside each image's own line: a folder's images named, a
    /// smaller copy made.
    pub notes: Vec<String>,
    pub why: Option<String>,
}

impl Loading {
    pub fn refused(why: impl Into<String>) -> Loading {
        Loading {
            why: Some(why.into()),
            ..Loading::default()
        }
    }

    fn say(&mut self, why: String) {
        self.why = Some(match self.why.take() {
            Some(before) => format!("{before} {why}"),
            None => why,
        });
    }

    fn add_text(&mut self, text: String) {
        match &mut self.text {
            Some(t) => {
                t.push_str("\n\n");
                t.push_str(&text);
            }
            None => self.text = Some(text),
        }
    }
}

/// How many more images, and bytes of them, a draft can take, and whether its backend fixes
/// input (FIX_INPUT): then there's no count, each image may be as much as all of them together,
/// and one too large is still read, so Kerdr can ask about a smaller copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub images: usize,
    pub bytes: usize,
    pub fix: bool,
}

impl Room {
    /// The room an older backend leaves: 0.35.0's limits.
    pub fn left(images: &[Image]) -> Room {
        Room::for_backend(images, false)
    }

    /// The room left for a draft whose backend fixes input or not.
    pub fn for_backend(images: &[Image], fix: bool) -> Room {
        let used: usize = images.iter().map(|i| i.bytes.len()).sum();
        Room {
            images: if fix {
                usize::MAX
            } else {
                MAX_IMAGES.saturating_sub(images.len())
            },
            bytes: MAX_IMAGES_BYTES.saturating_sub(used),
            fix,
        }
    }

    /// The most one image may be.
    pub fn per_image(&self) -> usize {
        if self.fix {
            FIX_IMAGE_BYTES
        } else {
            MAX_IMAGE_BYTES
        }
    }

    /// The most read of one image: just past what's sent for an older backend, so one too large
    /// is named as over it; MOST_READ for one that fixes input.
    fn read_cap(&self) -> usize {
        if self.fix { MOST_READ } else { MAX_IMAGE_BYTES }
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
/// it's read, so a folder's worth copied at once never fills memory. A folder among them gives
/// the image files directly inside it, each named (intent 0591, K3-60). For a backend that fixes
/// input, a file copied in a file manager that isn't named as an image is read for what its bytes
/// are: an image attaches, a text file's words go into the words (K3-58), anything else is named.
pub fn from_files(paths: &[PathBuf], room: Room) -> Loading {
    let mut out = Loading::default();
    let paths = expand_folders(paths, room.fix, &mut out);
    let mut used = 0;
    let mut read = 0;
    let until = Instant::now() + FILES_DEADLINE;
    for path in &paths {
        // Reading has gone on too long (a mount that's slow to answer): the rest is named, left.
        if Instant::now() >= until {
            out.say(format!(
                "Kerdr stopped at {}: reading the files took longer than {} seconds.",
                name_of(path),
                FILES_DEADLINE.as_secs()
            ));
            break;
        }
        if !room.fix && out.images.len() >= room.images {
            out.say(too_many());
            break;
        }
        let len = std::fs::metadata(path).map_or(0, |m| m.len() as usize);
        if !room.fix && len <= MAX_IMAGE_BYTES && used + len > room.bytes {
            out.say(too_much());
            break;
        }
        // Read at most MOST_READ in all: past that, the rest is named and left.
        if room.fix && read > 0 && read + len.min(room.read_cap()) > MOST_READ {
            out.say(format!(
                "Kerdr stopped at {}: what was pasted is past the {} it reads at once.",
                name_of(path),
                size(MOST_READ)
            ));
            break;
        }
        match from_file_any(path, room) {
            // A file that grew after its size was checked is held to the room by what was read.
            Ok(Got::Image(image)) if !room.fix && used + image.bytes.len() > room.bytes => {
                out.say(too_much());
                break;
            }
            Ok(Got::Image(image)) => {
                used += image.bytes.len();
                read += image.bytes.len();
                out.images.push(image);
            }
            Ok(Got::Text(text)) => {
                read += text.len();
                out.add_text(text);
            }
            Err(why) => out.say(why),
        }
    }
    out
}

/// What one pasted or copied file gave.
enum Got {
    Image(Loaded),
    Text(String),
}

/// The paths, each folder among them replaced by the image files directly inside it (of the kinds
/// its backend takes by their names, `any` for every kind), in name order, and said: what a
/// folder sends is never more than he sees named (K3-60).
fn expand_folders(paths: &[PathBuf], any: bool, out: &mut Loading) -> Vec<PathBuf> {
    let mut all = vec![];
    for path in paths {
        if !std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
            all.push(path.clone());
            continue;
        }
        // Bounded: a folder of thousands is never listed whole before its images are chosen. One
        // cut short is a read that didn't get everything, so words waiting to be sent ask before
        // they go without the rest (Codex's review 86, 8).
        let (mut inside, mut cut) = (vec![], false);
        if let Ok(entries) = std::fs::read_dir(path) {
            for (n, entry) in entries.flatten().enumerate() {
                if n >= FOLDER_ENTRIES {
                    cut = true;
                    break;
                }
                let p = entry.path();
                if image_name(&p, any) && p.is_file() {
                    if inside.len() >= FOLDER_IMAGES {
                        cut = true;
                        break;
                    }
                    inside.push(p);
                }
            }
        }
        inside.sort();
        let folder = name_of(path);
        if cut {
            out.say(format!(
                "{folder} holds more than Kerdr looks through at once ({FOLDER_ENTRIES} entries, {FOLDER_IMAGES} images), so only the first it found were read, and the rest didn't go in."
            ));
        }
        if inside.is_empty() {
            out.say(format!(
                "{folder} is a folder with no image files directly in it."
            ));
            continue;
        }
        let names: Vec<String> = inside.iter().map(|p| name_of(p)).collect();
        out.notes
            .push(format!("From the folder {folder}: {}.", and_list(&names)));
        all.extend(inside);
    }
    all
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The tag that stands for image `n` in the words.
pub fn tag(n: u32) -> String {
    format!("[Image #{n}]")
}

/// The image type its first bytes say it is, for any kind of image Kerdr knows by its bytes
/// (intent 0591, K3-56): PNG, JPEG, GIF and WebP, which every backend takes, and BMP, TIFF,
/// HEIC, HEIF, AVIF, ICO, JPEG XL, JPEG 2000, PSD, QOI, ICNS and DDS, the kinds a backend that
/// fixes input knows by their bytes too, which only it takes. None for bytes that are no image
/// Kerdr knows.
pub fn kind_of(bytes: &[u8]) -> Option<&'static str> {
    let at = |range: std::ops::Range<usize>| bytes.get(range);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if at(0..4) == Some(b"RIFF") && at(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") && bytes.len() >= 26 && at(6..10) == Some(&[0, 0, 0, 0]) {
        Some("image/bmp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if at(4..8) == Some(b"ftyp") {
        match at(8..12)? {
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" => Some("image/heic"),
            b"mif1" | b"msf1" => Some("image/heif"),
            b"avif" | b"avis" => Some("image/avif"),
            _ => None,
        }
    } else if bytes.starts_with(&[0, 0, 1, 0]) && bytes.len() >= 22 && at(4..6) != Some(&[0, 0]) {
        Some("image/x-icon")
    } else if bytes.starts_with(&[0xff, 0x0a]) || bytes.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        Some("image/jxl")
    } else if bytes.starts_with(b"\0\0\0\x0cjP  \r\n\x87\n")
        || bytes.starts_with(&[0xff, 0x4f, 0xff, 0x51])
    {
        Some("image/jp2")
    } else if bytes.starts_with(b"8BPS") {
        Some("image/vnd.adobe.photoshop")
    } else if bytes.starts_with(b"qoif") {
        Some("image/qoi")
    } else if bytes.starts_with(b"icns") {
        Some("image/icns")
    } else if bytes.starts_with(b"DDS ") {
        Some("image/vnd-ms.dds")
    } else {
        None
    }
}

/// Whether every backend takes this kind and an agent can show it.
pub fn shown(media_type: &str) -> bool {
    KINDS.contains(&media_type)
}

/// The image type its first bytes say it is, of the kinds every backend takes; None for any other.
pub fn media_type(bytes: &[u8]) -> Option<&'static str> {
    kind_of(bytes).filter(|k| shown(k))
}

/// A type's short name for messages: PNG, JPEG, GIF, WebP, BMP and so on.
pub fn kind_name(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/gif" => "GIF",
        "image/webp" => "WebP",
        "image/bmp" => "BMP",
        "image/tiff" => "TIFF",
        "image/heic" => "HEIC",
        "image/heif" => "HEIF",
        "image/avif" => "AVIF",
        "image/x-icon" => "ICO",
        "image/jxl" => "JPEG XL",
        "image/jp2" => "JPEG 2000",
        "image/vnd.adobe.photoshop" => "PSD",
        "image/qoi" => "QOI",
        "image/icns" => "ICNS",
        "image/vnd-ms.dds" => "DDS",
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

/// Checks bytes read from somewhere as one image Kerdr can attach in `room`. A reader stops just
/// past the limit, so an image that's too large is named as over it rather than by its size. For
/// a backend that fixes input, any kind of image is taken, and one past what may be sent is still
/// taken, to be asked about; only bytes that are no image at all are refused.
pub fn checked(bytes: Vec<u8>, from: &str, room: Room) -> Result<Loaded, String> {
    if bytes.len() > room.read_cap() {
        return Err(too_big(from, room));
    }
    match kind_of(&bytes) {
        Some(media_type) if room.fix || shown(media_type) => Ok(Loaded {
            media_type,
            bytes,
            from: from.to_string(),
        }),
        _ if !room.fix => Err(format!(
            "{from} isn't a PNG, JPEG, GIF or WebP image, the kinds Kerdr attaches."
        )),
        _ => Err(format!("{from} isn't an image.")),
    }
}

/// Why an image past what Kerdr reads for one didn't go in.
fn too_big(from: &str, room: Room) -> String {
    if room.fix {
        format!(
            "{from} is over {}, more than Kerdr reads for one image.",
            size(MOST_READ)
        )
    } else {
        format!(
            "{from} is over {}, the most Kerdr attaches for one image.",
            size(MAX_IMAGE_BYTES)
        )
    }
}

/// An image file, read and checked.
pub fn from_file(path: &Path, room: Room) -> Result<Loaded, String> {
    match from_file_any(path, room) {
        Ok(Got::Image(image)) => Ok(image),
        Ok(Got::Text(_)) => Err(format!("{} isn't an image.", name_of(path))),
        Err(why) => Err(why),
    }
}

/// The most of a text file whose words go into a new intent's words (K3-58).
const MOST_TEXT_FILE: usize = 256 * 1024;

/// A file, read and checked: an image, or for a backend that fixes input a text file's words
/// (indented, under its name), from a file whose name doesn't say it's an image.
fn from_file_any(path: &Path, room: Room) -> Result<Got, String> {
    let name = name_of(path);
    let meta = std::fs::metadata(path).map_err(|e| format!("Couldn't read {name}: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{name} isn't a file."));
    }
    let len = meta.len() as usize;
    let named_image = image_name(path, room.fix);
    if len > room.read_cap() && (named_image || !room.fix) {
        return Err(if room.fix {
            format!(
                "{name} is {}, more than Kerdr reads for one image.",
                size(len)
            )
        } else {
            format!(
                "{name} is {}; Kerdr attaches images up to {}.",
                size(len),
                size(MAX_IMAGE_BYTES)
            )
        });
    }
    let cap = if named_image || !room.fix {
        room.read_cap()
    } else {
        room.read_cap().max(MOST_TEXT_FILE)
    };
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(cap as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("Couldn't read {name}: {e}."))?;
    if named_image || !room.fix || kind_of(&bytes).is_some() {
        return checked(bytes, &name, room).map(Got::Image);
    }
    text_file(&name, &bytes).map(Got::Text)
}

/// A text file's words as they go into a new intent's words: under "File <name>:", each line
/// indented four spaces (K3-58). No line of it can then read as a heading or a fence: the backend
/// takes a line like one of the intent's own headings ("## Constraints") for that section even
/// inside a code block, so a fence wouldn't keep it apart (r5-b, the lead in round 3). Bytes that
/// aren't text are named instead.
fn text_file(name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(b"%PDF-") {
        return Err(format!(
            "{name} is a PDF; Kerdr attaches images and text files, so it wasn't attached."
        ));
    }
    if bytes.len() > MOST_TEXT_FILE {
        return Err(format!(
            "{name} is over {}, the most of a text file Kerdr puts in your words.",
            size(MOST_TEXT_FILE)
        ));
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(t) if !t.contains('\0') => t,
        _ => {
            return Err(format!(
                "{name} isn't an image or a text file, so Kerdr couldn't attach it."
            ));
        }
    };
    let lines: Vec<String> = text
        .trim_end_matches(['\n', '\r'])
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect();
    Ok(format!("File {name}:\n{}", lines.join("\n")))
}

/// Whether a file's name says it's an image: PNG, JPEG, GIF or WebP, and with `any`, every other
/// kind Kerdr knows (BMP, TIFF, HEIC, HEIF, AVIF, ICO, JPEG XL).
fn image_name(path: &Path, any: bool) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
            || (any
                && matches!(
                    e.as_str(),
                    "bmp"
                        | "tif"
                        | "tiff"
                        | "heic"
                        | "heif"
                        | "avif"
                        | "ico"
                        | "jxl"
                        | "jp2"
                        | "j2k"
                        | "psd"
                        | "qoi"
                        | "icns"
                        | "dds"
                ))
    })
}

/// The image files a paste names, when it is nothing but one or more paths to them (as a
/// terminal pastes a dropped file: quoted, backslash-escaped, or as a file:// address).
/// None when it is anything else, so it goes in as words. It reads the text only, never the
/// disk, so a path on a mount that doesn't answer can't stop the board; reading the files
/// says whether they're there. PNG, JPEG, GIF and WebP names only, as an older backend takes.
pub fn paths_in_paste(text: &str) -> Option<Vec<PathBuf>> {
    paths_in_paste_for(text, false)
}

/// As paths_in_paste, with `any` every kind of image name a backend that fixes input takes.
pub fn paths_in_paste_for(text: &str, any: bool) -> Option<Vec<PathBuf>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let all = |words: Vec<String>| -> Option<Vec<PathBuf>> {
        let paths: Vec<PathBuf> = words.iter().filter_map(|w| local_path(w)).collect();
        (!paths.is_empty()
            && paths.len() == words.len()
            && paths.iter().all(|p| image_name(p, any)))
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

// --- Smaller copies --------------------------------------------------------------------------

/// The longest side a smaller copy starts from, before it's made smaller still to fit.
const SMALLER_START: u32 = 8192;

/// A copy of `image` that fits in `most` bytes, made only when he said so (intent 0591, release
/// 5): a JPEG (or a PNG, for a picture with see-through parts) the same size or smaller, scaled
/// down step by step until it fits. The image itself is never touched. Why one couldn't be made.
pub fn smaller(image: &Loaded, most: usize) -> Result<Loaded, String> {
    let kind = kind_name(image.media_type);
    let picture = image::load_from_memory(&image.bytes).map_err(|_| {
        format!(
            "Kerdr couldn't make a smaller copy of {}: it can't read a {kind} image's picture.",
            image.from
        )
    })?;
    let see_through =
        picture.color().has_alpha() && picture.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let mut picture = if picture.width().max(picture.height()) > SMALLER_START {
        picture.resize(
            SMALLER_START,
            SMALLER_START,
            image::imageops::FilterType::Triangle,
        )
    } else {
        picture
    };
    for _ in 0..16 {
        let mut bytes = Vec::new();
        let mut out = std::io::Cursor::new(&mut bytes);
        let made = if see_through {
            picture.write_to(&mut out, image::ImageFormat::Png)
        } else {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
                .encode_image(&picture.to_rgb8())
        };
        made.map_err(|e| format!("Kerdr couldn't make a smaller copy of {}: {e}.", image.from))?;
        if bytes.len() <= most {
            return Ok(Loaded {
                media_type: if see_through {
                    "image/png"
                } else {
                    "image/jpeg"
                },
                bytes,
                from: image.from.clone(),
            });
        }
        let (w, h) = (picture.width(), picture.height());
        if w.max(h) <= 64 {
            break;
        }
        // Each step about two thirds the bytes of the last.
        picture = picture.resize(
            (w * 4 / 5).max(1),
            (h * 4 / 5).max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    Err(format!(
        "Kerdr couldn't make a copy of {} small enough to send.",
        image.from
    ))
}

/// Smaller copies of images too large to send, sharing the room left: each gets its share, by
/// how large it is, so together they fit.
fn smaller_copies(images: &[Loaded], room: Room) -> Loading {
    let mut out = Loading::default();
    let total: usize = images.iter().map(|i| i.bytes.len()).sum::<usize>().max(1);
    let left = room.bytes.min(MAX_IMAGES_BYTES);
    for image in images {
        let share = (left as f64 * image.bytes.len() as f64 / total as f64) as usize;
        let most = share.min(room.per_image());
        match smaller(image, most) {
            Ok(copy) => {
                out.notes.push(format!(
                    "{} went in as a smaller copy: a {} image of {}, made from the {} {}.",
                    copy.from,
                    kind_name(copy.media_type),
                    size(copy.bytes.len()),
                    size(image.bytes.len()),
                    kind_name(image.media_type)
                ));
                out.images.push(copy);
            }
            Err(why) => out.say(why),
        }
    }
    out
}

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
/// one image, or the files copied in a file manager, or text for the words, or plain words
/// saying why there's nothing to attach.
pub fn from_clipboard(room: Room) -> Loading {
    loading_of(take_clipboard(room), room)
}

/// What the clipboard holds, in hand: an image's bytes, the list of files a file manager copied,
/// or text. Nothing it offers is read later (Codex's review 86, 7).
pub fn take_clipboard(room: Room) -> Result<Found, String> {
    take_pasted(room, None)
}

/// The clipboard taken for a paste whose copy Kerdr's watch numbered as the paste was handled
/// (`pasted`): nothing goes in if he's copied anything since, at any point of the read, a reader
/// setting up included (Codex's reviews 88, 1, and 89). With no number (no watch here, or not set
/// up yet), each of Kerdr's own readers watches from its own start, as before.
pub fn take_pasted(room: Room, pasted: Option<Pasted>) -> Result<Found, String> {
    if room.images == 0 {
        return Err(too_many());
    }
    #[cfg(target_os = "macos")]
    let found = {
        let _ = pasted;
        macos_clipboard(room)
    };
    // Windows' number is checked once the clipboard is open, which nothing else can change then.
    #[cfg(windows)]
    let found = windows_clipboard(room, pasted.map(|p| p.number()));
    #[cfg(not(any(target_os = "macos", windows)))]
    let found = take_since(pasted.clone(), || {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let (wayland, x11) = (set("WAYLAND_DISPLAY"), set("DISPLAY"));
        let run = |program: &str, args: &[&str]| {
            let mut cmd = Command::new(program);
            cmd.args(args);
            capture(cmd, room.read_cap() + 1, READ_DEADLINE)
        };
        let readers = builtin::readers(wayland, x11, pasted.is_none());
        clipboard_here(wayland, x11, &readers, &run, room)
    });
    found
}

/// What `read` took for a paste, unless the watch has numbered another copy since the paste:
/// then nothing, and he's told. A watch that couldn't be brought up to date in time can't say, so
/// nothing goes in then either, and the next paste is read as one with no watch. Without a
/// number, as `read` took it.
pub fn take_since(
    pasted: Option<Pasted>,
    read: impl FnOnce() -> Result<Found, String>,
) -> Result<Found, String> {
    let found = read();
    match pasted.map(|p| p.changed()) {
        Some(Some(true)) => Err(COPIED_AGAIN.into()),
        Some(None) => Err(UNCHECKED.into()),
        _ => found,
    }
}

pub mod watch;
pub use watch::{ClipboardWatch, Pasted};

/// What was taken from the clipboard, read for the box: a copied file list's files read, a
/// bitmap made a PNG.
pub fn loading_of(found: Result<Found, String>, room: Room) -> Loading {
    match found {
        Ok(Found::Image(image)) => Loading {
            images: vec![image],
            ..Loading::default()
        },
        Ok(Found::Bitmap(bmp)) => {
            match png_of_bitmap(&bmp).and_then(|png| checked(png, "The clipboard's image", room)) {
                Ok(image) => Loading {
                    images: vec![image],
                    ..Loading::default()
                },
                Err(why) => Loading::refused(why),
            }
        }
        Ok(Found::Files(paths)) => from_files(&paths, room),
        Ok(Found::Text(text)) => Loading {
            text: Some(text),
            ..Loading::default()
        },
        Err(why) => Loading::refused(why),
    }
}

/// A bitmap's BMP bytes as a PNG, as Kerdr sent a Windows clipboard's picture before.
fn png_of_bitmap(bmp: &[u8]) -> Result<Vec<u8>, String> {
    let picture = image::load_from_memory_with_format(bmp, image::ImageFormat::Bmp)
        .map_err(|e| format!("Couldn't read the clipboard's picture: {e}."))?;
    let mut out = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("Couldn't make the clipboard's picture a PNG: {e}."))?;
    Ok(out.into_inner())
}

const NO_IMAGE: &str = "There's no image on the clipboard. Copy a screenshot or an image first, or paste an image file's path.";

/// The most text a clipboard read takes: past what the box holds, which keeps the rest for c, and
/// read whole, never cut, so no character is split (Codex's review 85, 5). Past this, nothing goes
/// in, and it's still on the clipboard.
const MOST_CLIPBOARD_TEXT: usize = 8_000_000;

/// Why copied text past MOST_CLIPBOARD_TEXT didn't go in.
fn too_much_text() -> String {
    format!(
        "The copied text is over {}, more than Kerdr reads at once, so none of it went in; it's still on your clipboard.",
        size(MOST_CLIPBOARD_TEXT)
    )
}

/// What a clipboard offers, as one way of reading it says.
pub enum Listing {
    /// Its types, best first as the clipboard lists them.
    Types(Vec<String>),
    /// Nothing is copied.
    Empty,
    /// This way can't be used here: the program isn't installed, or the desktop doesn't let a
    /// program read the clipboard this way. Why, in words.
    Missing(String),
    /// It was tried and failed: why.
    Failed(String),
}

/// How a way of reading the clipboard reads one type it offers, at most the given many bytes.
pub type ReadType<'a> = Box<dyn Fn(&str, usize) -> Result<Vec<u8>, String> + 'a>;

/// One way of reading the clipboard: Kerdr's own, or a program.
pub struct Reader<'a> {
    /// Its name in messages: "wl-paste", "Kerdr's own Wayland reader".
    pub name: String,
    pub list: Box<dyn Fn() -> Listing + 'a>,
    /// The bytes of one type, at most the given many (a reader stops just past).
    pub read: ReadType<'a>,
    /// After a read: whether he copied something else since `list` (Some(true)), or not
    /// (Some(false)), by what the desktop gives to tell one copy from the next; None for a way
    /// that can't tell, a program (Codex's review 88, 1).
    pub changed: Box<dyn Fn() -> Option<bool> + 'a>,
}

/// Why nothing went in when he copied something else while the clipboard was read.
pub const COPIED_AGAIN: &str =
    "You copied something else while Kerdr read the clipboard; paste again.";

/// Why nothing went in when Kerdr's watch couldn't say whether he had (Codex's review 90, 2).
pub const UNCHECKED: &str = "Kerdr couldn't check whether you copied something else while it read the clipboard; paste again.";

/// Linux and the BSDs, with programs only: wl-paste under Wayland, xclip under X11.
pub fn unix_clipboard(wayland: bool, x11: bool, run: Runner) -> Result<Found, String> {
    clipboard_here(wayland, x11, &[], run, Room::left(&[]))
}

/// Linux and the BSDs: Kerdr's own readers first (`builtin`, intent 0591, K3-63), then wl-paste
/// under Wayland and xclip under X11, each tried while the ones before it can't be used here.
pub fn clipboard_here(
    wayland: bool,
    x11: bool,
    builtin: &[Reader],
    run: Runner,
    room: Room,
) -> Result<Found, String> {
    let mut readers: Vec<&Reader> = builtin.iter().collect();
    let programs = program_readers(wayland, x11, run);
    readers.extend(programs.iter());
    let mut missing = vec![];
    for reader in readers {
        match (reader.list)() {
            Listing::Missing(why) => missing.push((reader.name.clone(), why)),
            Listing::Empty => return Err("The clipboard is empty.".into()),
            Listing::Failed(why) => return Err(why),
            Listing::Types(types) => {
                let found = clipboard_from(reader, &types, room);
                // What was read may be the next copy's, or a mix: nothing goes in.
                if (reader.changed)() == Some(true) {
                    return Err(COPIED_AGAIN.into());
                }
                return found;
            }
        }
    }
    Err(none_reads(wayland, x11, &missing))
}

/// Why no way of reading the clipboard could be used, naming what would let Kerdr read it.
fn none_reads(wayland: bool, x11: bool, missing: &[(String, String)]) -> String {
    if !wayland && !x11 {
        return "Kerdr can't reach a clipboard here: there's no Wayland or X11 desktop (over SSH, say). Paste an image file's path instead.".into();
    }
    let programs: Vec<&str> = missing
        .iter()
        .filter(|(name, _)| !name.starts_with("Kerdr"))
        .map(|(name, _)| name.as_str())
        .collect();
    let own: Vec<String> = missing
        .iter()
        .filter(|(name, _)| name.starts_with("Kerdr"))
        .map(|(_, why)| why.clone())
        .collect();
    let package = if wayland { "wl-clipboard" } else { "xclip" };
    let carry = if omarchy() {
        ", which omarchy-config should carry"
    } else {
        ""
    };
    let tool = match programs.as_slice() {
        [] => String::new(),
        [one] => format!("{one} isn't installed"),
        _ => "neither wl-paste nor xclip is installed".into(),
    };
    let own = match own.as_slice() {
        [] => String::new(),
        _ => format!("Kerdr couldn't read it itself ({})", own.join("; ")),
    };
    let why = [own, tool]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", and ");
    format!(
        "Kerdr couldn't read the clipboard here: {why}. Installing {package} would let it{carry}. Paste an image file's path instead."
    )
}

/// Whether this is one of Omarchy's desktops, whose packages omarchy-config carries.
fn omarchy() -> bool {
    std::env::var_os("OMARCHY_PATH").is_some()
        || crate::config::home().join(".local/share/omarchy").is_dir()
}

/// wl-paste and xclip as ways of reading the clipboard, run by `run`.
fn program_readers<'a>(wayland: bool, x11: bool, run: Runner<'a>) -> Vec<Reader<'a>> {
    let mut out = vec![];
    if wayland {
        out.push(program_reader(
            run,
            "wl-paste",
            "wl-paste (from wl-clipboard)",
            vec!["--list-types".into()],
            |t| vec!["--no-newline".into(), "--type".into(), t.into()],
        ));
    }
    if x11 {
        let base = ["-selection", "clipboard", "-o", "-t"].map(String::from);
        let list = [&base[..], &["TARGETS".to_string()]].concat();
        out.push(program_reader(run, "xclip", "xclip", list, move |t| {
            [&base[..], &[t.to_string()]].concat()
        }));
    }
    out
}

fn program_reader<'a>(
    run: Runner<'a>,
    program: &'static str,
    named: &'static str,
    list: Vec<String>,
    read: impl Fn(&str) -> Vec<String> + 'a,
) -> Reader<'a> {
    Reader {
        name: named.into(),
        list: Box::new(move || {
            let args: Vec<&str> = list.iter().map(String::as_str).collect();
            let listed = match run(program, &args) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Listing::Missing("not installed".into());
                }
                Err(e) => {
                    return Listing::Failed(format!(
                        "Couldn't read the clipboard with {program}: {e}."
                    ));
                }
                Ok(c) => c,
            };
            let types: Vec<String> = String::from_utf8_lossy(&listed.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if listed.status == Some(0) && !types.is_empty() {
                return Listing::Types(types);
            }
            let why = listed.stderr.trim().to_lowercase();
            if types.is_empty()
                && (why.is_empty()
                    || why.contains("nothing is copied")
                    || why.contains("no selection")
                    || why.contains("not available"))
            {
                return Listing::Empty;
            }
            let detail = listed.stderr.lines().next().unwrap_or("").trim();
            Listing::Failed(format!(
                "Couldn't read the clipboard with {program}{}.",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ))
        }),
        read: Box::new(move |t, cap| {
            let args = read(t);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match run(program, &args) {
                Ok(c) if c.status == Some(0) => Ok(c.stdout),
                Ok(c) if c.stdout.len() > cap => Ok(c.stdout),
                Ok(c) => Err(format!(
                    "Couldn't read the clipboard's {t} with {program}: {}",
                    c.stderr.lines().next().unwrap_or("it failed").trim()
                )),
                Err(e) => Err(format!("Couldn't read the clipboard with {program}: {e}.")),
            }
        }),
        // A program reads it afresh each time: nothing tells one copy from the next.
        changed: Box::new(|| None),
    }
}

/// The clipboard's text types, best first.
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

/// What the clipboard holds, read with `reader`, whose list it offers is `types`: the best image
/// type, else the files a file manager copied, else its text (intent 0591, K3-61). For a backend
/// that fixes input, an image of another kind is read too (K3-57).
fn clipboard_from(reader: &Reader, types: &[String], room: Room) -> Result<Found, String> {
    let has = |t: &str| types.iter().any(|o| o == t);
    let image = KINDS.iter().copied().find(|k| has(k)).or_else(|| {
        room.fix
            .then(|| {
                types
                    .iter()
                    .find(|t| t.starts_with("image/") && !t.contains("svg"))
                    .map(String::as_str)
            })
            .flatten()
    });
    if let Some(t) = image {
        return (reader.read)(t, room.read_cap())
            .and_then(|bytes| checked(bytes, "The clipboard's image", room).map(Found::Image));
    }
    if has("text/uri-list") {
        let listed =
            String::from_utf8_lossy(&(reader.read)("text/uri-list", 64 * 1024)?).into_owned();
        return copied_files(&listed, room.fix);
    }
    // An image this backend can't take: said, rather than its text pasted in its place.
    if let Some(other) = types.iter().find(|t| t.starts_with("image/")) {
        return Err(if room.fix {
            format!(
                "The clipboard's image is {other}, a kind Kerdr can't read; paste its file's path instead."
            )
        } else {
            format!("The clipboard's image is {other}; Kerdr attaches PNG, JPEG, GIF and WebP.")
        });
    }
    if let Some(t) = TEXT_TYPES.iter().find(|t| has(t)) {
        let bytes = (reader.read)(t, MOST_CLIPBOARD_TEXT)?;
        if bytes.len() > MOST_CLIPBOARD_TEXT {
            return Err(too_much_text());
        }
        let text = String::from_utf8_lossy(&bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if !text.is_empty() {
            return Ok(Found::Text(text));
        }
    }
    Err(NO_IMAGE.into())
}

/// The files a file manager copied, from its list of file:// addresses: the image files, or for
/// a backend that fixes input every file and folder, each read for what it is (K3-58, K3-60).
fn copied_files(uri_list: &str, any: bool) -> Result<Found, String> {
    let paths: Vec<PathBuf> = uri_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(local_path)
        .collect();
    if any && !paths.is_empty() {
        return Ok(Found::Files(paths));
    }
    let images: Vec<&PathBuf> = paths.iter().filter(|p| image_name(p, false)).collect();
    if images.is_empty() {
        return Err(if paths.is_empty() {
            NO_IMAGE.into()
        } else {
            "The copied file isn't an image Kerdr attaches (PNG, JPEG, GIF or WebP).".into()
        });
    }
    Ok(Found::Files(images.into_iter().cloned().collect()))
}

/// Kerdr's own ways of reading the clipboard, with no program to run (intent 0591, K3-63): the
/// Wayland data-control protocol, and X11's selections. Each runs on the paste's own read, which
/// the board waits for only CLIPBOARD_BOUND and counts until it ends (Codex's review 86, 6 and
/// 7), so a desktop that doesn't answer can't stop the board, nor leave threads uncounted.
#[cfg(not(any(target_os = "macos", windows)))]
mod builtin {
    use super::{Listing, READ_DEADLINE, Reader};
    use std::io::Read;

    /// Kerdr's own readers; with `own_watch`, each watches the clipboard from its own start, for a
    /// paste Kerdr's watch didn't number.
    pub fn readers(wayland: bool, x11: bool, own_watch: bool) -> Vec<Reader<'static>> {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut out = vec![];
        if wayland {
            // The offer listed is watched from the moment it's listed: any other since is a new
            // copy (Codex's review 88, 1).
            let watch: Rc<RefCell<Option<wl_clipboard_rs::watch::Watcher>>> = Rc::default();
            let (listing, checking) = (watch.clone(), watch);
            out.push(Reader {
                name: "Kerdr's own Wayland reader".into(),
                list: Box::new(move || wayland_list(&mut listing.borrow_mut(), own_watch)),
                read: Box::new(wayland_read),
                changed: Box::new(move || wayland_changed(checking.borrow_mut().take())),
            });
        }
        if x11 {
            // The selection's owner, and every change of it XFixes reports, from the moment it's
            // listed (Codex's review 88, 1).
            let watch: Rc<RefCell<Option<X11Watch>>> = Rc::default();
            let (listing, checking) = (watch.clone(), watch);
            out.push(Reader {
                name: "Kerdr's own X11 reader".into(),
                list: Box::new(move || {
                    if own_watch {
                        *listing.borrow_mut() = X11Watch::start();
                    }
                    x11_list()
                }),
                read: Box::new(x11_read),
                changed: Box::new(move || checking.borrow().as_ref().and_then(X11Watch::changed)),
            });
        }
        out
    }

    /// What the clipboard offers now, by a watch kept in `watch` from this moment: the offer's
    /// types, as the desktop gave them with the offer.
    fn wayland_list(watch: &mut Option<wl_clipboard_rs::watch::Watcher>, keep: bool) -> Listing {
        use wl_clipboard_rs::paste::{Error, Seat};
        use wl_clipboard_rs::watch::{self, ClipboardEvent, Watcher};
        let listed = Watcher::new(watch::ClipboardType::Regular, Seat::Unspecified).and_then(
            |mut watcher| {
                let types = match watcher.next_event()? {
                    Some(ClipboardEvent::Changed { mime_types, .. }) => mime_types,
                    Some(ClipboardEvent::Cleared { .. }) | None => vec![],
                };
                if keep {
                    *watch = Some(watcher);
                }
                Ok(types)
            },
        );
        match listed {
            Ok(types) if types.is_empty() => Listing::Empty,
            Ok(types) => Listing::Types(types),
            Err(Error::ClipboardEmpty | Error::NoMimeType) => Listing::Empty,
            Err(
                e @ (Error::MissingProtocol { .. }
                | Error::WaylandConnection(_)
                | Error::SocketOpenError(_)
                | Error::NoSeats),
            ) => Listing::Missing(e.to_string()),
            Err(e) => Listing::Missing(e.to_string()),
        }
    }

    /// Whether the desktop has offered anything since the watch began: a new selection, or the
    /// clipboard cleared. What it sent before this asks has arrived by now; the ask waits 50 ms
    /// for it at most. None with no watch, or when the desktop can't be asked.
    fn wayland_changed(watch: Option<wl_clipboard_rs::watch::Watcher>) -> Option<bool> {
        let mut watcher = watch?;
        let stop = watcher.cancel_handle();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            stop.cancel();
        });
        match watcher.next_event() {
            Ok(Some(_)) => Some(true),
            Ok(None) => Some(false),
            Err(_) => None,
        }
    }

    fn wayland_read(t: &str, cap: usize) -> Result<Vec<u8>, String> {
        use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};
        let (pipe, _) = get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific(t),
        )
        .map_err(|e| format!("Couldn't read the clipboard's {t}: {e}."))?;
        read_bounded(pipe, cap, READ_DEADLINE)
            .map_err(|e| format!("Couldn't read the clipboard's {t}: {e}."))
    }

    /// Reads a pipe to its end, at most `cap` bytes and one more, and for at most `deadline`: a
    /// program that copied and never writes can't hold the reader past it (Codex's review 85, 8).
    pub(super) fn read_bounded(
        mut pipe: impl Read + std::os::fd::AsRawFd,
        cap: usize,
        deadline: std::time::Duration,
    ) -> std::io::Result<Vec<u8>> {
        let until = std::time::Instant::now() + deadline;
        let mut bytes = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the program that copied it didn't answer in time",
                ));
            }
            let mut fd = libc::pollfd {
                fd: pipe.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd, for the pipe this function holds.
            let ready = unsafe { libc::poll(&mut fd, 1, left.as_millis().min(1000) as i32) };
            if ready < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if ready == 0 {
                continue;
            }
            let room = (cap + 1).saturating_sub(bytes.len()).min(buf.len());
            let n = pipe.read(&mut buf[..room])?;
            if n == 0 {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.len() > cap {
                return Ok(bytes);
            }
        }
    }

    /// The X11 clipboard's owner as a read began, and XFixes' word of each change of owner since:
    /// a program that copies again sets the selection again, whether or not it owned it.
    pub(super) struct X11Watch {
        conn: x11rb::rust_connection::RustConnection,
        clipboard: u32,
        owner: u32,
    }

    impl X11Watch {
        /// None where there's no X11 display or no XFixes: then nothing tells copies apart.
        pub(super) fn start() -> Option<X11Watch> {
            use x11rb::connection::Connection;
            use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
            use x11rb::protocol::xproto::ConnectionExt as _;
            let (conn, screen) = x11rb::connect(None).ok()?;
            conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
            let root = conn.setup().roots[screen].root;
            let clipboard = conn
                .intern_atom(false, b"CLIPBOARD")
                .ok()?
                .reply()
                .ok()?
                .atom;
            conn.xfixes_select_selection_input(
                root,
                clipboard,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )
            .ok()?
            .check()
            .ok()?;
            let owner = conn
                .get_selection_owner(clipboard)
                .ok()?
                .reply()
                .ok()?
                .owner;
            Some(X11Watch {
                conn,
                clipboard,
                owner,
            })
        }

        /// Whether the selection was set again, or its owner went, since `start`. The owner asked
        /// again is a round trip, so every change the server made before it has been reported.
        pub(super) fn changed(&self) -> Option<bool> {
            use x11rb::connection::Connection;
            use x11rb::protocol::Event;
            use x11rb::protocol::xproto::ConnectionExt as _;
            let owner = self
                .conn
                .get_selection_owner(self.clipboard)
                .ok()?
                .reply()
                .ok()?
                .owner;
            let mut changed = owner != self.owner;
            while let Ok(Some(event)) = self.conn.poll_for_event() {
                if matches!(event, Event::XfixesSelectionNotify(_)) {
                    changed = true;
                }
            }
            Some(changed)
        }
    }

    fn x11_list() -> Listing {
        match x11_get("TARGETS", 64 * 1024).map(|(_, names)| names) {
            Ok(types) if types.is_empty() => Listing::Empty,
            Ok(types) => Listing::Types(types),
            Err(X11::Empty) => Listing::Empty,
            Err(X11::Unusable(why)) => Listing::Missing(why),
            Err(X11::Failed(why)) => Listing::Failed(why),
        }
    }

    fn x11_read(t: &str, cap: usize) -> Result<Vec<u8>, String> {
        x11_get(t, cap)
            .map(|(bytes, _)| bytes)
            .map_err(|e| match e {
                X11::Empty => format!("Couldn't read the clipboard's {t}: it's no longer offered."),
                X11::Unusable(why) | X11::Failed(why) => {
                    format!("Couldn't read the clipboard's {t}: {why}.")
                }
            })
    }

    /// How many 32-bit units to ask a property for when `room` bytes are left: enough for them,
    /// never more (Codex's review 85, 7).
    pub(super) fn property_length(room: usize) -> u32 {
        u32::try_from(room.div_ceil(4))
            .unwrap_or(u32::MAX / 4)
            .clamp(1, u32::MAX / 4)
    }

    /// Adds what a property gave to `bytes`, never past `cap` and one byte more: whether there's
    /// room for more.
    pub(super) fn append_within(bytes: &mut Vec<u8>, value: &[u8], cap: usize) -> bool {
        let room = (cap + 1).saturating_sub(bytes.len());
        bytes.extend_from_slice(&value[..value.len().min(room)]);
        bytes.len() <= cap
    }

    /// How an X11 read went wrong.
    pub(super) enum X11 {
        /// Nothing is copied, or not in that type.
        Empty,
        /// There's no X11 display to read from here.
        Unusable(String),
        Failed(String),
    }

    /// The clipboard's `target` read from the X server, as ICCCM has a program read a selection:
    /// asked for into a property of a window of Kerdr's own, then read from it whole, or piece by
    /// piece (INCR) when it's large, at most `cap` bytes. With TARGETS, the names of the types it
    /// offers too.
    pub(super) fn x11_get(target: &str, cap: usize) -> Result<(Vec<u8>, Vec<String>), X11> {
        use std::time::Instant;
        use x11rb::connection::Connection;
        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::{
            AtomEnum, ConnectionExt, CreateWindowAux, EventMask, Property, WindowClass,
        };
        let failed = |e: &dyn std::fmt::Display| X11::Failed(e.to_string());
        let (conn, screen) =
            x11rb::connect(None).map_err(|e| X11::Unusable(format!("no X11 display ({e})")))?;
        let root = conn.setup().roots[screen].root;
        let visual = conn.setup().roots[screen].root_visual;
        let window = conn.generate_id().map_err(|e| failed(&e))?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| failed(&e))?;
        let atom = |name: &str| {
            conn.intern_atom(false, name.as_bytes())
                .map_err(|e| failed(&e))?
                .reply()
                .map(|r| r.atom)
                .map_err(|e| failed(&e))
        };
        let (clipboard, wanted, property, incr) = (
            atom("CLIPBOARD")?,
            atom(target)?,
            atom("KERDR_CLIPBOARD")?,
            atom("INCR")?,
        );
        conn.convert_selection(window, clipboard, wanted, property, x11rb::CURRENT_TIME)
            .map_err(|e| failed(&e))?;
        conn.flush().map_err(|e| failed(&e))?;
        let until = Instant::now() + READ_DEADLINE;
        let next = |what: &dyn Fn(&Event) -> bool| -> Result<Event, X11> {
            loop {
                match conn.poll_for_event().map_err(|e| failed(&e))? {
                    Some(event) if what(&event) => return Ok(event),
                    Some(_) => {}
                    None if Instant::now() >= until => {
                        return Err(X11::Failed(
                            "the program that copied it didn't answer".into(),
                        ));
                    }
                    None => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
        };
        let Event::SelectionNotify(notice) = next(&|e| matches!(e, Event::SelectionNotify(_)))?
        else {
            unreachable!()
        };
        if notice.property == u32::from(AtomEnum::NONE) {
            return Err(X11::Empty);
        }
        // The property, read in pieces each no longer than what's left of `cap` (and one byte
        // more, so one too large is known as that), its bytes_after followed, and never more
        // kept than that; then deleted, which an INCR transfer takes as "the next piece"
        // (Codex's review 85, 7). Its type, and whether it held nothing.
        let read_property = |bytes: &mut Vec<u8>| -> Result<(u32, bool), X11> {
            let mut offset = 0u32;
            let mut kind = 0;
            let mut empty = true;
            loop {
                let room = (cap + 1).saturating_sub(bytes.len());
                if room == 0 {
                    break;
                }
                let reply = conn
                    .get_property(
                        false,
                        window,
                        property,
                        AtomEnum::ANY,
                        offset,
                        property_length(room),
                    )
                    .map_err(|e| failed(&e))?
                    .reply()
                    .map_err(|e| failed(&e))?;
                kind = reply.type_;
                empty &= reply.value.is_empty();
                let more = append_within(bytes, &reply.value, cap);
                if reply.bytes_after == 0 || !more || reply.value.is_empty() {
                    break;
                }
                offset += (reply.value.len() / 4) as u32;
            }
            conn.delete_property(window, property)
                .map_err(|e| failed(&e))?;
            conn.flush().map_err(|e| failed(&e))?;
            Ok((kind, empty))
        };
        let mut first = Vec::new();
        let (first_kind, _) = read_property(&mut first)?;
        let (bytes, kind) = if first_kind == incr {
            // Large: it comes in pieces, each announced as the property is written again, until
            // an empty one, or until it's past the cap.
            let mut bytes = Vec::new();
            let mut kind = None;
            loop {
                next(&|e| {
                    matches!(e, Event::PropertyNotify(p)
                        if p.atom == property && p.state == Property::NEW_VALUE)
                })?;
                let (piece_kind, empty) = read_property(&mut bytes)?;
                if empty {
                    break;
                }
                kind.get_or_insert(piece_kind);
                if bytes.len() > cap {
                    break;
                }
            }
            (bytes, kind.unwrap_or_default())
        } else {
            (first, first_kind)
        };
        let mut names = vec![];
        if target == "TARGETS" && kind == u32::from(AtomEnum::ATOM) {
            for chunk in bytes.as_chunks::<4>().0 {
                let a = u32::from_ne_bytes(*chunk);
                if let Ok(reply) = conn
                    .get_atom_name(a)
                    .map_err(|_| ())
                    .and_then(|c| c.reply().map_err(|_| ()))
                {
                    names.push(String::from_utf8_lossy(&reply.name).into_owned());
                }
            }
        }
        let _ = conn.destroy_window(window);
        let _ = conn.flush();
        Ok((bytes, names))
    }
}

/// A private folder for a reader to write the clipboard's image into, and the path it writes:
/// a path nobody has open.
#[cfg(target_os = "macos")]
fn scratch() -> Result<(tempfile::TempDir, PathBuf), String> {
    let dir = tempfile::Builder::new()
        .prefix("kerdr-clipboard-")
        .tempdir()
        .map_err(|e| {
            format!(
                "Kerdr couldn't make a temporary folder to read the clipboard into ({e}); the temporary folder may be full or read-only."
            )
        })?;
    let file = dir.path().join("clipboard.png");
    Ok((dir, file))
}

#[cfg(target_os = "macos")]
fn macos_clipboard(room: Room) -> Result<Found, String> {
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
        return from_file(&file, room).map(Found::Image);
    }
    // A file copied in Finder.
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", "POSIX path of (the clipboard as «class furl»)"]);
    if let Ok(c) = capture(cmd, 64 * 1024, READ_DEADLINE)
        && c.status == Some(0)
    {
        let path = String::from_utf8_lossy(&c.stdout).trim().to_string();
        if !path.is_empty() {
            return copied_files(&format!("file://{}", path.replace('%', "%25")), room.fix);
        }
    }
    // Text: it goes into the words, as any paste would (intent 0591, K3-61).
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", "the clipboard as «class utf8»"]);
    if let Ok(c) = capture(cmd, MOST_CLIPBOARD_TEXT + 1, READ_DEADLINE)
        && c.status == Some(0)
    {
        if c.stdout.len() > MOST_CLIPBOARD_TEXT {
            return Err(too_much_text());
        }
        let text = String::from_utf8_lossy(&c.stdout)
            .trim_end_matches('\n')
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if !text.is_empty() {
            return Ok(Found::Text(text));
        }
    }
    Err(NO_IMAGE.into())
}

/// Windows' clipboard, read in Kerdr itself (clipboard-win), at once: PowerShell took seconds to
/// start, while what was copied could change (Codex's review 86, 7). The clipboard is open only
/// while what it holds is copied out: a PNG as one (what browsers and screenshot tools put
/// there), or else its bitmap, the files Explorer copied, or text.
#[cfg(windows)]
fn windows_clipboard(room: Room, pasted: Option<u64>) -> Result<Found, String> {
    use clipboard_win::{Clipboard, Getter, formats, raw};
    let failed = |e: clipboard_win::ErrorCode| format!("Couldn't read the clipboard: {e}.");
    // Which copy the clipboard held as the paste was handled: the number Windows keeps, taken
    // then by Kerdr's watch, before anything else was done for the paste (or here, for a read
    // with none). Once Kerdr has it open nothing else can change it, so a copy made since shows
    // as that number having moved by then (Codex's reviews 88, 1, and 89).
    let number = || raw::seq_num().map(|n| u64::from(n.get()));
    let pasted = pasted.or_else(number);
    let _open =
        Clipboard::new_attempts(10).map_err(|e| format!("Couldn't open the clipboard: {e}."))?;
    if number() != pasted {
        return Err(COPIED_AGAIN.into());
    }
    let held = |format: u32| raw::size(format).map_or(0, |n| n.get());
    if let Some(png) = raw::register_format("PNG")
        .map(|f| f.get())
        .filter(|f| raw::is_format_avail(*f))
    {
        if held(png) > room.read_cap() {
            return Err(too_big("The clipboard's image", room));
        }
        let mut bytes = Vec::new();
        formats::RawData(png)
            .read_clipboard(&mut bytes)
            .map_err(failed)?;
        return checked(bytes, "The clipboard's image", room).map(Found::Image);
    }
    if raw::is_format_avail(formats::CF_DIB) || raw::is_format_avail(formats::CF_BITMAP) {
        // Made a PNG once it's in hand, and checked then: a bitmap is larger than its PNG.
        if held(formats::CF_DIB) > MOST_READ {
            return Err(too_big("The clipboard's image", Room { fix: true, ..room }));
        }
        let mut bytes = Vec::new();
        formats::Bitmap.read_clipboard(&mut bytes).map_err(failed)?;
        return Ok(Found::Bitmap(bytes));
    }
    if raw::is_format_avail(formats::CF_HDROP) {
        use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
        let hdrop = raw::get_clipboard_data(formats::CF_HDROP)
            .map_err(failed)?
            .as_ptr() as HDROP;
        // SAFETY: the clipboard is open and holds this list while these run; a null buffer
        // asks only for a count or a length.
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
        let files = bounded_file_list(
            count,
            |i| unsafe { DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0) },
            |i, len| {
                let mut name = vec![0u16; len as usize + 1];
                // SAFETY: `name` has room for `len` units and the closing NUL.
                let got = unsafe { DragQueryFileW(hdrop, i, name.as_mut_ptr(), len + 1) };
                (got == len).then(|| String::from_utf16_lossy(&name[..len as usize]))
            },
        )?;
        if !files.is_empty() {
            let listed: String = files
                .iter()
                .map(|l| format!("file:///{}\n", l.trim().replace('%', "%25")))
                .collect();
            return copied_files(&listed, room.fix);
        }
    }
    if raw::is_format_avail(formats::CF_UNICODETEXT) {
        // Each UTF-16 unit is a byte or more of the words: past the most, it's never copied out.
        if held(formats::CF_UNICODETEXT) / 2 > MOST_CLIPBOARD_TEXT + 1 {
            return Err(too_much_text());
        }
        let mut text = String::new();
        formats::Unicode.read_clipboard(&mut text).map_err(failed)?;
        // Text goes into the words, as any paste would (intent 0591, K3-61).
        if text.len() > MOST_CLIPBOARD_TEXT {
            return Err(too_much_text());
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if !text.is_empty() {
            return Ok(Found::Text(text));
        }
    }
    if raw::count_formats() == Some(0) {
        return Err("The clipboard is empty.".into());
    }
    Err(NO_IMAGE.into())
}

/// The most files a copied list may name, and the most its names may be together (in UTF-16, as
/// Windows keeps them), before any name is read (Codex's review 88, 3).
const MOST_COPIED_FILES: u32 = 1000;
const MOST_COPIED_NAMES: usize = 1024 * 1024;

/// A file list copied in Explorer, read within bounds: `count` the files it names, `len(i)` how
/// long name `i` is in UTF-16 units, `read(i, len)` that name. Past MOST_COPIED_FILES, or names
/// together past MOST_COPIED_NAMES, it's refused, said, before a name is read.
#[cfg_attr(not(windows), allow(dead_code))]
fn bounded_file_list(
    count: u32,
    len: impl Fn(u32) -> u32,
    read: impl Fn(u32, u32) -> Option<String>,
) -> Result<Vec<String>, String> {
    if count > MOST_COPIED_FILES {
        return Err(format!(
            "You copied {count} files; Kerdr takes at most {MOST_COPIED_FILES} at once, so none of them went in."
        ));
    }
    let lens: Vec<u32> = (0..count).map(len).collect();
    let together: usize = lens.iter().map(|&n| n as usize * 2).sum();
    if together > MOST_COPIED_NAMES {
        return Err(format!(
            "The copied files' names come to over {}, more than Kerdr reads at once, so none of them went in.",
            size(MOST_COPIED_NAMES)
        ));
    }
    (0..count)
        .zip(lens)
        .map(|(i, n)| {
            read(i, n).ok_or_else(|| "Couldn't read the names of the files you copied.".to_string())
        })
        .collect()
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
    fn a_bitmap_taken_from_the_clipboard_goes_in_as_a_png_an_older_backend_takes() {
        // Codex's review 86, 7: Windows' clipboard is read in Kerdr itself now, and a picture it
        // holds only as a bitmap is made a PNG once it's in hand, as PowerShell made it before.
        let picture = image::RgbImage::from_pixel(3, 2, image::Rgb([200, 30, 40]));
        let mut bmp = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(picture)
            .write_to(&mut bmp, image::ImageFormat::Bmp)
            .unwrap();
        let got = loading_of(Ok(Found::Bitmap(bmp.into_inner())), Room::left(&[]));
        assert_eq!(got.why, None);
        assert_eq!(got.images.len(), 1);
        assert_eq!(got.images[0].media_type, "image/png");
        let back = image::load_from_memory(&got.images[0].bytes)
            .unwrap()
            .to_rgb8();
        assert_eq!((back.width(), back.height()), (3, 2));
        assert_eq!(back.get_pixel(2, 1), &image::Rgb([200, 30, 40]));
        let broken = loading_of(
            Ok(Found::Bitmap(b"BMnot a picture".to_vec())),
            Room::left(&[]),
        );
        assert!(broken.images.is_empty());
        assert!(
            broken
                .why
                .unwrap()
                .starts_with("Couldn't read the clipboard's picture")
        );
    }

    #[test]
    fn a_copied_file_list_is_bounded_before_any_name_is_read() {
        // Codex's review 88, 3: Windows' copied file list (CF_HDROP), read through stand-ins for
        // DragQueryFileW: its count first, then each name's length, and only then the names. Past
        // 1000 files, or names together past 1 MB, nothing is read, and he's told.
        use std::cell::Cell;
        let (asked, read) = (Cell::new(0), Cell::new(0));
        let len = |n: u32| {
            let asked = &asked;
            move |_i: u32| {
                asked.set(asked.get() + 1);
                n
            }
        };
        let name = |_i: u32, n: u32| {
            read.set(read.get() + 1);
            Some("C".repeat(n as usize))
        };
        let why = bounded_file_list(1001, len(20), name).unwrap_err();
        assert_eq!(
            why,
            "You copied 1001 files; Kerdr takes at most 1000 at once, so none of them went in."
        );
        assert_eq!(
            (asked.get(), read.get()),
            (0, 0),
            "names read past the count"
        );
        let why = bounded_file_list(1000, len(600), name).unwrap_err();
        assert!(
            why.starts_with("The copied files' names come to over"),
            "{why}"
        );
        assert_eq!(read.get(), 0, "names read past their size");
        let got = bounded_file_list(3, len(4), name).unwrap();
        assert_eq!(got, vec!["CCCC"; 3]);
        assert!(
            bounded_file_list(2, len(4), |_, _| None)
                .unwrap_err()
                .contains("Couldn't read the names")
        );
    }

    #[test]
    fn a_reader_that_can_tell_a_copy_made_while_it_read_puts_nothing_in() {
        // Codex's review 88, 1: the check runs after the read, so a copy made during it, read
        // within any deadline, is told; a reader that can't tell (a program) is taken as it read.
        let version = std::cell::Cell::new(1);
        let seen = std::cell::Cell::new(0);
        let (version, seen) = (&version, &seen);
        let reader = |tells: bool| Reader {
            name: "Kerdr's own Wayland reader".into(),
            list: Box::new(move || {
                seen.set(version.get());
                Listing::Types(vec!["image/png".into()])
            }),
            read: Box::new(move |_, _| {
                version.set(version.get() + 1);
                Ok(PNG.to_vec())
            }),
            changed: Box::new(move || tells.then(|| version.get() != seen.get())),
        };
        let none = |_: &str, _: &[&str]| -> std::io::Result<Captured> {
            Err(std::io::ErrorKind::NotFound.into())
        };
        let got = clipboard_here(true, false, &[reader(true)], &none, Room::left(&[]));
        assert_eq!(got, Err(COPIED_AGAIN.to_string()));
        let got = clipboard_here(true, false, &[reader(false)], &none, Room::left(&[]));
        assert!(matches!(got, Ok(Found::Image(_))), "{got:?}");
    }

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

    /// A room for a backend that fixes input, with nothing attached yet.
    fn fixing() -> Room {
        Room::for_backend(&[], true)
    }

    /// The first bytes of each kind Kerdr knows, enough for kind_of.
    fn bytes_of(kind: &str) -> Vec<u8> {
        let mut b: Vec<u8> = match kind {
            "bmp" => [
                b"BM".to_vec(),
                vec![0x46, 0, 0, 0],
                vec![0; 4],
                vec![0x36, 0, 0, 0],
            ]
            .concat(),
            "tiff" => b"II*\0\x08\0\0\0".to_vec(),
            "heic" => b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec(),
            "heif" => b"\0\0\0\x18ftypmif1\0\0\0\0mif1".to_vec(),
            "avif" => b"\0\0\0\x1cftypavif\0\0\0\0avifmif1".to_vec(),
            "ico" => vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0],
            "jxl" => vec![0xff, 0x0a, 0xfa, 0x1f],
            "psd" => b"8BPS\0\x01".to_vec(),
            "qoi" => b"qoif\0\0\0\x10".to_vec(),
            _ => panic!("{kind}"),
        };
        b.resize(64, 0);
        b
    }

    #[test]
    fn any_kind_of_image_is_known_by_its_bytes_and_only_what_isnt_one_is_refused() {
        // Intent 0591, K3-56: for a backend that fixes input, an image of another kind goes as
        // pasted (its backend saves a PNG copy beside it); only bytes that are no image at all are
        // refused, in those words. An older backend's words stay 0.35.0's.
        for (kind, media) in [
            ("bmp", "image/bmp"),
            ("tiff", "image/tiff"),
            ("heic", "image/heic"),
            ("heif", "image/heif"),
            ("avif", "image/avif"),
            ("ico", "image/x-icon"),
            ("jxl", "image/jxl"),
            ("psd", "image/vnd.adobe.photoshop"),
            ("qoi", "image/qoi"),
        ] {
            let bytes = bytes_of(kind);
            assert_eq!(kind_of(&bytes), Some(media), "{kind}");
            assert_eq!(
                media_type(&bytes),
                None,
                "{kind} isn't one every backend takes"
            );
            let got = checked(bytes.clone(), "shot", fixing()).unwrap();
            assert_eq!((got.media_type, got.bytes), (media, bytes.clone()));
            let why = checked(bytes, "shot", Room::left(&[])).unwrap_err();
            assert!(
                why.contains("isn't a PNG, JPEG, GIF or WebP image"),
                "{why}"
            );
        }
        assert_eq!(kind_of(b"hello, not an image at all"), None);
        assert_eq!(
            checked(b"hello".to_vec(), "notes.png", fixing()).unwrap_err(),
            "notes.png isn't an image."
        );
        assert_eq!(kind_name("image/bmp"), "BMP");
        assert_eq!(kind_name("image/heic"), "HEIC");
        // A file named as one: its name is enough to read it, its bytes say what it is.
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.BMP");
        std::fs::write(&shot, bytes_of("bmp")).unwrap();
        assert_eq!(
            paths_in_paste(shot.to_str().unwrap()),
            None,
            "an older backend: words"
        );
        assert_eq!(
            paths_in_paste_for(shot.to_str().unwrap(), true),
            Some(vec![shot.clone()])
        );
        let got = from_files(&[shot], fixing());
        assert_eq!(got.images[0].media_type, "image/bmp");
        assert_eq!(got.why, None);
    }

    #[test]
    fn copied_text_is_read_whole_never_cut_inside_a_character() {
        // Codex's review 85, 5: past the box's megabyte, the text is still read whole (the box
        // keeps what doesn't fit for c), so no character is split at a cap; past 8 MB nothing goes
        // in, and that's said.
        let euros = "€".repeat(400_000); // 1.2 MB, three bytes each
        let calls = RefCell::new(vec![]);
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"text/plain;charset=utf-8\n")
            } else {
                ok(euros.as_bytes())
            }
        });
        let Ok(Found::Text(text)) = unix_clipboard(true, false, &run) else {
            panic!("no text")
        };
        assert_eq!(text.len(), euros.len());
        assert!(!text.contains('\u{fffd}'), "a character was split");
        let huge = "x".repeat(MOST_CLIPBOARD_TEXT + 10);
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"text/plain;charset=utf-8\n")
            } else {
                ok(huge.as_bytes())
            }
        });
        let why = unix_clipboard(true, false, &run).unwrap_err();
        assert!(
            why.contains("none of it went in; it's still on your clipboard"),
            "{why}"
        );
    }

    #[test]
    fn a_clipboard_image_of_another_kind_is_read_as_it_is() {
        // Intent 0591, K3-57: the clipboard offers only BMP; a backend that fixes input takes it.
        let calls = RefCell::new(vec![]);
        let bmp = bytes_of("bmp");
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"image/bmp\ntext/plain\n")
            } else {
                ok(&bmp)
            }
        });
        let Ok(Found::Image(got)) = clipboard_here(true, false, &[], &run, fixing()) else {
            panic!("not read")
        };
        assert_eq!(got.media_type, "image/bmp");
        assert_eq!(
            calls.borrow().last().map(String::as_str),
            Some("wl-paste --no-newline --type image/bmp")
        );
        // An older backend: 0.35.0's words.
        let why = unix_clipboard(true, false, &run).unwrap_err();
        assert!(why.contains("image/bmp"), "{why}");
    }

    #[test]
    fn a_backend_that_fixes_input_takes_more_than_twenty_images_and_each_up_to_twenty_megabytes() {
        // Intent 0591, K3-53, K3-55: no count, each image up to 20 MB, still read when past that
        // so Kerdr can ask about a smaller copy.
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..25)
            .map(|n| {
                let p = dir.path().join(format!("{n:02}.png"));
                std::fs::write(&p, PNG).unwrap();
                p
            })
            .collect();
        let got = from_files(&paths, fixing());
        assert_eq!(got.images.len(), 25);
        assert_eq!(got.why, None);
        let room = fixing();
        assert!(room.images > MAX_IMAGES && room.per_image() == FIX_IMAGE_BYTES);
        let mut big = PNG.to_vec();
        big.resize(14_200_000, 0);
        let huge = dir.path().join("huge.png");
        std::fs::write(&huge, &big).unwrap();
        let got = from_files(std::slice::from_ref(&huge), fixing());
        assert_eq!(got.images.len(), 1, "{:?}", got.why);
        assert_eq!(got.images[0].bytes.len(), 14_200_000);
        assert_eq!(got.images[0].from, "huge.png");
        // An older backend: 0.35.0's.
        let got = from_files(&[huge], Room::left(&[]));
        assert!(got.images.is_empty());
        assert!(
            got.why
                .unwrap()
                .contains("Kerdr attaches images up to 10 MB")
        );
    }

    #[test]
    fn a_folder_gives_the_image_files_directly_in_it_each_named() {
        // Intent 0591, K3-60.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("shots.png");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("b.png"), PNG).unwrap();
        std::fs::write(folder.join("a.jpg"), [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        std::fs::write(folder.join("notes.txt"), "hi").unwrap();
        std::fs::create_dir(folder.join("deeper.png")).unwrap();
        std::fs::write(folder.join("deeper.png").join("c.png"), PNG).unwrap();
        let got = from_files(std::slice::from_ref(&folder), Room::left(&[]));
        assert_eq!(got.images.len(), 2, "{:?}", got.why);
        assert_eq!(got.images[0].media_type, "image/jpeg", "in name order");
        assert_eq!(got.notes, ["From the folder shots.png: a.jpg and b.png."]);
        assert_eq!(got.why, None);
        let empty = dir.path().join("empty.png");
        std::fs::create_dir(&empty).unwrap();
        let got = from_files(&[empty], Room::left(&[]));
        assert!(got.images.is_empty());
        assert!(got.why.unwrap().contains("no image files directly in it"));
    }

    #[test]
    fn files_copied_in_a_file_manager_attach_as_what_their_bytes_are() {
        // Intent 0591, K3-58: for a backend that fixes input, an image named oddly attaches, a
        // text file's words go into his words, indented under its name, and anything else is named.
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("Screenshot from today");
        std::fs::write(&odd, PNG).unwrap();
        let notes = dir.path().join("notes.log");
        std::fs::write(&notes, "boot failed\n```\nat line 3\n").unwrap();
        let pdf = dir.path().join("report.pdf");
        std::fs::write(&pdf, b"%PDF-1.7\n...").unwrap();
        let bin = dir.path().join("blob.bin");
        std::fs::write(&bin, [0u8, 159, 146, 150]).unwrap();
        let listed: String = [&odd, &notes, &pdf, &bin]
            .iter()
            .map(|p| format!("file://{}\n", p.display().to_string().replace(' ', "%20")))
            .collect();
        let Ok(Found::Files(paths)) = copied_files(&listed, true) else {
            panic!("not listed")
        };
        assert_eq!(paths.len(), 4);
        let got = from_files(&paths, fixing());
        assert_eq!(got.images.len(), 1);
        assert_eq!(got.images[0].media_type, "image/png");
        assert_eq!(
            got.text.as_deref(),
            Some("File notes.log:\n    boot failed\n    ```\n    at line 3")
        );
        let why = got.why.unwrap();
        assert!(why.contains("report.pdf is a PDF"), "{why}");
        assert!(
            why.contains("blob.bin isn't an image or a text file"),
            "{why}"
        );
        // An older backend: only image names, as 0.35.0.
        assert!(copied_files(&listed, false).is_err());
    }

    #[test]
    fn a_text_files_lines_go_in_indented_so_none_reads_as_one_of_the_intents_sections() {
        // The lead, round 3, from r5-b: the backend takes a line like "## Constraints" for the
        // intent's own section even inside a code block, so a pasted file's words are indented
        // four spaces under its name. Its patterns (bin/ark-cards' OWN_SECTION and FENCE_OPEN,
        // bin/ark-card-action's NEW_SECTION) allow at most three spaces before the mark, so no
        // indented line can match one.
        let dir = tempfile::tempdir().unwrap();
        let plan = dir.path().join("plan.txt");
        let text = "Steps\n## Acceptance criteria\n- it works\n\n## Constraints\n```\ncode\n## Images\n# Title\n";
        std::fs::write(&plan, text).unwrap();
        let got = from_files(std::slice::from_ref(&plan), fixing());
        assert_eq!(got.why, None);
        let words = got.text.unwrap();
        assert_eq!(
            words,
            "File plan.txt:\n    Steps\n    ## Acceptance criteria\n    - it works\n\n    ## Constraints\n    ```\n    code\n    ## Images\n    # Title"
        );
        for line in words.lines().skip(1) {
            assert!(line.is_empty() || line.starts_with("    "), "{line:?}");
            let mark = line.trim_start_matches(' ');
            let indent = line.len() - mark.len();
            for start in ["#", "```", "~~~"] {
                assert!(
                    line.is_empty() || !mark.starts_with(start) || indent > 3,
                    "{line:?} reads as a heading or a fence"
                );
            }
        }
    }

    #[test]
    fn kerdrs_own_reader_reads_the_clipboard_with_no_program_installed() {
        // Intent 0591, K3-63: Kerdr's own reader goes first; wl-paste and xclip are only tried
        // when it can't be used here.
        let calls = RefCell::new(vec![]);
        let missing = fake(&calls, |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        let own = Reader {
            name: "Kerdr's own Wayland reader".into(),
            list: Box::new(|| Listing::Types(vec!["image/png".into(), "text/plain".into()])),
            read: Box::new(|t, _| {
                assert_eq!(t, "image/png");
                Ok(PNG.to_vec())
            }),
            changed: Box::new(|| None),
        };
        let Ok(Found::Image(got)) = clipboard_here(true, false, &[own], &missing, Room::left(&[]))
        else {
            panic!("not read")
        };
        assert_eq!(got.bytes, PNG);
        assert!(calls.borrow().is_empty(), "no program was run");
        // It can't be used here (the desktop has no data-control): wl-paste is.
        let unusable = Reader {
            name: "Kerdr's own Wayland reader".into(),
            list: Box::new(|| Listing::Missing("the compositor lacks data-control".into())),
            read: Box::new(|_, _| unreachable!()),
            changed: Box::new(|| None),
        };
        let run = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"image/png\n")
            } else {
                ok(PNG)
            }
        });
        assert!(matches!(
            clipboard_here(true, false, &[unusable], &run, Room::left(&[])),
            Ok(Found::Image(_))
        ));
        // Neither: the message says why and names the package that would let it.
        let unusable = Reader {
            name: "Kerdr's own Wayland reader".into(),
            list: Box::new(|| Listing::Missing("the compositor lacks data-control".into())),
            read: Box::new(|_, _| unreachable!()),
            changed: Box::new(|| None),
        };
        let why = clipboard_here(true, false, &[unusable], &missing, Room::left(&[])).unwrap_err();
        assert!(
            why.contains("Kerdr couldn't read it itself (the compositor lacks data-control)"),
            "{why}"
        );
        assert!(
            why.contains("wl-paste (from wl-clipboard) isn't installed"),
            "{why}"
        );
        assert!(
            why.contains("Installing wl-clipboard would let it"),
            "{why}"
        );
    }

    /// A picture whose every pixel differs, so it doesn't compress to nothing: a stand-in for a
    /// photo, `w` by `h`.
    fn noise(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut seed = 0x2545_f491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xff) as u8
        };
        let mut bytes = Vec::new();
        if alpha {
            let img =
                image::RgbaImage::from_fn(w, h, |_, _| image::Rgba([next(), next(), next(), 128]));
            img.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        } else {
            let img = image::RgbImage::from_fn(w, h, |_, _| image::Rgb([next(), next(), next()]));
            img.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        }
        bytes
    }

    #[test]
    fn a_smaller_copy_fits_what_was_asked_and_keeps_see_through_parts() {
        // Intent 0591, release 5: made only once he said so, never touching the image itself.
        let original = Loaded {
            media_type: "image/png",
            bytes: noise(600, 400, false),
            from: "shot.png".into(),
        };
        let before = original.bytes.clone();
        let copy = smaller(&original, 60_000).unwrap();
        assert!(copy.bytes.len() <= 60_000, "{}", copy.bytes.len());
        assert_eq!(copy.media_type, "image/jpeg");
        assert_eq!(kind_of(&copy.bytes), Some("image/jpeg"));
        assert_eq!(original.bytes, before, "the image itself is untouched");
        let clear = Loaded {
            media_type: "image/png",
            bytes: noise(300, 200, true),
            from: "logo.png".into(),
        };
        let copy = smaller(&clear, clear.bytes.len() / 3).unwrap();
        assert_eq!(copy.media_type, "image/png", "see-through parts stay");
        assert!(copy.bytes.len() <= clear.bytes.len() / 3);
        // Bytes whose picture Kerdr can't read: said.
        let heic = Loaded {
            media_type: "image/heic",
            bytes: bytes_of("heic"),
            from: "IMG_1.heic".into(),
        };
        assert!(
            smaller(&heic, 10)
                .unwrap_err()
                .contains("can't read a HEIC image")
        );
        // Copies share the room left by how large each is.
        let got = Source::Smaller(vec![original.clone(), original], 0).load(Room {
            images: usize::MAX,
            bytes: 100_000,
            fix: true,
        });
        assert_eq!(got.images.len(), 2);
        assert!(got.images.iter().map(|i| i.bytes.len()).sum::<usize>() <= 100_000);
        assert!(got.notes[0].contains("shot.png went in as a smaller copy: a JPEG image"));
    }

    #[test]
    fn an_oversized_or_unknown_image_is_refused_with_its_limit_named() {
        let mut big = PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        let why = checked(big.clone(), "The clipboard's image", Room::left(&[])).unwrap_err();
        assert_eq!(
            why,
            "The clipboard's image is over 10 MB, the most Kerdr attaches for one image."
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("huge.png");
        big.resize(14_200_000, 0);
        std::fs::write(&file, &big).unwrap();
        assert_eq!(
            from_file(&file, Room::left(&[])).unwrap_err(),
            "huge.png is 14.2 MB; Kerdr attaches images up to 10 MB."
        );
        let why = checked(b"BM....".to_vec(), "shot.bmp", Room::left(&[])).unwrap_err();
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
        // Words, a file that isn't an image, a relative name: all stay words. A missing image
        // is found when it's read, off the board's thread.
        assert_eq!(paths_in_paste(&format!("look at {p}")), None);
        let missing = PathBuf::from(format!("{p}.missing.png"));
        assert_eq!(
            paths_in_paste(missing.to_str().unwrap()),
            Some(vec![missing.clone()])
        );
        let got = from_files(&[missing], Room::left(&[]));
        assert!(got.images.is_empty());
        assert!(
            got.why
                .unwrap()
                .starts_with("Couldn't read plain.PNG.missing.png")
        );
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
                fix: false,
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
                fix: false,
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
                bytes: 1,
                fix: false,
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
        // Text only: it goes into the words, as any paste would (intent 0591, K3-61).
        let text_only = fake(&calls, |_, args| {
            if args == ["--list-types"] {
                ok(b"text/plain;charset=utf-8\nUTF8_STRING\n")
            } else {
                ok(b"Fix the NAS\r\nby Friday")
            }
        });
        assert_eq!(
            unix_clipboard(true, false, &text_only),
            Ok(Found::Text("Fix the NAS\nby Friday".into()))
        );
        assert_eq!(
            calls.borrow().last().map(String::as_str),
            Some("wl-paste --no-newline --type text/plain;charset=utf-8")
        );
        // Nothing usable at all stays a message.
        let odd = fake(&calls, |_, _| ok(b"application/x-kde-cutselection\n"));
        assert!(
            unix_clipboard(true, false, &odd)
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
        assert!(
            why.contains("neither wl-paste nor xclip is installed"),
            "{why}"
        );
        assert!(
            why.contains("Installing wl-clipboard would let it"),
            "{why}"
        );
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
                copied_files(&format!("file://{}", notes.display()), false)
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

    /// As live_clipboard, read with Kerdr's own reader alone, every program missing (intent 0591,
    /// K3-63): `cargo test -- --ignored live_clipboard`.
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    #[ignore]
    fn live_clipboard_own_reader() {
        let path = std::env::var("KERDR_LIVE_CLIPBOARD_IMAGE")
            .expect("KERDR_LIVE_CLIPBOARD_IMAGE names the image put on the clipboard");
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let (wayland, x11) = (set("WAYLAND_DISPLAY"), set("DISPLAY"));
        let no_programs = |_: &str, _: &[&str]| -> std::io::Result<Captured> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        };
        let found = clipboard_here(
            wayland,
            x11,
            &builtin::readers(wayland, x11, true),
            &no_programs,
            Room::left(&[]),
        );
        let Ok(Found::Image(got)) = found else {
            panic!("Kerdr's own reader didn't read it: {found:?}")
        };
        assert_eq!(got.bytes, std::fs::read(&path).unwrap());
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn a_clipboard_pipe_that_never_writes_is_let_go_at_its_deadline() {
        // Codex's review 85, 8: the program that copied never writes; the read ends at its
        // deadline, and one that writes too much stops just past the cap.
        use std::io::Write;
        let (quiet, _held_open) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = Instant::now();
        let err = builtin::read_bounded(quiet, 100, Duration::from_millis(300)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        writer.write_all(&[7; 5000]).unwrap();
        drop(writer);
        let got = builtin::read_bounded(reader, 1000, Duration::from_secs(2)).unwrap();
        assert_eq!(got.len(), 1001);
    }

    #[test]
    fn a_folder_is_looked_through_only_so_far() {
        // Codex's review 85, 8: never listed whole before its images are chosen.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("many");
        std::fs::create_dir(&folder).unwrap();
        for n in 0..(FOLDER_ENTRIES + 20) {
            std::fs::write(folder.join(format!("{n:05}.txt")), "x").unwrap();
        }
        let got = from_files(std::slice::from_ref(&folder), Room::left(&[]));
        let why = got.why.unwrap_or_default();
        assert!(
            why.contains("more than Kerdr looks through at once"),
            "{why}"
        );
        // A folder of just the most images is read whole, and nothing said of a limit.
        let full = dir.path().join("full");
        std::fs::create_dir(&full).unwrap();
        for n in 0..FOLDER_IMAGES {
            std::fs::write(full.join(format!("{n:03}.png")), PNG).unwrap();
        }
        let got = from_files(std::slice::from_ref(&full), Room::for_backend(&[], true));
        assert_eq!((got.images.len(), got.why), (FOLDER_IMAGES, None));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn an_x11_property_is_asked_for_and_kept_only_within_the_cap() {
        // Codex's review 85, 7: a piece asked for is no longer than what's left, and what's kept
        // stops one byte past the cap, so one too large is known as that.
        use builtin::{append_within, property_length};
        assert_eq!(property_length(1), 1);
        assert_eq!(property_length(4), 1);
        assert_eq!(property_length(5), 2);
        assert_eq!(property_length(100_001), 25_001);
        assert_eq!(property_length(usize::MAX), u32::MAX / 4);
        let mut bytes = vec![];
        assert!(append_within(&mut bytes, &[1; 60], 100));
        assert!(
            !append_within(&mut bytes, &[2; 4_000_000], 100),
            "past the cap"
        );
        assert_eq!(bytes.len(), 101);
        assert!(!append_within(&mut bytes, &[3; 10], 100));
        assert_eq!(bytes.len(), 101, "nothing more once past it");
    }

    /// Kerdr's own X11 reader against a real clipboard owner (the image KERDR_LIVE_CLIPBOARD_IMAGE
    /// names, served whole or, when large, in INCR pieces): asked for half of it, it keeps no
    /// more than that and one byte, however the owner sends it (Codex's review 85, 7).
    /// The desktop the live tests run on: Some(true) for Wayland, Some(false) for X11.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn live_desktop() -> Option<bool> {
        let set = |n: &str| std::env::var_os(n).is_some_and(|v| !v.is_empty());
        match (set("WAYLAND_DISPLAY"), set("DISPLAY")) {
            (true, _) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    }

    /// A copy made as he'd make one: wl-copy or xclip (KERDR_LIVE_WL_COPY and KERDR_LIVE_XCLIP
    /// name them where they're not on PATH), given time to take the clipboard.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn live_copy(wayland: bool, path: &Path, kind: &str) {
        let program = |var: &str, name: &str| std::env::var_os(var).unwrap_or(name.into());
        let status = if wayland {
            Command::new(program("KERDR_LIVE_WL_COPY", "wl-copy"))
                .args(["--type", kind])
                .stdin(std::fs::File::open(path).unwrap())
                .status()
        } else {
            Command::new(program("KERDR_LIVE_XCLIP", "xclip"))
                .args(["-selection", "clipboard", "-t", kind, "-i"])
                .arg(path)
                .status()
        };
        assert!(status.unwrap().success(), "couldn't copy");
        std::thread::sleep(Duration::from_millis(200));
    }

    /// A file of words to copy: "<name> words".
    #[cfg(not(any(target_os = "macos", windows)))]
    fn live_words(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("{name} words")).unwrap();
        path
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    #[ignore]
    fn live_watch_knows_a_copy_made_as_the_reader_set_up() {
        // Codex's review 89, against a real desktop: Kerdr's watch, running from before the
        // paste, numbers it; a copy made after the paste, before any reader is set up, puts
        // nothing in; a paste with no copy after it reads its words. The image the other live
        // tests read is put back after.
        let Some(wayland) = live_desktop() else {
            eprintln!("skipped: no desktop");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let watch = ClipboardWatch::start();
        let until = Instant::now() + Duration::from_secs(5);
        while watch.now().is_none() {
            assert!(Instant::now() < until, "the watch never set up");
            std::thread::sleep(Duration::from_millis(20));
        }
        live_copy(wayland, &live_words(dir.path(), "first"), "text/plain");
        let none = |_: &str, _: &[&str]| -> std::io::Result<Captured> {
            Err(std::io::ErrorKind::NotFound.into())
        };
        let read = || {
            let readers = builtin::readers(wayland, !wayland, false);
            clipboard_here(wayland, !wayland, &readers, &none, Room::left(&[]))
        };
        let pasted = watch.pasted().expect("the paste numbered");
        live_copy(wayland, &live_words(dir.path(), "second"), "text/plain");
        assert_eq!(
            take_since(Some(pasted), read),
            Err(COPIED_AGAIN.to_string())
        );
        let pasted = watch.pasted().expect("the paste numbered");
        assert_eq!(
            take_since(Some(pasted), read),
            Ok(Found::Text("second words".into()))
        );
        if let Some(image) = std::env::var_os("KERDR_LIVE_CLIPBOARD_IMAGE") {
            live_copy(wayland, Path::new(&image), "image/png");
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    #[ignore]
    fn live_reader_knows_a_copy_made_while_it_read() {
        // Codex's review 88, 1, against a real desktop: Kerdr's own reader, X11's (XFixes) or
        // Wayland's (the offer it listed), says whether anything was copied between its list and
        // its check; a paste whose read is followed by another copy puts nothing in. The copies
        // are made with xclip or wl-copy (KERDR_LIVE_XCLIP and KERDR_LIVE_WL_COPY name them where
        // they're not on PATH); the image the other live tests read is put back after.
        let Some(wayland) = live_desktop() else {
            eprintln!("skipped: no desktop");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let copy = |path: &Path, kind: &str| live_copy(wayland, path, kind);
        let words = |name: &str| live_words(dir.path(), name);
        let own = || {
            builtin::readers(wayland, !wayland, true)
                .into_iter()
                .next()
                .unwrap()
        };
        copy(&words("first"), "text/plain");
        // Nothing copied between: unchanged.
        let reader = own();
        assert!(matches!((reader.list)(), Listing::Types(_)));
        assert_eq!((reader.changed)(), Some(false), "a change with no copy");
        // A copy between its list and its check.
        let reader = own();
        assert!(matches!((reader.list)(), Listing::Types(_)));
        copy(&words("second"), "text/plain");
        assert_eq!((reader.changed)(), Some(true), "a copy not seen");
        // A paste whose read is followed by another copy.
        let base = own();
        let third = words("third");
        let wrapped = Reader {
            name: base.name.clone(),
            list: Box::new(|| (base.list)()),
            read: Box::new(|t, cap| {
                let got = (base.read)(t, cap);
                copy(&third, "text/plain");
                got
            }),
            changed: Box::new(|| (base.changed)()),
        };
        let none = |_: &str, _: &[&str]| -> std::io::Result<Captured> {
            Err(std::io::ErrorKind::NotFound.into())
        };
        let got = clipboard_here(wayland, !wayland, &[wrapped], &none, Room::left(&[]));
        assert_eq!(got, Err(COPIED_AGAIN.to_string()));
        // And one with no copy after it reads its words.
        let got = clipboard_here(wayland, !wayland, &[own()], &none, Room::left(&[]));
        assert_eq!(got, Ok(Found::Text("third words".into())));
        if let Some(image) = std::env::var_os("KERDR_LIVE_CLIPBOARD_IMAGE") {
            copy(Path::new(&image), "image/png");
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    #[ignore]
    fn live_x11_read_stops_at_its_cap() {
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            eprintln!("skipped: no X11 display");
            return;
        }
        let path = std::env::var("KERDR_LIVE_CLIPBOARD_IMAGE").expect("the image on the clipboard");
        let whole = std::fs::read(path).unwrap().len();
        let cap = whole / 2;
        let Ok((bytes, _)) = builtin::x11_get("image/png", cap) else {
            panic!("not read")
        };
        assert_eq!(bytes.len(), cap + 1, "kept past the cap");
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
