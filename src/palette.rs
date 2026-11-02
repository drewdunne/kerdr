//! Kerdr's colours from your desktop theme.
//!
//! Every screen draws with the sixteen colours named in `theme` (BG, FG, RED, …), which are Tokyo
//! Night, as the design was. When a theme file is found, the finished frame is recoloured on its
//! way out: each cell coloured with one of those sixteen gets the theme's colour for the same job
//! instead. Nothing else in Kerdr sets a colour, so the swap is complete, and the screens keep
//! naming colours by what they're for.
//!
//! The theme file is an Omarchy `colors.toml` (`background = "#1a1b26"`, `red = "#f7768e"`, …, or
//! the older `color0` … `color15`). By default Kerdr reads Omarchy's current theme, and follows it
//! while open when you change theme; `[theme] colors` in Kerdr's config names another file, or
//! `""` keeps Tokyo Night.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::style::Color;

use crate::theme;

/// The sixteen colours Kerdr draws with, by the job each does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Color,
    pub panel: Color,
    pub raised: Color,
    pub sel: Color,
    pub border: Color,
    pub fg: Color,
    pub fg2: Color,
    pub dim: Color,
    pub faint: Color,
    pub cyan: Color,
    pub blue: Color,
    pub orange: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub purple: Color,
}

impl Palette {
    /// The colours the screens are drawn with: Tokyo Night.
    pub const fn tokyo_night() -> Palette {
        Palette {
            bg: theme::BG,
            panel: theme::PANEL,
            raised: theme::RAISED,
            sel: theme::SEL,
            border: theme::BORDER,
            fg: theme::FG,
            fg2: theme::FG2,
            dim: theme::DIM,
            faint: theme::FAINT,
            cyan: theme::CYAN,
            blue: theme::BLUE,
            orange: theme::ORANGE,
            red: theme::RED,
            green: theme::GREEN,
            yellow: theme::YELLOW,
            purple: theme::PURPLE,
        }
    }

    fn pairs(&self) -> [Color; 16] {
        [
            self.bg,
            self.panel,
            self.raised,
            self.sel,
            self.border,
            self.fg,
            self.fg2,
            self.dim,
            self.faint,
            self.cyan,
            self.blue,
            self.orange,
            self.red,
            self.green,
            self.yellow,
            self.purple,
        ]
    }

    /// A palette from an Omarchy `colors.toml`. None when it has no background and foreground.
    ///
    /// Names are looked up the way Omarchy's own `omarchy-theme-color` does, so a theme that only
    /// has `color0` … `color15` works too. Text colours too close to the background to read are
    /// moved toward the foreground until they can be.
    pub fn from_colors_toml(text: &str) -> Option<Palette> {
        let raw = parse(text);
        let get = |keys: &[&str]| keys.iter().find_map(|k| raw.get(*k).copied());
        let bg = get(&["background", "bg", "color0"])?;
        let fg2 = get(&["foreground", "fg", "color7"])?;
        let fg = get(&["bright_foreground", "bright_fg", "color15"]).unwrap_or(fg2);
        let faint = get(&["dark_foreground", "dark_fg", "color8"]).unwrap_or(mix(bg, fg2, 0.36));
        let lighter = get(&["lighter_background", "lighter_bg"]).filter(|c| *c != bg);
        let raised = lighter.unwrap_or(mix(bg, fg2, 0.06));
        let sel =
            get(&["selection", "selection_background", "color8"]).unwrap_or(mix(bg, fg2, 0.1));
        let border = get(&["muted", "color8"]).unwrap_or(mix(bg, fg2, 0.2));
        let red = get(&["red", "color1"]).unwrap_or(theme::RED);
        let green = get(&["green", "color2"]).unwrap_or(theme::GREEN);
        let yellow = get(&["yellow", "color3"]).unwrap_or(theme::YELLOW);
        let blue = get(&["blue", "color4"]).unwrap_or(theme::BLUE);
        let purple = get(&["magenta", "purple", "color5"]).unwrap_or(theme::PURPLE);
        let cyan = get(&["cyan", "color6"]).unwrap_or(theme::CYAN);
        let orange = get(&["orange"]).unwrap_or(yellow);

        let readable = |c: Color, ratio: f64| legible(c, bg, fg, ratio);
        Some(Palette {
            bg,
            panel: mix(bg, raised, 0.5),
            raised,
            sel,
            border,
            fg: readable(fg, 4.5),
            fg2: readable(fg2, 4.5),
            dim: readable(mix(fg2, faint, 0.3), 3.0),
            faint: readable(faint, 2.0),
            cyan: readable(cyan, 3.0),
            blue: readable(blue, 3.0),
            orange: readable(orange, 3.0),
            red: readable(red, 3.0),
            green: readable(green, 3.0),
            yellow: readable(yellow, 3.0),
            purple: readable(purple, 3.0),
        })
    }

    /// The palette in a file, or None when it can't be read or has no colours.
    pub fn from_file(path: &Path) -> Option<Palette> {
        Palette::from_colors_toml(&std::fs::read_to_string(path).ok()?)
    }

    /// Gives each cell drawn in a Tokyo Night colour this palette's colour for the same job.
    pub fn recolor(&self, buf: &mut Buffer) {
        if *self == Palette::tokyo_night() {
            return;
        }
        let from = Palette::tokyo_night().pairs();
        let to = self.pairs();
        let swap = |c: Color| from.iter().position(|f| *f == c).map_or(c, |i| to[i]);
        for cell in buf.content.iter_mut() {
            cell.fg = swap(cell.fg);
            cell.bg = swap(cell.bg);
            cell.underline_color = swap(cell.underline_color);
        }
    }
}

/// `key = "#rrggbb"` lines; anything else (comments, sections, other values) is skipped.
fn parse(text: &str) -> HashMap<String, Color> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches(|c| c == '"' || c == '\'');
        let value = value.trim();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split_whitespace().next().unwrap_or(""),
        };
        if let Some(c) = hex(value) {
            out.insert(key.to_string(), c);
        }
    }
    out
}

/// "#rrggbb" (or "#rrggbbaa", alpha ignored) as a colour.
fn hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#')?;
    if !(s.len() == 6 || s.len() == 8) || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// `a` moved `amount` (0 to 1) of the way to `b`.
fn mix(a: Color, b: Color, amount: f64) -> Color {
    let (a, b) = (rgb(a), rgb(b));
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * amount).round() as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn luminance(c: Color) -> f64 {
    let (r, g, b) = rgb(c);
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// How far apart two colours are to the eye, from 1 (the same) to 21 (black on white).
fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `c`, or `c` moved toward the foreground until it stands `ratio` apart from the background
/// (as far as the foreground itself does, at most).
fn legible(c: Color, bg: Color, fg: Color, ratio: f64) -> Color {
    let target = ratio.min(contrast(fg, bg));
    if contrast(c, bg) >= target {
        return c;
    }
    let toward = if contrast(fg, bg) >= target {
        fg
    } else if luminance(bg) < 0.5 {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(0, 0, 0)
    };
    (1..=20)
        .map(|step| mix(c, toward, step as f64 / 20.0))
        .find(|m| contrast(*m, bg) >= target)
        .unwrap_or(toward)
}

/// Omarchy's current theme's colours, where this machine has Omarchy.
pub fn omarchy_colors() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::config::home().join(".local/state"));
    [
        state.join("omarchy/current/theme/colors.toml"),
        // Omarchy before its state folder kept the current theme with its config.
        crate::config::home().join(".config/omarchy/current/theme/colors.toml"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The palette screens are drawn in; None keeps Tokyo Night. Set only by the running board.
static ACTIVE: RwLock<Option<Palette>> = RwLock::new(None);

/// Recolours a finished frame with the active palette, if there is one.
pub fn recolor_active(buf: &mut Buffer) {
    if let Ok(guard) = ACTIVE.read()
        && let Some(p) = guard.as_ref()
    {
        p.recolor(buf);
    }
}

fn set_active(p: Option<Palette>) {
    if let Ok(mut guard) = ACTIVE.write() {
        *guard = p;
    }
}

/// Keeps the active palette matched to a theme file, reading it again when it changes.
pub struct ThemeWatch {
    /// The file named in the config; None means look for Omarchy's each time.
    named: Option<PathBuf>,
    off: bool,
    seen: Option<(PathBuf, Option<SystemTime>, u64)>,
    checked: Option<Instant>,
}

const EVERY: Duration = Duration::from_secs(1);

impl ThemeWatch {
    /// `setting` is `[theme] colors` from the config: None looks for Omarchy's theme, "" is off.
    pub fn new(setting: Option<&str>) -> ThemeWatch {
        let off = setting.is_some_and(|s| s.trim().is_empty());
        let named = setting
            .filter(|s| !s.trim().is_empty())
            .map(|s| PathBuf::from(crate::jump::expand_home(s.trim())));
        ThemeWatch {
            named,
            off,
            seen: None,
            checked: None,
        }
    }

    fn path(&self) -> Option<PathBuf> {
        if self.off {
            return None;
        }
        match &self.named {
            Some(p) => Some(p.clone()),
            None => omarchy_colors(),
        }
    }

    /// Reads the theme again when its file changed; at most once a second unless `now`.
    pub fn check(&mut self, now: bool) {
        if !now && self.checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let found = self.path().and_then(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            Some((p, meta.modified().ok(), meta.len()))
        });
        if found == self.seen {
            return;
        }
        // A theme being swapped in can be half-written for a moment; keep the last good palette
        // until a full one reads, but drop it when the file is gone for good.
        match &found {
            Some((p, _, _)) => match Palette::from_file(p) {
                Some(palette) => set_active(Some(palette)),
                None => return,
            },
            None => set_active(None),
        }
        self.seen = found;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    const OMARCHY_TOKYO_NIGHT: &str = r##"mode = "dark"

accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"

background = "#1a1b26"
lighter_background = "#24283b"

foreground = "#a9b1d6"
dark_foreground = "#565f89"
bright_foreground = "#c0caf5"

red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
"##;

    const LEGACY: &str = r##"accent = "#684c59"
foreground = "#e3a68c"
background = "#1c1213"
color0 = "#1c1213"
color1 = "#c24f57"
color2 = "#806c61"
color3 = "#f28171"
color4 = "#684c59"
color5 = "#a63650"
color6 = "#6b6566"
color7 = "#a87569"
color8 = "#6d4745"
color15 = "#e3a68c"
"##;

    #[test]
    fn reads_an_omarchy_theme() {
        let p = Palette::from_colors_toml(OMARCHY_TOKYO_NIGHT).unwrap();
        assert_eq!(p.bg, Color::Rgb(0x1a, 0x1b, 0x26));
        assert_eq!(p.fg, Color::Rgb(0xc0, 0xca, 0xf5));
        assert_eq!(p.fg2, Color::Rgb(0xa9, 0xb1, 0xd6));
        assert_eq!(p.raised, Color::Rgb(0x24, 0x28, 0x3b));
        assert_eq!(p.sel, Color::Rgb(0x29, 0x2e, 0x42));
        assert_eq!(p.red, Color::Rgb(0xf7, 0x76, 0x8e));
        assert_eq!(p.orange, Color::Rgb(0xeb, 0x92, 0x7b));
        assert_eq!(p.purple, Color::Rgb(0xad, 0x8e, 0xe6));
    }

    #[test]
    fn reads_a_theme_with_only_numbered_colours() {
        let p = Palette::from_colors_toml(LEGACY).unwrap();
        assert_eq!(p.bg, Color::Rgb(0x1c, 0x12, 0x13));
        assert_eq!(p.fg, Color::Rgb(0xe3, 0xa6, 0x8c));
        assert_eq!(p.red, Color::Rgb(0xc2, 0x4f, 0x57));
        // No orange: Omarchy uses yellow for it.
        assert_eq!(p.orange, p.yellow);
        // Every text colour can be read on the background.
        for c in [
            p.fg, p.fg2, p.dim, p.cyan, p.blue, p.red, p.green, p.yellow, p.purple,
        ] {
            assert!(contrast(c, p.bg) >= 3.0, "{c:?} on {:?}", p.bg);
        }
        // color4 (#684c59) is too dark to read as text there, so blue was lifted.
        assert_ne!(p.blue, Color::Rgb(0x68, 0x4c, 0x59));
    }

    #[test]
    fn light_themes_stay_readable() {
        let p = Palette::from_colors_toml(
            "background = \"#ffffff\"\nforeground = \"#222222\"\nyellow = \"#f0e68c\"\n",
        )
        .unwrap();
        assert!(contrast(p.yellow, p.bg) >= 3.0);
        assert!(luminance(p.yellow) < luminance(Color::Rgb(0xf0, 0xe6, 0x8c)));
    }

    #[test]
    fn needs_a_background_and_foreground() {
        assert_eq!(Palette::from_colors_toml("red = \"#ff0000\""), None);
        assert_eq!(Palette::from_colors_toml("not a theme"), None);
    }

    #[test]
    fn recolors_only_kerdrs_own_colours() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf[(0, 0)].set_style(Style::default().fg(theme::FG).bg(theme::BG));
        buf[(1, 0)].set_style(Style::default().fg(theme::RED).bg(theme::SEL));
        buf[(2, 0)].set_style(Style::default().fg(Color::Rgb(1, 2, 3)));
        let p = Palette::from_colors_toml(LEGACY).unwrap();
        p.recolor(&mut buf);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (p.fg, p.bg));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (p.red, p.sel));
        assert_eq!(buf[(2, 0)].fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn tokyo_night_changes_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf[(0, 0)].set_style(Style::default().fg(theme::FG).bg(theme::BG));
        let before = buf.clone();
        Palette::tokyo_night().recolor(&mut buf);
        assert_eq!(buf, before);
    }

    #[test]
    fn a_watch_follows_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("colors.toml");
        let mut w = ThemeWatch::new(Some(path.to_str().unwrap()));
        assert_eq!(w.path(), Some(path.clone()));
        w.check(true);
        assert!(w.seen.is_none());
        std::fs::write(&path, LEGACY).unwrap();
        // check() sets the shared palette, which other tests' drawings would see, so this test
        // reads the file the way check() does instead.
        assert_eq!(Palette::from_file(&path), Palette::from_colors_toml(LEGACY));
        assert!(ThemeWatch::new(Some("")).path().is_none());
    }
}
