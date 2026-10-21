//! Kerdr's two keys inside herdr, and the herdr config Kerdr's own herdr windows use.
//!
//! The keys are herdr custom commands, so they live in herdr's config on each machine and show
//! in herdr's keybinds menu (prefix+?) under "custom". Each leaves a short-lived token on the
//! workspace you're in; Kerdr watches for it, closes its herdr window and does what you asked.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use crate::config::{cache_dir, config_dir};
use crate::herdr::JUMP_TOKEN;

pub const MARKER: &str = "kerdr_jump";

pub const SNIPPET: &str = r#"
# Kerdr (https://github.com/drewdunne/kerdr): two keys that step out of herdr to Kerdr's board.
# herdr lists them in its keybinds menu (prefix+?) under custom. Outside Kerdr they do nothing.
[[keys.command]]
key = "prefix+space"
type = "shell"
command = "\"$HERDR_BIN_PATH\" workspace report-metadata \"$HERDR_ACTIVE_WORKSPACE_ID\" --source kerdr --token kerdr_jump=next:$(date +%s) --ttl-ms 15000"
description = "kerdr · next card that needs you"

[[keys.command]]
key = "prefix+backspace"
type = "shell"
command = "\"$HERDR_BIN_PATH\" workspace report-metadata \"$HERDR_ACTIVE_WORKSPACE_ID\" --source kerdr --token kerdr_jump=board:$(date +%s) --ttl-ms 15000"
description = "kerdr · back to the board"
"#;

/// herdr's own config file on this machine.
pub fn herdr_config_path() -> PathBuf {
    std::env::var_os("HERDR_CONFIG_PATH")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty() && !p.starts_with(cache_dir()))
        .unwrap_or_else(|| config_dir().join("herdr").join("config.toml"))
}

pub fn has_keys(text: &str) -> bool {
    text.contains(MARKER)
}

/// Adds Kerdr's keys to a herdr config, keeping everything else as it was. Returns whether it
/// changed anything.
pub fn install(path: &Path) -> Result<bool, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if has_keys(&text) {
        return Ok(false);
    }
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(SNIPPET);
    out.parse::<DocumentMut>().map_err(|e| {
        format!(
            "{} wouldn't be valid TOML with Kerdr's keys added: {e}",
            path.display()
        )
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// The config Kerdr's herdr windows use: yours, with the sidebar hidden so the workspace fills
/// the screen (prefix+b still shows it) and herdr's welcome screen off. Kerdr's two keys aren't
/// added here: herdr takes a key's command from the server's own config, so they work only once
/// `kerdr herdr-keys --install` has put them there. Settings changed inside a Kerdr window last
/// until Kerdr opens the next one.
pub fn client_config_text(user: &str) -> String {
    let mut doc: DocumentMut = user.parse().unwrap_or_default();
    if !doc.contains_key("ui") || !doc["ui"].is_table() {
        doc["ui"] = Item::Table(Table::new());
    }
    doc["ui"]["sidebar_start_collapsed"] = value(true);
    doc["ui"]["sidebar_collapsed_mode"] = value("hidden");
    if !doc.contains_key("onboarding") {
        doc["onboarding"] = value(false);
    }
    doc.to_string()
}

/// herdr's prefix as its config sets it, for the hints Kerdr shows: `ctrl+b` by default.
pub fn prefix() -> String {
    let text = std::fs::read_to_string(herdr_config_path()).unwrap_or_default();
    prefix_in(&text)
}

pub fn prefix_in(text: &str) -> String {
    text.parse::<DocumentMut>()
        .ok()
        .and_then(|doc| doc.get("keys")?.get("prefix")?.as_str().map(str::to_string))
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "ctrl+b".to_string())
}

pub fn write_client_config() -> Result<PathBuf, String> {
    let user = std::fs::read_to_string(herdr_config_path()).unwrap_or_default();
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("herdr-client.toml");
    std::fs::write(&path, client_config_text(&user))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// What a `kerdr_jump` token asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Board,
    Next,
}

pub fn parse_request(value: &str) -> Option<Request> {
    match value.split(':').next()?.trim() {
        "next" => Some(Request::Next),
        "board" => Some(Request::Board),
        _ => None,
    }
}

pub const TOKEN: &str = JUMP_TOKEN;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_the_sidebar_and_keeps_your_settings() {
        let user = "onboarding = false\n[keys]\nprefix = \"ctrl+space\"\n[ui]\nagent_panel_sort = \"priority\"\n";
        let text = client_config_text(user);
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["keys"]["prefix"].as_str(), Some("ctrl+space"));
        assert_eq!(doc["ui"]["agent_panel_sort"].as_str(), Some("priority"));
        assert_eq!(doc["ui"]["sidebar_start_collapsed"].as_bool(), Some(true));
        assert_eq!(doc["ui"]["sidebar_collapsed_mode"].as_str(), Some("hidden"));
        assert_eq!(doc["onboarding"].as_bool(), Some(false));
        assert!(
            !has_keys(&text),
            "the keys belong in the server's config, not the window's"
        );
    }

    #[test]
    fn reads_the_prefix() {
        assert_eq!(prefix_in("[keys]\nprefix = \"ctrl+space\"\n"), "ctrl+space");
        assert_eq!(prefix_in("[ui]\n"), "ctrl+b");
        assert_eq!(prefix_in("not toml ["), "ctrl+b");
    }

    #[test]
    fn installs_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[keys]\nprefix = \"ctrl+space\"\n").unwrap();
        assert_eq!(install(&path), Ok(true));
        assert_eq!(install(&path), Ok(false));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("kerdr_jump=next").count(), 1);
        assert!(text.parse::<DocumentMut>().is_ok());
    }

    #[test]
    fn reads_requests() {
        assert_eq!(parse_request("next:1792583052"), Some(Request::Next));
        assert_eq!(parse_request("board"), Some(Request::Board));
        assert_eq!(parse_request("dance"), None);
    }
}
