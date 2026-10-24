//! Running commands on a machine: this one directly, others over SSH. Every SSH command shares
//! one connection per machine (OpenSSH's ControlMaster), never prompts, and has a deadline.

use std::io::{Read, Seek, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
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
#[cfg(unix)]
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
    let args = [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=2",
    ];
    let mut args: Vec<String> = args.into_iter().map(String::from).collect();
    // Microsoft's native OpenSSH has no Unix control sockets.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = control_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        args.extend([
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", dir.join("ssh-%C").display()),
            "-o".into(),
            "ControlPersist=60".into(),
        ]);
    }
    #[cfg(windows)]
    args.extend(["-o".into(), "ControlMaster=no".into()]);
    args
}

/// A PowerShell literal, without interpolation (including apostrophes and newlines).
pub fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Powershell 5.1 is present on supported Windows systems. EncodedCommand keeps scripts out
/// of stdin, leaving it available for adapter JSON, and preserves Unicode source text.
pub fn powershell_command(script: &str) -> Command {
    use base64::Engine;
    let utf8 = "$ProgressPreference = 'SilentlyContinue'; [Console]::InputEncoding = New-Object System.Text.UTF8Encoding $false; [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $OutputEncoding = [Console]::OutputEncoding; ";
    let bytes: Vec<u8> = format!("{utf8}{script}")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-InputFormat",
        "Text",
        "-OutputFormat",
        "Text",
        "-EncodedCommand",
        &base64::engine::general_purpose::STANDARD.encode(bytes),
    ]);
    cmd
}

pub fn clean_herdr_env(cmd: &mut Command) {
    for var in HERDR_VARS {
        cmd.env_remove(var);
    }
}

/// Runs POSIX source remotely and on Unix; local Windows source uses PowerShell.
pub fn run_script(target: &Target, script: &str, deadline: Duration) -> Output {
    let mut cmd = match target {
        Target::Local => {
            #[cfg(not(windows))]
            let mut c = {
                let mut c = Command::new("sh");
                c.arg("-s");
                c
            };
            #[cfg(windows)]
            let mut c = powershell_command(script);
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
    run(&mut cmd, Some(script.as_bytes()), deadline, true)
}

/// Runs herdr with these arguments on the target.
pub fn run_herdr(target: &Target, herdr: &str, args: &[&str], deadline: Duration) -> Output {
    #[cfg(windows)]
    if *target == Target::Local {
        let mut cmd = Command::new(crate::jump::expand_home(herdr));
        cmd.args(args);
        clean_herdr_env(&mut cmd);
        // Never put Herdr clients in a kill-on-close job: they may start a persistent server.
        return run(&mut cmd, None, deadline, false);
    }
    let quoted: Vec<String> = args.iter().map(|a| quote(a)).collect();
    let script = format!(
        "{PATH_PREFIX}\nunset {}\nexec {} {}\n",
        HERDR_VARS.join(" "),
        herdr_word(herdr),
        quoted.join(" ")
    );
    run_script(target, &script, deadline)
}

/// Runs a configured adapter with optional JSON on stdin.
pub fn run_adapter(
    target: &Target,
    command: &str,
    input: Option<&str>,
    deadline: Duration,
) -> Output {
    #[cfg(windows)]
    if *target == Target::Local {
        let mut cmd = powershell_command(&format!(
            "$ErrorActionPreference = 'Stop'; & {{ {command} }}; if ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }}"
        ));
        clean_herdr_env(&mut cmd);
        cmd.current_dir(crate::config::home());
        // Actions may intentionally launch persistent work. Bound this invocation only;
        // its backend must own that work's lifetime, including after an unknown result.
        return run(
            &mut cmd,
            input.map(str::as_bytes),
            deadline,
            input.is_none(),
        );
    }
    let script = format!(
        "{PATH_PREFIX}\nunset {}\nprintf '%s' {} | {command}\n",
        HERDR_VARS.join(" "),
        quote(input.unwrap_or_default())
    );
    run_script(target, &script, deadline)
}

fn run(cmd: &mut Command, input: Option<&[u8]>, deadline: Duration, _kill_tree: bool) -> Output {
    let started = Instant::now();
    // A temporary input stream avoids an unbounded writer thread when an action leaves a
    // persistent descendant holding stdin open. Adapters still receive exactly their JSON.
    let captures = (|| -> std::io::Result<_> {
        let stdin = if let Some(input) = input {
            let mut file = tempfile::tempfile()?;
            file.write_all(input)?;
            file.rewind()?;
            Stdio::from(file)
        } else {
            Stdio::null()
        };
        // Independent reader handles avoid moving an inherited writer's file position.
        // Unlinking a temporary capture leaves persistent children with writable handles.
        let stdout = tempfile::NamedTempFile::new()?;
        let stderr = tempfile::NamedTempFile::new()?;
        let out_reader = std::fs::File::open(stdout.path())?;
        let err_reader = std::fs::File::open(stderr.path())?;
        cmd.stdin(stdin)
            .stdout(Stdio::from(stdout.as_file().try_clone()?))
            .stderr(Stdio::from(stderr.as_file().try_clone()?));
        Ok((stdout, stderr, out_reader, err_reader))
    })();
    let (stdout_file, stderr_file, out_reader, err_reader) = match captures {
        Ok(captures) => captures,
        Err(e) => {
            return Output {
                status: Some(127),
                stderr: e.to_string(),
                took: started.elapsed(),
                ..Default::default()
            };
        }
    };
    const MAX_CAPTURE: u64 = 32 * 1024 * 1024;
    // Its own process group, so the deadline, and the end of the run, stop everything it
    // started, not only the shell.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    if _kill_tree {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        // Enroll disposable commands before they can start descendants.
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
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
    #[cfg(unix)]
    let pid = child.id() as i32;
    #[cfg(windows)]
    let _job = if _kill_tree {
        match crate::windows::ProcessJob::assign(&child) {
            Ok(job) => Some(job),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Output {
                    status: Some(127),
                    stderr: format!("couldn't bound adapter lifetime: {e}"),
                    took: started.elapsed(),
                    ..Default::default()
                };
            }
        }
    } else {
        None
    };
    let mut timed_out = false;
    let mut overflow = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None)
                if stdout_file
                    .as_file()
                    .metadata()
                    .is_ok_and(|m| m.len() > MAX_CAPTURE)
                    || stderr_file
                        .as_file()
                        .metadata()
                        .is_ok_and(|m| m.len() > MAX_CAPTURE) =>
            {
                overflow = true;
                let _ = child.kill();
                break child.wait().ok().and_then(|s| s.code());
            }
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
    #[cfg(windows)]
    drop(_job);
    let capture = |reader: std::fs::File| {
        let mut data = Vec::new();
        let _ = reader.take(MAX_CAPTURE + 1).read_to_end(&mut data);
        data
    };
    let stdout = capture(out_reader);
    let stderr = capture(err_reader);
    if overflow || stdout.len() as u64 > MAX_CAPTURE || stderr.len() as u64 > MAX_CAPTURE {
        return Output {
            status: Some(125),
            stderr: "command output exceeded 32 MiB; its result is unknown".into(),
            took: started.elapsed(),
            ..Default::default()
        };
    }
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
    fn installed_openssh_accepts_platform_options_without_connecting() {
        let args = ssh_args();
        #[cfg(windows)]
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("ControlPath=") || a == "ControlMaster=auto")
        );
        let out = Command::new("ssh")
            .args(&args)
            .args(["-G", "example.invalid"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
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
        let script = if cfg!(windows) {
            "Start-Process powershell.exe -NoNewWindow -ArgumentList '-NoProfile -Command Start-Sleep 6'; Write-Output hi"
        } else {
            "sleep 6 & echo hi"
        };
        let out = run_script(&Target::Local, script, Duration::from_secs(3));
        assert_eq!(out.stdout.trim(), "hi");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn runs_scripts_locally_with_a_deadline() {
        let out = run_script(
            &Target::Local,
            if cfg!(windows) {
                "Write-Output hi; [Console]::Error.WriteLine('oops')"
            } else {
                "echo hi; echo oops >&2"
            },
            Duration::from_secs(5),
        );
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hi");
        assert_eq!(out.stderr.trim(), "oops");
        let slow = run_script(
            &Target::Local,
            if cfg!(windows) {
                "Start-Sleep 5"
            } else {
                "sleep 5"
            },
            Duration::from_millis(200),
        );
        assert!(slow.timed_out);
    }
}
