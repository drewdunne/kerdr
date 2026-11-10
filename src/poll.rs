//! One reader per machine. Each read is a single round trip: a short shell script that prints
//! herdr's version, its sessions, a snapshot of every running session and, when it's due, the
//! card feed. Failures move the machine through the connection states the board shows.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

use crate::config::Machine;
use crate::feed::{self, Feed};
use crate::herdr::{self, SessionInfo, Snapshot};
use crate::transport::{self, HERDR_VARS, PATH_PREFIX, Problem, Target};

/// How a machine is answering.
#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    /// Not heard from yet.
    Connecting,
    Live {
        latency: Duration,
    },
    /// Dropped after answering before; retrying with longer waits.
    Reconnecting {
        since: DateTime<Local>,
        attempt: u32,
        retry_at: DateTime<Local>,
        detail: String,
    },
    /// No answer for two minutes, or never answered.
    Unreachable {
        since: DateTime<Local>,
        retry_at: DateTime<Local>,
        detail: String,
    },
    /// SSH wants a decision about the host key. Kerdr never answers it for you.
    HostKey {
        detail: String,
    },
    /// SSH wants a password, a passphrase or another key.
    Auth {
        detail: String,
    },
    HerdrMissing,
    HerdrTooOld {
        version: String,
    },
    Hidden,
}

impl Conn {
    pub fn is_live(&self) -> bool {
        matches!(self, Conn::Live { .. })
    }

    /// Whether what's wrong has lasted long enough to be said (intent 0591): a drop is quiet for
    /// its first QUIET_FOR seconds, since most come back within a try or two; anything that needs
    /// Andrew, or a machine that can't be reached at all, is said at once.
    pub fn lasting(&self, now: DateTime<Local>) -> bool {
        match self {
            Conn::Connecting | Conn::Live { .. } | Conn::Hidden => false,
            Conn::Reconnecting { since, .. } => (now - *since).num_seconds() >= QUIET_FOR,
            _ => true,
        }
    }

    /// Why it isn't answering, in a few words, for `R` (intent 0591): "SSH timed out, still
    /// trying". None when it is, or when nothing keeps it from answering that Kerdr can name.
    pub fn briefly(&self) -> Option<String> {
        let detail = match self {
            Conn::Reconnecting { detail, .. } | Conn::Unreachable { detail, .. } => detail,
            Conn::HostKey { .. } | Conn::Auth { .. } => return Some("SSH needs you".into()),
            _ => return None,
        };
        let d = detail.to_lowercase();
        let what = if d.contains("timed out") {
            "SSH timed out"
        } else if d.contains("refused") {
            "its connection was refused"
        } else if d.contains("resolve") || d.contains("name or service") {
            "its name can't be found"
        } else if d.contains("no route") || d.contains("unreachable") {
            "there's no route to it"
        } else if d.contains("broken pipe") || d.contains("reset") || d.contains("dropped") {
            "the connection dropped"
        } else {
            "no answer"
        };
        Some(format!("{what}, still trying"))
    }

    pub fn needs_you(&self) -> bool {
        matches!(self, Conn::HostKey { .. } | Conn::Auth { .. })
    }

    pub fn glyph(&self) -> &'static str {
        match self {
            Conn::Connecting => "◌",
            Conn::Live { .. } => "●",
            Conn::Reconnecting { .. } => "⟳",
            Conn::Unreachable { .. } => "○",
            Conn::HostKey { .. } | Conn::Auth { .. } => "●",
            Conn::HerdrMissing | Conn::HerdrTooOld { .. } => "■",
            Conn::Hidden => "⊘",
        }
    }

    /// A few words for a tab or a list.
    pub fn short(&self, now: DateTime<Local>) -> String {
        match self {
            Conn::Connecting => "connecting".into(),
            Conn::Live { .. } => "live".into(),
            Conn::Reconnecting {
                attempt, retry_at, ..
            } => {
                let secs = (*retry_at - now).num_seconds().max(0);
                format!("reconnecting · try {} in {}s", attempt + 1, secs)
            }
            Conn::Unreachable { .. } => "can't reach".into(),
            Conn::HostKey { .. } => "needs you · trust its host key?".into(),
            Conn::Auth { .. } => "needs you · SSH wants a key or password".into(),
            Conn::HerdrMissing => "herdr isn't installed there".into(),
            Conn::HerdrTooOld { version } => format!(
                "herdr {version} · needs {} or newer",
                herdr::version_string(herdr::MIN_VERSION)
            ),
            Conn::Hidden => "hidden".into(),
        }
    }
}

/// What one successful read brought back.
#[derive(Clone, Debug, Default)]
pub struct MachineData {
    pub host: Option<String>,
    pub version: Option<String>,
    pub sessions: Vec<SessionInfo>,
    pub snapshots: BTreeMap<String, Result<Snapshot, String>>,
    /// Whether Kerdr's two keys are in that machine's herdr config (`kerdr herdr-keys --install`).
    pub keys: bool,
    /// None when the feed wasn't due this time; the last one still stands.
    pub feed: Option<Result<Feed, String>>,
    /// Why herdr's list of sessions couldn't be read this time, when it couldn't: the machine
    /// answered, so it's live, and the last list stands (intent 0591).
    pub sessions_failed: Option<String>,
    /// A feed read that failed after the good one above, when two reads were merged into one.
    pub feed_failed_after: Option<String>,
    /// The same for a session: its last good snapshot, where a later read in the merge failed.
    pub kept_snapshots: BTreeMap<String, Snapshot>,
    /// When each snapshot here, or in `kept_snapshots`, that an earlier read brought into a merge
    /// was read: it's shown as of then, never as of this read (intent 0591). One not named here was
    /// read with this data.
    pub read_at: BTreeMap<String, DateTime<Local>>,
    /// When the list of sessions here was read, for one an earlier read brought into a merge
    /// because a newer read of herdr's list failed: it's that read's list, an empty one too, and
    /// the board shows it rather than its own older one (intent 0591). None: read with this data,
    /// or not read at all (`sessions_failed`).
    pub sessions_read_at: Option<DateTime<Local>>,
    /// What herdr or the feed printed, as it printed it, where a read of it failed: for the error
    /// log alone, never the board, whose words are Kerdr's own (intent 0591). Keyed as the board's
    /// problems are: `feed`, `sessions`, `session <name>`.
    pub said: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Event {
    pub machine: usize,
    pub conn: Conn,
    pub data: Option<MachineData>,
    /// When `data` was read.
    pub at: DateTime<Local>,
}

/// A newer read in place of one the board hasn't applied yet. It keeps the older read's feed
/// when it brought none, and the older read's data when it brought none (a dropped connection),
/// so the board ends up where applying both in turn would have left it.
fn replace(old: Event, mut new: Event) -> Event {
    match (&mut new.data, old.data) {
        (Some(data), Some(mut old_data)) => {
            // What the older read's failures printed stands where the newer's didn't say again.
            for (what, printed) in std::mem::take(&mut old_data.said) {
                data.said.entry(what).or_insert(printed);
            }
            // When each of the older read's snapshots was read: with it, or earlier still.
            let old_read_at = std::mem::take(&mut old_data.read_at);
            let read_when = |session: &str| old_read_at.get(session).copied().unwrap_or(old.at);
            // The newer read has no list of sessions where the older had one, good or carried on
            // from a good one before it: that list stands, an empty one too, through any number of
            // failures, as of when it was read (intent 0591).
            let old_has_list =
                old_data.sessions_failed.is_none() || old_data.sessions_read_at.is_some();
            if data.sessions_failed.is_some() && data.sessions_read_at.is_none() && old_has_list {
                data.sessions = old_data.sessions.clone();
                data.snapshots = old_data.snapshots.clone();
                data.sessions_read_at = Some(old_data.sessions_read_at.unwrap_or(old.at));
                for session in data.snapshots.keys() {
                    data.read_at.insert(session.clone(), read_when(session));
                }
            }
            // A session that failed after a good read stays as that read left it, not as the
            // board last showed it (intent 0491).
            let failed = |data: &MachineData, session: &str| {
                matches!(data.snapshots.get(session), Some(Err(_)))
            };
            let good = old_data.kept_snapshots.into_iter().chain(
                old_data
                    .snapshots
                    .into_iter()
                    .filter_map(|(s, snap)| Some((s, snap.ok()?))),
            );
            for (session, snap) in good {
                if failed(data, &session) {
                    data.read_at.insert(session.clone(), read_when(&session));
                    data.kept_snapshots.insert(session, snap);
                }
            }
            match (data.feed.take(), old_data.feed) {
                // No feed this time: the older read's stands, with any failure that followed it.
                (None, old_feed) => {
                    data.feed = old_feed;
                    data.feed_failed_after = old_data.feed_failed_after;
                }
                // A failure after good cards keeps the cards and says it failed, as applying the
                // two reads in turn would have.
                (Some(Err(e)), Some(Ok(feed))) => {
                    data.feed = Some(Ok(feed));
                    data.feed_failed_after = Some(e);
                }
                (new_feed, _) => data.feed = new_feed,
            }
        }
        (None, Some(old_data)) => {
            new.data = Some(old_data);
            new.at = old.at;
        }
        _ => {}
    }
    new
}

/// Each machine's newest read that the board hasn't applied yet. While you're inside herdr
/// nobody applies them, so a newer read takes the place of an older one: coming back, the board
/// gets each machine as it is now, never a queue of what it was, and nothing new is dropped.
#[derive(Default)]
pub struct Inbox {
    /// Per machine, under one lock so a reader can't be retired between its check and its read
    /// landing.
    slots: Mutex<Vec<Slot>>,
    arrived: Condvar,
    /// Told of every feed a machine's reader brings, as it lands, whether or not the board is
    /// there to apply it: the runner starts work waiting on a write-up from it while Andrew is
    /// inside a workspace (intent 0445).
    watch: Mutex<Option<FeedWatch>>,
}

type FeedWatch = Box<dyn Fn(usize, &crate::feed::Feed) + Send>;

#[derive(Default)]
struct Slot {
    /// Which reader's reads count. A reader that was stopped (a machine hidden, then shown
    /// again) may still be mid-read; what it finds is older than its replacement's.
    reader: u64,
    /// The read, and when it began.
    read: Option<(Event, Instant)>,
    /// A read with the feed in it is running: nothing it finds lands for seconds, so nothing
    /// waiting on this machine's next read gains from waiting (intent 0491).
    reading_feed: bool,
    /// When the newest read that brought the feed began, even one a newer read has merged into.
    feed_started: Option<Instant>,
}

impl Inbox {
    fn slots(&self, i: usize) -> MutexGuard<'_, Vec<Slot>> {
        // A reader that panicked can only have left whole reads behind.
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.len() <= i {
            slots.resize_with(i + 1, Slot::default);
        }
        slots
    }

    /// A new reader for machine `i`: from now on only its reads count, and an older reader's
    /// unread one is dropped. Also called when a reader stops, so nothing it was still reading
    /// lands.
    pub fn next_reader(&self, i: usize) -> u64 {
        let mut slots = self.slots(i);
        slots[i].reader += 1;
        slots[i].read = None;
        slots[i].reading_feed = false;
        slots[i].feed_started = None;
        slots[i].reader
    }

    /// Reader `reader` is about to read machine `i`'s feed. `put` ends it, whatever the read found.
    pub fn reading_feed(&self, i: usize, reader: u64) {
        let mut slots = self.slots(i);
        if slots[i].reader == reader {
            slots[i].reading_feed = true;
            self.arrived.notify_all();
        }
    }

    /// A read that began at `started`, from reader `reader` (see `next_reader`).
    pub fn put(&self, ev: Event, started: Instant, reader: u64) {
        let i = ev.machine;
        let mut slots = self.slots(i);
        if slots[i].reader != reader {
            return;
        }
        slots[i].reading_feed = false;
        if ev.data.as_ref().is_some_and(|d| d.feed.is_some()) {
            slots[i].feed_started = Some(started);
        }
        if let Some(Some(Ok(feed))) = ev.data.as_ref().map(|d| d.feed.as_ref())
            && let Some(watch) = self
                .watch
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
        {
            watch(i, feed);
        }
        let ev = match slots[i].read.take() {
            Some((old, _)) => replace(old, ev),
            None => ev,
        };
        slots[i].read = Some((ev, started));
        self.arrived.notify_all();
    }

    /// Whether a read of machine `i`'s feed that began at `since` or later has landed, good or not.
    pub fn feed_read_since(&self, i: usize, since: Instant) -> bool {
        self.slots(i)[i].feed_started.is_some_and(|t| t >= since)
    }

    /// Has `watch` told of each machine's feed as its reader brings it, from the reader's thread.
    pub fn watch_feeds(&self, watch: impl Fn(usize, &crate::feed::Feed) + Send + 'static) {
        *self.watch.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(watch));
    }

    /// Every read waiting, in machine order.
    pub fn take(&self) -> Vec<Event> {
        self.slots(0)
            .iter_mut()
            .filter_map(|s| s.read.take().map(|(ev, _)| ev))
            .collect()
    }

    /// Waits until each of `machines` has a read waiting that began at `since` or later, or is in
    /// the middle of a feed read, which takes seconds and so can't bring anything fresher in time;
    /// or until `timeout` passes. Says whether it ended before `timeout`. Going on while a feed
    /// runs chooses from what that machine last said, at most a few seconds old (intent 0491).
    /// Only for machines already read once: the callers' are all live.
    pub fn wait_since(&self, machines: &[usize], since: Instant, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut slots = self.slots(0);
        loop {
            let fresh = machines.iter().all(|&i| {
                slots.get(i).is_some_and(|s| {
                    s.reading_feed || matches!(&s.read, Some((_, started)) if *started >= since)
                })
            });
            let left = deadline.saturating_duration_since(Instant::now());
            if fresh || left.is_zero() {
                return fresh;
            }
            slots = self
                .arrived
                .wait_timeout(slots, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

pub enum Wake {
    Now,
    Feed,
    Stop,
}

/// What the wakes waiting ask of the reader, all taken at once: after every change that succeeds
/// each machine gets a feed wake, and read one per read they'd queue into feed reads back to back
/// (intent 0491).
#[derive(Debug, Default, PartialEq)]
struct Asked {
    /// What herdr shows now, which takes tens of milliseconds.
    now: bool,
    /// The feed too, which takes seconds.
    feed: bool,
    stop: bool,
}

impl Asked {
    fn gather(first: Wake, rest: &Receiver<Wake>) -> Asked {
        let mut asked = Asked::default();
        for wake in std::iter::once(first).chain(rest.try_iter()) {
            match wake {
                Wake::Now => asked.now = true,
                Wake::Feed => asked.feed = true,
                Wake::Stop => asked.stop = true,
            }
        }
        asked
    }
}

pub struct Poller {
    pub wake: Sender<Wake>,
}

pub struct Timing {
    pub every: Duration,
    pub feed_every: Duration,
}

/// The longest wait between tries once a connection drops, so a laptop back from sleep has its
/// machines on the board again within seconds.
pub const RETRY_CAP: Duration = Duration::from_secs(15);

/// A machine whose SSH needs you (a host key, a password) waits longer: only a person fixes it.
const NEEDS_YOU_WAIT: Duration = Duration::from_secs(120);

pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2u64 << attempt.min(3)).min(RETRY_CAP)
}

const GIVE_UP_AFTER: i64 = 120;

/// How long, in seconds, a machine that dropped, or a herdr session or a feed that fails, stays
/// quiet before the board says why (intent 0591): the last good state shows meanwhile.
pub const QUIET_FOR: i64 = 30;

/// How many failed feed reads in a row the board waits for before it says the feed is failing,
/// unless QUIET_FOR passes first. A failed read is read again at once, once.
pub const FEED_QUIET_READS: u32 = 3;

/// How soon a failed feed read is read again.
const FEED_REREAD: Duration = Duration::from_secs(1);

pub fn spawn(
    index: usize,
    machine: Machine,
    timing: Timing,
    inbox: Arc<Inbox>,
    reader: u64,
) -> Poller {
    let (wake_tx, wake_rx) = mpsc::channel();
    thread::spawn(move || {
        let target = match &machine.ssh {
            Some(host) => Target::Ssh(host.clone()),
            None => Target::Local,
        };
        let mut attempt = 0u32;
        let mut first_failure: Option<DateTime<Local>> = None;
        let mut ever_live = false;
        let mut last_feed: Option<std::time::Instant> = None;
        let mut fed_once = false;
        // A wake that wants what herdr shows now: that first, with any feed owed straight after,
        // rather than making it wait seconds behind the feed. Never a machine's first read, or
        // an empty board would say there are no cards before its feed has come (intent 0491).
        let mut quick_first = false;
        // A failed feed read is read again at once, once in a run of failures (intent 0591): a
        // cut-off read is most often a dropped connection.
        let mut feed_reread = false;
        let mut list_most = SESSION_MOST;
        loop {
            let feed_owed = machine.feed.is_some()
                && last_feed.is_none_or(|t| t.elapsed() >= timing.feed_every);
            let feed_due = feed_owed && !quick_first;
            if feed_due {
                inbox.reading_feed(index, reader);
            }
            let started = Instant::now();
            let out = collect(&machine, feed_due, list_most);
            let now = Local::now();
            let (conn, data, wait) = match read(&target, &out, feed_due) {
                Ok(data) => {
                    // A list that ran out of time gets longer next pass (intent 0591).
                    list_most = if out.stdout.contains(SESSIONS_SILENT) {
                        SESSIONS_MOST_AFTER_SILENCE
                    } else {
                        SESSION_MOST
                    };
                    attempt = 0;
                    first_failure = None;
                    ever_live = true;
                    let feed_failed = matches!(data.feed, Some(Err(_)));
                    let again = feed_due && feed_failed && !feed_reread;
                    // The inbox keeps this feed even if a newer read takes this one's place.
                    if feed_due {
                        last_feed = (!again).then(Instant::now);
                        fed_once = true;
                        feed_reread = feed_failed;
                    }
                    let wait = if again {
                        FEED_REREAD
                    } else if feed_owed && !feed_due {
                        Duration::ZERO
                    } else {
                        timing.every
                    };
                    (Conn::Live { latency: out.took }, Some(data), wait)
                }
                Err(Failure::Herdr(conn)) => (conn, None, RETRY_CAP),
                Err(Failure::Ssh(problem)) => {
                    attempt += 1;
                    let since = *first_failure.get_or_insert(now);
                    let wait = backoff(attempt - 1);
                    let retry_at = now + chrono::Duration::from_std(wait).unwrap_or_default();
                    let detail = problem.detail().to_string();
                    let conn = match problem {
                        Problem::HostKey(detail) => Conn::HostKey { detail },
                        Problem::Auth(detail) => Conn::Auth { detail },
                        _ if !ever_live && attempt >= 2 => Conn::Unreachable {
                            since,
                            retry_at,
                            detail,
                        },
                        _ if (now - since).num_seconds() >= GIVE_UP_AFTER => Conn::Unreachable {
                            since,
                            retry_at,
                            detail,
                        },
                        _ => Conn::Reconnecting {
                            since,
                            attempt,
                            retry_at,
                            detail,
                        },
                    };
                    let wait = if conn.needs_you() {
                        NEEDS_YOU_WAIT
                    } else {
                        wait
                    };
                    (conn, None, wait)
                }
            };
            inbox.put(
                Event {
                    machine: index,
                    conn,
                    data,
                    at: now,
                },
                started,
                reader,
            );
            let asked = match wake_rx.recv_timeout(wait) {
                Ok(first) => Asked::gather(first, &wake_rx),
                Err(RecvTimeoutError::Timeout) => Asked::default(),
                Err(RecvTimeoutError::Disconnected) => return,
            };
            if asked.stop {
                return;
            }
            if asked.feed {
                last_feed = None;
            }
            quick_first = asked.now && fed_once;
        }
    });
    Poller { wake: wake_tx }
}

enum Failure {
    Ssh(Problem),
    Herdr(Conn),
}

const MARK: &str = "@@KERDR ";

/// The longest one herdr session has to answer: one that's stuck must not take the whole read, and
/// with it its machine, off the board (intent 0491). Generous, since LAB-Server's load spikes.
pub const SESSION_MOST: Duration = Duration::from_secs(5);

/// What a feed cut off by its 12-second limit fails with.
pub const FEED_CUT_OFF: &str = "it took over 12 s and was cut off";

/// What a feed that printed nothing at all fails with.
pub const FEED_EMPTY: &str = "it printed nothing";

/// What a session that ran out of that time shows in its row.
pub const SESSION_SILENT: &str = "this herdr session didn't answer in 5 s";

/// What the machine shows when herdr's list of sessions ran out of that time: no list, no read.
pub const SESSIONS_SILENT: &str = "herdr didn't list its sessions in 5 s";

/// How long that list gets on the read after one that ran out of time (intent 0591): herdr slow
/// under load isn't the machine dropping.
const SESSIONS_MOST_AFTER_SILENCE: Duration = Duration::from_secs(10);

/// Sets the command that keeps herdr to the time in M.
const LIMIT_TO_M: &str = "T=limited; command -v timeout >/dev/null 2>&1 && T=\"timeout $M\"\n";

/// That limit, for machines with no `timeout` command, macOS among them: the command in the
/// background and a watchdog that stops it, with `timeout`'s exit status. The watchdog's output
/// goes nowhere, or it would hold the read open, and what the shell says of a job it lost isn't let
/// into the answer.
const WATCHDOG: &str = r#"limited() {
  "$@" & p=$!
  ( sleep "$M" & z=$!; trap 'kill $z 2>/dev/null; exit 0' TERM; wait $z; kill $p ) >/dev/null 2>&1 &
  w=$!
  { wait $p; } 2>/dev/null; r=$?
  { kill $w; wait $w; } 2>/dev/null
  [ $r = 143 ] && return 124
  return $r
}
"#;

/// The script one read runs on the machine.
pub fn build_script(herdr: &str, feed: Option<&str>) -> String {
    build_script_within(herdr, feed, SESSION_MOST)
}

/// As build_script, herdr's list of sessions given `list_most` to answer.
fn build_script_within(herdr: &str, feed: Option<&str>, list_most: Duration) -> String {
    let mut s = String::new();
    s.push_str(PATH_PREFIX);
    s.push('\n');
    s.push_str(&format!("unset {}\n", HERDR_VARS.join(" ")));
    s.push_str(&format!("H={}\n", transport::herdr_word(herdr)));
    s.push_str(&format!("M={}\n", list_most.as_secs()));
    s.push_str(WATCHDOG);
    s.push_str(LIMIT_TO_M);
    s.push_str(&format!("echo '{MARK}host'; uname -n\n"));
    s.push_str(&format!(
        "if ! command -v \"$H\" >/dev/null 2>&1; then echo '{MARK}missing'; exit 0; fi\n"
    ));
    s.push_str(&format!("echo '{MARK}version'; \"$H\" --version 2>&1\n"));
    s.push_str(&format!(
        "echo '{MARK}keys'; grep -q kerdr_jump \"${{HERDR_CONFIG_PATH:-$HOME/.config/herdr/config.toml}}\" 2>/dev/null && echo yes || echo no\n"
    ));
    s.push_str(&format!(
        "S=$($T \"$H\" session list --json 2>&1) || {{ [ $? = 124 ] && S={}; }}; echo '{MARK}sessions'; printf '%s\\n' \"$S\"\n",
        transport::quote(SESSIONS_SILENT)
    ));
    s.push_str(&format!("M={}\n", SESSION_MOST.as_secs()));
    s.push_str(LIMIT_TO_M);
    // Session names only split into words here; never expanded as file patterns.
    s.push_str("set -f\n");
    s.push_str(
        "for s in $(printf '%s' \"$S\" | tr '{' '\\n' | grep '\"running\":true' | sed -n 's/.*\"name\":\"\\([^\"]*\\)\".*/\\1/p'); do\n",
    );
    s.push_str(&format!(
        "  echo \"{MARK}snapshot $s\"; $T \"$H\" --session \"$s\" api snapshot 2>&1 || {{ [ $? = 124 ] && echo {}; }}\ndone\n",
        transport::quote(SESSION_SILENT)
    ));
    if let Some(cmd) = feed {
        s.push_str(&format!("KERDR_FEED={}\n", transport::quote(cmd)));
        // A feed cut off by its time limit says so, rather than looking empty or half-written,
        // and one that didn't end its line can't hide the marker after it (intent 0591).
        s.push_str(&format!(
            "echo '{MARK}feed'; cd \"$HOME\" && if command -v timeout >/dev/null 2>&1; then timeout 12 sh -c \"$KERDR_FEED\" </dev/null 2>/dev/null; else sh -c \"$KERDR_FEED\" </dev/null 2>/dev/null; fi; r=$?; echo; [ $r = 124 ] && printf '{MARK}feed_error\\n%s\\n' {}\n",
            transport::quote(FEED_CUT_OFF)
        ));
    }
    s.push_str(&format!("echo '{MARK}end'\n"));
    s
}

#[derive(Debug, Default, PartialEq)]
pub struct Sections {
    pub host: Option<String>,
    pub missing: bool,
    pub version: Option<String>,
    pub sessions: Option<String>,
    pub keys: Option<bool>,
    pub snapshots: Vec<(String, String)>,
    pub feed: Option<String>,
    pub feed_error: Option<String>,
    pub complete: bool,
}

pub fn split_sections(text: &str) -> Sections {
    let mut out = Sections::default();
    let mut current: Option<(String, String)> = None;
    let flush = |cur: Option<(String, String)>, out: &mut Sections| {
        if let Some((name, body)) = cur {
            let body = body.trim_end().to_string();
            match name.as_str() {
                "host" => out.host = Some(body.trim().to_string()),
                "version" => out.version = Some(body.trim().to_string()),
                "sessions" => out.sessions = Some(body),
                "keys" => out.keys = Some(body.trim() == "yes"),
                "feed" => out.feed = Some(body),
                "feed_error" => out.feed_error = Some(body),
                other => {
                    if let Some(session) = other.strip_prefix("snapshot ") {
                        out.snapshots.push((session.to_string(), body));
                    }
                }
            }
        }
    };
    for line in text.lines() {
        if let Some(name) = line.strip_prefix(MARK) {
            flush(current.take(), &mut out);
            match name.trim() {
                "missing" => out.missing = true,
                "end" => out.complete = true,
                n => current = Some((n.to_string(), String::new())),
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(current.take(), &mut out);
    out
}

fn read(target: &Target, out: &transport::Output, feed_due: bool) -> Result<MachineData, Failure> {
    if let Some(problem) = transport::classify(target, out) {
        return Err(Failure::Ssh(problem));
    }
    let sections = split_sections(&out.stdout);
    if sections.missing {
        return Err(Failure::Herdr(Conn::HerdrMissing));
    }
    if !sections.complete && sections.sessions.is_none() {
        let detail = out
            .stderr
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("the connection dropped partway through a read")
            .trim()
            .to_string();
        return Err(Failure::Ssh(Problem::Other(detail)));
    }
    let version_text = sections.version.clone().unwrap_or_default();
    if let Some(v) = herdr::parse_version(&version_text)
        && !herdr::version_ok(v)
    {
        return Err(Failure::Herdr(Conn::HerdrTooOld {
            version: herdr::version_string(v),
        }));
    }
    // What herdr or the feed printed where a read failed: the error log's, never the board's.
    let mut said = BTreeMap::new();
    // A list that can't be read is herdr's trouble, not the machine's, which has just answered.
    let (sessions, sessions_failed) =
        match herdr::parse_sessions(sections.sessions.as_deref().unwrap_or("")) {
            Ok(sessions) => (sessions, None),
            Err(e) => {
                let (words, printed) = sessions_trouble(&e);
                if let Some(printed) = printed {
                    said.insert("sessions".to_string(), printed);
                }
                (vec![], Some(words))
            }
        };
    let snapshots = sections
        .snapshots
        .into_iter()
        .map(|(name, body)| {
            let snapshot = herdr::parse_snapshot(&body).map_err(|e| {
                let (words, printed) = snapshot_trouble(&e);
                if let Some(printed) = printed {
                    said.insert(format!("session {name}"), printed);
                }
                words
            });
            (name, snapshot)
        })
        .collect();
    let feed = if feed_due {
        Some(match (sections.feed_error, sections.feed.as_deref()) {
            (Some(error), _) => Err(error),
            (_, Some(body)) if !body.trim().is_empty() => {
                feed::parse_keeping(body).map_err(|unread| {
                    if let Some(printed) = unread.printed {
                        said.insert("feed".to_string(), printed);
                    }
                    unread.words
                })
            }
            _ => Err(FEED_EMPTY.to_string()),
        })
    } else {
        None
    };
    Ok(MachineData {
        host: sections.host,
        version: herdr::parse_version(&version_text).map(herdr::version_string),
        sessions,
        snapshots,
        keys: sections.keys.unwrap_or(false),
        feed,
        sessions_failed,
        feed_failed_after: None,
        kept_snapshots: Default::default(),
        read_at: Default::default(),
        sessions_read_at: None,
        said,
    })
}

/// How much of what herdr printed in place of an answer goes to the error log.
const MOST_PRINTED: usize = 1024;

/// What herdr printed, made short for the error log.
fn printed(error: &str) -> String {
    error.trim().chars().take(MOST_PRINTED).collect()
}

/// Why herdr's list of sessions failed, in words for the board: herdr's trouble, not the
/// machine's, in Kerdr's own words, and what herdr printed, when it printed something, for the
/// error log (intent 0591): its error codes and paths never reach the board.
fn sessions_trouble(error: &str) -> (String, Option<String>) {
    let error = error.trim();
    if error.is_empty() || error == "herdr printed nothing" {
        return (
            "herdr printed nothing for its list of sessions".into(),
            None,
        );
    }
    if error == SESSIONS_SILENT {
        return (
            "herdr is slow there: it didn't list its sessions in time".into(),
            None,
        );
    }
    let lower = error.to_lowercase();
    let setup =
        if lower.contains("version") || lower.contains("mismatch") || lower.contains("differ") {
            " On the machines screen (M), u runs herdr's own setup there."
        } else {
            ""
        };
    (
        format!(
            "herdr answered its list of sessions with something Kerdr can't read (what it printed is in Kerdr's error log).{setup}"
        ),
        Some(printed(error)),
    )
}

/// Why a herdr session's snapshot failed, as sessions_trouble says it of the list: Kerdr's own
/// words stay as they are, and anything herdr printed goes to the error log alone.
fn snapshot_trouble(error: &str) -> (String, Option<String>) {
    let error = error.trim();
    if [
        SESSION_SILENT,
        "herdr printed nothing",
        "herdr's snapshot couldn't be read",
    ]
    .contains(&error)
    {
        return (error.to_string(), None);
    }
    (
        "herdr answered with something Kerdr can't read (what it printed is in Kerdr's error log)"
            .into(),
        Some(printed(error)),
    )
}

fn collect(machine: &Machine, feed_due: bool, list_most: Duration) -> transport::Output {
    #[cfg(windows)]
    if machine.is_local() {
        let _ = list_most;
        return crate::windows::poll(machine, feed_due);
    }
    let target = crate::jump::target(machine);
    let script = build_script_within(
        &machine.herdr,
        machine.feed.as_deref().filter(|_| feed_due),
        list_most,
    );
    transport::run_script(&target, &script, Duration::from_secs(25))
}

/// One read, now, for `kerdr doctor` and `kerdr cards`.
pub fn read_once(machine: &Machine) -> (Conn, Option<MachineData>) {
    let target = match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    };
    let out = collect(machine, machine.feed.is_some(), SESSION_MOST);
    let now = Local::now();
    match read(&target, &out, machine.feed.is_some()) {
        Ok(data) => (Conn::Live { latency: out.took }, Some(data)),
        Err(Failure::Herdr(conn)) => (conn, None),
        Err(Failure::Ssh(Problem::HostKey(detail))) => (Conn::HostKey { detail }, None),
        Err(Failure::Ssh(Problem::Auth(detail))) => (Conn::Auth { detail }, None),
        Err(Failure::Ssh(p)) => (
            Conn::Unreachable {
                since: now,
                retry_at: now,
                detail: p.detail().to_string(),
            },
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_to_fifteen_seconds() {
        let secs: Vec<u64> = (0..9).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 15, 15, 15, 15, 15, 15]);
        assert!((0..100).chain([u32::MAX]).all(|a| backoff(a) <= RETRY_CAP));
    }

    fn live() -> Conn {
        Conn::Live {
            latency: Duration::from_millis(5),
        }
    }

    fn read_of(machine: usize, conn: Conn, host: Option<&str>, feed: bool) -> Event {
        Event {
            machine,
            conn,
            data: host.map(|h| MachineData {
                host: Some(h.into()),
                feed: feed.then(|| Ok(Feed::default())),
                ..Default::default()
            }),
            at: Local::now(),
        }
    }

    fn host(ev: &Event) -> Option<&str> {
        ev.data.as_ref().and_then(|d| d.host.as_deref())
    }

    #[test]
    fn a_newer_read_takes_the_place_of_an_unread_older_one() {
        let inbox = Inbox::default();
        let t = Instant::now();
        inbox.put(read_of(1, live(), Some("before you went in"), false), t, 0);
        inbox.put(read_of(0, live(), Some("another machine"), false), t, 0);
        inbox.put(read_of(1, live(), Some("now"), false), t, 0);
        let reads = inbox.take();
        assert_eq!(reads.len(), 2);
        assert_eq!(
            (reads[0].machine, host(&reads[0])),
            (0, Some("another machine"))
        );
        assert_eq!((reads[1].machine, host(&reads[1])), (1, Some("now")));
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn every_feed_a_current_reader_brings_is_watched_as_it_lands() {
        // Whether or not the board takes it: the runner hears of it while Andrew is inside a
        // workspace (intent 0445).
        let inbox = Inbox::default();
        let (tx, rx) = mpsc::channel();
        inbox.watch_feeds(move |machine, _| {
            let _ = tx.send(machine);
        });
        let t = Instant::now();
        inbox.put(read_of(1, live(), Some("with a feed"), true), t, 0);
        inbox.put(read_of(1, live(), Some("without"), false), t, 0);
        // A retired reader's read never lands, nor is watched.
        inbox.next_reader(0);
        inbox.put(read_of(0, live(), Some("retired"), true), t, 0);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn a_feed_is_never_lost_to_a_newer_read_without_one() {
        let inbox = Inbox::default();
        let t = Instant::now();
        inbox.put(read_of(0, live(), Some("with the feed"), true), t, 0);
        inbox.put(read_of(0, live(), Some("without"), false), t, 0);
        let ev = inbox.take().pop().unwrap();
        assert_eq!(host(&ev), Some("without"));
        assert!(matches!(ev.data.unwrap().feed, Some(Ok(_))));
    }

    #[test]
    fn a_failed_feed_read_never_hides_the_good_one_before_it() {
        let inbox = Inbox::default();
        let t = Instant::now();
        inbox.put(read_of(0, live(), Some("good cards"), true), t, 0);
        let mut failed = read_of(0, live(), Some("feed failed"), false);
        failed.data.as_mut().unwrap().feed = Some(Err("feed timed out".into()));
        inbox.put(failed, t, 0);
        inbox.put(read_of(0, live(), Some("feed not due"), false), t, 0);
        let ev = inbox.take().pop().unwrap();
        assert_eq!(host(&ev), Some("feed not due"));
        let data = ev.data.unwrap();
        assert!(matches!(data.feed, Some(Ok(_))));
        assert_eq!(data.feed_failed_after.as_deref(), Some("feed timed out"));
    }

    #[test]
    fn a_retired_reader_can_never_land_a_read() {
        let inbox = Arc::new(Inbox::default());
        for _ in 0..200 {
            let old = inbox.next_reader(0);
            inbox.put(
                read_of(0, live(), Some("old, unread"), false),
                Instant::now(),
                old,
            );
            let writer = {
                let inbox = inbox.clone();
                std::thread::spawn(move || {
                    inbox.put(
                        read_of(0, live(), Some("old, late"), false),
                        Instant::now(),
                        old,
                    )
                })
            };
            let new = inbox.next_reader(0);
            writer.join().unwrap();
            inbox.put(read_of(0, live(), Some("new"), false), Instant::now(), new);
            let reads = inbox.take();
            assert_eq!(reads.len(), 1);
            assert_eq!(host(&reads[0]), Some("new"));
        }
    }

    #[test]
    fn a_stopped_reader_never_overwrites_its_replacement() {
        let inbox = Inbox::default();
        let t = Instant::now();
        let first = inbox.next_reader(0);
        let second = inbox.next_reader(0);
        inbox.put(read_of(0, live(), Some("new reader"), false), t, second);
        inbox.put(
            read_of(0, live(), Some("old reader, late"), false),
            t,
            first,
        );
        let ev = inbox.take().pop().unwrap();
        assert_eq!(host(&ev), Some("new reader"));
        // Stopping a reader retires it too.
        inbox.next_reader(0);
        inbox.put(read_of(0, live(), Some("stopped"), false), t, second);
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn a_dropped_connection_keeps_the_newer_data_it_replaced() {
        let inbox = Inbox::default();
        let t = Instant::now();
        let good = read_of(0, live(), Some("last good"), true);
        let at = good.at;
        inbox.put(good, t, 0);
        let dropped = Conn::Reconnecting {
            since: Local::now(),
            attempt: 1,
            retry_at: Local::now(),
            detail: "Connection timed out".into(),
        };
        inbox.put(read_of(0, dropped, None, false), t, 0);
        let ev = inbox.take().pop().unwrap();
        assert!(matches!(ev.conn, Conn::Reconnecting { .. }));
        assert_eq!(host(&ev), Some("last good"));
        assert_eq!(ev.at, at);
        assert!(matches!(ev.data.unwrap().feed, Some(Ok(_))));
    }

    #[test]
    fn waiting_for_fresh_reads_ends_when_they_come_or_time_runs_out() {
        let inbox = Arc::new(Inbox::default());
        let old = Instant::now();
        let since = old + Duration::from_millis(1);
        inbox.put(read_of(0, live(), Some("from before"), false), old, 0);
        assert!(!inbox.wait_since(&[0], since, Duration::from_millis(30)));
        let reader = {
            let inbox = inbox.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                inbox.put(read_of(0, live(), Some("fresh"), false), Instant::now(), 0);
            })
        };
        assert!(inbox.wait_since(&[0], since, Duration::from_secs(5)));
        reader.join().unwrap();
        assert_eq!(host(&inbox.take()[0]), Some("fresh"));
    }

    #[test]
    fn waiting_for_fresh_reads_never_waits_out_a_feed_read() {
        // A feed read takes seconds, so a key that meets one goes on from what's known (intent
        // 0491): the wait costs nothing, and a machine that can still answer is still waited for.
        let inbox = Arc::new(Inbox::default());
        let since = Instant::now();
        inbox.reading_feed(0, 0);
        let long = Duration::from_secs(30);
        let t = Instant::now();
        assert!(inbox.wait_since(&[0], since, long));
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(!inbox.wait_since(&[0, 1], since, Duration::from_millis(30)));
        // Whatever the read found ends it.
        inbox.put(
            read_of(0, live(), None, false),
            since - Duration::from_millis(1),
            0,
        );
        assert!(!inbox.wait_since(&[0], since, Duration::from_millis(30)));
        // One that begins during the wait ends the wait: a woken read that's due a feed is one.
        let begins = {
            let inbox = inbox.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                inbox.reading_feed(0, 0);
            })
        };
        let t = Instant::now();
        assert!(inbox.wait_since(&[0], since, long));
        assert!(t.elapsed() < Duration::from_secs(5));
        begins.join().unwrap();
        // A retired reader's says nothing, and a new reader starts with none running.
        inbox.next_reader(0);
        assert!(!inbox.wait_since(&[0], since, Duration::from_millis(30)));
        inbox.reading_feed(0, 0);
        assert!(!inbox.wait_since(&[0], since, Duration::from_millis(30)));
    }

    #[test]
    fn a_feed_read_counts_only_from_when_it_began_and_only_with_its_feed() {
        let inbox = Inbox::default();
        let before = Instant::now();
        let since = before + Duration::from_millis(5);
        let after = since + Duration::from_millis(5);
        assert!(!inbox.feed_read_since(0, since));
        // Already running when R came: what it says may be from before.
        inbox.put(read_of(0, live(), Some("old feed"), true), before, 0);
        assert!(!inbox.feed_read_since(0, since));
        // A later read without the feed says nothing about it.
        inbox.put(read_of(0, live(), Some("no feed"), false), after, 0);
        assert!(!inbox.feed_read_since(0, since));
        inbox.put(read_of(0, live(), Some("new feed"), true), after, 0);
        assert!(inbox.feed_read_since(0, since));
        // Still so once the board has taken it, or a newer read without one has merged with it.
        inbox.put(read_of(0, live(), Some("no feed"), false), after, 0);
        inbox.take();
        assert!(inbox.feed_read_since(0, since));
        // Not for a reader since retired, nor another machine.
        assert!(!inbox.feed_read_since(1, since));
        inbox.next_reader(0);
        assert!(!inbox.feed_read_since(0, since));
    }

    #[test]
    fn wakes_waiting_are_taken_together() {
        let (tx, rx) = mpsc::channel();
        for wake in [Wake::Feed, Wake::Feed, Wake::Now] {
            tx.send(wake).unwrap();
        }
        let first = rx.recv().unwrap();
        let asked = Asked::gather(first, &rx);
        assert_eq!(
            asked,
            Asked {
                now: true,
                feed: true,
                stop: false
            }
        );
        assert!(rx.try_recv().is_err(), "none left for the next read");
        tx.send(Wake::Stop).unwrap();
        tx.send(Wake::Feed).unwrap();
        let first = rx.recv().unwrap();
        assert!(Asked::gather(first, &rx).stop);
    }

    #[cfg(unix)]
    #[test]
    fn a_wake_for_now_reads_without_the_feed_first_and_feed_wakes_make_one_feed_read() {
        // Back from a card: what herdr shows comes in tens of milliseconds, and only then the
        // feed, which takes seconds, however many changes asked for one (intent 0491).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let bin = dir.path().join("herdr");
        std::fs::write(
            &bin,
            format!(
                r#"#!/bin/sh
case "$*" in
  --version) echo read >>"{log}"; echo "herdr 0.9.1";;
  "session list --json") echo '{{"sessions":[]}}';;
esac
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: bin.display().to_string(),
            feed: Some(format!(
                r#"echo feed >>"{}"; sleep 0.5; echo '{{"kerdr_feed":1}}'"#,
                log.display()
            )),
            hidden: false,
        };
        let lines = || -> Vec<String> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        };
        let until = |n: usize| {
            let by = Instant::now() + Duration::from_secs(20);
            while lines().len() < n && Instant::now() < by {
                thread::sleep(Duration::from_millis(5));
            }
        };
        let inbox = Arc::new(Inbox::default());
        let timing = Timing {
            every: Duration::from_secs(600),
            feed_every: Duration::from_secs(3600),
        };
        let poller = spawn(0, machine, timing, inbox.clone(), 0);
        // In the middle of its first read, which is always a feed read.
        until(2);
        assert_eq!(lines(), ["read", "feed"]);
        for wake in [Wake::Now, Wake::Feed, Wake::Feed, Wake::Feed] {
            poller.wake.send(wake).unwrap();
        }
        until(5);
        thread::sleep(Duration::from_millis(800));
        assert_eq!(lines(), ["read", "feed", "read", "read", "feed"]);
        // On its own, a wake for now reads what herdr shows and owes nothing.
        poller.wake.send(Wake::Now).unwrap();
        until(6);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(lines().len(), 6);
        assert_eq!(lines()[5], "read");
        let _ = poller.wake.send(Wake::Stop);
    }

    #[test]
    fn splits_a_read_into_sections() {
        let text = "@@KERDR host\nbuild-box\n@@KERDR version\nherdr 0.9.1\n@@KERDR sessions\n{\"sessions\":[]}\n@@KERDR snapshot default\n{\"a\":1}\n@@KERDR feed\n{\"kerdr_feed\":1}\n@@KERDR end\n";
        let s = split_sections(text);
        assert_eq!(s.host.as_deref(), Some("build-box"));
        assert_eq!(s.version.as_deref(), Some("herdr 0.9.1"));
        assert_eq!(
            s.snapshots,
            vec![("default".to_string(), "{\"a\":1}".to_string())]
        );
        assert_eq!(s.feed.as_deref(), Some("{\"kerdr_feed\":1}"));
        assert!(s.complete);
    }

    #[cfg(unix)]
    /// A stand-in herdr for this machine, which notes each read in `log`, and a machine that reads
    /// it with `feed` as its feed.
    #[cfg(unix)]
    fn noting_machine(dir: &std::path::Path, log: &std::path::Path, feed: &str) -> Machine {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("herdr");
        std::fs::write(
            &bin,
            format!(
                r#"#!/bin/sh
case "$*" in
  --version) echo read >>"{log}"; echo "herdr 0.9.1";;
  "session list --json") echo '{{"sessions":[]}}';;
esac
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Machine {
            name: "here".into(),
            ssh: None,
            herdr: bin.display().to_string(),
            feed: Some(feed.to_string()),
            hidden: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_feed_read_is_read_again_at_once_but_only_once_in_a_row() {
        // A cut-off read is most often a dropped connection: the next one is asked for at once,
        // not at the next turn of the feed (intent 0591).
        let lines = |log: &std::path::Path| -> Vec<String> {
            std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        };
        let timing = || Timing {
            every: Duration::from_secs(600),
            feed_every: Duration::from_secs(3600),
        };
        // Empty the first time, whole the second.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let feed = format!(
            r#"echo feed >>"{l}"; [ "$(grep -c feed "{l}")" -lt 2 ] && exit 0; echo '{{"kerdr_feed":1}}'"#,
            l = log.display()
        );
        let inbox = Arc::new(Inbox::default());
        let poller = spawn(
            0,
            noting_machine(dir.path(), &log, &feed),
            timing(),
            inbox,
            0,
        );
        let by = Instant::now() + Duration::from_secs(20);
        while lines(&log).len() < 4 && Instant::now() < by {
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(1500));
        assert_eq!(lines(&log), ["read", "feed", "read", "feed"]);
        let _ = poller.wake.send(Wake::Stop);
        // Empty every time: read again once, then it waits its turn.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let feed = format!(r#"echo feed >>"{}""#, log.display());
        let inbox = Arc::new(Inbox::default());
        let poller = spawn(
            0,
            noting_machine(dir.path(), &log, &feed),
            timing(),
            inbox,
            0,
        );
        let by = Instant::now() + Duration::from_secs(20);
        while lines(&log).len() < 4 && Instant::now() < by {
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(2500));
        assert_eq!(lines(&log), ["read", "feed", "read", "feed"]);
        let _ = poller.wake.send(Wake::Stop);
    }

    #[cfg(unix)]
    #[test]
    fn a_feed_cut_off_by_its_limit_says_so_and_an_empty_one_says_it_printed_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let run = |feed: &str| {
            let machine = noting_machine(dir.path(), &log, feed);
            let script = build_script(&machine.herdr, machine.feed.as_deref())
                .replace("timeout 12", "timeout 1");
            let out = transport::run_script(&Target::Local, &script, Duration::from_secs(10));
            read(&Target::Local, &out, true)
                .ok()
                .expect("the read works")
                .feed
                .unwrap()
        };
        if std::process::Command::new("sh")
            .args(["-c", "command -v timeout"])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            assert_eq!(
                run(r#"printf '{"kerdr_feed":1,"cards":['; sleep 5"#).unwrap_err(),
                FEED_CUT_OFF
            );
        }
        assert_eq!(run("true").unwrap_err(), FEED_EMPTY);
        assert_eq!(
            run(r#"printf '{"kerdr_feed":1,"cards":['"#).unwrap_err(),
            "its answer was cut off partway (most often the connection dropping)"
        );
        assert!(run(r#"echo '{"kerdr_feed":1}'"#).is_ok());
    }

    #[test]
    fn the_script_finds_running_sessions_without_jq() {
        let script = build_script("herdr", None);
        let fake = r#"#!/bin/sh
case "$*" in
  --version) echo "herdr 0.9.1";;
  "session list --json") echo '{"sessions":[{"default":true,"name":"default","running":true},{"default":false,"name":"idle-one","running":false},{"default":false,"name":"web","running":true}]}';;
  *"api snapshot") echo "{\"result\":{\"snapshot\":{\"workspaces\":[]}}}";;
esac
"#;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, fake).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = script.replace("H=herdr", &format!("H={}", bin.display()));
        let out = transport::run_script(&Target::Local, &script, Duration::from_secs(5));
        let data = read(&Target::Local, &out, false).ok().expect("read works");
        let names: Vec<_> = data.snapshots.keys().cloned().collect();
        assert_eq!(names, ["default", "web"]);
        assert_eq!(data.version.as_deref(), Some("0.9.1"));
    }

    /// The script for a stand-in herdr, with a limit of a second, and either with or without a
    /// `timeout` command to hold it to that.
    #[cfg(unix)]
    fn limited_script(dir: &std::path::Path, fake: &str, has_timeout: bool) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("herdr");
        std::fs::write(&bin, fake).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = build_script("herdr", None)
            .replace("H=herdr", &format!("H={}", bin.display()))
            .replace("M=5\n", "M=1\n");
        if has_timeout {
            script
        } else {
            script.replace("command -v timeout", "command -v no-such-timeout-here")
        }
    }

    #[cfg(unix)]
    const ONE_STUCK: &str = r#"#!/bin/sh
case "$*" in
  --version) echo "herdr 0.9.1";;
  "session list --json") echo '{"sessions":[{"name":"stuck","running":true},{"name":"web","running":true}]}';;
  *"stuck api snapshot") exec sleep 60;;
  *"api snapshot") echo "{\"result\":{\"snapshot\":{\"workspaces\":[]}}}";;
esac
"#;

    #[cfg(unix)]
    #[test]
    fn a_stuck_session_costs_a_read_only_its_own_snapshot() {
        // Not the whole read and with it the machine, as when the snapshots had no limit (intent
        // 0491), and also where there's no `timeout` command, as on a Mac. `exec`: a child left
        // running would hold the read open.
        let dir = tempfile::tempdir().unwrap();
        for has_timeout in [true, false] {
            if has_timeout && !timeout_here() {
                continue;
            }
            let script = limited_script(dir.path(), ONE_STUCK, has_timeout);
            let t = Instant::now();
            let out = transport::run_script(&Target::Local, &script, Duration::from_secs(20));
            assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
            let data = read(&Target::Local, &out, false).ok().expect("read works");
            assert!(data.snapshots["web"].is_ok());
            assert_eq!(
                data.snapshots["stuck"].as_ref().err().unwrap(),
                SESSION_SILENT
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_limit_without_a_timeout_command_works_where_sh_is_bash() {
        // macOS's sh is bash, which says more about a job it lost than Linux's dash does.
        let bash = std::path::Path::new("/bin/bash");
        if !bash.exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = limited_script(dir.path(), ONE_STUCK, false);
        let out = std::process::Command::new(bash)
            .args(["--posix", "-c", &script])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let out = transport::Output {
            status: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            ..Default::default()
        };
        let data = read(&Target::Local, &out, false).ok().expect("read works");
        assert_eq!(
            data.snapshots["stuck"].as_ref().err().unwrap(),
            SESSION_SILENT
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_session_list_past_the_limit_says_so_in_plain_words() {
        // The machine still counts as not answering, but says why: an empty list used to read
        // "herdr printed nothing" (intent 0491).
        let fake = r#"#!/bin/sh
case "$*" in
  --version) echo "herdr 0.9.1";;
  "session list --json") exec sleep 60;;
esac
"#;
        let dir = tempfile::tempdir().unwrap();
        for has_timeout in [true, false] {
            if has_timeout && !timeout_here() {
                continue;
            }
            let script = limited_script(dir.path(), fake, has_timeout);
            let t = Instant::now();
            let out = transport::run_script(&Target::Local, &script, Duration::from_secs(20));
            assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
            // herdr is slow; the machine has answered, so it stays live (intent 0591).
            match read(&Target::Local, &out, false) {
                Ok(data) => {
                    assert_eq!(
                        data.sessions_failed.as_deref(),
                        Some("herdr is slow there: it didn't list its sessions in time")
                    );
                    assert!(data.sessions.is_empty());
                }
                _ => panic!("expected it to say why: {}", out.stdout),
            }
        }
    }

    #[test]
    fn what_herdr_says_of_its_list_of_sessions_is_herdr_s_trouble_in_plain_words() {
        assert_eq!(
            sessions_trouble("herdr printed nothing"),
            (
                "herdr printed nothing for its list of sessions".into(),
                None
            )
        );
        assert_eq!(
            sessions_trouble("Error: socket closed"),
            (
                "herdr answered its list of sessions with something Kerdr can't read (what it printed is in Kerdr's error log).".into(),
                Some("Error: socket closed".into())
            )
        );
        assert!(
            sessions_trouble("herdr client and server versions differ")
                .0
                .contains("u runs herdr's own setup")
        );
        // The list gets longer on the read after one that ran out of time, and only the list.
        let script = build_script_within("herdr", None, SESSIONS_MOST_AFTER_SILENCE);
        assert!(script.contains("M=10\n"), "{script}");
        let list = script.find("session list").unwrap();
        let back = script.find("M=5\n").unwrap();
        assert!(back > list, "{script}");
    }

    #[test]
    fn what_herdr_or_a_feed_printed_never_reaches_the_board_and_is_kept_for_the_log() {
        // herdr's own error, codes and paths and all, for its list and for a session's snapshot,
        // and a feed that printed a traceback in place of its cards.
        let herdr_error = r#"{"id":"1","error":{"code":"ERR_SOCKET_GONE","message":"no socket at /run/user/1000/herdr/main.sock"}}"#;
        let out = transport::Output {
            stdout: format!(
                "{MARK}host\nlab\n{MARK}version\nherdr 0.9.1\n{MARK}keys\nyes\n{MARK}sessions\n{herdr_error}\n{MARK}snapshot web\n{herdr_error}\n{MARK}feed\nTraceback (most recent call last):\n  File \"/home/x/bin/ark-cards\", line 9\nKeyError: 'state'\n{MARK}end\n"
            ),
            stderr: String::new(),
            status: Some(0),
            timed_out: false,
            took: Duration::from_millis(5),
        };
        let Ok(data) = read(&Target::Local, &out, true) else {
            panic!("a read herdr answered is a read");
        };
        let shown = [
            data.sessions_failed.clone().unwrap(),
            data.snapshots["web"].clone().unwrap_err(),
            data.feed.clone().unwrap().unwrap_err(),
        ];
        for words in &shown {
            for raw in [
                "ERR_SOCKET_GONE",
                "/run/user",
                "Traceback",
                "KeyError",
                "/home/x",
            ] {
                assert!(!words.contains(raw), "{words}");
            }
            assert!(words.contains("Kerdr's error log"), "{words}");
        }
        assert!(data.said["sessions"].contains("ERR_SOCKET_GONE"));
        assert!(data.said["session web"].contains("main.sock"));
        assert!(data.said["feed"].contains("KeyError: 'state'"));
        // Through a merge with a newer read that failed the same way, without saying it again.
        let event = |data, at| Event {
            machine: 0,
            conn: live(),
            data: Some(data),
            at,
        };
        let inbox = Inbox::default();
        inbox.put(event(data, Local::now()), Instant::now(), 0);
        let mut quiet = MachineData {
            sessions_failed: Some(
                "herdr is slow there: it didn't list its sessions in time".into(),
            ),
            ..Default::default()
        };
        quiet
            .snapshots
            .insert("web".into(), Err(SESSION_SILENT.into()));
        inbox.put(event(quiet, Local::now()), Instant::now(), 0);
        let merged = inbox.take().pop().unwrap().data.unwrap();
        assert!(merged.said["feed"].contains("KeyError"));
        // A feed that says nothing failed: no words are made up.
        assert_eq!(
            feed::parse_keeping("").unwrap_err(),
            feed::Unread {
                words: "it printed no cards".into(),
                printed: None
            }
        );
    }

    /// Whether this machine has a `timeout` command to test.
    #[cfg(unix)]
    fn timeout_here() -> bool {
        std::process::Command::new("sh")
            .args(["-c", "command -v timeout"])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[cfg(unix)]
    #[test]
    fn the_read_works_where_there_is_no_timeout_command() {
        let script = build_script("herdr", None);
        let fake = r#"#!/bin/sh
case "$*" in
  --version) echo "herdr 0.9.1";;
  "session list --json") echo '{"sessions":[{"name":"web","running":true}]}';;
  *"api snapshot") echo "{\"result\":{\"snapshot\":{\"workspaces\":[]}}}";;
esac
"#;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, fake).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = script
            .replace("H=herdr", &format!("H={}", bin.display()))
            .replace("command -v timeout", "command -v no-such-timeout-here");
        let out = transport::run_script(&Target::Local, &script, Duration::from_secs(5));
        let data = read(&Target::Local, &out, false).ok().expect("read works");
        assert!(data.snapshots["web"].is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_of_its_own_is_passed_on_and_nothing_is_left_behind() {
        // The stand-in for `timeout` keeps herdr's own exit, and its watchdog ends with it. A
        // leftover would hold a remote read open till the connection's limit.
        let dir = tempfile::tempdir().unwrap();
        let script = limited_script(dir.path(), "#!/bin/sh\nexit 3\n", false);
        let script = script
            .split("echo '@@KERDR host'")
            .next()
            .unwrap()
            .to_string()
            + "$T \"$H\" anything; echo \"status $?\"\n";
        let t = Instant::now();
        let out = transport::run_script(&Target::Local, &script, Duration::from_secs(20));
        assert_eq!(out.stdout.trim(), "status 3");
        assert!(
            t.elapsed() < Duration::from_millis(900),
            "{:?}",
            t.elapsed()
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_poll_collects_running_sessions_and_a_unicode_feed() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fake herdr.cmd");
        std::fs::write(&bin, r#"@echo off
if "%~1"=="--version" (
echo herdr 0.9.5
exit /b 0
)
if "%~1"=="session" (
echo {"sessions":[{"name":"default","running":true},{"name":"idle-one","running":false},{"name":"web","running":true}]}
exit /b 0
)
echo {"result":{"snapshot":{"workspaces":[]}}}
"#).unwrap();
        let machine = Machine { name: "native".into(), ssh: None,
            herdr: bin.to_str().unwrap().into(), hidden: false,
            feed: Some("Write-Output '{\"kerdr_feed\":1,\"cards\":[{\"id\":\"1\",\"title\":\"🦀 café\"}]}'".into()) };
        let (conn, data) = read_once(&machine);
        assert!(conn.is_live(), "{conn:?}");
        let data = data.unwrap();
        assert_eq!(
            data.snapshots
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["default", "web"]
        );
        assert_eq!(data.version.as_deref(), Some("0.9.5"));
        assert_eq!(data.feed.unwrap().unwrap().cards[0].title, "🦀 café");
    }

    #[test]
    fn old_herdr_is_its_own_state() {
        let out = transport::Output {
            status: Some(0),
            stdout: "@@KERDR host\nbox\n@@KERDR version\nherdr 0.8.2\n@@KERDR sessions\n{\"sessions\":[]}\n@@KERDR end\n".into(),
            ..Default::default()
        };
        match read(&Target::Local, &out, false) {
            Err(Failure::Herdr(Conn::HerdrTooOld { version })) => assert_eq!(version, "0.8.2"),
            _ => panic!("expected too old"),
        }
    }
}
