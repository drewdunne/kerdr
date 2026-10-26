//! What Kerdr reads from herdr: its version, its sessions, and one snapshot per running session
//! (`herdr --session NAME api snapshot`), plus the tokens Kerdr's own keys leave on a workspace.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The oldest herdr Kerdr works with: 0.9.1 has `api snapshot`, workspace tokens and the
/// keybind help that lists custom commands.
pub const MIN_VERSION: (u32, u32, u32) = (0, 9, 1);

/// Token Kerdr's in-herdr keys set on the workspace you're in, so Kerdr knows to step out.
pub const JUMP_TOKEN: &str = "kerdr_jump";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub name: String,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Deserialize)]
struct SessionList {
    sessions: Vec<SessionInfo>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub protocol: Option<u32>,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub agents: Vec<Agent>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub pane_count: Option<u32>,
    #[serde(default)]
    pub tab_count: Option<u32>,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Agent {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub focused: bool,
    /// The conversation herdr sees in the pane (`agent_session.value`), when it knows it.
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct AgentSession {
    #[serde(default)]
    pub value: Option<String>,
}

impl Snapshot {
    pub fn agents_in<'a>(&'a self, workspace_id: &'a str) -> impl Iterator<Item = &'a Agent> {
        self.agents
            .iter()
            .filter(move |a| a.workspace_id == workspace_id)
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct SnapshotResult {
    snapshot: Snapshot,
}

#[derive(Deserialize)]
struct WorkspaceListResult {
    workspaces: Vec<Workspace>,
}

fn json_part(text: &str) -> Result<&str, String> {
    let start = text
        .find('{')
        .ok_or_else(|| first_line_or(text, "herdr printed nothing"))?;
    Ok(&text[start..])
}

fn first_line_or(text: &str, fallback: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub fn parse_sessions(text: &str) -> Result<Vec<SessionInfo>, String> {
    let list: SessionList =
        serde_json::from_str(json_part(text)?).map_err(|e| format!("herdr's session list: {e}"))?;
    Ok(list.sessions)
}

pub fn parse_snapshot(text: &str) -> Result<Snapshot, String> {
    let env: Envelope<SnapshotResult> = serde_json::from_str(json_part(text)?)
        .map_err(|_| first_line_or(text, "herdr's snapshot couldn't be read"))?;
    Ok(env.result.snapshot)
}

pub fn parse_workspace_list(text: &str) -> Result<Vec<Workspace>, String> {
    let env: Envelope<WorkspaceListResult> = serde_json::from_str(json_part(text)?)
        .map_err(|_| first_line_or(text, "herdr's workspace list couldn't be read"))?;
    Ok(env.result.workspaces)
}

/// `herdr 0.9.1` → (0, 9, 1).
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text
        .split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = word
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

pub fn version_ok(v: (u32, u32, u32)) -> bool {
    v >= MIN_VERSION
}

pub fn version_string(v: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// The workspace label herdr shows, cut of the pinboard's pin when it's in a pane title.
pub fn clean_title(title: &str) -> String {
    title.trim_start_matches("📌").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_session_list() {
        let text = r#"{"sessions":[{"default":true,"name":"default","running":true,"session_dir":"/x","socket_path":"/x/herdr.sock"},{"default":false,"name":"web","running":false}]}"#;
        let sessions = parse_sessions(text).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].running && sessions[0].default);
        assert!(!sessions[1].running);
    }

    #[test]
    fn reads_snapshot_with_tokens() {
        let text = r#"{"id":"cli:api:snapshot","result":{"snapshot":{"agents":[{"agent":"claude","agent_status":"working","pane_id":"w7:p1","tab_id":"w7:t1","title":"📌 #66 Under way · One search bar","workspace_id":"w7"}],"protocol":22,"version":"0.9.1","workspaces":[{"active_tab_id":"w7:t1","agent_status":"working","focused":true,"label":"66-one-search-bar","number":1,"pane_count":2,"tab_count":1,"tokens":{"kerdr_stage":"build"},"workspace_id":"w7"}]},"type":"session_snapshot"}}"#;
        let snap = parse_snapshot(text).unwrap();
        assert_eq!(snap.protocol, Some(22));
        assert_eq!(
            snap.workspaces[0].tokens.get("kerdr_stage").unwrap(),
            "build"
        );
        assert_eq!(snap.agents_in("w7").count(), 1);
    }

    #[test]
    fn reports_herdr_errors_plainly() {
        let err = parse_snapshot("error: unknown command 'api'\n").unwrap_err();
        assert_eq!(err, "error: unknown command 'api'");
    }

    #[test]
    fn reads_versions() {
        assert_eq!(parse_version("herdr 0.9.1"), Some((0, 9, 1)));
        assert_eq!(parse_version("herdr v0.10.0-preview.2"), Some((0, 10, 0)));
        assert_eq!(parse_version("herdr 1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("command not found"), None);
        assert!(version_ok((0, 9, 1)) && version_ok((0, 10, 0)) && !version_ok((0, 8, 2)));
    }

    #[test]
    fn cleans_pinboard_titles() {
        assert_eq!(
            clean_title("📌 #66 Under way · One search bar"),
            "#66 Under way · One search bar"
        );
    }
}
