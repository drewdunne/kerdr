//! Jumping in: Kerdr steps aside and runs herdr full screen on the card's workspace, then takes
//! the screen back when you leave. herdr runs as Kerdr's child, attached locally or with
//! `herdr --remote`, so its own detach (prefix+q) is the way back.

use std::io::stdout;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self as crossterm_event, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture,
    EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::config::Machine;
use crate::herdr;
use crate::herdr_keys::{self, Request};
use crate::transport::{self, HERDR_VARS, Target};

pub struct Jump {
    pub machine: Machine,
    pub session: String,
    pub workspace: String,
    /// The tab in that workspace to land on, when the card's work runs in one tab of it.
    pub tab: Option<String>,
    /// Only that tab will do: when it has closed, the jump stops (`Outcome::TabGone`) rather
    /// than land on whichever tab the workspace shows, which may be other work's.
    pub strict: bool,
    pub label: String,
    /// How many other cards need you, for the hint shown inside herdr.
    pub others: usize,
    pub via: Via,
    /// herdr's prefix as you type it (`ctrl+b` unless your config says otherwise).
    pub prefix: String,
    /// Whether Kerdr's two keys are in that machine's herdr config.
    pub keys: bool,
}

/// While herdr or SSH has the terminal, Ctrl+C belongs to them: Kerdr shares their process group,
/// so it catches the signal and does nothing. (Ignoring it outright would pass that on to them.)
struct CtrlCToHerdr {
    #[cfg(unix)]
    previous: libc::sighandler_t,
}

#[cfg(unix)]
extern "C" fn nothing(_: libc::c_int) {}

impl CtrlCToHerdr {
    fn new() -> CtrlCToHerdr {
        #[cfg(unix)]
        {
            let handler = nothing as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // SAFETY: installs a handler that does nothing; it touches no state.
            let previous = unsafe { libc::signal(libc::SIGINT, handler) };
            CtrlCToHerdr { previous }
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(windows_ctrl_c), 1);
        }
        #[cfg(not(unix))]
        CtrlCToHerdr {}
    }
}

#[cfg(windows)]
unsafe extern "system" fn windows_ctrl_c(event: u32) -> i32 {
    i32::from(
        event == windows_sys::Win32::System::Console::CTRL_C_EVENT
            || event == windows_sys::Win32::System::Console::CTRL_BREAK_EVENT,
    )
}

impl Drop for CtrlCToHerdr {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(windows_ctrl_c), 0);
        }
        #[cfg(unix)]
        // SAFETY: puts back the handler that was there before.
        unsafe {
            libc::signal(libc::SIGINT, self.previous);
        }
    }
}

/// The terminal's line settings as they were before herdr's window opened. herdr puts them back
/// when it exits cleanly, but one stopped by a signal, or cut off, can leave the terminal raw:
/// Enter then sends a bare carriage return, so the pause after it could never end, and the board
/// would save that raw state as its own to restore on quit (intent 0411).
struct TtyModes {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl TtyModes {
    fn capture() -> TtyModes {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr only fills the struct it's given; zeroed is a valid termios.
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            let ok = unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut t) } == 0;
            TtyModes {
                saved: ok.then_some(t),
            }
        }
        #[cfg(not(unix))]
        TtyModes {}
    }

    fn restore(&self) {
        #[cfg(unix)]
        if let Some(t) = &self.saved {
            // SAFETY: sets back settings tcgetattr read from the same terminal.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, t);
            }
        }
    }
}

/// How herdr's window reaches a session on another machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// This machine's own session.
    Local,
    /// `herdr --remote`: herdr draws here and the session runs there.
    Remote,
    /// herdr runs there and is drawn over SSH. Used when that session's herdr is older than this
    /// machine's: `herdr --remote` would then stop to ask whether to restart the other machine's
    /// server, which stops every agent in it.
    Ssh,
}

/// Picks how to attach: plain SSH when the session's server is older than this machine's herdr.
pub fn choose_via(machine: &Machine, server: Option<&str>, here: Option<&str>) -> Via {
    if machine.is_local() {
        return Via::Local;
    }
    let (Some(server), Some(here)) = (
        server.and_then(herdr::parse_version),
        here.and_then(herdr::parse_version),
    ) else {
        return Via::Ssh;
    };
    if server < here { Via::Ssh } else { Via::Remote }
}

/// Whether a session can be reached by a held connection: another machine, whose herdr is the very
/// same version as this one's. Anything else goes through `herdr --remote` or SSH, which handle a
/// difference between them.
pub fn same_herdr(machine: &Machine, server: Option<&str>, here: Option<&str>) -> bool {
    choose_via(machine, server, here) == Via::Remote
        && server.and_then(herdr::parse_version) == here.and_then(herdr::parse_version)
}

/// The herdr config a window run over SSH uses there: that machine's own, with the sidebar
/// hidden. Written into its cache, so Kerdr needs nothing installed there.
const REMOTE_CLIENT_CONFIG: &str = r#"C="${HERDR_CONFIG_PATH:-$HOME/.config/herdr/config.toml}"
D="${XDG_CACHE_HOME:-$HOME/.cache}/kerdr"
mkdir -p "$D"
if [ -f "$C" ]; then src="$C"; else src=/dev/null; fi
awk 'BEGIN { done = 0 }
  /^[ \t]*sidebar_(start_collapsed|collapsed_mode)[ \t]*=/ { next }
  { print }
  /^\[ui\][ \t]*$/ && !done { print "sidebar_start_collapsed = true"; print "sidebar_collapsed_mode = \"hidden\""; done = 1 }
  END { if (!done) { print ""; print "[ui]"; print "sidebar_start_collapsed = true"; print "sidebar_collapsed_mode = \"hidden\"" } }' "$src" > "$D/herdr-client.toml"
"#;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Board,
    Next,
    /// herdr says the workspace isn't there: it closed after the board last read it.
    Closed,
    /// herdr says the tab a strict jump was for isn't there; nothing was opened.
    TabGone,
    /// herdr says the session's server isn't running: it stopped since the board last read it,
    /// so the workspace is gone with it, as `Closed`, but for a reason worth saying.
    Stopped,
    Failed(String),
}

pub fn target(machine: &Machine) -> Target {
    match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    }
}

fn herdr_args(session: &str, rest: &[&str]) -> Vec<String> {
    let mut v = vec!["--session".to_string(), session.to_string()];
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

fn first_line(out: &transport::Output) -> String {
    out.stderr
        .lines()
        .chain(out.stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no answer")
        .to_string()
}

/// herdr's own refusal, from the JSON it prints on a failed command (`{"error":{"code","message"}}`,
/// on stderr since 0.9.1): its code and its message.
fn herdr_error(out: &transport::Output) -> Option<(String, String)> {
    out.stderr
        .lines()
        .chain(out.stdout.lines())
        .filter_map(|line| line.find('{').map(|i| &line[i..]))
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .find_map(|v| {
            let error = v.get("error")?;
            Some((
                error["code"].as_str().unwrap_or_default().to_string(),
                error["message"].as_str().unwrap_or_default().to_string(),
            ))
        })
}

/// Why herdr refused, in its own words: its error's message rather than the JSON around it.
fn why(out: &transport::Output) -> String {
    match herdr_error(out) {
        Some((_, message)) if !message.trim().is_empty() => message.trim().to_string(),
        _ => first_line(out),
    }
}

/// What the board says when herdr on a machine didn't answer a focus in time.
fn no_answer(machine: &str) -> String {
    format!("herdr on {machine} didn't answer in 12 s; ⏎ tries again.")
}

/// The workspace herdr says a tab it focused is in, from `tab focus`'s answer.
fn tab_workspace(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let v: serde_json::Value = serde_json::from_str(&text[start..]).ok()?;
    v.pointer("/result/tab/workspace_id")?
        .as_str()
        .map(str::to_string)
}

/// Focuses the card's workspace, then its tab when it names one; `run` runs one herdr command.
/// None when herdr is ready to attach. Unless herdr answers that it focused the tab in the card's
/// workspace, the workspace is focused again: a tab that has closed leaves it on its own without
/// an error, and a tab herdr finds in another workspace, or an answer lost on the way, may have
/// taken herdr to another workspace.
fn focus(
    t: &Target,
    jump: &Jump,
    mut run: impl FnMut(&[String]) -> transport::Output,
) -> Option<Outcome> {
    let workspace = herdr_args(&jump.session, &["workspace", "focus", &jump.workspace]);
    if let Some(refused) = focus_refused(t, jump, &run(&workspace)) {
        return Some(refused);
    }
    if let Some(tab) = &jump.tab {
        let out = run(&herdr_args(&jump.session, &["tab", "focus", tab]));
        let in_place = out.ok() && tab_workspace(&out.stdout).is_some_and(|w| w == jump.workspace);
        if jump.strict && !in_place {
            // Only that tab will do: anything short of herdr saying it focused it, in the card's
            // workspace, stops the jump. Herdr is put back on the workspace in case the tab took
            // it elsewhere, but nothing is opened.
            let _ = run(&workspace);
            if out.timed_out {
                return Some(Outcome::Failed(no_answer(&jump.machine.name)));
            }
            if let Some(problem) = transport::classify(t, &out) {
                return Some(Outcome::Failed(format!(
                    "Couldn't reach {}: {}",
                    jump.machine.name,
                    problem.detail()
                )));
            }
            if tab_gone(&out) {
                return Some(Outcome::TabGone);
            }
            if session_stopped(&out) {
                return Some(Outcome::Stopped);
            }
            return Some(Outcome::Failed(format!(
                "herdr couldn't open {}'s tab: {}",
                jump.label,
                why(&out)
            )));
        }
        if !in_place {
            return focus_refused(t, jump, &run(&workspace));
        }
    }
    None
}

/// Why herdr's answer to focusing the card's workspace stops the jump, if it does.
fn focus_refused(t: &Target, jump: &Jump, out: &transport::Output) -> Option<Outcome> {
    if out.timed_out {
        return Some(Outcome::Failed(no_answer(&jump.machine.name)));
    }
    if let Some(problem) = transport::classify(t, out) {
        return Some(Outcome::Failed(format!(
            "Couldn't reach {}: {}",
            jump.machine.name,
            problem.detail()
        )));
    }
    if !out.ok() {
        if workspace_gone(out) {
            return Some(Outcome::Closed);
        }
        if session_stopped(out) {
            return Some(Outcome::Stopped);
        }
        return Some(Outcome::Failed(format!(
            "herdr couldn't open {}: {}",
            jump.label,
            why(out)
        )));
    }
    None
}

/// Whether herdr refused because the tab no longer exists.
fn tab_gone(out: &transport::Output) -> bool {
    !out.ok()
        && [&out.stdout, &out.stderr]
            .iter()
            .any(|s| s.contains("\"tab_not_found\""))
}

/// Whether herdr refused because the workspace no longer exists.
fn workspace_gone(out: &transport::Output) -> bool {
    herdr_error(out).is_some_and(|(code, _)| code == "workspace_not_found")
        || [&out.stdout, &out.stderr]
            .iter()
            .any(|s| s.contains("\"workspace_not_found\""))
}

/// Whether herdr refused because the session's server isn't running: it stopped (a restart, an
/// update, a crash) since the board last read it, and every workspace in it with it.
fn session_stopped(out: &transport::Output) -> bool {
    !out.ok()
        && (herdr_error(out).is_some_and(|(code, _)| code == "server_not_running")
            || [&out.stdout, &out.stderr]
                .iter()
                .any(|s| s.contains("\"server_not_running\"")))
}

pub struct TerminalFlags {
    pub keyboard: bool,
}

/// What the board says while it gets herdr ready on a card: herdr's own calls come first, and can
/// take seconds over a slow connection (intent 0411).
pub fn opening(label: &str, machine: &str, local: bool) -> String {
    if local {
        format!("Opening {label}…")
    } else {
        format!("Opening {label} on {machine}…")
    }
}

/// Throws away keys pressed while the board was waiting on herdr, so they never reach an agent's
/// prompt once herdr is attached, nor jump again once back on the board.
fn drop_pending_keys() {
    while crossterm_event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = crossterm_event::read();
    }
}

pub fn leave(flags: &TerminalFlags) {
    let mut out = stdout();
    if flags.keyboard {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

pub fn enter(terminal: &mut DefaultTerminal, flags: &TerminalFlags) {
    let _ = enable_raw_mode();
    let mut out = stdout();
    #[cfg(windows)]
    {
        use std::io::Write;
        // A forcibly closed Herdr client cannot undo the host's Windows input protocol or
        // modifyOtherKeys mode itself. Return the host to the board's ordinary key path.
        let _ = out.write_all(b"\x1b[?9001l\x1b[>4;0m\x1b[?7h");
    }
    // herdr resets these when it exits, but not when its window is cut off over SSH. The
    // board takes pastes whole (the new-intent box keeps their line breaks).
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableFocusChange,
        EnableBracketedPaste,
        EnterAlternateScreen,
        ratatui::crossterm::cursor::Hide
    );
    if flags.keyboard {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if flags.keyboard {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let _ = terminal.clear();
    drop_pending_keys();
}

/// How herdr's window reaches a session through a connection Kerdr holds: this machine's herdr
/// client, connected to the session's forwarded socket, as `herdr --remote` runs it behind its own
/// bridge.
fn held_client(host: &str, session: &str, socket: &Path) -> Command {
    let mut c = Command::new("herdr");
    c.arg("client")
        .env("HERDR_CLIENT_SOCKET_PATH", socket)
        .env("HERDR_REMOTE_KEYBINDINGS", "server")
        .env(
            "HERDR_REATTACH_COMMAND",
            format!("herdr --remote {host} --session {session}"),
        );
    c
}

/// A window that closed with an error this soon never really opened: a held connection that had
/// just gone, say. Going in the usual way is what to try before saying anything.
const FAILED_AT_ONCE: Duration = Duration::from_secs(3);

/// How a herdr window ended.
struct Attached {
    status: Option<std::process::ExitStatus>,
    request: Option<Request>,
    took: Duration,
}

impl Attached {
    fn failed_at_once(&self) -> bool {
        self.request.is_none()
            && self.took < FAILED_AT_ONCE
            && self.status.is_some_and(|s| !s.success())
    }
}

/// While you're in herdr: every this often, `tick` lets the board keep up.
const TICK_EVERY: Duration = Duration::from_secs(1);

/// Runs one herdr window until it closes, watching for Kerdr's keys.
fn attach(
    cmd: &mut Command,
    t: &Target,
    jump: &Jump,
    tick: &mut dyn FnMut(),
) -> std::io::Result<Attached> {
    let begun = std::time::Instant::now();
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut child = cmd.spawn()?;

    // Watch for Kerdr's keys: a kerdr_jump token on any of the session's workspaces.
    let stop = Arc::new(AtomicBool::new(false));
    let request: Arc<Mutex<Option<Request>>> = Arc::new(Mutex::new(None));
    {
        let (stop, request) = (stop.clone(), request.clone());
        let (t, herdr_bin, session) = (t.clone(), jump.machine.herdr.clone(), jump.session.clone());
        let body = if jump.keys && jump.others > 0 {
            format!(
                "{p} q: back to the board · {p} space: next that needs you ({n} more)",
                p = jump.prefix,
                n = jump.others
            )
        } else {
            format!("{} q: back to the board", jump.prefix)
        };
        thread::spawn(move || {
            let run = |rest: &[&str], secs: u64| {
                let args = herdr_args(&session, rest);
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                transport::run_herdr(&t, &herdr_bin, &refs, Duration::from_secs(secs))
            };
            let clear = |wid: &str| {
                run(
                    &[
                        "workspace",
                        "report-metadata",
                        wid,
                        "--source",
                        "kerdr",
                        "--clear-token",
                        herdr_keys::TOKEN,
                    ],
                    6,
                )
            };
            thread::sleep(Duration::from_millis(600));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            run(
                &[
                    "notification",
                    "show",
                    "kerdr",
                    "--body",
                    &body,
                    "--position",
                    "top-right",
                ],
                6,
            );
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(700));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let out = run(&["workspace", "list"], 6);
                let Ok(list) = herdr::parse_workspace_list(&out.stdout) else {
                    continue;
                };
                let Some((wid, value)) = crate::board::jump_request(&list) else {
                    continue;
                };
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                clear(&wid);
                // A key pressed before this visit began is old news.
                let fresh = value
                    .split(':')
                    .nth(1)
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .is_none_or(|at| at + 1 >= started);
                if let (true, Some(r)) = (fresh, herdr_keys::parse_request(&value)) {
                    *request.lock().unwrap() = Some(r);
                    break;
                }
            }
        });
    }
    let mut asked_to_close = false;
    let mut ticked = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if !asked_to_close && request.lock().unwrap().is_some() {
                    close_client(&mut child);
                    asked_to_close = true;
                }
                if ticked.elapsed() >= TICK_EVERY {
                    tick();
                    ticked = std::time::Instant::now();
                }
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break None,
        }
    };
    stop.store(true, Ordering::Relaxed);
    let request = *request.lock().unwrap();
    Ok(Attached {
        status,
        request,
        took: begun.elapsed(),
    })
}

/// Goes into herdr on the card's workspace. `held` is the socket of a connection Kerdr holds for
/// this session, when it has one; `tick` runs now and then while you're in there. `opened` says
/// whether a window really opened, whatever way it ended.
#[allow(clippy::too_many_arguments)]
pub fn run(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    jump: &Jump,
    client_config: &Path,
    held: Option<&crate::hold::HeldWay>,
    tick: &mut dyn FnMut(),
    opened: &mut bool,
) -> Outcome {
    let t = target(&jump.machine);
    let herdr_bin = jump.machine.herdr.clone();

    // A newly attached herdr window opens on the session's focused workspace, on its active tab.
    let focused = focus(&t, jump, |args| {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        transport::run_herdr(&t, &herdr_bin, &refs, Duration::from_secs(12))
    });
    if let Some(outcome) = focused {
        // Keys pressed while herdr didn't answer (another ⏎) mustn't start a second try before
        // the reason is read.
        drop_pending_keys();
        return outcome;
    }

    let session_args: Vec<&str> = if jump.session == "default" {
        vec![]
    } else {
        vec!["--session", &jump.session]
    };
    let mut cmd = match (&jump.machine.ssh, jump.via) {
        (Some(host), Via::Ssh) => {
            let out = transport::run_script(&t, REMOTE_CLIENT_CONFIG, Duration::from_secs(12));
            if !out.ok() {
                drop_pending_keys();
                return Outcome::Failed(format!(
                    "Couldn't prepare herdr's window on {}: {}",
                    jump.machine.name,
                    first_line(&out)
                ));
            }
            let words: Vec<String> = session_args.iter().map(|a| transport::quote(a)).collect();
            let script = format!(
                "{}; unset {}; HERDR_CONFIG_PATH=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/kerdr/herdr-client.toml\" KERDR=1 exec {} {}",
                transport::PATH_PREFIX,
                HERDR_VARS.join(" "),
                transport::herdr_word(&herdr_bin),
                words.join(" ")
            );
            let mut c = Command::new("ssh");
            c.args(transport::ssh_args().iter().filter(|a| *a != "-T"))
                .arg("-t")
                .arg("--")
                .arg(host)
                .arg(format!("sh -c {}", transport::quote(&script)));
            c
        }
        (Some(host), _) => {
            let mut c = Command::new("herdr");
            c.args(["--remote", host, "--remote-keybindings", "server"]);
            c.args(&session_args);
            c
        }
        (None, _) => {
            let mut c = Command::new(expand_home(&herdr_bin));
            c.args(&session_args);
            c
        }
    };
    for var in HERDR_VARS {
        cmd.env_remove(var);
    }
    if !client_config.as_os_str().is_empty() {
        cmd.env("HERDR_CONFIG_PATH", client_config);
    }
    cmd.env("KERDR", "1");

    // A held connection first, if its machine answers through it; when that window never really
    // opens, the usual way.
    let held = held.filter(|h| h.answers(crate::hold::ANSWER_WITHIN));
    let mut through_held = match (&jump.machine.ssh, jump.via, held) {
        (Some(host), Via::Remote, Some(way)) => {
            let mut c = held_client(host, &jump.session, &way.socket);
            for var in HERDR_VARS {
                c.env_remove(var);
            }
            if !client_config.as_os_str().is_empty() {
                c.env("HERDR_CONFIG_PATH", client_config);
            }
            c.env("KERDR", "1");
            Some(c)
        }
        _ => None,
    };
    let ctrl_c = CtrlCToHerdr::new();
    drop_pending_keys();
    leave(flags);
    let tty = TtyModes::capture();
    #[cfg(windows)]
    let console_modes = crate::windows::ConsoleModes::capture();
    let first = through_held
        .as_mut()
        .and_then(|c| attach(c, &t, jump, tick).ok())
        .filter(|a| !a.failed_at_once());
    tty.restore();
    let attached = match first {
        Some(a) => a,
        None => {
            if through_held.is_some() {
                // A client stopped short may have left its modes on, which aren't line settings.
                leave(flags);
                let _ = execute!(stdout(), DisableFocusChange);
            }
            match attach(&mut cmd, &t, jump, tick) {
                Ok(a) => a,
                Err(e) => {
                    #[cfg(windows)]
                    drop(console_modes);
                    enter(terminal, flags);
                    return Outcome::Failed(format!("Couldn't start herdr: {e}"));
                }
            }
        }
    };
    #[cfg(windows)]
    drop(console_modes);
    tty.restore();
    *opened = !attached.failed_at_once();
    let (status, result) = (attached.status, attached.request);
    // herdr's own words on why its window closed (SSH failing, an update under way, versions
    // that don't match) are on the plain screen: they stay there until you've read them, since
    // the board would cover them (intent 0411).
    let outcome = finish_client(
        status.map(|s| s.code()),
        result,
        &jump.machine.name,
        |text| {
            use std::io::Write;
            println!("\n{text}");
            let _ = stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        },
    );
    enter(terminal, flags);
    drop(ctrl_c);
    outcome
}

/// What a herdr window's end means. `exit` is None when its status couldn't be read, and
/// Some(None) when a signal stopped it. A clean exit is herdr's own detach, however soon it
/// came. When it failed, `pause` is given what to say on the plain screen, under herdr's own
/// words, before the board comes back.
fn finish_client(
    exit: Option<Option<i32>>,
    request: Option<Request>,
    machine: &str,
    pause: impl FnOnce(&str),
) -> Outcome {
    let outcome = outcome_of(exit, request, machine);
    if let Outcome::Failed(_) = outcome {
        let how = match exit {
            Some(Some(c)) => format!("with exit code {c}"),
            Some(None) => "when a signal stopped it".to_string(),
            None => "and Kerdr couldn't read why".to_string(),
        };
        pause(&format!(
            "herdr's window on {machine} closed {how}. What it said is above. Press Enter to go back to Kerdr."
        ));
    }
    outcome
}

fn outcome_of(exit: Option<Option<i32>>, request: Option<Request>, machine: &str) -> Outcome {
    match (request, exit) {
        (Some(Request::Next), _) => Outcome::Next,
        (Some(Request::Board), _) => Outcome::Board,
        (None, Some(Some(0))) => Outcome::Board,
        (None, Some(Some(c))) => Outcome::Failed(format!(
            "herdr's window on {machine} closed with exit code {c}; its own message is on the screen you just saw. The board is as it was."
        )),
        (None, Some(None)) => Outcome::Failed(format!(
            "herdr's window on {machine} was stopped by a signal. The board is as it was."
        )),
        (None, None) => Outcome::Failed(format!(
            "herdr's window on {machine} closed, and Kerdr couldn't read how. The board is as it was."
        )),
    }
}

/// Close only the attached client. Never terminate a tree or a server when stepping out.
pub fn close_client(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: child is our live, unreaped client. SIGTERM lets Herdr restore the terminal.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = child.kill();
    }
}

/// Hands the whole terminal to something that needs it (your editor, for a backlog item's text)
/// as a jump hands it to herdr, runs `f`, and takes it back; the board redraws after. Ctrl+C
/// belongs to it meanwhile, and the terminal's own settings are put back however it ended.
pub fn hand_over<T>(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    f: impl FnOnce() -> T,
) -> T {
    let ctrl_c = CtrlCToHerdr::new();
    drop_pending_keys();
    leave(flags);
    let tty = TtyModes::capture();
    #[cfg(windows)]
    let console_modes = crate::windows::ConsoleModes::capture();
    let out = f();
    #[cfg(windows)]
    drop(console_modes);
    tty.restore();
    enter(terminal, flags);
    drop(ctrl_c);
    out
}

/// Runs a command in the foreground on the plain terminal, for things that need you: answering
/// SSH, or herdr's own setup. Returns whether it exited cleanly.
pub fn foreground(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    intro: &str,
    cmd: &mut Command,
) -> bool {
    let _ctrl_c = CtrlCToHerdr::new();
    leave(flags);
    #[cfg(windows)]
    let console_modes = crate::windows::ConsoleModes::capture();
    println!("\n{intro}\n");
    for var in HERDR_VARS {
        cmd.env_remove(var);
    }
    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
    #[cfg(windows)]
    drop(console_modes);
    if !ok {
        println!("\nThat didn't finish cleanly. Press Enter to go back to Kerdr.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    enter(terminal, flags);
    ok
}

pub fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", crate::config::home().display()),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(ssh: Option<&str>) -> Machine {
        Machine {
            name: "m".into(),
            ssh: ssh.map(Into::into),
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        }
    }

    #[test]
    fn a_held_connection_needs_the_same_herdr_at_both_ends_on_another_machine() {
        let remote = machine(Some("server"));
        assert!(same_herdr(&remote, Some("0.9.1"), Some("0.9.1")));
        assert!(!same_herdr(&remote, Some("0.9.1"), Some("0.9.3")));
        assert!(!same_herdr(&remote, Some("0.9.3"), Some("0.9.1")));
        assert!(!same_herdr(&remote, None, Some("0.9.1")));
        assert!(!same_herdr(&machine(None), Some("0.9.1"), Some("0.9.1")));
    }

    #[test]
    fn a_held_window_is_herdrs_client_on_the_forwarded_socket() {
        let c = held_client("server", "ARK", Path::new("/run/h1-2.sock"));
        assert_eq!(c.get_program(), "herdr");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["client"]);
        let envs: Vec<_> = c
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_str().unwrap().to_string(),
                    v.unwrap().to_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(envs.contains(&("HERDR_CLIENT_SOCKET_PATH".into(), "/run/h1-2.sock".into())));
        assert!(envs.contains(&("HERDR_REMOTE_KEYBINDINGS".into(), "server".into())));
        assert!(envs.contains(&(
            "HERDR_REATTACH_COMMAND".into(),
            "herdr --remote server --session ARK".into()
        )));
    }

    #[cfg(unix)]
    #[test]
    fn only_a_quick_failure_goes_in_the_usual_way_instead() {
        use std::os::unix::process::ExitStatusExt;
        let failed = std::process::ExitStatus::from_raw(1 << 8);
        let clean = std::process::ExitStatus::from_raw(0);
        let ended = |status, request, secs| Attached {
            status: Some(status),
            request,
            took: Duration::from_secs(secs),
        };
        assert!(ended(failed, None, 0).failed_at_once());
        assert!(!ended(clean, None, 0).failed_at_once(), "prefix q");
        assert!(!ended(failed, None, 3).failed_at_once(), "it was open");
        assert!(!ended(failed, Some(Request::Board), 0).failed_at_once());
    }

    #[test]
    fn opening_names_the_machine_only_when_its_not_this_one() {
        assert_eq!(
            opening("0411", "LAB-Server", false),
            "Opening 0411 on LAB-Server…"
        );
        assert_eq!(opening("0411", "AMD-LZM4", true), "Opening 0411…");
    }

    #[test]
    fn older_servers_are_reached_over_plain_ssh() {
        let remote = machine(Some("server"));
        assert_eq!(choose_via(&remote, Some("0.9.1"), Some("0.9.3")), Via::Ssh);
        assert_eq!(
            choose_via(&remote, Some("0.9.5"), Some("0.9.5")),
            Via::Remote
        );
        assert_eq!(
            choose_via(&remote, Some("0.9.5"), Some("0.9.3")),
            Via::Remote
        );
        assert_eq!(choose_via(&remote, None, Some("0.9.3")), Via::Ssh);
        assert_eq!(
            choose_via(&machine(None), Some("0.9.1"), Some("0.9.3")),
            Via::Local
        );
    }

    #[test]
    fn a_closed_workspace_is_told_apart_from_other_failures() {
        // What herdr 0.9.1 prints for `workspace focus` on a workspace that has closed.
        let gone = transport::Output {
            status: Some(1),
            stdout: r#"{"error":{"code":"workspace_not_found","message":"workspace w1 not found"},"id":"cli:workspace:focus"}"#.into(),
            ..Default::default()
        };
        assert!(workspace_gone(&gone));
        let other = transport::Output {
            status: Some(1),
            stderr: "error: couldn't connect to the herdr server".into(),
            ..Default::default()
        };
        assert!(!workspace_gone(&other));
    }

    fn jump(tab: Option<&str>) -> Jump {
        Jump {
            machine: machine(None),
            session: "web".into(),
            workspace: "w4".into(),
            tab: tab.map(Into::into),
            strict: false,
            label: "41-checkout".into(),
            others: 0,
            via: Via::Local,
            prefix: "ctrl+b".into(),
            keys: false,
        }
    }

    /// Runs `focus` against a stand-in herdr that answers each command with `answer`, and returns
    /// the outcome and the commands it was given.
    fn focus_with(
        jump: &Jump,
        mut answer: impl FnMut(&str) -> transport::Output,
    ) -> (Option<Outcome>, Vec<String>) {
        let mut calls = Vec::new();
        let outcome = focus(&Target::Local, jump, |args| {
            let line = args.join(" ");
            let out = answer(&line);
            calls.push(line);
            out
        });
        (outcome, calls)
    }

    fn says(status: i32, stdout: &str) -> transport::Output {
        transport::Output {
            status: Some(status),
            stdout: stdout.into(),
            ..Default::default()
        }
    }

    // What herdr 0.9.1 prints for each command.
    const WORKSPACE_FOCUSED: &str = r#"{"id":"cli:workspace:focus","result":{"type":"workspace_info","workspace":{"active_tab_id":"w4:t1","focused":true,"workspace_id":"w4"}}}"#;
    const TAB_FOCUSED: &str = r#"{"id":"cli:tab:focus","result":{"tab":{"focused":true,"label":"child","tab_id":"w4:t2","workspace_id":"w4"},"type":"tab_info"}}"#;
    const TAB_GONE: &str = r#"{"error":{"code":"tab_not_found","message":"tab w4:t2 not found"},"id":"cli:tab:focus"}"#;
    const TAB_ELSEWHERE: &str = r#"{"id":"cli:tab:focus","result":{"tab":{"focused":true,"label":"1","tab_id":"w4:t2","workspace_id":"w7"},"type":"tab_info"}}"#;
    const WORKSPACE_GONE: &str = r#"{"error":{"code":"workspace_not_found","message":"workspace w4 not found"},"id":"cli:workspace:focus"}"#;
    const FOCUS_W4: &str = "--session web workspace focus w4";
    const FOCUS_T2: &str = "--session web tab focus w4:t2";

    /// A stand-in herdr that keeps which workspace it has focused: the card's workspace focuses
    /// w4 and answers `restore` once the tab has been tried; the tab moves herdr to `moves_to`
    /// and answers `tab`.
    fn herdr_with(
        moves_to: Option<&'static str>,
        tab: transport::Output,
        restore: transport::Output,
    ) -> (
        impl FnMut(&str) -> transport::Output,
        std::rc::Rc<std::cell::RefCell<&'static str>>,
    ) {
        let focused = std::rc::Rc::new(std::cell::RefCell::new("w1"));
        let state = focused.clone();
        let (mut tab, mut restore) = (Some(tab), Some(restore));
        let mut tried = false;
        let answer = move |c: &str| {
            if c == FOCUS_T2 {
                tried = true;
                if let Some(w) = moves_to {
                    *state.borrow_mut() = w;
                }
                return tab.take().expect("the tab is tried once");
            }
            assert_eq!(c, FOCUS_W4);
            if !tried {
                *state.borrow_mut() = "w4";
                return says(0, WORKSPACE_FOCUSED);
            }
            let out = restore
                .take()
                .expect("the workspace is focused again at most once");
            if out.ok() {
                *state.borrow_mut() = "w4";
            }
            out
        };
        (answer, focused)
    }

    #[test]
    fn a_card_without_a_tab_focuses_only_its_workspace() {
        let (outcome, calls) = focus_with(&jump(None), |_| says(0, WORKSPACE_FOCUSED));
        assert_eq!(outcome, None);
        assert_eq!(calls, [FOCUS_W4]);
    }

    #[test]
    fn a_card_with_a_tab_focuses_it_after_its_workspace() {
        let (answer, focused) = herdr_with(Some("w4"), says(0, TAB_FOCUSED), says(0, ""));
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), answer);
        assert_eq!(outcome, None);
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2]);
        assert_eq!(*focused.borrow(), "w4");
    }

    #[test]
    fn a_tab_that_has_closed_leaves_the_workspace_without_an_error() {
        let (answer, focused) = herdr_with(None, says(1, TAB_GONE), says(0, WORKSPACE_FOCUSED));
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), answer);
        assert_eq!(outcome, None);
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2, FOCUS_W4]);
        assert_eq!(*focused.borrow(), "w4");
    }

    #[test]
    fn a_strict_jump_to_a_tab_that_has_closed_stops_instead_of_landing_on_another() {
        let (answer, _) = herdr_with(None, says(1, TAB_GONE), says(0, WORKSPACE_FOCUSED));
        let strict = Jump {
            strict: true,
            ..jump(Some("w4:t2"))
        };
        let (outcome, calls) = focus_with(&strict, answer);
        assert_eq!(outcome, Some(Outcome::TabGone));
        assert_eq!(
            calls,
            [FOCUS_W4, FOCUS_T2, FOCUS_W4],
            "herdr put back, nothing opened"
        );
        // Any other answer short of "focused, in its workspace" stops a strict jump too.
        for (moves_to, answer) in [
            (Some("w7"), says(0, TAB_ELSEWHERE)),
            (None, says(0, "garbled")),
            (None, says(1, "error: busy")),
        ] {
            let (answer_fn, focused) = herdr_with(moves_to, answer, says(0, WORKSPACE_FOCUSED));
            let strict = Jump {
                strict: true,
                ..jump(Some("w4:t2"))
            };
            let outcome = focus_with(&strict, answer_fn).0;
            assert!(matches!(outcome, Some(Outcome::Failed(_))), "{outcome:?}");
            assert_eq!(*focused.borrow(), "w4");
        }
        // A strict jump to a tab that's there goes in as any other.
        let (answer, _) = herdr_with(Some("w4"), says(0, TAB_FOCUSED), says(0, ""));
        let strict = Jump {
            strict: true,
            ..jump(Some("w4:t2"))
        };
        assert_eq!(focus_with(&strict, answer).0, None);
    }

    #[test]
    fn a_tab_in_another_workspace_goes_back_to_the_cards_workspace() {
        // herdr focuses a tab of another workspace by moving to that workspace.
        let (answer, focused) = herdr_with(
            Some("w7"),
            says(0, TAB_ELSEWHERE),
            says(0, WORKSPACE_FOCUSED),
        );
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), answer);
        assert_eq!(outcome, None);
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2, FOCUS_W4]);
        assert_eq!(*focused.borrow(), "w4");
    }

    #[test]
    fn a_lost_answer_after_the_tab_moved_herdr_goes_back_to_the_cards_workspace() {
        // The tab focus took effect in another workspace, then its answer never came back.
        let lost = transport::Output {
            timed_out: true,
            ..Default::default()
        };
        let (answer, focused) = herdr_with(Some("w7"), lost, says(0, WORKSPACE_FOCUSED));
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), answer);
        assert_eq!(outcome, None);
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2, FOCUS_W4]);
        assert_eq!(*focused.borrow(), "w4");
        // An answer that can't be read counts the same.
        let (answer, focused) = herdr_with(Some("w7"), says(0, "garbled"), says(0, ""));
        assert_eq!(focus_with(&jump(Some("w4:t2")), answer).0, None);
        assert_eq!(*focused.borrow(), "w4");
    }

    #[test]
    fn a_workspace_that_cant_be_focused_again_stops_the_jump() {
        let unreachable = transport::Output {
            timed_out: true,
            ..Default::default()
        };
        let (answer, _) = herdr_with(Some("w7"), says(0, TAB_ELSEWHERE), unreachable);
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), answer);
        assert!(matches!(outcome, Some(Outcome::Failed(_))), "{outcome:?}");
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2, FOCUS_W4]);
        // One that closed meanwhile is reported as closed.
        let (answer, _) = herdr_with(None, says(1, TAB_GONE), says(1, WORKSPACE_GONE));
        assert_eq!(
            focus_with(&jump(Some("w4:t2")), answer).0,
            Some(Outcome::Closed)
        );
    }

    #[test]
    fn a_closed_workspace_is_reported_before_any_tab_is_tried() {
        let (outcome, calls) = focus_with(&jump(Some("w4:t2")), |_| says(1, WORKSPACE_GONE));
        assert_eq!(outcome, Some(Outcome::Closed));
        assert_eq!(calls, [FOCUS_W4]);
    }

    fn says_err(status: i32, stderr: &str) -> transport::Output {
        transport::Output {
            status: Some(status),
            stderr: stderr.into(),
            ..Default::default()
        }
    }

    // What herdr 0.9.1 prints on stderr when the session's server isn't running.
    const NOT_RUNNING: &str = r#"{"id":"cli:workspace:focus","error":{"code":"server_not_running","message":"no herdr server is running at /x/sessions/web/herdr.sock; run `herdr session attach web` to start or attach it"}}"#;

    #[test]
    fn herdr_refusals_on_stderr_are_read_as_on_stdout() {
        let (outcome, calls) = focus_with(&jump(None), |_| says_err(1, WORKSPACE_GONE));
        assert_eq!(outcome, Some(Outcome::Closed));
        assert_eq!(calls, [FOCUS_W4]);
        let (answer, _) = herdr_with(None, says_err(1, TAB_GONE), says(0, WORKSPACE_FOCUSED));
        let strict = Jump {
            strict: true,
            ..jump(Some("w4:t2"))
        };
        assert_eq!(focus_with(&strict, answer).0, Some(Outcome::TabGone));
    }

    #[test]
    fn a_stopped_session_is_told_apart_from_a_failure() {
        let (outcome, calls) = focus_with(&jump(None), |_| says_err(1, NOT_RUNNING));
        assert_eq!(outcome, Some(Outcome::Stopped));
        assert_eq!(calls, [FOCUS_W4]);
    }

    #[test]
    fn an_unknown_herdr_refusal_is_said_in_plain_words() {
        let refusal = r#"{"id":"cli:workspace:focus","error":{"code":"some_new_code","message":"something broke"}}"#;
        let (outcome, _) = focus_with(&jump(None), |_| says_err(1, refusal));
        let Some(Outcome::Failed(m)) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(m.contains("something broke"), "{m}");
        assert!(!m.contains('{') && !m.contains("some_new_code"), "{m}");
        // Words that aren't herdr's JSON are said as they are.
        let (outcome, _) = focus_with(&jump(None), |_| says_err(1, "error: busy"));
        assert_eq!(
            outcome,
            Some(Outcome::Failed(
                "herdr couldn't open 41-checkout: error: busy".into()
            ))
        );
    }

    #[test]
    fn a_focus_that_times_out_says_herdr_didnt_answer_and_enter_tries_again() {
        let lost = || transport::Output {
            timed_out: true,
            ..Default::default()
        };
        let (outcome, _) = focus_with(&jump(None), |_| lost());
        let Some(Outcome::Failed(m)) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(m.contains("didn't answer in 12 s"), "{m}");
        assert!(m.contains("⏎ tries again"), "{m}");
        // A strict tab whose focus timed out says the same, with herdr put back.
        let (answer, _) = herdr_with(None, lost(), says(0, WORKSPACE_FOCUSED));
        let strict = Jump {
            strict: true,
            ..jump(Some("w4:t2"))
        };
        let (outcome, calls) = focus_with(&strict, answer);
        let Some(Outcome::Failed(m)) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(m.contains("didn't answer in 12 s"), "{m}");
        assert!(!m.contains("no answer"), "{m}");
        assert_eq!(calls, [FOCUS_W4, FOCUS_T2, FOCUS_W4]);
    }

    #[test]
    fn a_herdr_window_that_fails_says_so_and_waits_on_the_plain_screen() {
        let failed = |o: Outcome| match o {
            Outcome::Failed(m) => m,
            other => panic!("{other:?}"),
        };
        let m = failed(outcome_of(Some(Some(1)), None, "LAB-Server"));
        assert!(
            m.contains("exit code 1") && m.contains("its own message"),
            "{m}"
        );
        assert!(m.contains("LAB-Server"), "{m}");
        // herdr's own detach is a clean exit, however soon it comes.
        assert_eq!(outcome_of(Some(Some(0)), None, "m"), Outcome::Board);
        assert_eq!(
            outcome_of(Some(Some(1)), Some(Request::Board), "m"),
            Outcome::Board
        );
        assert_eq!(
            outcome_of(Some(Some(0)), Some(Request::Next), "m"),
            Outcome::Next
        );
        let m = failed(outcome_of(Some(None), None, "m"));
        assert!(m.contains("stopped by a signal"), "{m}");
        // A failure waits for Enter on the plain screen, where herdr's own words are.
        let mut paused = vec![];
        finish_client(Some(Some(1)), None, "m", |t| paused.push(t.to_string()));
        assert_eq!(paused.len(), 1);
        assert!(paused[0].contains("exit code 1"), "{}", paused[0]);
        assert!(paused[0].contains("Press Enter"), "{}", paused[0]);
        // Leaving herdr as usual doesn't.
        let mut paused = 0;
        let outcome = finish_client(Some(Some(0)), None, "m", |_| paused += 1);
        assert_eq!((outcome, paused), (Outcome::Board, 0));
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_window_config_hides_the_sidebar() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[keys]\nprefix = \"ctrl+space\"\n[ui]\nsidebar_start_collapsed = false\naccent = \"x\"\n").unwrap();
        let script = format!(
            "HOME={}\nHERDR_CONFIG_PATH={}\nXDG_CACHE_HOME={}\n{}",
            dir.path().display(),
            cfg.display(),
            dir.path().display(),
            REMOTE_CLIENT_CONFIG
        );
        let out = transport::run_script(&Target::Local, &script, Duration::from_secs(5));
        assert!(out.ok(), "{}", out.stderr);
        let text = std::fs::read_to_string(dir.path().join("kerdr/herdr-client.toml")).unwrap();
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(doc["ui"]["sidebar_start_collapsed"].as_bool(), Some(true));
        assert_eq!(doc["ui"]["sidebar_collapsed_mode"].as_str(), Some("hidden"));
        assert_eq!(doc["ui"]["accent"].as_str(), Some("x"));
        assert_eq!(doc["keys"]["prefix"].as_str(), Some("ctrl+space"));
    }
}
