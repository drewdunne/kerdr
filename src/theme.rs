//! Colours (Tokyo Night, like the design) and the small text helpers every screen uses.

use chrono::{DateTime, Datelike, Local};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::Attention;

pub const BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
pub const PANEL: Color = Color::Rgb(0x1f, 0x23, 0x35);
pub const RAISED: Color = Color::Rgb(0x24, 0x28, 0x3b);
pub const SEL: Color = Color::Rgb(0x29, 0x2e, 0x42);
pub const BORDER: Color = Color::Rgb(0x3b, 0x42, 0x61);
pub const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
pub const FG2: Color = Color::Rgb(0xa9, 0xb1, 0xd6);
pub const DIM: Color = Color::Rgb(0x8f, 0x97, 0xbf);
pub const FAINT: Color = Color::Rgb(0x56, 0x5f, 0x89);
pub const CYAN: Color = Color::Rgb(0x7d, 0xcf, 0xff);
pub const BLUE: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
pub const ORANGE: Color = Color::Rgb(0xff, 0x9e, 0x64);
pub const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);
pub const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
pub const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
pub const PURPLE: Color = Color::Rgb(0xbb, 0x9a, 0xf7);

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

pub fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

pub fn attention_color(a: Attention) -> Color {
    match a {
        Attention::NeedsYou => ORANGE,
        Attention::Blocked => RED,
        Attention::Running | Attention::Waiting => BLUE,
        Attention::Idle | Attention::Unreachable => DIM,
        Attention::Done => GREEN,
    }
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cuts text to a display width, ending in … when it had to cut.
pub fn trunc(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Word-wraps text to a width, keeping at most `max_lines` (the last one cut with …).
pub fn wrap(s: &str, max_width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if width(&candidate) <= max_width {
            current = candidate;
        } else {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            current = if width(word) > max_width {
                trunc(word, max_width)
            } else {
                word.to_string()
            };
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines && max_lines > 0 {
        let rest = lines[max_lines - 1..].join(" ");
        lines.truncate(max_lines - 1);
        lines.push(trunc(&rest, max_width));
    }
    lines
}

/// "35m", "1h34m", "3d": how long ago, short.
pub fn ago(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (now - t).num_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => {
            let (h, m) = (s / 3600, (s % 3600) / 60);
            if m == 0 || h >= 10 {
                format!("{h}h")
            } else {
                format!("{h}h{m}m")
            }
        }
        s => format!("{}d", s / 86_400),
    }
}

pub fn clock(t: DateTime<Local>) -> String {
    t.format("%H:%M").to_string()
}

/// "today", a weekday within the week, else month-day.
pub fn day(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => "today".into(),
        1..=6 => t.weekday().to_string(),
        _ => format!("{:02}-{:02}", t.month(), t.day()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cuts_and_wraps() {
        assert_eq!(trunc("Find the next flywheel", 10), "Find the …");
        assert_eq!(trunc("short", 10), "short");
        assert_eq!(
            wrap("Knowledge browser web app for every bundle", 18, 2),
            ["Knowledge browser", "web app for every…"]
        );
        assert_eq!(wrap("one", 10, 2), ["one"]);
    }

    #[test]
    fn says_how_long_ago() {
        let now = Local.with_ymd_and_hms(2026, 10, 21, 18, 51, 0).unwrap();
        assert_eq!(ago(now - chrono::Duration::minutes(35), now), "35m");
        assert_eq!(ago(now - chrono::Duration::minutes(94), now), "1h34m");
        assert_eq!(ago(now - chrono::Duration::days(3), now), "3d");
        assert_eq!(day(now, now), "today");
    }
}
