//! Kerdr's watch on this desktop's clipboard (Codex's reviews 89 and 90): set up as Kerdr starts,
//! on a thread of its own, and kept running, so a paste's copy is numbered the instant the paste
//! is handled, before any reader is set up, and a copy made at any point since is known.
//!
//! The thread counts each copy the desktop tells it of. A paste reads that count, an atomic load,
//! with no wait and no round trip, so the board never waits on the watch. After the read, on the
//! paste's own worker, the watch is brought up to date, bounded: under X11 by a round trip of its
//! own (a property changed on Kerdr's window, whose notice comes back after every notice the
//! server sent before it); under Wayland, which gives no round trip here, by a short wait. Then
//! the count is compared. A watch that can't be brought up to date in time is retired: its count
//! becomes unknown, and readers watch from their own start again. Windows numbers copies itself
//! (GetClipboardSequenceNumber), read directly; macOS has no watch.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How long a paste's check may take to bring the watch up to date.
pub const SYNC_BOUND: Duration = Duration::from_secs(1);
/// Under Wayland, how long a check waits for notices already sent to come in.
#[cfg(not(any(target_os = "macos", windows)))]
const WAYLAND_SETTLE: Duration = Duration::from_millis(50);

/// Kerdr's clipboard watch. Clones share one watch.
#[derive(Clone)]
pub struct ClipboardWatch {
    shared: Arc<Shared>,
}

/// A paste's copy, as the watch numbered it when the paste was handled.
#[derive(Clone)]
pub struct Pasted {
    watch: ClipboardWatch,
    copy: u64,
}

/// Brings the count up to date within the bound: true once every notice sent before it is in.
type SyncFn = Box<dyn Fn(Duration) -> bool + Send + Sync>;
/// What a watch's thread runs.
#[cfg(not(any(target_os = "macos", windows)))]
type Watching = Box<dyn FnOnce(&Arc<Shared>) + Send>;

/// What the watch's thread and its owner share.
#[derive(Default)]
pub(crate) struct Shared {
    /// Whether `copies` counts from a watch that's set up and running.
    known: AtomicBool,
    copies: AtomicU64,
    sync: OnceLock<SyncFn>,
    /// A number the system keeps itself, read directly (Windows).
    number: OnceLock<Box<dyn Fn() -> Option<u64> + Send + Sync>>,
}

impl Shared {
    fn now(&self) -> Option<u64> {
        if let Some(number) = self.number.get() {
            return number();
        }
        if !self.known.load(SeqCst) {
            return None;
        }
        Some(self.copies.load(SeqCst))
    }

    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    /// Set up: copies are counted from here.
    fn ready(&self, sync: SyncFn) {
        let _ = self.sync.set(sync);
        self.copies.store(0, SeqCst);
        self.known.store(true, SeqCst);
    }

    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    /// One copy the desktop told of.
    pub(crate) fn copied(&self) {
        self.copies.fetch_add(1, SeqCst);
    }

    /// Its count unknown from now.
    fn retire(&self) {
        self.known.store(false, SeqCst);
    }

    fn sync(&self, bound: Duration) -> bool {
        if self.number.get().is_some() {
            return true;
        }
        self.known.load(SeqCst) && self.sync.get().is_some_and(|sync| sync(bound))
    }
}

impl ClipboardWatch {
    fn with(shared: Arc<Shared>) -> ClipboardWatch {
        ClipboardWatch { shared }
    }

    /// This desktop's watch, set up away from the board, which never waits for it: a paste before
    /// it's ready is read as one with no watch.
    pub fn start() -> ClipboardWatch {
        #[cfg(windows)]
        {
            let shared = Arc::new(Shared::default());
            let _ = shared.number.set(Box::new(|| {
                clipboard_win::raw::seq_num().map(|n| u64::from(n.get()))
            }));
            ClipboardWatch::with(shared)
        }
        #[cfg(target_os = "macos")]
        {
            ClipboardWatch::with(Arc::default())
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
            if set("WAYLAND_DISPLAY") {
                ClipboardWatch::running(Box::new(wayland))
            } else if set("DISPLAY") {
                ClipboardWatch::running(Box::new(|shared| x11(shared, None)))
            } else {
                ClipboardWatch::with(Arc::default())
            }
        }
    }

    /// A watch whose thread runs `watch`.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn running(watch: Watching) -> ClipboardWatch {
        let shared = Arc::new(Shared::default());
        let theirs = shared.clone();
        let _ = std::thread::Builder::new()
            .name("kerdr-clipboard-watch".into())
            .spawn(move || watch(&theirs));
        ClipboardWatch::with(shared)
    }

    /// A watch with no thread, its notices given by the test, already set up.
    #[cfg(test)]
    pub(crate) fn counted() -> (ClipboardWatch, Notices) {
        let shared = Arc::new(Shared::default());
        shared.known.store(true, SeqCst);
        (ClipboardWatch::with(shared.clone()), Notices(shared))
    }

    /// The copy the clipboard holds now, as far as the watch has been told: an atomic load.
    /// None until it's set up, or once it's retired.
    pub fn now(&self) -> Option<u64> {
        self.shared.now()
    }

    /// As a paste is handled: the copy it's for, at once.
    pub fn pasted(&self) -> Option<Pasted> {
        Some(Pasted {
            watch: self.clone(),
            copy: self.now()?,
        })
    }
}

impl Pasted {
    /// The number the paste's copy was given.
    pub fn number(&self) -> u64 {
        self.copy
    }

    /// After the read, on the paste's worker: whether he's copied anything since the paste,
    /// every notice sent before now brought in first. None when the watch can't be brought up
    /// to date in time: then it's retired.
    pub fn changed(&self) -> Option<bool> {
        let shared = &self.watch.shared;
        if !shared.sync(SYNC_BOUND) {
            shared.retire();
            return None;
        }
        shared.now().map(|now| now != self.copy)
    }
}

/// A test's way of telling a watch of copies, and of how it's brought up to date.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Notices(Arc<Shared>);

#[cfg(test)]
impl Notices {
    /// One copy's notice, delivered.
    pub(crate) fn copied(&self) {
        self.0.copied();
    }

    /// How a check brings the watch up to date.
    pub(crate) fn synced_by(&self, sync: impl Fn(Duration) -> bool + Send + Sync + 'static) {
        let _ = self.0.sync.set(Box::new(sync));
    }
}

/// Kerdr's X11 watch: XFixes' notice of each change of the CLIPBOARD selection, counted.
#[cfg(not(any(target_os = "macos", windows)))]
fn x11(shared: &Arc<Shared>, socket_at: Option<&std::path::Path>) {
    use std::sync::{Condvar, Mutex};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, WindowClass,
    };
    use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
    use x11rb::reexports::x11rb_protocol::xauth::get_auth;
    use x11rb::rust_connection::{DefaultStream, RustConnection};
    use x11rb::wrapper::ConnectionExt as _;
    // DISPLAY, as x11rb reads it; a test's own socket by its path.
    let (stream, auth, screen) = match socket_at {
        None => {
            let Ok(parsed) = parse_display(None) else {
                return;
            };
            let Some((stream, (family, address))) = parsed
                .connect_instruction()
                .find_map(|at| DefaultStream::connect(&at).ok())
            else {
                return;
            };
            let auth = get_auth(family, &address, parsed.display)
                .ok()
                .flatten()
                .unwrap_or_default();
            (stream, auth, usize::from(parsed.screen))
        }
        Some(path) => {
            let at = ConnectAddress::Socket(path.to_string_lossy().into_owned());
            let Ok((stream, _)) = DefaultStream::connect(&at) else {
                return;
            };
            (stream, Default::default(), 0)
        }
    };
    let (auth_name, auth_data) = auth;
    let Ok(conn) =
        RustConnection::connect_to_stream_with_auth_info(stream, screen, auth_name, auth_data)
    else {
        return;
    };
    let conn = Arc::new(conn);
    let set_up = || -> Option<(u32, u32)> {
        conn.xfixes_query_version(5, 0).ok()?.reply().ok()?;
        let atom = |name: &[u8]| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let (clipboard, ping) = (atom(b"CLIPBOARD")?, atom(b"KERDR_WATCH_PING")?);
        let setup = conn.setup();
        let root = setup.roots.get(screen)?;
        let window = conn.generate_id().ok()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .ok()?
        .check()
        .ok()?;
        conn.xfixes_select_selection_input(
            window,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )
        .ok()?
        .check()
        .ok()?;
        Some((window, ping))
    };
    let Some((window, ping)) = set_up() else {
        return;
    };
    // Each check pings: a property changed on Kerdr's window, in order, one at a time to send.
    // Its notice comes back after every notice the server sent before it, and this thread counts
    // those in order, so once it's answered every copy made before the check is counted.
    let answered = Arc::new((Mutex::new(0u64), Condvar::new()));
    let sync: SyncFn = {
        let (conn, answered, sent) = (conn.clone(), answered.clone(), Mutex::new(0u64));
        Box::new(move |bound| {
            let want = {
                let mut sent = sent.lock().unwrap_or_else(|e| e.into_inner());
                let pinged = conn
                    .change_property8(PropMode::REPLACE, window, ping, AtomEnum::STRING, b"k")
                    .is_ok()
                    && conn.flush().is_ok();
                if !pinged {
                    return false;
                }
                *sent += 1;
                *sent
            };
            let (count, came) = &*answered;
            let count = count.lock().unwrap_or_else(|e| e.into_inner());
            let waited = came.wait_timeout_while(count, bound, |n| *n < want);
            waited.is_ok_and(|(_, timeout)| !timeout.timed_out())
        })
    };
    shared.ready(sync);
    loop {
        match conn.wait_for_event() {
            Ok(Event::XfixesSelectionNotify(_)) => shared.copied(),
            Ok(Event::PropertyNotify(e)) if e.window == window && e.atom == ping => {
                let (count, came) = &*answered;
                *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                came.notify_all();
            }
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

/// Kerdr's Wayland watch: each selection the desktop offers through data-control, counted as it
/// comes (wl-clipboard-rs's watch). The first is the clipboard as the watch began. It gives no
/// round trip, so a check waits WAYLAND_SETTLE for notices already sent.
#[cfg(not(any(target_os = "macos", windows)))]
fn wayland(shared: &Arc<Shared>) {
    use wl_clipboard_rs::paste::Seat;
    use wl_clipboard_rs::watch::{ClipboardType, Watcher};
    let Ok(mut watcher) = Watcher::new(ClipboardType::Regular, Seat::Unspecified) else {
        return;
    };
    let mut first = true;
    loop {
        match watcher.next_event() {
            Ok(Some(_)) if first => {
                first = false;
                let settle: SyncFn = Box::new(|bound| {
                    std::thread::sleep(bound.min(WAYLAND_SETTLE));
                    true
                });
                shared.ready(settle);
            }
            Ok(Some(_)) => shared.copied(),
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{COPIED_AGAIN, Found, UNCHECKED, take_since};
    use std::time::Instant;

    #[test]
    fn a_paste_is_numbered_at_once_and_a_notice_that_comes_late_is_still_told() {
        // Codex's review 90, 1. He copies C as he presses paste: its notice comes 10 ms later,
        // while the paste is handled. The paste's number is taken at once, so C isn't in it; the
        // check after the read brings every notice in first, so C is told: nothing goes in.
        let (watch, notices) = ClipboardWatch::counted();
        // A notice still on its way when the read ends comes in with the check, as a round trip
        // brings in every notice sent before it.
        let on_its_way = Arc::new(AtomicBool::new(false));
        {
            let (notices, on_its_way) = (notices.clone(), on_its_way.clone());
            let theirs = notices.clone();
            theirs.synced_by(move |_| {
                if on_its_way.swap(false, SeqCst) {
                    notices.copied();
                }
                true
            });
        }
        let read = || {
            std::thread::sleep(Duration::from_millis(30));
            Ok(Found::Text("<C>".into()))
        };
        let (go, gone) = std::sync::mpsc::channel();
        let late = {
            let notices = notices.clone();
            std::thread::spawn(move || {
                gone.recv().unwrap();
                std::thread::sleep(Duration::from_millis(10));
                notices.copied();
            })
        };
        go.send(()).unwrap();
        let pasted = watch.pasted().expect("numbered");
        assert_eq!(
            take_since(Some(pasted), read),
            Err(COPIED_AGAIN.to_string()),
            "a copy made as the paste was handled went in"
        );
        late.join().unwrap();
        // He copies during the read, and its notice is still on its way when the read ends.
        let pasted = watch.pasted().expect("numbered");
        let copying = || {
            on_its_way.store(true, SeqCst);
            Ok(Found::Text("<D>".into()))
        };
        assert_eq!(
            take_since(Some(pasted), copying),
            Err(COPIED_AGAIN.to_string()),
            "a notice that came late was missed"
        );
        // No copy since: what was read goes in.
        let pasted = watch.pasted().expect("numbered");
        assert_eq!(
            take_since(Some(pasted), read),
            Ok(Found::Text("<C>".into()))
        );
    }

    #[test]
    fn a_watch_that_stops_answering_is_retired_and_pastes_stay_instant() {
        // Codex's review 90, 2: the watch's server stops answering. A paste's check waits at
        // most SYNC_BOUND, on the paste's worker, then retires the watch: nothing goes in from
        // that paste, and he's told. The board's pastes meanwhile take no time at all, and once
        // it's retired a paste is read as one with no watch.
        let (watch, notices) = ClipboardWatch::counted();
        notices.synced_by(|bound| {
            std::thread::sleep(bound);
            false
        });
        let began = Instant::now();
        let pasted = watch.pasted();
        assert!(
            began.elapsed() < Duration::from_millis(50),
            "the paste waited on the watch: {:?}",
            began.elapsed()
        );
        let pasted = pasted.expect("numbered");
        let check = std::thread::spawn(move || {
            let began = Instant::now();
            let got = take_since(Some(pasted), || Ok(Found::Text("<B>".into())));
            (got, began.elapsed())
        });
        std::thread::sleep(Duration::from_millis(50));
        let began = Instant::now();
        for _ in 0..100 {
            let _ = watch.pasted();
        }
        assert!(
            began.elapsed() < Duration::from_millis(100),
            "pastes waited on the watch: {:?}",
            began.elapsed()
        );
        let (got, took) = check.join().unwrap();
        assert_eq!(got, Err(UNCHECKED.to_string()));
        assert!(took < SYNC_BOUND + Duration::from_millis(500), "{took:?}");
        assert!(
            watch.pasted().is_none(),
            "a retired watch still numbers pastes"
        );
        assert_eq!(
            take_since(watch.pasted(), || Ok(Found::Text("<B>".into()))),
            Ok(Found::Text("<B>".into()))
        );
    }
}
