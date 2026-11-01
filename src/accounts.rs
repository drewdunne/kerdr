//! Optional account data supplied by the action backend. Credentials are never part of a feed.

use std::collections::BTreeMap;

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
    /// The provider and program in people's words, as the feed's `providers` name them.
    pub provider_label: Option<String>,
    pub program_label: Option<String>,
    /// Whether a sign-in of this account takes a pasted token. Missing, Kerdr works it out.
    pub paste: Option<bool>,
}

/// One provider the backend knows (the feed's `providers`): everything Kerdr needs to list, add,
/// sign in and show its accounts, so a provider added later needs no Kerdr release.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Provider {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    /// An account code's last letter: the owner's code, `_`, then this.
    #[serde(deserialize_with = "null_default")]
    pub letter: String,
    /// The kinds of account it has, the one to offer first first.
    #[serde(deserialize_with = "null_default")]
    pub kinds: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub kind_labels: BTreeMap<String, String>,
    /// The programs that can run its accounts, the usual one first.
    #[serde(deserialize_with = "null_default")]
    pub programs: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub program_labels: BTreeMap<String, String>,
    /// How a sign-in goes: `page_code` (a link, then paste the code its page shows),
    /// `device_code` (a link and a code to type on the provider's page; nothing comes back), or
    /// none at all.
    pub signin: Option<String>,
    /// The usage windows it may report, as usage keys (`five`, `week`, `w<minutes>`).
    #[serde(deserialize_with = "null_default")]
    pub windows: Vec<String>,
    /// What else an account of it needs when added (a local model's address, say): the add form
    /// asks each.
    #[serde(deserialize_with = "null_default")]
    pub fields: Vec<ProviderField>,
}

/// One more thing the add form asks for a provider's account.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ProviderField {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    #[serde(deserialize_with = "null_default")]
    pub required: bool,
}

impl Provider {
    pub fn name(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// A kind of its account in its own words.
    pub fn kind_label(&self, kind: &str) -> String {
        self.kind_labels
            .get(kind)
            .cloned()
            .unwrap_or_else(|| kind.to_string())
    }

    /// Whether its sign-in shows a code to type on its page, with nothing pasted back.
    pub fn types_code(&self) -> bool {
        self.signin.as_deref() == Some("device_code")
    }
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

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
    /// Every other key: windows of other lengths (`w<minutes>`, `w<minutes>_at`), anything newer,
    /// and a window the reading names with no figure (`"five": null`), kept so it still shows,
    /// as unknown.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde_json::Value;
        let mut map = Option::<BTreeMap<String, Value>>::deserialize(d)?.unwrap_or_default();
        // A figure given as null names a window whose figure isn't known: it stays in `other`.
        let figure = |key: &str, map: &mut BTreeMap<String, Value>| match map.remove(key) {
            Some(Value::Null) => {
                map.insert(key.into(), Value::Null);
                None
            }
            v => v.and_then(|v| v.as_f64()),
        };
        let text = |key: &str, map: &mut BTreeMap<String, Value>| {
            map.remove(key).and_then(|v| v.as_str().map(str::to_string))
        };
        Ok(Usage {
            five: figure("five", &mut map),
            five_at: text("five_at", &mut map),
            week: figure("week", &mut map),
            week_at: text("week_at", &mut map),
            seen: text("seen", &mut map),
            other: map,
        })
    }
}

/// One usage window an account reports: its feed key, its length, the figure and its reset time.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub key: String,
    pub minutes: u64,
    pub used: Option<f64>,
    pub resets: Option<String>,
}

impl Window {
    fn empty(key: &str) -> Option<Window> {
        Some(Window {
            minutes: window_minutes(key)?,
            key: key.to_string(),
            used: None,
            resets: None,
        })
    }

    /// The figure as it stands now: none once its window has reset since the reading, since
    /// what's been used since isn't known.
    pub fn current(&self, now: chrono::DateTime<chrono::Local>) -> Option<f64> {
        self.used
            .filter(|v| v.is_finite())
            .filter(|_| time(self.resets.as_deref()).is_none_or(|t| t > now))
    }

    pub fn name(&self) -> String {
        window_name(self.minutes)
    }
}

/// A usage key's length in minutes: `five` and `week`, or `w<minutes>`.
pub fn window_minutes(key: &str) -> Option<u64> {
    match key {
        "five" => Some(300),
        "week" => Some(10080),
        _ => key
            .strip_prefix('w')
            .filter(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_digit()))
            .and_then(|m| m.parse().ok())
            .filter(|&m| m > 0),
    }
}

/// A window's name for sentences: `five-hour`, `weekly`, `daily`, `90-minute`.
pub fn window_name(minutes: u64) -> String {
    match minutes {
        10080 => "weekly".into(),
        1440 => "daily".into(),
        60 => "hourly".into(),
        m if m % 10080 == 0 => format!("{}-week", m / 10080),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", number_word(m / 60)),
        m => format!("{m}-minute"),
    }
}

fn number_word(n: u64) -> String {
    match n {
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        6 => "six".into(),
        8 => "eight".into(),
        12 => "twelve".into(),
        n => n.to_string(),
    }
}

/// What a lone window's figure is a share of: `this week`, `today`, `this five-hour window`.
pub fn window_period(minutes: u64) -> String {
    match minutes {
        10080 => "this week".into(),
        1440 => "today".into(),
        60 => "this hour".into(),
        m => format!("this {} window", window_name(m)),
    }
}

fn time(value: Option<&str>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value?).ok()
}

impl Usage {
    /// The windows this reading has, shortest first: those with a figure or a reset time. A plan
    /// with only a weekly limit reports only `week`, and shows one number.
    pub fn windows(&self) -> Vec<Window> {
        let named = [
            ("five", self.five, self.five_at.clone()),
            ("week", self.week, self.week_at.clone()),
        ]
        .into_iter()
        .filter(|(key, used, resets)| {
            used.is_some() || resets.is_some() || self.other.contains_key(*key)
        })
        .map(|(key, used, resets)| (key.to_string(), used, resets));
        // Other lengths, by their figure or their reset time alone.
        let mut keys: Vec<&str> = self
            .other
            .keys()
            .map(|k| k.strip_suffix("_at").unwrap_or(k))
            .filter(|k| !matches!(*k, "five" | "week") && window_minutes(k).is_some())
            .collect();
        keys.dedup();
        let others = keys.into_iter().map(|key| {
            let resets = self.other.get(&format!("{key}_at"));
            (
                key.to_string(),
                self.other.get(key).and_then(|v| v.as_f64()),
                resets.and_then(|v| v.as_str()).map(str::to_string),
            )
        });
        let mut out: Vec<Window> = Vec::new();
        for (key, used, resets) in named.chain(others) {
            let Some(minutes) = window_minutes(&key) else {
                continue;
            };
            // Two keys for one length (`w300` beside `five`) show once, as the named one.
            if !out.iter().any(|w| w.minutes == minutes) {
                out.push(Window {
                    key,
                    minutes,
                    used,
                    resets,
                });
            }
        }
        out.sort_by_key(|w| w.minutes);
        out
    }

    /// When the reading was taken, if the feed says.
    pub fn seen_at(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        time(self.seen.as_deref())
    }

    /// The windows whose reset time has passed since this reading, with that time: what the
    /// reading counted no longer applies, and what's been used since isn't known.
    pub fn reset_since(&self, now: chrono::DateTime<chrono::Local>) -> Vec<(String, String)> {
        self.windows()
            .into_iter()
            .filter_map(|w| {
                let at = w.resets.as_deref()?;
                time(Some(at))
                    .filter(|t| *t <= now)
                    .map(|_| (window_name(w.minutes), reset_time(at)))
            })
            .collect()
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

/// How an account's figures read: bare numbers where space is short (the top bar), or with a
/// lone window saying what it's a share of (`7 this week`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Figures {
    Short,
    Worded,
}

/// The windows to show for an account: those its reading has. For a provider no feed describes
/// (an older backend), the five-hour and weekly ones always, as before. With no reading, the
/// provider's own windows, each unknown; a provider with none shows nothing.
pub fn shown_windows(usage: Option<&Usage>, provider: Option<&Provider>) -> Vec<Window> {
    let mut windows = usage.map(Usage::windows).unwrap_or_default();
    let fill: Vec<String> = match provider {
        None => legacy_windows(),
        Some(_) if !windows.is_empty() => vec![],
        Some(p) => p.windows.clone(),
    };
    for key in &fill {
        if let Some(w) =
            Window::empty(key).filter(|w| !windows.iter().any(|x| x.minutes == w.minutes))
        {
            windows.push(w);
        }
    }
    windows.sort_by_key(|w| w.minutes);
    windows
}

/// The windows a provider can have that these lack, by name: why a plan shows one number where
/// others show two.
pub fn lacking(provider: Option<&Provider>, shown: &[Window]) -> Vec<String> {
    provider
        .map(|p| p.windows.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|key| window_minutes(key))
        .filter(|m| !shown.iter().any(|w| w.minutes == *m))
        .map(window_name)
        .collect()
}

/// Each window's use, shortest first, as `five│week`: `86│100` for a plan with both, `7` for one
/// with only a weekly limit (`7 this week` worded). A figure whose window has reset since the
/// reading shows `—`, like one the feed doesn't have, rather than an old number as if current.
pub fn numbers(
    windows: &[Window],
    figures: Figures,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let shown: Vec<String> = windows
        .iter()
        .map(|w| w.current(now).map_or("—".into(), |v| format!("{v:.0}")))
        .collect();
    match (figures, windows) {
        (Figures::Worded, [one]) if shown[0] != "—" => {
            format!("{} {}", shown[0], window_period(one.minutes))
        }
        _ => shown.join("│"),
    }
}

/// The windows a feed without `providers` shows for every account: five-hour and weekly.
pub fn legacy_windows() -> Vec<String> {
    vec!["five".into(), "week".into()]
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
    /// Its short code (`KBZ`), which a new account's code starts with.
    pub code: Option<String>,
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
    /// The program running it (a provider's `programs`); missing on older feeds.
    #[serde(deserialize_with = "null_default")]
    pub program: String,
    /// The agent it took over from, when it was handed over from another program.
    pub handed_from: Option<HandedFrom>,
}

/// Where a handed-over agent's work came from: the account and program before, the conversation
/// left there, why (`usage_limit`, `sign_in`, `manual`, …) and when.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct HandedFrom {
    #[serde(deserialize_with = "null_default")]
    pub account: String,
    #[serde(deserialize_with = "null_default")]
    pub program: String,
    #[serde(deserialize_with = "null_default")]
    pub conversation: String,
    #[serde(deserialize_with = "null_default")]
    pub why: String,
    pub at: Option<String>,
}

impl HandedFrom {
    /// Why it was handed over, in words, given the account it came from.
    pub fn reason(&self) -> String {
        let account = &self.account;
        match self.why.as_str() {
            "usage_limit" => format!("{account} ran out"),
            "sign_in" => format!("{account}'s sign-in was refused"),
            "throttled" => format!("{account} was held back by its provider"),
            "manual" => "moved by hand".into(),
            "rule" | "" => "its list moved it".into(),
            other => other.replace('_', " "),
        }
    }
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
    pub(crate) fn first_copy(&self, c: &AccountCopy) -> bool {
        self.copies.first().is_some_and(|f| f.id == c.id)
    }

    /// Whether a copy needs no sign-in: tested working, or the copy launches use while the
    /// account is ready and no test says otherwise.
    pub fn copy_ready(&self, c: &AccountCopy) -> bool {
        c.ready() || (self.first_copy(c) && self.ready() && c.stored && c.works != Some(false))
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

pub fn reset_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| value.to_string())
}

/// A moment in sentences: its time today (`18:08`), else its day and time (`Fri 18:08`).
pub fn moment(value: &str, now: chrono::DateTime<chrono::Local>) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| {
            let t = t.with_timezone(&chrono::Local);
            t.format(if t.date_naive() == now.date_naive() {
                "%H:%M"
            } else {
                "%a %H:%M"
            })
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
            ..Default::default()
        };
        // A provider no feed describes (None) shows five-hour and weekly, as before.
        let short = |u: Option<&Usage>| numbers(&shown_windows(u, None), Figures::Short, now);
        assert_eq!(short(Some(&u)), "—│19");
        assert_eq!(
            u.reset_since(now),
            vec![("five-hour".to_string(), reset_time(&at(17)))]
        );
        let reset = Usage {
            week_at: Some(at(18)),
            ..u.clone()
        };
        assert_eq!(short(Some(&reset)), "—│—");
        assert_eq!(reset.reset_since(now).len(), 2);
        let no_reset_time = Usage {
            five_at: None,
            week_at: Some("not a time".into()),
            ..u
        };
        assert_eq!(short(Some(&no_reset_time)), "40│19");
        assert!(no_reset_time.reset_since(now).is_empty());
        assert_eq!(short(None), "—│—");
    }

    fn reading_of(json: &str) -> Usage {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn an_account_shows_the_windows_it_has() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 31, 18, 0, 0)
            .single()
            .unwrap();
        let described = Provider {
            id: "p".into(),
            windows: legacy_windows(),
            ..Provider::default()
        };
        let p = Some(&described);
        let fig = |u: &Usage, f| numbers(&shown_windows(Some(u), p), f, now);
        // A plan with only a weekly limit reports only `week`: one number, worded where there's room.
        let weekly = reading_of(r#"{"week":7,"week_at":"2026-11-04T01:48:00-05:00"}"#);
        assert_eq!(fig(&weekly, Figures::Short), "7");
        assert_eq!(fig(&weekly, Figures::Worded), "7 this week");
        assert_eq!(lacking(p, &shown_windows(Some(&weekly), p)), ["five-hour"]);
        // Both windows read as before, whichever way.
        assert_eq!(
            fig(&reading_of(r#"{"five":86,"week":100}"#), Figures::Worded),
            "86│100"
        );
        // A window named with no figure yet is there, unknown; so is one known by its reset alone.
        assert_eq!(
            fig(&reading_of(r#"{"five":null,"week":7}"#), Figures::Short),
            "—│7"
        );
        assert_eq!(fig(&reading_of(r#"{"week":null}"#), Figures::Short), "—");
        assert_eq!(
            fig(
                &reading_of(r#"{"w60_at":"2026-10-31T19:00:00-05:00","week":40}"#),
                Figures::Short
            ),
            "—│40"
        );
        // Another length comes in order of length; two keys for one length show once.
        let hourly = reading_of(r#"{"w60":3,"w60_at":"2026-10-31T19:00:00-05:00","week":40}"#);
        assert_eq!(
            hourly
                .windows()
                .iter()
                .map(|w| w.minutes)
                .collect::<Vec<_>>(),
            [60, 10080]
        );
        assert_eq!(fig(&hourly, Figures::Short), "3│40");
        assert_eq!(
            fig(
                &reading_of(r#"{"five":10,"w300":99,"week":5}"#),
                Figures::Short
            ),
            "10│5"
        );
        assert_eq!(
            fig(&reading_of(r#"{"w60":3}"#), Figures::Worded),
            "3 this hour"
        );
        // Something that isn't a window is kept but never shown as one.
        assert_eq!(
            reading_of(r#"{"week":5,"wobble":9,"w":1,"w0":2}"#)
                .windows()
                .len(),
            1
        );
        // No reading: the provider's windows, unknown; a provider with none shows nothing.
        assert_eq!(
            numbers(&shown_windows(None, p), Figures::Worded, now),
            "—│—"
        );
        let none = Provider::default();
        assert_eq!(
            numbers(&shown_windows(None, Some(&none)), Figures::Short, now),
            ""
        );
        // A provider no feed describes keeps both windows, though the reading lacks one.
        assert_eq!(
            numbers(&shown_windows(Some(&weekly), None), Figures::Worded, now),
            "—│7"
        );
        assert!(lacking(None, &shown_windows(Some(&weekly), None)).is_empty());
        // A lone window that has reset is unknown, with no words after the dash; Window::current
        // says the same, for the detail panel.
        let gone = reading_of(r#"{"week":7,"week_at":"2026-10-30T01:48:00-05:00"}"#);
        assert_eq!(fig(&gone, Figures::Worded), "—");
        assert_eq!(gone.windows()[0].current(now), None);
        assert_eq!(gone.reset_since(now)[0].0, "weekly");
        // Read back, a null figure is still a window.
        let back: Usage = serde_json::from_value(
            serde_json::to_value(reading_of(r#"{"five":null,"week":7}"#)).unwrap(),
        )
        .unwrap();
        assert_eq!(back.windows().len(), 2);
    }

    #[test]
    fn windows_name_themselves_for_sentences() {
        assert_eq!(window_name(300), "five-hour");
        assert_eq!(window_name(10080), "weekly");
        assert_eq!(window_name(1440), "daily");
        assert_eq!(window_name(180), "three-hour");
        assert_eq!(window_name(90), "90-minute");
        assert_eq!(window_name(20160), "2-week");
        assert_eq!(window_period(10080), "this week");
        assert_eq!(window_period(300), "this five-hour window");
        assert_eq!(window_minutes("w45"), Some(45));
        assert_eq!(window_minutes("w"), None);
        assert_eq!(window_minutes("wx"), None);
    }

    #[test]
    fn a_hand_over_says_why_in_words() {
        let mut h = HandedFrom {
            account: "KBZ_C".into(),
            why: "usage_limit".into(),
            ..Default::default()
        };
        assert_eq!(h.reason(), "KBZ_C ran out");
        h.why = "manual".into();
        assert_eq!(h.reason(), "moved by hand");
        h.why = "sign_in".into();
        assert_eq!(h.reason(), "KBZ_C's sign-in was refused");
        h.why = "something_new".into();
        assert_eq!(h.reason(), "something new");
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
