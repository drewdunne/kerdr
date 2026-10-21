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
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
        #[cfg(not(unix))]
        CtrlCToHerdr {}
    }
}

impl Drop for CtrlCToHerdr {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: puts back the handler that was there before.
        unsafe {
            libc::signal(libc::SIGINT, self.previous);
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

pub struct TerminalFlags {
    pub keyboard: bool,
}

pub fn leave(flags: &TerminalFlags) {
    let mut out = stdout();
    if flags.keyboard {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableMouseCapture,
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

pub fn enter(terminal: &mut DefaultTerminal, flags: &TerminalFlags) {
    let _ = enable_raw_mode();
    let mut out = stdout();
    // herdr resets these when it exits, but not when its window is cut off over SSH.
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
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
}

pub fn run(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    jump: &Jump,
    client_config: &Path,
) -> Outcome {
    let t = target(&jump.machine);
    let herdr_bin = jump.machine.herdr.clone();

    // A newly attached herdr window opens on the session's focused workspace.
    let args = herdr_args(&jump.session, &["workspace", "focus", &jump.workspace]);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = transport::run_herdr(&t, &herdr_bin, &arg_refs, Duration::from_secs(12));
    if let Some(problem) = transport::classify(&t, &out) {
        return Outcome::Failed(format!(
            "Couldn't reach {}: {}",
            jump.machine.name,
            problem.detail()
        ));
    }
    if !out.ok() {
        return Outcome::Failed(format!(
            "herdr couldn't open {}: {}",
            jump.label,
            first_line(&out)
        ));
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

    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ctrl_c = CtrlCToHerdr::new();
    leave(flags);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            enter(terminal, flags);
            return Outcome::Failed(format!("Couldn't start herdr: {e}"));
        }
    };

    // Watch for Kerdr's keys: a kerdr_jump token on any of the session's workspaces.
    let stop = Arc::new(AtomicBool::new(false));
    let request: Arc<Mutex<Option<Request>>> = Arc::new(Mutex::new(None));
    {
        let (stop, request) = (stop.clone(), request.clone());
        let (t, herdr_bin, session) = (t.clone(), herdr_bin.clone(), jump.session.clone());
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if !asked_to_close && request.lock().unwrap().is_some() {
                    // SAFETY: signals our own child, which hasn't been waited for yet.
                    unsafe {
                        libc::kill(child.id() as i32, libc::SIGTERM);
                    }
                    asked_to_close = true;
                }
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break None,
        }
    };
    stop.store(true, Ordering::Relaxed);
    enter(terminal, flags);
    drop(ctrl_c);
    let result = *request.lock().unwrap();
    match result {
        Some(Request::Next) => Outcome::Next,
        Some(Request::Board) => Outcome::Board,
        None => match status {
            Some(s) if !s.success() => Outcome::Failed(format!(
                "herdr closed unexpectedly ({}). The board is as it was.",
                s.code()
                    .map(|c| format!("exit code {c}"))
                    .unwrap_or_else(|| "stopped by a signal".into())
            )),
            _ => Outcome::Board,
        },
    }
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
    println!("\n{intro}\n");
    for var in HERDR_VARS {
        cmd.env_remove(var);
    }
    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
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
