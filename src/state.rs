//! What Kerdr remembers between runs: only the machines you hid with x.

use serde::{Deserialize, Serialize};

use crate::config::state_dir;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_dir().join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = state_dir();
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = serde_json::to_string_pretty(self)
        {
            let _ = std::fs::write(dir.join("state.json"), text);
        }
    }
}
