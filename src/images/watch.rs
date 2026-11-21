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
//! becomes unknown, its thread is stopped, and readers watch from their own start again. Windows
//! numbers copies itself (GetClipboardSequenceNumber), read directly; macOS has no watch.
//!
//! The thread stops with its owner: when the watch is dropped, when it's retired, and when it
//! isn't set up within START_BOUND. Under X11 that shuts its socket, waking whatever it waits on,
//! its first answer from the server included; under Wayland it's the watch's own cancel handle.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How long the watch may take to set up before it's given up and its thread stopped.
pub const START_BOUND: Duration = Duration::from_secs(3);
/// How long a paste's check may take to bring the watch up to date.
pub const SYNC_BOUND: Duration = Duration::from_secs(1);
/// Under Wayland, how long a check waits for notices already sent to come in.
#[cfg(not(any(target_os = "macos", windows)))]
const WAYLAND_SETTLE: Duration = Duration::from_millis(50);

/// Kerdr's clipboard watch. Clones share one watch; it stops when the last is dropped.
#[derive(Clone)]
pub struct ClipboardWatch {
    owner: Arc<Owner>,
}

/// A paste's copy, as the watch numbered it when the paste was handled.
#[derive(Clone)]
pub struct Pasted {
    watch: ClipboardWatch,
    copy: u64,
}

/// What stops the watch once nothing holds it.
struct Owner(Arc<Shared>);

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// Brings the count up to date within the bound: true once every notice sent before it is in.
type SyncFn = Box<dyn Fn(Duration) -> bool + Send + Sync>;
/// What stops the watch's thread.
type Stop = Box<dyn FnOnce() + Send>;
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
    stopped: AtomicBool,
    stops: Mutex<Vec<Stop>>,
    /// The thread has ended.
    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    ended: AtomicBool,
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
    /// Set up: copies are counted from here. False when it's been stopped meanwhile.
    fn ready(&self, sync: SyncFn) -> bool {
        let _ = self.sync.set(sync);
        self.copies.store(0, SeqCst);
        let stops = self.stops.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(SeqCst) {
            return false;
        }
        self.known.store(true, SeqCst);
        drop(stops);
        true
    }

    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    /// One copy the desktop told of.
    pub(crate) fn copied(&self) {
        self.copies.fetch_add(1, SeqCst);
    }

    /// Its count unknown from now, and its thread stopped.
    fn retire(&self) {
        self.known.store(false, SeqCst);
        self.stop();
    }

    fn stop(&self) {
        let stops = {
            let mut stops = self.stops.lock().unwrap_or_else(|e| e.into_inner());
            self.known.store(false, SeqCst);
            self.stopped.store(true, SeqCst);
            std::mem::take(&mut *stops)
        };
        for stop in stops {
            stop();
        }
    }

    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    /// Runs `stop` when the watch stops; at once, and false, if it has already.
    fn on_stop(&self, stop: Stop) -> bool {
        let mut stops = self.stops.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(SeqCst) {
            drop(stops);
            stop();
            return false;
        }
        stops.push(stop);
        true
    }

    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    fn stopped(&self) -> bool {
        self.stopped.load(SeqCst)
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
        ClipboardWatch {
            owner: Arc::new(Owner(shared)),
        }
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
                ClipboardWatch::running(Box::new(wayland), START_BOUND)
            } else if set("DISPLAY") {
                ClipboardWatch::running(Box::new(|shared| x11(shared, None)), START_BOUND)
            } else {
                ClipboardWatch::with(Arc::default())
            }
        }
    }

    /// A watch whose thread runs `watch`, given up and stopped if it isn't set up within `bound`.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn running(watch: Watching, bound: Duration) -> ClipboardWatch {
        let shared = Arc::new(Shared::default());
        let theirs = shared.clone();
        let started = std::thread::Builder::new()
            .name("kerdr-clipboard-watch".into())
            .spawn(move || {
                watch(&theirs);
                theirs.retire();
                theirs.ended.store(true, SeqCst);
            });
        if started.is_err() {
            shared.ended.store(true, SeqCst);
            return ClipboardWatch::with(shared);
        }
        // Not set up in time: given up, its thread stopped with it. The timer holds the watch
        // only weakly, so it never keeps one alive.
        let late = Arc::downgrade(&shared);
        let _ = std::thread::Builder::new()
            .name("kerdr-clipboard-watch-start".into())
            .spawn(move || {
                std::thread::sleep(bound);
                if let Some(shared) = late.upgrade()
                    && !shared.known.load(SeqCst)
                {
                    shared.retire();
                }
            });
        ClipboardWatch::with(shared)
    }

    /// An X11 watch on the display at the socket `path`, given up after `bound`, for tests.
    #[cfg(all(test, not(any(target_os = "macos", windows))))]
    pub(crate) fn x11_at(path: &std::path::Path, bound: Duration) -> ClipboardWatch {
        let path = path.to_path_buf();
        ClipboardWatch::running(Box::new(move |shared| x11(shared, Some(&path))), bound)
    }

    /// A watch with no thread, its notices given by the test, already set up.
    #[cfg(test)]
    pub(crate) fn counted() -> (ClipboardWatch, Notices) {
        let shared = Arc::new(Shared::default());
        shared.known.store(true, SeqCst);
        (ClipboardWatch::with(shared.clone()), Notices(shared))
    }

    /// Whether the watch's thread has ended.
    #[cfg(all(test, not(any(target_os = "macos", windows))))]
    pub(crate) fn ended(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let shared = self.owner.0.clone();
        Arc::new(move || shared.ended.load(SeqCst))
    }

    /// The copy the clipboard holds now, as far as the watch has been told: an atomic load.
    /// None until it's set up, or once it's retired.
    pub fn now(&self) -> Option<u64> {
        self.owner.0.now()
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
        let shared = &self.watch.owner.0;
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

/// Kerdr's X11 watch: XFixes' notice of each change of the CLIPBOARD selection, counted. The
/// socket is Kerdr's from before the first word to the server, so stopping the watch shuts it,
/// and the thread wakes from any wait, its first answer included.
#[cfg(not(any(target_os = "macos", windows)))]
fn x11(shared: &Arc<Shared>, socket_at: Option<&std::path::Path>) {
    use std::os::fd::{AsFd, AsRawFd};
    use std::sync::Condvar;
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
    let Ok(socket) = stream.as_fd().try_clone_to_owned() else {
        return;
    };
    let shut = Box::new(move || {
        // SAFETY: shuts down the socket this duplicate refers to; the duplicate is Kerdr's own
        // and still open here.
        unsafe { libc::shutdown(socket.as_raw_fd(), libc::SHUT_RDWR) };
        drop(socket);
    });
    if !shared.on_stop(shut) {
        return;
    }
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
    if !shared.ready(sync) {
        return;
    }
    while !shared.stopped() {
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
/// comes (wl-clipboard-rs's watch, whose cancel handle stops it). The first is the clipboard as
/// the watch began. It gives no round trip, so a check waits WAYLAND_SETTLE for notices already
/// sent. wl-clipboard-rs makes its connection itself: a desktop that never answers its first
/// round trip holds this thread until it does, though nothing waits on it (a known limit).
#[cfg(not(any(target_os = "macos", windows)))]
fn wayland(shared: &Arc<Shared>) {
    use wl_clipboard_rs::paste::Seat;
    use wl_clipboard_rs::watch::{ClipboardType, Watcher};
    let Ok(mut watcher) = Watcher::new(ClipboardType::Regular, Seat::Unspecified) else {
        return;
    };
    let cancel = watcher.cancel_handle();
    if !shared.on_stop(Box::new(move || cancel.cancel())) {
        return;
    }
    let mut first = true;
    while !shared.stopped() {
        match watcher.next_event() {
            Ok(Some(_)) if first => {
                first = false;
                let settle: SyncFn = Box::new(|bound| {
                    std::thread::sleep(bound.min(WAYLAND_SETTLE));
                    true
                });
                if !shared.ready(settle) {
                    return;
                }
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

    /// Waits up to `most` for `done`.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn within(most: Duration, done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + most;
        while !done() {
            if Instant::now() > until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

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

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn an_x11_server_that_never_answers_leaves_pastes_instant_and_its_watch_stops() {
        // Codex's review 90, 2 and 3: a display that takes the connection and never answers.
        // Pastes stay instant; the watch, never set up, is given up at its start-up bound and its
        // thread ends; one dropped before then ends at once.
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("X0");
        let listener = UnixListener::bind(&path).unwrap();
        let kept = Arc::new(Mutex::new(vec![]));
        let server = {
            let kept = kept.clone();
            std::thread::spawn(move || {
                for _ in 0..2 {
                    if let Ok((stream, _)) = listener.accept() {
                        kept.lock().unwrap().push(stream);
                    }
                }
            })
        };
        let watch = ClipboardWatch::x11_at(&path, Duration::from_millis(300));
        let ended = watch.ended();
        let began = Instant::now();
        for _ in 0..100 {
            assert!(watch.pasted().is_none());
        }
        assert!(
            began.elapsed() < Duration::from_millis(100),
            "{:?}",
            began.elapsed()
        );
        assert!(
            within(Duration::from_secs(2), || kept.lock().unwrap().len() == 1),
            "it never reached the display"
        );
        assert!(
            within(Duration::from_secs(3), || ended()),
            "the watch's thread never stopped"
        );
        // Dropped while it waits for the server: its thread ends with it.
        let watch = ClipboardWatch::x11_at(&path, Duration::from_secs(60));
        let ended = watch.ended();
        assert!(within(Duration::from_secs(2), || kept
            .lock()
            .unwrap()
            .len()
            == 2));
        drop(watch);
        assert!(
            within(Duration::from_secs(2), || ended()),
            "a dropped watch's thread kept running"
        );
        server.join().unwrap();
    }
}
