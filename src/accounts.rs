//! Optional account data supplied by the action backend. Credentials are never part of a feed.

use serde::{Deserialize, Serialize};

pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Account {
    #[serde(deserialize_with = "null_default")]
    pub code: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub provider: String,
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub runs: String,
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    pub until: Option<String>,
    pub pool: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub five: Option<f64>,
    pub five_at: Option<String>,
    pub week: Option<f64>,
    pub week_at: Option<String>,
    pub seen: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Owner {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub parent: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub models: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub own: bool,
    #[serde(deserialize_with = "null_default")]
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct AccountAgent {
    #[serde(deserialize_with = "null_default")]
    pub pane: String,
    #[serde(deserialize_with = "null_default")]
    pub session: String,
    #[serde(deserialize_with = "null_default")]
    pub conversation: String,
    #[serde(deserialize_with = "null_default")]
    pub account: String,
    #[serde(deserialize_with = "null_default")]
    pub how: String,
    pub waiting: Option<Waiting>,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Waiting {
    #[serde(deserialize_with = "null_default")]
    pub why: String,
    pub until: Option<String>,
}

impl AccountAgent {
    pub fn waiting_for_usage(&self) -> bool {
        self.waiting.as_ref().is_some_and(|w| w.why == "usage")
    }
}

impl Account {
    pub fn numbers(&self) -> String {
        let number = |n: Option<f64>| {
            n.filter(|v| v.is_finite())
                .map_or("—".into(), |v| format!("{v:.0}"))
        };
        format!(
            "{}│{}",
            number(self.usage.as_ref().and_then(|u| u.five)),
            number(self.usage.as_ref().and_then(|u| u.week))
        )
    }

    pub fn status_label(&self) -> String {
        match self.status.as_str() {
            "sign_in" => "✕ sign in".into(),
            "missing" => "✕ not signed in".into(),
            "out" => format!(
                "{} until {}{}",
                pause_glyph(),
                self.until
                    .as_deref()
                    .map(reset_time)
                    .unwrap_or_else(|| "reset".into()),
                self.pool
                    .as_ref()
                    .map(|s| format!(", {s}"))
                    .unwrap_or_default()
            ),
            "ready" => "ready".into(),
            _ => "unknown".into(),
        }
    }
}

pub fn provider_name(provider: &str) -> &str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "local" => "Local",
        other => other,
    }
}

pub fn reset_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

/// Text presentation, with a conservative fallback for terminals that use emoji presentation.
pub fn pause_glyph() -> &'static str {
    if cfg!(windows) || std::env::var("KERDR_PAUSE_GLYPH").is_ok_and(|v| v == "x") {
        "✕"
    } else {
        "⏸\u{fe0e}"
    }
}

/// Only web URLs can reach an OS opener. No shell, file, or custom-protocol actions.
pub fn valid_signin_url(url: &str) -> bool {
    url.len() <= 8192
        && url.starts_with("https://")
        && url[8..]
            .split('/')
            .next()
            .is_some_and(|host| !host.is_empty() && !host.contains('@'))
        && !url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '"' | '<' | '>'))
}

pub fn open_signin_url(url: &str) -> std::io::Result<()> {
    if !valid_signin_url(url) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a valid HTTPS sign-in link",
        ));
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
        // No command interpreter: URL data goes directly to the system URL handler.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if (result as isize) <= 32 {
            return Err(std::io::Error::other(
                "the system URL handler refused the link",
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::process::{Command, Stdio};
        let mut child = Command::new(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        })
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opener_rejects_commands_and_non_web_protocols() {
        for bad in [
            "file:///tmp/a",
            "https://",
            "https://user@example.com",
            "https://example.com/\ncommand",
            "https://example.com/\"bad",
            "javascript:alert(1)",
        ] {
            assert!(!valid_signin_url(bad), "{bad}");
        }
        assert!(valid_signin_url(
            "https://example.com/authorize?scope=a%20b&state=c"
        ));
    }
    #[test]
    fn pause_is_text_or_the_one_cell_fallback() {
        assert!(matches!(pause_glyph(), "⏸\u{fe0e}" | "✕"));
        if cfg!(windows) {
            assert_eq!(pause_glyph(), "✕");
        }
    }
}
