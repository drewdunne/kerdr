//! Running commands on a machine: this one directly, others over SSH. Every SSH command shares
//! one connection per machine (OpenSSH's ControlMaster), never prompts, and has a deadline.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::cache_dir;

/// herdr installs into ~/.local/bin, which a non-login SSH command doesn't have on its PATH.
pub const PATH_PREFIX: &str =
    r#"PATH="$HOME/.local/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH""#;

/// herdr refuses to start inside itself and aims its CLI at the session in these variables, so
/// commands Kerdr runs drop them.
pub const HERDR_VARS: [&str; 7] = [
    "HERDR_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_ENV",
    "HERDR_BIN_PATH",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local,
    Ssh(String),
}

#[derive(Debug, Default)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub took: Duration,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0) && !self.timed_out
    }
}

/// Where SSH keeps its shared connections: the runtime folder when there is one, since a
/// socket's path can't be longer than about 100 bytes.
fn control_dir() -> std::path::PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let dir = runtime
        .filter(|d| d.is_absolute())
        .map(|d| d.join("kerdr"))
        .unwrap_or_else(cache_dir);
    if dir.as_os_str().len() > 60 {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        return std::path::PathBuf::from(format!("/tmp/kerdr-{uid}"));
    }
    dir
}

pub fn ssh_args() -> Vec<String> {
    let dir = control_dir();
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let control = dir.join("ssh-%C");
    [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=2",
        "-o",
        "ControlMaster=auto",
        "-o",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        format!("ControlPath={}", control.display()),
        "-o".to_string(),
        "ControlPersist=60".to_string(),
    ])
    .collect()
}

/// Runs a POSIX shell script on the target, feeding it on standard input.
pub fn run_script(target: &Target, script: &str, deadline: Duration) -> Output {
    let mut cmd = match target {
        Target::Local => {
            let mut c = Command::new("sh");
            c.arg("-s");
            for var in HERDR_VARS {
                c.env_remove(var);
            }
            c
        }
        Target::Ssh(host) => {
            let mut c = Command::new("ssh");
            c.args(ssh_args()).arg("--").arg(host).arg("sh -s");
            c
        }
    };
    run(&mut cmd, Some(script.as_bytes()), deadline)
}

/// Runs herdr with these arguments on the target.
pub fn run_herdr(target: &Target, herdr: &str, args: &[&str], deadline: Duration) -> Output {
    let quoted: Vec<String> = args.iter().map(|a| quote(a)).collect();
    let script = format!(
        "{PATH_PREFIX}\nunset {}\nexec {} {}\n",
        HERDR_VARS.join(" "),
        herdr_word(herdr),
        quoted.join(" ")
    );
    run_script(target, &script, deadline)
}

fn run(cmd: &mut Command, input: Option<&[u8]>, deadline: Duration) -> Output {
    let started = Instant::now();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so the deadline, and the end of the run, stop everything it
    // started, not only the shell.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Output {
                status: Some(127),
                stderr: e.to_string(),
                took: started.elapsed(),
                ..Default::default()
            };
        }
    };
    let pid = child.id() as i32;
    if let Some(mut stdin) = child.stdin.take() {
        let data = input.unwrap_or_default().to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let reader = |stream: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = mpsc::channel();
        if let Some(mut s) = stream {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        }
        rx
    };
    let out_rx = reader(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err_rx = reader(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok().and_then(|s| s.code());
            }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    // Whatever it left running in the background goes too, so nothing holds its output open.
    #[cfg(unix)]
    // SAFETY: signals the process group this function created for the child.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let grace = deadline
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(250));
    let stdout = out_rx.recv_timeout(grace).unwrap_or_default();
    let stderr = err_rx
        .recv_timeout(Duration::from_millis(250))
        .unwrap_or_default();
    Output {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
        took: started.elapsed(),
    }
}

/// Single-quotes a word for a POSIX shell.
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The herdr command as a shell word, with a leading `~/` left to the shell to expand.
pub fn herdr_word(herdr: &str) -> String {
    match herdr.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
        None => quote(herdr),
    }
}

/// What went wrong reaching a machine, in the terms the board shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No answer: asleep, off, off the network, or the name doesn't resolve.
    Unreachable(String),
    /// SSH wants a decision about the host's key.
    HostKey(String),
    /// SSH wants a password, a passphrase or another key.
    Auth(String),
    /// It answered, but something else failed.
    Other(String),
}

impl Problem {
    pub fn detail(&self) -> &str {
        match self {
            Problem::Unreachable(d)
            | Problem::HostKey(d)
            | Problem::Auth(d)
            | Problem::Other(d) => d,
        }
    }
}

/// Reads an SSH failure. Returns None when SSH itself worked.
pub fn classify(target: &Target, out: &Output) -> Option<Problem> {
    if out.timed_out {
        return Some(Problem::Unreachable("no answer in time".into()));
    }
    if *target == Target::Local || out.status != Some(255) {
        return None;
    }
    let err = out.stderr.to_lowercase();
    let line = out
        .stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("SSH failed")
        .to_string();
    Some(
        if err.contains("host key verification failed")
            || err.contains("remote host identification has changed")
            || err.contains("host key for")
            || err.contains("no ed25519 host key")
        {
            Problem::HostKey(line)
        } else if err.contains("permission denied")
            || err.contains("too many authentication failures")
            || err.contains("passphrase")
            || err.contains("authentication")
        {
            Problem::Auth(line)
        } else if err.contains("could not resolve")
            || err.contains("timed out")
            || err.contains("no route to host")
            || err.contains("connection refused")
            || err.contains("network is unreachable")
            || err.contains("connection closed")
            || err.contains("broken pipe")
            || err.contains("connection reset")
            || err.contains("name or service not known")
        {
            Problem::Unreachable(line)
        } else {
            Problem::Other(line)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stderr: &str) -> Output {
        Output {
            status: Some(255),
            stderr: stderr.into(),
            ..Default::default()
        }
    }

    #[test]
    fn quotes_shell_words() {
        assert_eq!(quote("w7"), "w7");
        assert_eq!(quote("kerdr_jump=next"), "kerdr_jump=next");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn expands_home_in_herdr_path() {
        assert_eq!(
            herdr_word("~/.local/bin/herdr"),
            "\"$HOME\"/.local/bin/herdr"
        );
        assert_eq!(herdr_word("herdr"), "herdr");
    }

    #[test]
    fn reads_ssh_failures() {
        let t = Target::Ssh("box".into());
        assert!(matches!(
            classify(
                &t,
                &failed("ssh: connect to host 100.70.38.69 port 22: Connection timed out")
            ),
            Some(Problem::Unreachable(_))
        ));
        assert!(matches!(
            classify(&t, &failed("Host key verification failed.")),
            Some(Problem::HostKey(_))
        ));
        assert!(matches!(
            classify(&t, &failed("drew@box: Permission denied (publickey).")),
            Some(Problem::Auth(_))
        ));
        assert!(matches!(
            classify(
                &t,
                &failed("ssh: Could not resolve hostname box: Name or service not known")
            ),
            Some(Problem::Unreachable(_))
        ));
        let fine = Output {
            status: Some(0),
            ..Default::default()
        };
        assert_eq!(classify(&t, &fine), None);
    }

    #[test]
    fn a_background_process_cant_hold_a_read_open() {
        let started = Instant::now();
        let out = run_script(&Target::Local, "sleep 6 & echo hi", Duration::from_secs(1));
        assert_eq!(out.stdout.trim(), "hi");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn runs_scripts_locally_with_a_deadline() {
        let out = run_script(
            &Target::Local,
            "echo hi; echo oops >&2",
            Duration::from_secs(5),
        );
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hi");
        assert_eq!(out.stderr.trim(), "oops");
        let slow = run_script(&Target::Local, "sleep 5", Duration::from_millis(200));
        assert!(slow.timed_out);
    }
}
