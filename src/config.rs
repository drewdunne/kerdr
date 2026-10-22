//! Kerdr's settings: which machines to read, how to reach them, the card feed, and the board's
//! work-in-progress limits. Everything has a default, so Kerdr runs with no config at all.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// This machine's name on the board. Defaults to its hostname.
    pub name: Option<String>,
    pub board: BoardConfig,
    pub feed: FeedConfig,
    pub actions: ActionsConfig,
    pub machines: MachinesConfig,
    #[serde(rename = "machine")]
    pub machine: Vec<MachineConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BoardConfig {
    /// Workspaces with an agent a session holds before Kerdr calls it full.
    pub slots: u32,
    /// Sessions with no limit, such as a lane for background agents.
    pub unlimited: Vec<String>,
    /// Show only these sessions (empty: all of them).
    pub only: Vec<String>,
    /// Never show these sessions.
    pub skip: Vec<String>,
}

impl BoardConfig {
    pub fn wants(&self, session: &str) -> bool {
        (self.only.is_empty() || self.only.iter().any(|s| s == session))
            && !self.skip.iter().any(|s| s == session)
    }
}

impl Default for BoardConfig {
    fn default() -> Self {
        BoardConfig {
            slots: 3,
            unlimited: vec!["background".to_string()],
            only: vec![],
            skip: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeedConfig {
    /// A command each machine runs to print its cards (docs/feed.md). None: herdr only.
    pub command: Option<String>,
    /// Seconds between feed runs.
    pub every: u64,
}

impl Default for FeedConfig {
    fn default() -> Self {
        FeedConfig {
            command: None,
            every: 15,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActionsConfig {
    /// An opt-in command that receives one JSON action on standard input (docs/feed.md).
    pub command: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MachinesConfig {
    /// Also read the machines the herdr session picker lists (~/.config/herdr-sessions/machines).
    pub picker: bool,
    /// Seconds between reads of this machine, and of machines over SSH.
    pub local_every: f64,
    pub remote_every: f64,
}

impl Default for MachinesConfig {
    fn default() -> Self {
        MachinesConfig {
            picker: true,
            local_every: 2.0,
            remote_every: 4.0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MachineConfig {
    pub name: String,
    /// SSH target (a host from your SSH config, user@host, or ssh://user@host:port).
    pub ssh: Option<String>,
    /// herdr's path there, when it isn't on the PATH an SSH command gets.
    pub herdr: Option<String>,
    /// This machine's own feed command; "" turns the feed off for it.
    pub feed: Option<String>,
    pub hidden: bool,
}

/// One machine Kerdr reads, after the config, the picker's list and this host are combined.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub name: String,
    /// None for this machine.
    pub ssh: Option<String>,
    pub herdr: String,
    pub feed: Option<String>,
    pub hidden: bool,
}

impl Machine {
    pub fn is_local(&self) -> bool {
        self.ssh.is_none()
    }
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("KERDR_CONFIG") {
        return PathBuf::from(p);
    }
    config_dir().join("kerdr").join("config.toml")
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local").join("state"))
        .join("kerdr")
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".cache"))
        .join("kerdr")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into a buffer we own.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).to_string();
        if !name.is_empty() {
            return name.split('.').next().unwrap_or(&name).to_string();
        }
    }
    "this machine".to_string()
}

impl Config {
    pub fn load() -> Result<Config, String> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// The machines to read: this one first, then the config's, then the picker's list.
    pub fn machines(&self, host: &str, picker_list: &[String]) -> Vec<Machine> {
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let here_name = self.name.clone().unwrap_or_else(|| host.to_string());
        let feed_for = |m: Option<&MachineConfig>| -> Option<String> {
            match m.and_then(|m| m.feed.clone()) {
                Some(f) if f.trim().is_empty() => None,
                Some(f) => Some(f),
                None => self.feed.command.clone().filter(|c| !c.trim().is_empty()),
            }
        };
        let herdr_for = |m: Option<&MachineConfig>| {
            m.and_then(|m| m.herdr.clone())
                .unwrap_or_else(|| "herdr".into())
        };

        let here_cfg = self
            .machine
            .iter()
            .find(|m| same(&m.name, &here_name) || same(&m.name, host));
        let mut out = vec![Machine {
            name: here_name.clone(),
            ssh: None,
            herdr: herdr_for(here_cfg),
            feed: feed_for(here_cfg),
            hidden: false,
        }];

        for m in &self.machine {
            if same(&m.name, &here_name) || same(&m.name, host) {
                continue;
            }
            let ssh = m.ssh.clone().unwrap_or_else(|| m.name.clone());
            if out.iter().any(|o| o.ssh.as_deref() == Some(ssh.as_str())) {
                continue;
            }
            out.push(Machine {
                name: m.name.clone(),
                ssh: Some(ssh),
                herdr: herdr_for(Some(m)),
                feed: feed_for(Some(m)),
                hidden: m.hidden,
            });
        }

        if self.machines.picker {
            for target in picker_list {
                let short = target.rsplit('@').next().unwrap_or(target);
                if same(short, host) || same(short, &here_name) {
                    continue;
                }
                let known = out
                    .iter()
                    .any(|o| o.ssh.as_deref() == Some(target.as_str()) || same(&o.name, short));
                if !known {
                    out.push(Machine {
                        name: short.to_string(),
                        ssh: Some(target.clone()),
                        herdr: "herdr".into(),
                        feed: feed_for(None),
                        hidden: false,
                    });
                }
            }
        }
        out
    }
}

/// The herdr session picker's machine list: one SSH target per line, `#` for comments.
pub fn picker_machines() -> Vec<String> {
    let path = config_dir().join("herdr-sessions").join("machines");
    std::fs::read_to_string(path)
        .map(|text| parse_picker(&text))
        .unwrap_or_default()
}

pub fn parse_picker(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn no_config_means_this_machine_only() {
        let machines = Config::default().machines("laptop", &[]);
        assert_eq!(machines.len(), 1);
        assert!(machines[0].is_local());
        assert_eq!(machines[0].name, "laptop");
    }

    #[test]
    fn one_config_works_on_every_machine() {
        let c = cfg(r#"
            [feed]
            command = "~/bin/cards --json"
            [[machine]]
            name = "build-box"
            ssh = "box"
            [[machine]]
            name = "laptop"
        "#);
        let on_laptop = c.machines("laptop", &[]);
        assert_eq!(on_laptop.len(), 2);
        assert!(on_laptop[0].is_local() && on_laptop[0].name == "laptop");
        assert_eq!(on_laptop[1].ssh.as_deref(), Some("box"));
        assert_eq!(on_laptop[1].feed.as_deref(), Some("~/bin/cards --json"));

        let on_server = c.machines("Build-Box", &[]);
        assert_eq!(on_server[0].name, "Build-Box");
        assert_eq!(on_server[1].name, "laptop");
        assert_eq!(on_server[1].ssh.as_deref(), Some("laptop"));
    }

    #[test]
    fn picker_list_adds_machines_but_skips_this_one_and_known_ones() {
        let c = cfg(r#"
            [[machine]]
            name = "build-box"
            ssh = "box"
        "#);
        let list = parse_picker("# comment\nbuild-box\nold-desktop\nlaptop\n");
        let ms = c.machines("laptop", &list);
        let names: Vec<_> = ms.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["laptop", "build-box", "old-desktop"]);
    }

    #[test]
    fn an_empty_feed_turns_it_off_for_one_machine() {
        let c = cfg(r#"
            [feed]
            command = "cards"
            [[machine]]
            name = "box"
            feed = ""
        "#);
        let ms = c.machines("here", &[]);
        assert_eq!(ms[0].feed.as_deref(), Some("cards"));
        assert_eq!(ms[1].feed, None);
    }

    #[test]
    fn unknown_keys_are_an_error() {
        assert!(toml::from_str::<Config>("[board]\nslotz = 4\n").is_err());
    }

    #[test]
    fn action_commands_are_an_explicit_opt_in() {
        assert!(Config::default().actions.command.is_none());
        assert_eq!(
            cfg("[actions]\ncommand = '~/bin/cards-action'\n")
                .actions
                .command
                .as_deref(),
            Some("~/bin/cards-action")
        );
        assert!(toml::from_str::<Config>("[actions]\ncomand = 'oops'\n").is_err());
    }
}
