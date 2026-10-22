//! One reader per machine. Each read is a single round trip: a short shell script that prints
//! herdr's version, its sessions, a snapshot of every running session and, when it's due, the
//! card feed. Failures move the machine through the connection states the board shows.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

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
}

#[derive(Debug)]
pub struct Event {
    pub machine: usize,
    pub conn: Conn,
    pub data: Option<MachineData>,
    pub at: DateTime<Local>,
}

pub enum Wake {
    Now,
    Feed,
    Stop,
}

pub struct Poller {
    pub wake: Sender<Wake>,
}

pub struct Timing {
    pub every: Duration,
    pub feed_every: Duration,
}

pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((2u64 << attempt.min(6)).min(120))
}

const GIVE_UP_AFTER: i64 = 120;

pub fn spawn(index: usize, machine: Machine, timing: Timing, tx: SyncSender<Event>) -> Poller {
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
        loop {
            let feed_due = machine.feed.is_some()
                && last_feed.is_none_or(|t| t.elapsed() >= timing.feed_every);
            let script = build_script(&machine.herdr, machine.feed.as_deref().filter(|_| feed_due));
            let out = transport::run_script(&target, &script, Duration::from_secs(25));
            let now = Local::now();
            let (conn, data, wait) = match read(&target, &out, feed_due) {
                Ok(data) => {
                    attempt = 0;
                    first_failure = None;
                    ever_live = true;
                    if feed_due {
                        last_feed = Some(std::time::Instant::now());
                    }
                    (Conn::Live { latency: out.took }, Some(data), timing.every)
                }
                Err(Failure::Herdr(conn)) => (conn, None, Duration::from_secs(120)),
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
                        Duration::from_secs(120)
                    } else {
                        wait
                    };
                    (conn, None, wait)
                }
            };
            // A full queue means the board is busy elsewhere (you're inside herdr): this read is
            // dropped rather than piling up, and the next one brings it up to date.
            if let Err(TrySendError::Disconnected(_)) = tx.try_send(Event {
                machine: index,
                conn,
                data,
                at: now,
            }) {
                return;
            }
            match wake_rx.recv_timeout(wait) {
                Ok(Wake::Feed) => {
                    last_feed = None;
                    continue;
                }
                Ok(Wake::Now) | Err(RecvTimeoutError::Timeout) => continue,
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    Poller { wake: wake_tx }
}

enum Failure {
    Ssh(Problem),
    Herdr(Conn),
}

const MARK: &str = "@@KERDR ";

/// The script one read runs on the machine.
pub fn build_script(herdr: &str, feed: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(PATH_PREFIX);
    s.push('\n');
    s.push_str(&format!("unset {}\n", HERDR_VARS.join(" ")));
    s.push_str(&format!("H={}\n", transport::herdr_word(herdr)));
    s.push_str(&format!("echo '{MARK}host'; uname -n\n"));
    s.push_str(&format!(
        "if ! command -v \"$H\" >/dev/null 2>&1; then echo '{MARK}missing'; exit 0; fi\n"
    ));
    s.push_str(&format!("echo '{MARK}version'; \"$H\" --version 2>&1\n"));
    s.push_str(&format!(
        "echo '{MARK}keys'; grep -q kerdr_jump \"${{HERDR_CONFIG_PATH:-$HOME/.config/herdr/config.toml}}\" 2>/dev/null && echo yes || echo no\n"
    ));
    s.push_str(&format!(
        "S=$(\"$H\" session list --json 2>&1); echo '{MARK}sessions'; printf '%s\\n' \"$S\"\n"
    ));
    // Session names only split into words here; never expanded as file patterns.
    s.push_str("set -f\n");
    s.push_str(
        "for s in $(printf '%s' \"$S\" | tr '{' '\\n' | grep '\"running\":true' | sed -n 's/.*\"name\":\"\\([^\"]*\\)\".*/\\1/p'); do\n",
    );
    s.push_str(&format!(
        "  echo \"{MARK}snapshot $s\"; \"$H\" --session \"$s\" api snapshot 2>&1\ndone\n"
    ));
    if let Some(cmd) = feed {
        s.push_str(&format!("KERDR_FEED={}\n", transport::quote(cmd)));
        s.push_str(&format!(
            "echo '{MARK}feed'; cd \"$HOME\" && if command -v timeout >/dev/null 2>&1; then timeout 12 sh -c \"$KERDR_FEED\" </dev/null 2>/dev/null; else sh -c \"$KERDR_FEED\" </dev/null 2>/dev/null; fi\n"
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
            .unwrap_or("the read stopped part way")
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
    let sessions = herdr::parse_sessions(sections.sessions.as_deref().unwrap_or(""))
        .map_err(|e| Failure::Ssh(Problem::Other(e)))?;
    let snapshots = sections
        .snapshots
        .into_iter()
        .map(|(name, body)| (name, herdr::parse_snapshot(&body)))
        .collect();
    let feed = if feed_due {
        Some(match sections.feed.as_deref() {
            Some(body) if !body.trim().is_empty() => feed::parse(body),
            _ => Err("the feed printed nothing".to_string()),
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
    })
}

/// One read, now, for `kerdr doctor` and `kerdr cards`.
pub fn read_once(machine: &Machine) -> (Conn, Option<MachineData>) {
    let target = match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    };
    let script = build_script(&machine.herdr, machine.feed.as_deref());
    let out = transport::run_script(&target, &script, Duration::from_secs(25));
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
    fn backoff_grows_to_two_minutes() {
        let secs: Vec<u64> = (0..9).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 64, 120, 120, 120]);
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
