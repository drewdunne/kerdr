//! Kerdr's settings: which machines to read, how to reach them, the card feed, and the board's
//! work-in-progress limits. Everything has a default, so Kerdr runs with no config at all.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// This machine's name on the board. Defaults to its hostname.
    pub name: Option<String>,
    /// What `e` opens a backlog item's text in, with any arguments (`code --wait`); the file's
    /// path goes after them. None: `$VISUAL`, else `$EDITOR`, else nvim, else vim.
    pub editor: Option<String>,
    pub board: BoardConfig,
    pub feed: FeedConfig,
    pub actions: ActionsConfig,
    pub machines: MachinesConfig,
    pub theme: ThemeConfig,
    #[serde(rename = "machine")]
    pub machine: Vec<MachineConfig>,
    /// What in the config file couldn't be used, each said as the board says it: Kerdr opens with
    /// the rest, and these parts at their defaults (intent 0591, K3-67, K3-68). Never written back.
    #[serde(skip)]
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BoardConfig {
    /// Workspaces with an agent a session holds before Kerdr calls it full.
    pub slots: u32,
    /// Sessions with no limit; * matches any text in a session name.
    pub unlimited: Vec<String>,
    /// Show only these sessions (empty: all of them).
    pub only: Vec<String>,
    /// Never show these sessions.
    pub skip: Vec<String>,
    /// Columns the board leaves out, giving their room to the others: `backlog`, `maintain`, and
    /// any stage by its id (`deploy`, `learn`), on every stage list (intent 0595). `hide` names
    /// the same.
    #[serde(alias = "hide")]
    pub hide_columns: Vec<String>,
    /// What the top bar splits the board by: `owners` (the default) when a feed lists them, else
    /// machines; `machines` keeps the machine and session split whatever the feeds list.
    pub split: Split,
}

/// What the top bar splits the board by (intent 0595).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Owners, then each owner's teams, whenever a feed lists owners; else machines.
    #[default]
    Owners,
    /// Machines, then each machine's herdr sessions.
    Machines,
}

impl BoardConfig {
    pub fn is_unlimited(&self, session: &str) -> bool {
        self.unlimited.iter().any(|pattern| {
            let Some((prefix, rest)) = pattern.split_once('*') else {
                return pattern == session;
            };
            let Some(mut remaining) = session.strip_prefix(prefix) else {
                return false;
            };
            let mut parts = rest.split('*').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_none() {
                    return remaining.ends_with(part);
                }
                let Some((_, after)) = remaining.split_once(part) else {
                    return false;
                };
                remaining = after;
            }
            true
        })
    }

    /// Whether the config leaves this stage's column off the board: Backlog and Maintain by any
    /// of their names, any other stage by its id, on whatever list it's on.
    pub fn hides(&self, stage: crate::model::Stage) -> bool {
        use crate::model::Stage;
        self.hide_columns.iter().any(|c| match stage {
            Stage::Backlog | Stage::Maintain => Stage::parse(c) == Some(stage),
            _ => c.trim().eq_ignore_ascii_case(stage.id()),
        })
    }

    /// The names in hide_columns that hide nothing a column can lose: Misc, whose cards have no
    /// screen of their own, and an empty name. Each is left out, so its column shows, and said
    /// (intent 0591, K3-68): hiding it would hide work.
    fn drop_unhidable(&mut self) -> Vec<String> {
        let mut said = vec![];
        self.hide_columns.retain(|c| {
            let c = c.trim();
            if c.eq_ignore_ascii_case("misc") {
                said.push(
                    "[board] hide_columns: \"misc\" is left showing, since its cards have no other screen; name \"backlog\", \"maintain\" or a stage's id.".to_string(),
                );
                false
            } else if c.is_empty() {
                said.push(
                    "[board] hide_columns: an empty name hides nothing, so it's left out; name \"backlog\", \"maintain\" or a stage's id.".to_string(),
                );
                false
            } else {
                true
            }
        });
        said
    }

    pub fn wants(&self, session: &str) -> bool {
        (self.only.is_empty() || self.only.iter().any(|s| s == session))
            && !self.skip.iter().any(|s| s == session)
    }
}

impl Config {
    /// The default config keeping the machine split, as boards were before owners (intent 0595):
    /// for the tests written about that split.
    #[cfg(test)]
    pub fn machine_split() -> Config {
        let mut c = Config::default();
        c.board.split = Split::Machines;
        c
    }
}

impl Default for BoardConfig {
    fn default() -> Self {
        BoardConfig {
            slots: 3,
            unlimited: vec!["background".to_string()],
            only: vec![],
            skip: vec![],
            hide_columns: vec![],
            split: Split::Owners,
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

impl FeedConfig {
    /// The actions command of the same system as this feed, for a config that names none (intent
    /// 0591, K1-12): ark-sdlc's `ark-card-action` beside its `ark-cards`, whose feed it answers
    /// for. Any other feed's backend is never guessed.
    pub fn ark_card_action(&self) -> Option<String> {
        let command = self.command.as_deref()?.trim();
        let program = command.split_whitespace().next()?;
        let (dir, name) = match program.rsplit_once('/') {
            Some((dir, name)) => (Some(dir), name),
            None => (None, program),
        };
        (name == "ark-cards").then(|| match dir {
            Some(dir) => format!("{dir}/ark-card-action"),
            None => "ark-card-action".into(),
        })
    }
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// A colors.toml to take Kerdr's colours from, followed while Kerdr is open. None (the
    /// default): Omarchy's current theme where there is one, else Tokyo Night. "": Tokyo Night.
    pub colors: Option<String>,
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
        .unwrap_or_else(platform_config_dir)
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(platform_state_dir)
        .join("kerdr")
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(platform_cache_dir)
        .join("kerdr")
}

fn platform_config_dir() -> PathBuf {
    #[cfg(windows)]
    return std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData/Roaming"));
    #[cfg(not(windows))]
    home().join(".config")
}

fn platform_state_dir() -> PathBuf {
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData/Local"));
    #[cfg(not(windows))]
    home().join(".local/state")
}

fn platform_cache_dir() -> PathBuf {
    #[cfg(windows)]
    return platform_state_dir().join("cache");
    #[cfg(not(windows))]
    home().join(".cache")
}

pub fn home() -> PathBuf {
    #[cfg(windows)]
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(profile);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .map(|h| h.split('.').next().unwrap_or(&h).to_string())
        .unwrap_or_else(|| "this machine".to_string())
}

impl Config {
    pub fn load() -> Result<Config, String> {
        Self::load_from(&config_path())
    }

    /// The config at `path`, or the defaults when there's none. A file that can't be read, or
    /// parts of it that don't parse, never keep Kerdr from opening (intent 0591, K3-67): it opens
    /// with the parts that do, the rest at their defaults, each problem in `problems` with its line
    /// for the board to say. The file itself is never written.
    pub fn load_from(path: &Path) -> Result<Config, String> {
        let shown = path.display();
        let mut cfg = match std::fs::read_to_string(path) {
            Ok(text) => {
                let (mut cfg, problems) = parse_leniently(&text);
                cfg.problems = problems
                    .into_iter()
                    .map(|p| format!("{shown}: {p}"))
                    .collect();
                cfg
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => Config {
                problems: vec![format!(
                    "{shown}: couldn't read it ({e}), so Kerdr opened with its defaults"
                )],
                ..Config::default()
            },
        };
        for p in cfg.board.drop_unhidable() {
            cfg.problems.push(format!("{shown}: {p}"));
        }
        if cfg.actions.command.is_none() {
            cfg.actions.command = cfg.feed.ark_card_action();
        }
        Ok(cfg)
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

/// A config file's text as Kerdr's settings, keeping every part that parses: a line that doesn't
/// (an unknown key, a value of the wrong kind) is left out, and a table whose fault is the table
/// itself goes whole, each said with its line, until the rest reads. Text that isn't TOML at all
/// leaves every setting at its default, said the same way. Nothing here writes the file.
pub fn parse_leniently(text: &str) -> (Config, Vec<String>) {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let mut problems = vec![];
    // Each round leaves out at least one line, so this ends; the bound is only a guard.
    for _ in 0..64 {
        let now = lines.join("\n");
        let e = match toml::from_str::<Config>(&now) {
            Ok(cfg) => return (cfg, problems),
            Err(e) => e,
        };
        let line = e
            .span()
            .map(|span| now[..span.start.min(now.len())].matches('\n').count());
        let said = e.message().trim().trim_end_matches('.').to_string();
        // Not TOML at all: nothing in it can be trusted to mean what it says.
        if toml::from_str::<toml::Table>(&now).is_err() || line.is_none() {
            problems.push(match line {
                Some(n) => format!(
                    "line {}: {said}, so Kerdr opened with its defaults; fix the file and open Kerdr again",
                    n + 1
                ),
                None => format!(
                    "{said}, so Kerdr opened with its defaults; fix the file and open Kerdr again"
                ),
            });
            return (Config::default(), problems);
        }
        let n = line.unwrap_or(0);
        let gone = leave_out(&mut lines, n, e.span().unwrap_or(0..0), &now);
        problems.push(format!(
            "line {}: {said}, so Kerdr left {gone} out and opened with the rest",
            n + 1
        ));
    }
    problems.push("too many lines didn't parse, so Kerdr opened with its defaults".into());
    (Config::default(), problems)
}

/// Blanks the lines a fault in the config covers, keeping every other line where it was, so the
/// next fault's line is still its line in the file: the lines of the value or key at fault, or a
/// whole table from its heading to the next one's when the fault is the table's. What was left
/// out, in words.
fn leave_out(lines: &mut [String], at: usize, span: std::ops::Range<usize>, text: &str) -> String {
    let heading = |l: &str| l.trim_start().starts_with('[');
    if lines.get(at).is_some_and(|l| heading(l)) {
        let name = lines[at].trim().to_string();
        let end = (at + 1..lines.len())
            .find(|&i| heading(&lines[i]))
            .unwrap_or(lines.len());
        for l in &mut lines[at..end] {
            l.clear();
        }
        return format!("{name} and its settings");
    }
    let last = text[..span.end.min(text.len())]
        .matches('\n')
        .count()
        .max(at)
        .min(lines.len().saturating_sub(1));
    let key = lines[at]
        .split('=')
        .next()
        .map(str::trim)
        .filter(|k| !k.is_empty() && !k.starts_with('['))
        .map(|k| format!("`{k}`"))
        .unwrap_or_else(|| "that line".into());
    for l in &mut lines[at..=last] {
        l.clear();
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn hide_columns_takes_backlog_maintain_and_any_stage_by_id_on_every_list() {
        use crate::model::Stage;
        let dir = std::env::temp_dir().join(format!("kerdr-hide-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[board]\nhide_columns = ['backlog', 'done']\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert!(c.board.hides(Stage::Backlog) && c.board.hides(Stage::Maintain));
        assert!(!c.board.hides(Stage::Build));
        assert!(
            !Config::default().board.hides(Stage::Backlog),
            "every column shows by default"
        );
        // A stage by its id, on every list (intent 0595); `hide` names the same.
        std::fs::write(&path, "[board]\nhide = ['deploy', 'learn']\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert!(c.board.hides(Stage::Deploy));
        assert!(
            c.board
                .hides(Stage::work("tinyhost/marketing", "learn", "Learn"))
        );
        assert!(
            c.board
                .hides(Stage::work("arkship/marketing", "deploy", "Deploy"))
        );
        assert!(!c.board.hides(Stage::Build));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_column_that_cant_be_hidden_is_left_showing_and_said() {
        // Intent 0591, K3-68: Kerdr opens anyway, every other name still hides its column, and
        // the board says which name was left out, and why.
        use crate::model::Stage;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[board]\nhide_columns = ['misc', 'backlog']\n").unwrap();
        let c = Config::load_from(&path).expect("Kerdr opens with a name it can't hide");
        assert!(c.board.hides(Stage::Backlog));
        assert!(!c.board.hides(Stage::Misc));
        assert_eq!(c.problems.len(), 1, "{:?}", c.problems);
        assert!(
            c.problems[0]
                .contains("\"misc\" is left showing, since its cards have no other screen"),
            "{:?}",
            c.problems
        );
        std::fs::write(&path, "[board]\nhide_columns = ['']\n").unwrap();
        let c = Config::load_from(&path).expect("Kerdr opens with an empty name");
        assert!(
            c.problems[0].contains("an empty name hides nothing"),
            "{:?}",
            c.problems
        );
    }

    #[test]
    fn a_config_that_doesnt_parse_opens_with_the_parts_that_do_and_says_where() {
        // Intent 0591, K3-67: never a refusal to start. The lines at fault are left out, each said
        // with its line, and everything else in the file still counts; the file isn't touched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "editor = \"hx\"\n[board]\nslotz = 4\nslots = 5\nsplit = 'sideways'\n[feed]\nevery = 30\n";
        std::fs::write(&path, text).unwrap();
        let c = Config::load_from(&path).expect("Kerdr opens with a config that half parses");
        assert_eq!(c.editor.as_deref(), Some("hx"));
        assert_eq!(c.board.slots, 5);
        assert_eq!(c.board.split, Split::Owners, "the bad value is the default");
        assert_eq!(c.feed.every, 30);
        assert_eq!(c.problems.len(), 2, "{:?}", c.problems);
        assert!(c.problems[0].contains("line 3:"), "{:?}", c.problems);
        assert!(c.problems[0].contains("slotz"), "{:?}", c.problems);
        assert!(c.problems[1].contains("line 5:"), "{:?}", c.problems);
        assert!(c.problems[1].contains("`split`"), "{:?}", c.problems);
        assert!(c.problems[0].starts_with(&path.display().to_string()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "never rewritten"
        );
        // A table Kerdr doesn't know goes whole, with its settings; the rest still reads.
        std::fs::write(&path, "[bord]\nslots = 9\n[feed]\nevery = 7\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert_eq!((c.board.slots, c.feed.every), (3, 7));
        assert!(
            c.problems[0].contains("line 1:") && c.problems[0].contains("[bord]"),
            "{:?}",
            c.problems
        );
        // Text that isn't TOML at all: the defaults, said with the line.
        std::fs::write(&path, "[board\nslots = 4\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.board.slots, 3);
        assert!(
            c.problems[0].contains("line 1:") && c.problems[0].contains("defaults"),
            "{:?}",
            c.problems
        );
        // One that can't be read at all opens with the defaults too.
        #[cfg(unix)]
        {
            std::fs::create_dir(dir.path().join("dir.toml")).unwrap();
            let c = Config::load_from(&dir.path().join("dir.toml")).unwrap();
            assert!(
                c.problems[0].contains("couldn't read it"),
                "{:?}",
                c.problems
            );
        }
    }

    #[test]
    fn split_is_owners_unless_the_config_keeps_machines() {
        let dir = std::env::temp_dir().join(format!("kerdr-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        assert_eq!(Config::default().board.split, Split::Owners);
        std::fs::write(&path, "[board]\nsplit = 'machines'\n").unwrap();
        assert_eq!(
            Config::load_from(&path).unwrap().board.split,
            Split::Machines
        );
        // A split it doesn't know: the default, and said (intent 0591, K3-67).
        std::fs::write(&path, "[board]\nsplit = 'sideways'\n").unwrap();
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.board.split, Split::Owners);
        assert_eq!(c.problems.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_ark_cards_feed_with_no_actions_command_uses_the_ark_card_action_beside_it() {
        // Intent 0591, K1-12: the backend of the same system, and only that one.
        let dir = std::env::temp_dir().join(format!("kerdr-actions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let actions = |text: &str| {
            std::fs::write(&path, text).unwrap();
            Config::load_from(&path).unwrap().actions.command
        };
        assert_eq!(
            actions("[feed]\ncommand = \"~/Projects/ark-sdlc/bin/ark-cards --json\"\n"),
            Some("~/Projects/ark-sdlc/bin/ark-card-action".into())
        );
        assert_eq!(
            actions("[feed]\ncommand = \"ark-cards --json\"\n"),
            Some("ark-card-action".into())
        );
        // Another feed's backend is never guessed, and a command he set is his.
        assert_eq!(actions("[feed]\ncommand = \"my-tracker --json\"\n"), None);
        assert_eq!(actions("[feed]\ncommand = \"not-ark-cards\"\n"), None);
        assert_eq!(actions(""), None);
        assert_eq!(
            actions("[feed]\ncommand = \"ark-cards\"\n[actions]\ncommand = \"mine\"\n"),
            Some("mine".into())
        );
        // An empty one turns actions off on purpose.
        assert_eq!(
            actions("[feed]\ncommand = \"ark-cards\"\n[actions]\ncommand = \"\"\n"),
            Some(String::new())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_config_means_this_machine_only() {
        let machines = Config::default().machines("laptop", &[]);
        assert_eq!(machines.len(), 1);
        assert!(machines[0].is_local());
        assert_eq!(machines[0].name, "laptop");
    }

    #[test]
    fn unlimited_patterns_match_the_whole_session_with_stars_anywhere() {
        let c = cfg(
            "[board]\nunlimited = ['*-bg', 'background', 'jobs-*-night*', 'a**b', 'literal?']\n",
        );
        for name in [
            "pick4me-bg",
            "arknet-bg",
            "background",
            "jobs-shop-nightly",
            "jobs--night",
            "ab",
            "a猫b",
            "literal?",
        ] {
            assert!(c.board.is_unlimited(name), "{name}");
        }
        for name in [
            "pick4me",
            "arknet",
            "pick4me-bg-extra",
            "mybackground",
            "jobs-shop-day",
            "jobs-night",
            "literalX",
        ] {
            assert!(!c.board.is_unlimited(name), "{name}");
        }
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

    #[test]
    fn the_editor_is_a_setting_of_its_own_and_none_by_default() {
        assert!(Config::default().editor.is_none());
        assert_eq!(
            cfg("editor = 'code --wait'\n[board]\nslots = 3\n")
                .editor
                .as_deref(),
            Some("code --wait")
        );
    }
}
