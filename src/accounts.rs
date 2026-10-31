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
    #[serde(deserialize_with = "null_default")]
    pub copies: Vec<AccountCopy>,
}

/// One stored sign-in of an account on this machine, such as your agents' or the tenants'.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct AccountCopy {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    #[serde(deserialize_with = "null_default")]
    pub stored: bool,
    pub works: Option<bool>,
    pub checked: Option<String>,
    pub note: Option<String>,
    /// Whether this copy takes a pasted token. Missing, it follows the account's kind.
    pub paste: Option<bool>,
}

impl AccountCopy {
    /// Stored and tested working: signing it in again would only replace a working sign-in.
    pub fn ready(&self) -> bool {
        self.stored && self.works == Some(true)
    }

    pub fn name(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    pub fn stored_label(&self) -> &'static str {
        if self.stored {
            "✓ stored"
        } else {
            "✕ not stored"
        }
    }

    pub fn works_label(&self) -> String {
        let when = self
            .checked
            .as_deref()
            .map(|t| format!(" · {}", reset_time(t)))
            .unwrap_or_default();
        match self.works {
            Some(true) => format!("✓ works{when}"),
            Some(false) => format!("✕ doesn't work{when}"),
            None => "– not checked".into(),
        }
    }
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

fn time(value: Option<&str>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value?).ok()
}

impl Usage {
    /// When the reading was taken, if the feed says.
    pub fn seen_at(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        time(self.seen.as_deref())
    }

    /// The windows whose reset time has passed since this reading, with that time: what the
    /// reading counted no longer applies, and what's been used since isn't known.
    pub fn reset_since(&self, now: chrono::DateTime<chrono::Local>) -> Vec<(&'static str, String)> {
        [
            ("five-hour", self.five_at.as_deref()),
            ("weekly", self.week_at.as_deref()),
        ]
        .into_iter()
        .filter_map(|(window, at)| {
            time(at)
                .filter(|t| *t <= now)
                .map(|_| (window, reset_time(at.unwrap_or_default())))
        })
        .collect()
    }

    fn current(
        figure: Option<f64>,
        resets: Option<&str>,
        now: chrono::DateTime<chrono::Local>,
    ) -> Option<f64> {
        figure.filter(|_| time(resets).is_none_or(|t| t > now))
    }
}

/// The reading to show for an account, given each machine's: the one with the newest `seen`.
/// Usage belongs to the account, not to a machine, and each machine's feed only has what its own
/// sessions last saw, so a machine that hasn't run the account lately holds figures hours old.
/// Readings without `seen` count as oldest; on a tie the first one given wins.
pub fn freshest<'a>(readings: impl IntoIterator<Item = &'a Usage>) -> Option<&'a Usage> {
    readings.into_iter().fold(None, |best, u| match best {
        Some(b) if b.seen_at() >= u.seen_at() => Some(b),
        _ => Some(u),
    })
}

/// Five-hour and weekly use as `five│week`. A figure whose window has reset since the reading
/// shows `—`, like one the feed doesn't have, rather than an old number as if it were current.
pub fn numbers(usage: Option<&Usage>, now: chrono::DateTime<chrono::Local>) -> String {
    let number = |n: Option<f64>| {
        n.filter(|v| v.is_finite())
            .map_or("—".into(), |v| format!("{v:.0}"))
    };
    format!(
        "{}│{}",
        number(usage.and_then(|u| Usage::current(u.five, u.five_at.as_deref(), now))),
        number(usage.and_then(|u| Usage::current(u.week, u.week_at.as_deref(), now)))
    )
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
    /// Copies get lines of their own only when there are two or more; one is today's display.
    pub fn shown_copies(&self) -> &[AccountCopy] {
        if self.copies.len() >= 2 {
            &self.copies
        } else {
            &[]
        }
    }

    pub fn copy(&self, id: &str) -> Option<&AccountCopy> {
        self.shown_copies().iter().find(|c| c.id == id)
    }

    pub fn ready(&self) -> bool {
        self.status == "ready"
    }

    /// Signed out, or never signed in, on this machine.
    pub fn needs_signin(&self) -> bool {
        matches!(self.status.as_str(), "sign_in" | "missing")
    }

    /// The first copy is the one launches use, so the account's own status is that copy's.
    fn first_copy(&self, c: &AccountCopy) -> bool {
        self.copies.first().is_some_and(|f| f.id == c.id)
    }

    /// Whether a copy needs no sign-in: tested working, or the copy launches use while the
    /// account is ready and no test says otherwise.
    pub fn copy_ready(&self, c: &AccountCopy) -> bool {
        c.ready() || (self.first_copy(c) && self.ready() && c.stored && c.works != Some(false))
    }

    /// Whether a sign-in of this account (or of one copy) can take a pasted token instead of the
    /// browser: a token account's can, a saved sign-in's can't, and a copy may say otherwise. A
    /// feed from before `paste` leaves any other copy without one, as root's takes none.
    pub fn takes_paste(&self, copy: Option<&AccountCopy>) -> bool {
        match copy {
            Some(c) => c
                .paste
                .unwrap_or(self.kind == "token" && self.first_copy(c)),
            None => self.kind == "token",
        }
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
    use chrono::TimeZone;

    fn reading(seen: Option<&str>, five: f64) -> Usage {
        Usage {
            five: Some(five),
            seen: seen.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn freshest_reading_is_the_newest_seen_and_ties_keep_the_first() {
        let old = reading(Some("2026-10-26T11:44:03-05:00"), 0.0);
        let new = reading(Some("2026-10-26T18:53:11-05:00"), 65.0);
        // The same moment written in another zone is a tie, not a newer reading.
        let same = reading(Some("2026-10-26T23:53:11Z"), 1.0);
        let unknown = reading(None, 9.0);
        assert_eq!(freshest([&old, &new]), Some(&new));
        assert_eq!(freshest([&new, &old]), Some(&new));
        assert_eq!(freshest([&new, &same]), Some(&new));
        assert_eq!(freshest([&unknown, &old]), Some(&old));
        assert_eq!(freshest([&unknown]), Some(&unknown));
        assert_eq!(freshest(std::iter::empty::<&Usage>()), None);
    }

    #[test]
    fn a_figure_whose_window_has_reset_shows_as_unknown() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 26, 18, 0, 0)
            .single()
            .unwrap();
        let at = |h| {
            chrono::Local
                .with_ymd_and_hms(2026, 10, 26, h, 0, 0)
                .single()
                .unwrap()
                .to_rfc3339()
        };
        let u = Usage {
            five: Some(40.0),
            five_at: Some(at(17)),
            week: Some(19.0),
            week_at: Some(at(19)),
            seen: Some(at(12)),
        };
        assert_eq!(numbers(Some(&u), now), "—│19");
        assert_eq!(u.reset_since(now), vec![("five-hour", reset_time(&at(17)))]);
        let both = Usage {
            week_at: Some(at(18)),
            ..u.clone()
        };
        assert_eq!(numbers(Some(&both), now), "—│—");
        assert_eq!(both.reset_since(now).len(), 2);
        let no_reset_time = Usage {
            five_at: None,
            week_at: Some("not a time".into()),
            ..u
        };
        assert_eq!(numbers(Some(&no_reset_time), now), "40│19");
        assert!(no_reset_time.reset_since(now).is_empty());
        assert_eq!(numbers(None, now), "—│—");
    }

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
