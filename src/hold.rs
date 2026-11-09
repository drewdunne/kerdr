//! Holding connections ready, so going into a card on another machine has no wait (intent 0481).
//!
//! `herdr --remote` takes seconds because it starts from nothing: a new SSH connection, a few
//! commands to check the other machine's herdr, then a bridge. All of that is only a way to reach
//! one socket, the session's client socket. So Kerdr keeps one SSH connection per machine and
//! forwards that socket through it to a socket here, and `herdr client` connects to it directly.
//! herdr closes a client that hasn't said hello within seconds, so a connection can't be left open
//! ahead of time: a held session is the forward, ready to connect on the first key.
//!
//! What's held follows you. A session is held while a workspace in it has something waiting on you
//! that you haven't been into yet, while you're in it, and for a minute after you leave it. A wait
//! you've been into counts again only once it has ended and another begins. Anything else is
//! released, and going into a session with nothing held works the slow way, as it always has.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::family;
use crate::jump;

/// How long a session stays held after you leave it.
pub const AFTER_LEAVING: Duration = Duration::from_secs(60);

/// A herdr session on a machine.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub machine: usize,
    pub session: String,
}

/// A workspace in a session.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub place: Place,
    pub workspace: String,
}

/// Which sessions to hold: only the rule, with no connections, so it can be read and tested alone.
#[derive(Debug, Default)]
pub struct Policy {
    /// Workspaces with something waiting on you now.
    waiting: BTreeSet<Spot>,
    /// Those you've got into during this wait. Each wait counts until you have.
    been_into: BTreeSet<Spot>,
    inside: Option<Spot>,
    /// Sessions you've left, and when.
    left: BTreeMap<Place, Instant>,
}

impl Policy {
    /// What waits on you now, from the board. A place it couldn't read says nothing about a wait
    /// there: only seeing it gone ends one you've been into, or a failed read would make it new.
    pub fn saw(&mut self, waiting: BTreeSet<Spot>, unread: impl Fn(&Place) -> bool) {
        self.been_into
            .retain(|s| waiting.contains(s) || unread(&s.place));
        self.waiting = waiting;
    }

    pub fn enter(&mut self, spot: Spot) {
        self.inside = Some(spot);
    }

    /// The minute starts here, when you leave, however long you were in there. `reached` is
    /// whether you got in at all: a try that never opened hasn't touched the wait.
    pub fn leave(&mut self, now: Instant, reached: bool) {
        if let Some(spot) = self.inside.take()
            && reached
        {
            self.left.insert(spot.place.clone(), now);
            self.been_into.insert(spot);
        }
    }

    /// The sessions to hold now, each with when to let it go by itself: only the minute after
    /// leaving has an end. The connections keep that time, so it ends on time whatever screen
    /// you're on (your editor, say).
    pub fn wanted(&mut self, now: Instant) -> BTreeMap<Place, Option<Instant>> {
        self.left
            .retain(|_, at| now.saturating_duration_since(*at) < AFTER_LEAVING);
        let mut places: BTreeMap<Place, Option<Instant>> = self
            .left
            .iter()
            .map(|(place, at)| (place.clone(), Some(*at + AFTER_LEAVING)))
            .collect();
        let held_on = self.inside.iter().map(|s| &s.place).chain(
            self.waiting
                .iter()
                .filter(|s| !self.been_into.contains(s) && self.inside.as_ref() != Some(s))
                .map(|s| &s.place),
        );
        for place in held_on {
            places.insert(place.clone(), None);
        }
        places
    }
}

/// Every open workspace on a machine over SSH where the card or a piece inside it waits on you.
pub fn waiting_spots(app: &App) -> BTreeSet<Spot> {
    app.board
        .cards
        .iter()
        .filter(|c| family::needs(c))
        .filter_map(|c| {
            let machine = app.machines.get(c.machine_index)?;
            machine.machine.ssh.as_ref()?;
            Some(Spot {
                place: Place {
                    machine: c.machine_index,
                    session: c.session.clone()?,
                },
                workspace: c.workspace_id.clone()?,
            })
        })
        .collect()
}

/// Whether the board couldn't read this session just now: its machine isn't answering, or herdr's
/// answer for that session failed. A session simply gone from the list has been read.
pub fn unread(app: &App, place: &Place) -> bool {
    let Some(m) = app.machines.get(place.machine) else {
        return false;
    };
    let failed =
        |d: &crate::poll::MachineData| matches!(d.snapshots.get(&place.session), Some(Err(_)));
    !m.conn.is_live() || m.data.as_ref().is_none_or(failed)
}

/// The session's client socket from its API socket, as herdr names it: `herdr.sock` beside
/// `herdr-client.sock`. None for a path SSH's forward can't take, since `:` separates its parts.
pub fn client_socket(api_socket: &str) -> Option<String> {
    let path = api_socket.strip_suffix(".sock")?;
    if !path.starts_with('/') || path.contains(':') {
        return None;
    }
    Some(format!("{path}-client.sock"))
}

/// Where to hold a session: the SSH host and the client socket there. None when it can't be held:
/// this machine's session, a machine not answering, or a different herdr at each end, which
/// `herdr --remote` is the one to handle (it may need to ask you).
pub fn target(app: &App, place: &Place) -> Option<(String, String)> {
    let m = app.machines.get(place.machine)?;
    if !m.conn.is_live() {
        return None;
    }
    let host = m.machine.ssh.clone()?;
    let server = m.snapshot(&place.session)?.version.as_deref();
    let here = app.machines.first()?.data.as_ref()?.version.as_deref();
    if !jump::same_herdr(&m.machine, server, here) {
        return None;
    }
    let info = m
        .data
        .as_ref()?
        .sessions
        .iter()
        .find(|s| s.running && s.name == place.session)?;
    Some((host, client_socket(info.socket_path.as_deref()?)?))
}

/// The rule and the connections together: what the board talks to.
pub struct Holds {
    policy: Policy,
    holder: Option<Holder>,
    /// Where you are now, as it was reached. Your window may run through this connection, so a
    /// read of the board that fails while you're in there never lets it go: it ends with the
    /// window, or when the connection itself drops.
    inside: Option<(Place, String, String)>,
}

impl Holds {
    /// Holds nothing: the demo, and Windows, whose OpenSSH has no shared connections.
    pub fn off() -> Holds {
        Holds {
            policy: Policy::default(),
            holder: None,
            inside: None,
        }
    }

    pub fn on() -> Holds {
        Holds {
            policy: Policy::default(),
            holder: Holder::start("ssh", Duration::from_secs(2)),
            inside: None,
        }
    }

    /// Brings what's held in line with the board. Cheap: it only sends a change.
    pub fn sync(&mut self, app: &App) {
        let Some(holder) = self.holder.as_mut() else {
            return;
        };
        self.policy.saw(waiting_spots(app), |p| unread(app, p));
        let mut wanted: Wanted = BTreeMap::new();
        for (place, until) in self.policy.wanted(Instant::now()) {
            let found = target(app, &place).or_else(|| {
                let (at, host, socket) = self.inside.as_ref()?;
                (*at == place).then(|| (host.clone(), socket.clone()))
            });
            if let Some((host, socket)) = found {
                wanted
                    .entry(host)
                    .or_default()
                    .insert(place.session, Hold { socket, until });
            }
        }
        holder.want(wanted);
    }

    /// You're going in: the socket to connect to, when this session can be reached that way now.
    /// A connection already up to its machine takes a moment to forward one more session.
    pub fn entering(&mut self, app: &App, spot: Spot) -> Option<HeldWay> {
        let place = spot.place.clone();
        self.policy.enter(spot);
        self.inside = target(app, &place).map(|(host, socket)| (place.clone(), host, socket));
        self.sync(app);
        let host = self.inside.as_ref()?.1.clone();
        self.holder
            .as_ref()?
            .ready_soon(&host, &place.session, Duration::from_millis(250))
    }

    /// You're back. `reached` is whether you got in, rather than the jump stopping on the way.
    pub fn left(&mut self, app: &App, reached: bool) {
        self.policy.leave(Instant::now(), reached);
        self.inside = None;
        self.sync(app);
        if let Some(holder) = &self.holder {
            holder.free();
        }
    }
}

/// One session to hold: its client socket on its machine, and when to let it go by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    socket: String,
    until: Option<Instant>,
}

/// By SSH host, each session to hold.
type Wanted = BTreeMap<String, BTreeMap<String, Hold>>;

/// A held way into a session, found ready: the socket to connect to, and the answer to a question
/// sent through its connection as you went in.
pub struct HeldWay {
    pub socket: PathBuf,
    answered: std::sync::mpsc::Receiver<bool>,
}

impl HeldWay {
    /// Whether the machine really answered through it, within `within`. Connecting to the socket
    /// only proves SSH is still here: after a laptop sleeps, a connection can look fine for up to
    /// a minute with nothing behind it, and herdr's window would hang on it. Asked alongside the
    /// other steps of going in, this adds no wait of its own to a connection that's fine.
    pub fn answers(&self, within: Duration) -> bool {
        self.answered.recv_timeout(within).unwrap_or(false)
    }
}

/// One SSH connection per machine with something to hold, each kept by a thread of its own so a
/// machine that's slow to answer never holds up the board or another machine.
#[cfg(unix)]
pub struct Holder {
    program: String,
    dir: PathBuf,
    tick: Duration,
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    hosts: BTreeMap<String, Worker>,
    sent: Wanted,
}

#[cfg(not(unix))]
pub struct Holder;

#[cfg(not(unix))]
impl Holder {
    fn start(_program: &str, _tick: Duration) -> Option<Holder> {
        None
    }
    fn want(&mut self, _wanted: Wanted) {}
    fn ready_soon(&self, _host: &str, _session: &str, _within: Duration) -> Option<HeldWay> {
        None
    }
    fn free(&self) {}
}

#[cfg(unix)]
#[derive(Default)]
struct Shared {
    /// Machines whose connection is up, with its control socket.
    up: BTreeMap<String, PathBuf>,
    /// Connections, by control socket, that failed to answer: to be closed and opened anew.
    suspect: BTreeSet<PathBuf>,
    /// By (host, session), the way someone has gone in by: never let go until they're back, whatever
    /// the time says, since the connection now carries their window.
    in_use: BTreeSet<(String, String)>,
    /// By (host, session), the socket here that reaches that session.
    ready: BTreeMap<(String, String), PathBuf>,
}

#[cfg(unix)]
struct Worker {
    tx: std::sync::mpsc::Sender<Message>,
    /// Closes when the thread has cleaned up.
    done: std::sync::mpsc::Receiver<()>,
}

#[cfg(unix)]
enum Message {
    Want(BTreeMap<String, Hold>),
    Stop,
}

/// A connection that keeps failing is tried less often, up to this.
#[cfg(unix)]
const LONGEST_RETRY: Duration = Duration::from_secs(60);

/// How long a connection gets to answer before it counts as gone.
pub const ANSWER_WITHIN: Duration = Duration::from_millis(1500);

#[cfg(unix)]
impl Holder {
    fn start(program: &str, tick: Duration) -> Option<Holder> {
        let dir = crate::transport::private_control_dir()?;
        sweep(&dir);
        Some(Holder::in_dir(program, tick, dir))
    }

    fn in_dir(program: &str, tick: Duration, dir: PathBuf) -> Holder {
        Holder {
            program: program.to_string(),
            dir,
            tick,
            shared: Default::default(),
            hosts: BTreeMap::new(),
            sent: BTreeMap::new(),
        }
    }

    fn want(&mut self, wanted: Wanted) {
        if wanted == self.sent {
            return;
        }
        for host in self.sent.keys() {
            if !wanted.contains_key(host)
                && let Some(worker) = self.hosts.get(host)
            {
                let _ = worker.tx.send(Message::Want(BTreeMap::new()));
            }
        }
        for (host, sessions) in &wanted {
            if self.sent.get(host) == Some(sessions) {
                continue;
            }
            let worker = self.hosts.entry(host.clone()).or_insert_with(|| {
                spawn_worker(
                    self.program.clone(),
                    host.clone(),
                    self.dir.clone(),
                    self.tick,
                    self.shared.clone(),
                )
            });
            let _ = worker.tx.send(Message::Want(sessions.clone()));
        }
        self.sent = wanted;
    }

    /// The socket for a session once it's ready to connect to, waiting only while its machine's
    /// connection is already up, and never longer than `within`.
    fn ready_soon(&self, host: &str, session: &str, within: Duration) -> Option<HeldWay> {
        let key = (host.to_string(), session.to_string());
        let deadline = Instant::now() + within;
        loop {
            let (socket, up) = {
                let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
                let socket = shared.ready.get(&key).cloned();
                if socket.is_some() {
                    shared.in_use.insert(key.clone());
                }
                (socket, shared.up.get(host).cloned())
            };
            if let (Some(socket), Some(control)) = (socket, &up) {
                // Refused when the connection behind it has just gone.
                std::os::unix::net::UnixStream::connect(&socket).ok()?;
                return Some(HeldWay {
                    socket,
                    answered: self.ask(host, control),
                });
            }
            if up.is_none() || Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Nobody's window runs through a held way now.
    fn free(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.in_use.clear();
    }

    /// Sends a question through the connection, away from the board's thread. No answer marks it
    /// for closing, so the next go has a good one, unless it has been replaced since.
    fn ask(&self, host: &str, control: &Path) -> std::sync::mpsc::Receiver<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut cmd = std::process::Command::new(&self.program);
        cmd.arg("-S").arg(control);
        cmd.args(["-o", "BatchMode=yes", "-o", "ControlMaster=no", "-T", "--"]);
        cmd.args([host, "true"]);
        let (host, control, shared) =
            (host.to_string(), control.to_path_buf(), self.shared.clone());
        std::thread::spawn(move || {
            let ok = crate::transport::run_plain(&mut cmd, ANSWER_WITHIN).ok();
            if !ok {
                let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
                if shared.up.get(&host) == Some(&control) {
                    shared.suspect.insert(control);
                }
            }
            let _ = tx.send(ok);
        });
        rx
    }
}

#[cfg(unix)]
impl Drop for Holder {
    fn drop(&mut self) {
        for worker in self.hosts.values() {
            let _ = worker.tx.send(Message::Stop);
        }
        // Quitting isn't held up by a machine slow to answer: what's left closes with Kerdr, and
        // the next Kerdr sweeps any socket left behind.
        let deadline = Instant::now() + Duration::from_millis(500);
        for worker in self.hosts.values() {
            let left = deadline.saturating_duration_since(Instant::now());
            let _ = worker.done.recv_timeout(left);
        }
    }
}

/// The file names Kerdr uses here start with its process id, kept short because a socket's path
/// can't be longer than about 100 bytes.
#[cfg(unix)]
fn socket_name(pid: u32, n: u64, kind: &str) -> String {
    format!("h{pid}-{n}.{kind}")
}

#[cfg(unix)]
fn name_pid(name: &str) -> Option<i32> {
    name.strip_prefix('h')?.split('-').next()?.parse().ok()
}

/// Removes the sockets a Kerdr that was killed left behind.
#[cfg(unix)]
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(name_pid) else {
            continue;
        };
        // SAFETY: signal 0 only asks whether the process exists.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn spawn_worker(
    program: String,
    host: String,
    dir: PathBuf,
    tick: Duration,
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
) -> Worker {
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut held = Connection {
            program,
            host,
            dir,
            shared,
            master: None,
            forwards: BTreeMap::new(),
            failures: 0,
            retry_at: None,
            wedged: std::cell::Cell::new(false),
        };
        let mut wanted = BTreeMap::new();
        loop {
            match rx.recv_timeout(tick) {
                Ok(Message::Want(w)) => wanted = w,
                Ok(Message::Stop) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            held.reconcile(&wanted);
        }
        held.reconcile(&BTreeMap::new());
        let _ = done_tx.send(());
    });
    Worker { tx, done }
}

#[cfg(unix)]
struct Master {
    child: std::process::Child,
    control: PathBuf,
}

#[cfg(unix)]
struct Forward {
    local: PathBuf,
    remote: String,
}

/// One machine's connection and the sessions forwarded through it.
#[cfg(unix)]
struct Connection {
    program: String,
    host: String,
    dir: PathBuf,
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    master: Option<Master>,
    forwards: BTreeMap<String, Forward>,
    failures: u32,
    retry_at: Option<Instant>,
    /// A request to the connection got no reply, so it may or may not have happened: what it holds
    /// can't be known, so it's closed and opened anew rather than guessed at.
    wedged: std::cell::Cell<bool>,
}

#[cfg(unix)]
impl Connection {
    fn reconcile(&mut self, all: &BTreeMap<String, Hold>) {
        self.notice_lost();
        let now = Instant::now();
        // Under one lock with taking what goes out of `ready`: going in either finds it gone, and
        // goes the usual way, or has marked it in use first, and it stays.
        let (wanted, stale) = {
            let mut shared = self.lock();
            let in_use = |session: &String| {
                shared
                    .in_use
                    .contains(&(self.host.clone(), session.clone()))
            };
            let wanted: BTreeMap<&String, &String> = all
                .iter()
                .filter(|(session, hold)| {
                    hold.until.is_none_or(|until| now < until) || in_use(session)
                })
                .map(|(session, hold)| (session, &hold.socket))
                .collect();
            let stale: Vec<String> = self
                .forwards
                .iter()
                .filter(|(session, f)| wanted.get(*session) != Some(&&f.remote) && !in_use(session))
                .map(|(session, _)| session.clone())
                .collect();
            for session in &stale {
                shared.ready.remove(&(self.host.clone(), session.clone()));
            }
            (wanted, stale)
        };
        let all_stale = stale.len() == self.forwards.len();
        for session in stale {
            self.cancel(&session);
        }
        if wanted.is_empty() && all_stale {
            self.close();
            self.failures = 0;
            self.retry_at = None;
            return;
        }
        if self.master.is_none() && !self.open() {
            return;
        }
        for (session, remote) in wanted {
            if !self.forwards.contains_key(session.as_str()) {
                self.forward(session, remote);
            }
        }
    }

    /// A connection that dropped takes its forwards with it, and one that stopped answering goes
    /// the same way. Trying again starts straight away.
    fn notice_lost(&mut self) {
        let dropped = self
            .master
            .as_mut()
            .is_some_and(|m| !matches!(m.child.try_wait(), Ok(None)));
        let control = self.master.as_ref().map(|m| m.control.clone());
        let suspect = control.is_some_and(|c| self.lock().suspect.remove(&c));
        if dropped || suspect || self.wedged.get() {
            self.close();
        }
    }

    fn open(&mut self) -> bool {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        match self.start_master() {
            Some(master) => {
                let control = master.control.clone();
                self.master = Some(master);
                self.failures = 0;
                self.retry_at = None;
                self.lock().up.insert(self.host.clone(), control);
                true
            }
            None => {
                self.failures = self.failures.saturating_add(1);
                let wait = Duration::from_secs(5u64 << self.failures.min(4));
                self.retry_at = Some(Instant::now() + wait.min(LONGEST_RETRY));
                false
            }
        }
    }

    /// The remote command does nothing but end when Kerdr does, however Kerdr ends: its input
    /// closes, and the connection goes with it. Keepalives are gentler than the ones Kerdr reads
    /// the board with: your window into a session runs through this one, so a short drop on the
    /// network mustn't close it.
    fn start_master(&self) -> Option<Master> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let control = self.path("ctl");
        let mut cmd = Command::new(&self.program);
        cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"])
            .args([
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=4",
            ])
            .args(["-o", "ControlMaster=yes", "-o", "ControlPersist=no", "-o"])
            .arg(format!("ControlPath={}", control.display()))
            .args(["--", &self.host, "cat >/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Never in the terminal's way, nor stopped by a Ctrl+C meant for something else.
            .process_group(0);
        let mut child = cmd.spawn().ok()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            if self.control(&control, &["check"], &[]) {
                return Some(Master { child, control });
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&control);
        None
    }

    fn forward(&mut self, session: &str, remote: &str) {
        let Some(master) = &self.master else {
            return;
        };
        let local = self.path("sock");
        let spec = format!("{}:{remote}", local.display());
        if self.control(&master.control, &["forward"], &["-L", &spec]) {
            self.lock()
                .ready
                .insert((self.host.clone(), session.to_string()), local.clone());
            self.forwards.insert(
                session.to_string(),
                Forward {
                    local,
                    remote: remote.to_string(),
                },
            );
        } else {
            let _ = std::fs::remove_file(&local);
        }
    }

    fn cancel(&mut self, session: &str) {
        let Some(f) = self.forwards.remove(session) else {
            return;
        };
        self.lock()
            .ready
            .remove(&(self.host.clone(), session.to_string()));
        if let Some(master) = &self.master {
            let spec = format!("{}:{}", f.local.display(), f.remote);
            self.control(&master.control, &["cancel"], &["-L", &spec]);
        }
        let _ = std::fs::remove_file(&f.local);
    }

    /// Lets go of the machine: its connection and everything forwarded through it.
    fn close(&mut self) {
        let sessions: Vec<String> = self.forwards.keys().cloned().collect();
        for session in sessions {
            self.cancel(&session);
        }
        if let Some(mut master) = self.master.take() {
            let _ = master.child.kill();
            let _ = master.child.wait();
            let _ = std::fs::remove_file(&master.control);
        }
        self.lock().up.remove(&self.host);
        self.wedged.set(false);
    }

    /// Asks the connection to do something: `ssh -S <its socket> -O <what> ...`.
    fn control(&self, control: &Path, what: &[&str], rest: &[&str]) -> bool {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.arg("-S").arg(control).arg("-O").args(what).args(rest);
        cmd.args(["-o", "BatchMode=yes", "--", &self.host]);
        let out = crate::transport::run_plain(&mut cmd, Duration::from_secs(3));
        if out.timed_out {
            self.wedged.set(true);
        }
        out.ok()
    }

    fn path(&self, kind: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.dir.join(socket_name(std::process::id(), n, kind))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(session: &str, workspace: &str) -> Spot {
        Spot {
            place: Place {
                machine: 1,
                session: session.into(),
            },
            workspace: workspace.into(),
        }
    }

    fn places(names: &[&str]) -> BTreeSet<Place> {
        names
            .iter()
            .map(|session| Place {
                machine: 1,
                session: (*session).into(),
            })
            .collect()
    }

    fn set(spots: &[Spot]) -> BTreeSet<Spot> {
        spots.iter().cloned().collect()
    }

    impl Policy {
        /// With every session read.
        fn saw_all(&mut self, waiting: BTreeSet<Spot>) {
            self.saw(waiting, |_| false);
        }
    }

    #[test]
    fn a_failed_read_doesnt_make_a_wait_youve_been_into_new() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.saw_all(set(&[spot("ARK", "w1")]));
        p.enter(spot("ARK", "w1"));
        p.leave(t0, true);
        p.saw(set(&[]), |_| true);
        p.saw_all(set(&[spot("ARK", "w1")]));
        assert!(p.wanted(t0 + AFTER_LEAVING).is_empty());
    }

    fn names(p: &mut Policy, now: Instant) -> BTreeSet<Place> {
        p.wanted(now).into_keys().collect()
    }

    #[test]
    fn a_try_that_never_got_in_leaves_the_wait_as_it_was() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.saw_all(set(&[spot("ARK", "w1")]));
        p.enter(spot("ARK", "w1"));
        p.leave(t0, false);
        p.saw_all(set(&[spot("ARK", "w1")]));
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(600)),
            places(&["ARK"])
        );
        assert_eq!(
            p.wanted(t0).values().next(),
            Some(&None),
            "held until you've been in"
        );
    }

    #[test]
    fn only_the_minute_after_leaving_has_an_end() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.enter(spot("ARK", "w1"));
        assert_eq!(
            p.wanted(t0).values().next(),
            Some(&None),
            "while you're in it"
        );
        p.leave(t0, true);
        assert_eq!(
            p.wanted(t0).values().next(),
            Some(&Some(t0 + AFTER_LEAVING))
        );
        // Something new waits there: held for that, with no end.
        p.saw_all(set(&[spot("ARK", "w2")]));
        assert_eq!(p.wanted(t0).values().next(), Some(&None));
    }

    #[test]
    fn a_session_is_held_while_something_in_it_waits_on_you_and_no_longer() {
        let mut p = Policy::default();
        let now = Instant::now();
        assert!(p.wanted(now).is_empty());
        p.saw_all(set(&[spot("ARK", "w1")]));
        assert_eq!(names(&mut p, now), places(&["ARK"]));
        // It stopped waiting without your going in: nothing needs it now.
        p.saw_all(set(&[]));
        assert!(p.wanted(now).is_empty());
    }

    #[test]
    fn the_minute_starts_when_you_leave_however_long_you_stayed() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.saw_all(set(&[spot("ARK", "w1")]));
        p.enter(spot("ARK", "w1"));
        // Half an hour in the card: still held, and the wait you're in doesn't end it.
        p.saw_all(set(&[]));
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(1800)),
            places(&["ARK"])
        );
        let left = t0 + Duration::from_secs(1800);
        p.leave(left, true);
        assert_eq!(
            names(&mut p, left + Duration::from_secs(59)),
            places(&["ARK"])
        );
        assert!(p.wanted(left + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn a_wait_you_have_been_into_is_released_after_the_minute_even_if_it_still_shows() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.saw_all(set(&[spot("ARK", "w1")]));
        p.enter(spot("ARK", "w1"));
        p.leave(t0, true);
        p.saw_all(set(&[spot("ARK", "w1")]));
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(30)),
            places(&["ARK"])
        );
        assert!(p.wanted(t0 + Duration::from_secs(60)).is_empty());
        // It ended, and another began: that one counts.
        p.saw_all(set(&[]));
        p.saw_all(set(&[spot("ARK", "w1")]));
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(61)),
            places(&["ARK"])
        );
    }

    #[test]
    fn another_workspace_waiting_keeps_the_session_held() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.saw_all(set(&[spot("ARK", "w1"), spot("ARK", "w2")]));
        p.enter(spot("ARK", "w1"));
        p.leave(t0, true);
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(600)),
            places(&["ARK"])
        );
    }

    #[test]
    fn hopping_holds_where_you_were_and_where_you_are() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.enter(spot("ARK", "w1"));
        p.leave(t0, true);
        p.enter(spot("P4M", "w1"));
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(5)),
            places(&["ARK", "P4M"])
        );
        assert_eq!(
            names(&mut p, t0 + Duration::from_secs(61)),
            places(&["P4M"])
        );
    }

    #[test]
    fn going_back_in_starts_the_minute_again_from_the_new_leaving() {
        let mut p = Policy::default();
        let t0 = Instant::now();
        p.enter(spot("ARK", "w1"));
        p.leave(t0, true);
        p.enter(spot("ARK", "w1"));
        let later = t0 + Duration::from_secs(50);
        p.leave(later, true);
        assert_eq!(
            names(&mut p, later + Duration::from_secs(59)),
            places(&["ARK"])
        );
    }

    #[test]
    fn the_client_socket_is_named_as_herdr_names_it() {
        assert_eq!(
            client_socket("/home/d/.config/herdr/sessions/ARK/herdr.sock").as_deref(),
            Some("/home/d/.config/herdr/sessions/ARK/herdr-client.sock")
        );
        assert_eq!(client_socket("relative/herdr.sock"), None);
        assert_eq!(client_socket("/a:b/herdr.sock"), None);
        assert_eq!(client_socket("/a/herdr"), None);
    }

    mod board {
        use super::*;
        use crate::board::MachineState;
        use crate::config::{Config, Machine};
        use crate::herdr::{Agent, SessionInfo, Snapshot, Workspace};
        use crate::poll::{Conn, MachineData};

        fn machine(name: &str, ssh: bool, server: &str, status: &str) -> MachineState {
            let mut m = MachineState::new(Machine {
                name: name.into(),
                ssh: ssh.then(|| name.to_lowercase()),
                herdr: "herdr".into(),
                feed: None,
                hidden: false,
            });
            m.conn = Conn::Live {
                latency: Default::default(),
            };
            let snap = Snapshot {
                version: Some(server.into()),
                workspaces: vec![Workspace {
                    workspace_id: "w1".into(),
                    label: "01-a".into(),
                    ..Default::default()
                }],
                agents: vec![Agent {
                    agent: Some("claude".into()),
                    agent_status: Some(status.into()),
                    pane_id: "w1:p1".into(),
                    tab_id: Some("w1:t1".into()),
                    workspace_id: "w1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            m.data = Some(MachineData {
                host: Some(name.into()),
                version: Some(server.into()),
                sessions: vec![SessionInfo {
                    name: "ARK".into(),
                    running: true,
                    default: false,
                    socket_path: Some("/h/.config/herdr/sessions/ARK/herdr.sock".into()),
                }],
                snapshots: [("ARK".to_string(), Ok(snap))].into_iter().collect(),
                keys: true,
                feed: None,
                sessions_failed: None,
                feed_failed_after: None,
                kept_snapshots: Default::default(),
            });
            m
        }

        fn app(machines: Vec<MachineState>) -> App {
            let mut app = App::new(Config::default(), machines);
            app.rebuild();
            app
        }

        fn ark() -> Place {
            Place {
                machine: 1,
                session: "ARK".into(),
            }
        }

        #[test]
        fn only_another_machines_waiting_workspaces_count() {
            let a = app(vec![
                machine("here", false, "0.9.1", "blocked"),
                machine("server", true, "0.9.1", "blocked"),
            ]);
            let spots = waiting_spots(&a);
            assert_eq!(spots.len(), 1, "{spots:?}");
            assert_eq!(spots.iter().next().unwrap().place, ark());
            assert_eq!(spots.iter().next().unwrap().workspace, "w1");
        }

        #[test]
        fn nothing_counts_while_the_agent_works() {
            let a = app(vec![
                machine("here", false, "0.9.1", "working"),
                machine("server", true, "0.9.1", "working"),
            ]);
            assert!(waiting_spots(&a).is_empty());
        }

        #[test]
        fn a_session_is_held_only_with_the_same_herdr_at_both_ends() {
            let same = app(vec![
                machine("here", false, "0.9.1", "blocked"),
                machine("server", true, "0.9.1", "blocked"),
            ]);
            assert_eq!(
                target(&same, &ark()),
                Some((
                    "server".to_string(),
                    "/h/.config/herdr/sessions/ARK/herdr-client.sock".to_string()
                ))
            );
            let older = app(vec![
                machine("here", false, "0.9.5", "blocked"),
                machine("server", true, "0.9.1", "blocked"),
            ]);
            assert_eq!(target(&older, &ark()), None);
            let newer = app(vec![
                machine("here", false, "0.9.1", "blocked"),
                machine("server", true, "0.9.5", "blocked"),
            ]);
            assert_eq!(target(&newer, &ark()), None);
        }

        #[test]
        fn a_session_is_unread_only_when_reading_it_failed() {
            let mut failing = machine("server", true, "0.9.1", "blocked");
            let data = failing.data.as_mut().unwrap();
            data.snapshots.insert("broken".into(), Err("no".into()));
            let mut away = machine("gone", true, "0.9.1", "blocked");
            away.conn = Conn::Connecting;
            let a = app(vec![
                machine("here", false, "0.9.1", "blocked"),
                failing,
                away,
            ]);
            let at = |machine: usize, session: &str| Place {
                machine,
                session: session.into(),
            };
            assert!(unread(&a, &at(1, "broken")));
            assert!(!unread(&a, &at(1, "ARK")));
            assert!(!unread(&a, &at(1, "stopped")), "gone from the list");
            assert!(unread(&a, &at(2, "ARK")));
        }

        #[test]
        fn a_machine_not_answering_is_not_held() {
            let mut away = machine("server", true, "0.9.1", "blocked");
            away.conn = Conn::Connecting;
            let a = app(vec![machine("here", false, "0.9.1", "blocked"), away]);
            assert_eq!(target(&a, &ark()), None);
        }
    }

    #[cfg(unix)]
    mod connections {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Stands in for ssh: logs what it's asked, and counts as a connection while it runs.
        fn fake_ssh(dir: &Path, master_works: bool) -> String {
            let path = dir.join("fake-ssh");
            let script = format!(
                r#"#!/bin/sh
D='{d}'
case " $* " in
  *" -O check "*) test -f "$D/up" ;;
  *" -O forward "*) echo "forward $*" >>"$D/log" ;;
  *" -O cancel "*) echo "cancel $*" >>"$D/log" ;;
  *" -S "*" true "*) test ! -f "$D/silent" ;;
  *)
    echo "master $*" >>"$D/log"
    {fail}
    touch "$D/up"
    trap 'rm -f "$D/up"; exit 0' TERM
    while :; do sleep 0.02; done
    ;;
esac
"#,
                d = dir.display(),
                fail = if master_works { "" } else { "exit 255" }
            );
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        }

        fn wanted(host: &str, sessions: &[(&str, &str)]) -> Wanted {
            [(
                host.to_string(),
                sessions
                    .iter()
                    .map(|(s, r)| {
                        (
                            s.to_string(),
                            Hold {
                                socket: r.to_string(),
                                until: None,
                            },
                        )
                    })
                    .collect(),
            )]
            .into_iter()
            .collect()
        }

        fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ok() {
                assert!(Instant::now() < deadline, "never happened: {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn log(dir: &Path) -> String {
            std::fs::read_to_string(dir.join("log")).unwrap_or_default()
        }

        fn ready(h: &Holder, host: &str, session: &str) -> Option<PathBuf> {
            let shared = h.shared.lock().unwrap();
            shared
                .ready
                .get(&(host.to_string(), session.to_string()))
                .cloned()
        }

        #[test]
        fn holding_forwards_through_one_connection_and_releasing_lets_it_go() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("ARK ready", || ready(&h, "server", "ARK").is_some());
            let local = ready(&h, "server", "ARK").unwrap();
            let name = local.file_name().unwrap().to_str().unwrap().to_string();
            assert!(
                name.starts_with(&format!("h{}-", std::process::id())),
                "{name}"
            );

            h.want(wanted(
                "server",
                &[("ARK", "/r/ark-client.sock"), ("P4M", "/r/p4m-client.sock")],
            ));
            wait_for("P4M ready", || ready(&h, "server", "P4M").is_some());
            let text = log(dir.path());
            assert_eq!(text.matches("master ").count(), 1, "one connection: {text}");
            assert!(text.contains("ControlPersist=no"), "{text}");
            assert!(text.contains("ServerAliveInterval=15"), "{text}");
            assert!(
                text.contains(&format!("{}:/r/ark-client.sock", local.display())),
                "{text}"
            );

            // One session released: the other stays, on the same connection.
            h.want(wanted("server", &[("P4M", "/r/p4m-client.sock")]));
            wait_for("ARK released", || ready(&h, "server", "ARK").is_none());
            assert!(dir.path().join("up").exists());
            wait_for("the cancel", || log(dir.path()).contains("cancel "));

            // The last one: the connection goes too.
            h.want(BTreeMap::new());
            wait_for("all released", || ready(&h, "server", "P4M").is_none());
            wait_for("connection closed", || {
                !h.shared.lock().unwrap().up.contains_key("server")
            });
        }

        #[test]
        fn a_connection_that_drops_comes_back_with_its_sessions() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("ready", || ready(&h, "server", "ARK").is_some());
            let first = ready(&h, "server", "ARK").unwrap();
            let pids = std::process::Command::new("pkill")
                .args(["-TERM", "-f", &ssh])
                .status()
                .unwrap();
            assert!(pids.success());
            wait_for("a new forward", || {
                ready(&h, "server", "ARK").is_some_and(|p| p != first)
            });
            assert_eq!(log(dir.path()).matches("master ").count(), 2);
        }

        #[test]
        fn a_machine_that_refuses_is_tried_again_only_after_a_wait() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), false);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(10), dir.path().to_path_buf());
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("the first try", || log(dir.path()).contains("master "));
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(log(dir.path()).matches("master ").count(), 1);
            assert!(ready(&h, "server", "ARK").is_none());
            assert!(
                h.ready_soon("server", "ARK", Duration::from_secs(5))
                    .is_none()
            );
        }

        #[test]
        fn nothing_held_answers_at_once_when_no_connection_is_up() {
            let dir = tempfile::tempdir().unwrap();
            let h = Holder::in_dir("true", Duration::from_millis(10), dir.path().to_path_buf());
            let t = Instant::now();
            assert!(
                h.ready_soon("server", "ARK", Duration::from_secs(5))
                    .is_none()
            );
            assert!(t.elapsed() < Duration::from_secs(1));
        }

        #[test]
        fn a_socket_nothing_listens_on_any_more_is_not_ready() {
            let dir = tempfile::tempdir().unwrap();
            let h = Holder::in_dir("true", Duration::from_millis(10), dir.path().to_path_buf());
            let path = dir.path().join("gone.sock");
            drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
            h.shared
                .lock()
                .unwrap()
                .ready
                .insert(("server".into(), "ARK".into()), path);
            h.shared
                .lock()
                .unwrap()
                .up
                .insert("server".into(), dir.path().join("ctl"));
            assert!(
                h.ready_soon("server", "ARK", Duration::from_secs(1))
                    .is_none()
            );
        }

        #[test]
        fn the_minute_ends_by_itself_with_nothing_more_said() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            let mut w = wanted("server", &[("ARK", "/r/ark-client.sock")]);
            let until = Instant::now() + Duration::from_millis(500);
            w.get_mut("server").unwrap().get_mut("ARK").unwrap().until = Some(until);
            h.want(w);
            wait_for("held", || ready(&h, "server", "ARK").is_some());
            wait_for("let go", || ready(&h, "server", "ARK").is_none());
            assert!(Instant::now() >= until);
            wait_for("connection closed", || {
                !h.shared.lock().unwrap().up.contains_key("server")
            });
        }

        #[test]
        fn a_way_in_use_outlasts_its_minute_until_youre_back() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            let mut w = wanted("server", &[("ARK", "/r/ark-client.sock")]);
            let until = Instant::now() + Duration::from_millis(300);
            w.get_mut("server").unwrap().get_mut("ARK").unwrap().until = Some(until);
            h.want(w);
            wait_for("held", || ready(&h, "server", "ARK").is_some());
            h.shared
                .lock()
                .unwrap()
                .in_use
                .insert(("server".into(), "ARK".into()));
            std::thread::sleep(Duration::from_millis(600));
            assert!(
                ready(&h, "server", "ARK").is_some(),
                "it outlasted the time"
            );
            // Not even gone from the list lets it go.
            h.want(BTreeMap::new());
            std::thread::sleep(Duration::from_millis(200));
            assert!(ready(&h, "server", "ARK").is_some());
            assert!(h.shared.lock().unwrap().up.contains_key("server"));
            h.free();
            wait_for("let go", || ready(&h, "server", "ARK").is_none());
            wait_for("connection closed", || {
                !h.shared.lock().unwrap().up.contains_key("server")
            });
        }

        #[test]
        fn a_late_failure_from_a_replaced_connection_leaves_the_new_one_alone() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("ready", || ready(&h, "server", "ARK").is_some());
            std::fs::write(dir.path().join("silent"), "").unwrap();
            assert!(
                !h.ask("server", &dir.path().join("an-older.ctl"))
                    .recv()
                    .unwrap()
            );
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(log(dir.path()).matches("master ").count(), 1);
            assert!(h.shared.lock().unwrap().suspect.is_empty());
        }

        #[test]
        fn a_connection_that_stops_answering_is_replaced() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("ready", || ready(&h, "server", "ARK").is_some());
            let control = h.shared.lock().unwrap().up["server"].clone();
            // The machine answers: it stays.
            assert!(h.ask("server", &control).recv().unwrap());
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(log(dir.path()).matches("master ").count(), 1);
            // It doesn't: closed, and opened anew.
            std::fs::write(dir.path().join("silent"), "").unwrap();
            assert!(!h.ask("server", &control).recv().unwrap());
            wait_for("a new connection", || {
                log(dir.path()).matches("master ").count() == 2
            });
        }

        #[test]
        fn a_request_that_gets_no_reply_replaces_the_connection() {
            let dir = tempfile::tempdir().unwrap();
            let ssh = fake_ssh(dir.path(), true);
            let mut h = Holder::in_dir(&ssh, Duration::from_millis(20), dir.path().to_path_buf());
            // Forwarding hangs, past the three seconds a request gets.
            let script = std::fs::read_to_string(&ssh).unwrap().replace(
                r#"*" -O forward "*) echo"#,
                r#"*" -O forward "*) test -f "$D/hang" && sleep 30; echo"#,
            );
            std::fs::write(&ssh, script).unwrap();
            std::fs::write(dir.path().join("hang"), "").unwrap();
            h.want(wanted("server", &[("ARK", "/r/ark-client.sock")]));
            wait_for("a new connection", || {
                log(dir.path()).matches("master ").count() >= 2
            });
            std::fs::remove_file(dir.path().join("hang")).unwrap();
            wait_for("ready on it", || ready(&h, "server", "ARK").is_some());
        }

        #[test]
        fn a_killed_kerdrs_sockets_are_swept_and_a_running_ones_kept() {
            let dir = tempfile::tempdir().unwrap();
            let mine = dir.path().join(socket_name(std::process::id(), 1, "sock"));
            // A process that has finished: its id is free.
            let mut child = std::process::Command::new("true").spawn().unwrap();
            let dead = child.id();
            child.wait().unwrap();
            let theirs = dir.path().join(socket_name(dead, 1, "sock"));
            let other = dir.path().join("ssh-abc");
            for p in [&mine, &theirs, &other] {
                std::fs::write(p, "").unwrap();
            }
            sweep(dir.path());
            assert!(mine.exists() && other.exists());
            assert!(!theirs.exists());
        }
    }
}
