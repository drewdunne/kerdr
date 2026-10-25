//! Renders the README's screenshots from the demo board, through Kerdr's own drawing code.
//!
//!   cargo run --example shots -- <out-dir> <regular.ttf> <bold.ttf> [fallback.ttf...]

use std::path::{Path, PathBuf};

use fontdue::{Font, FontSettings};
use image::{Rgb, RgbImage};
use kerdr::app::{App, Draft, Mode, Selection};
use kerdr::config::Config;
use kerdr::{demo, theme, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

const COLS: u16 = 176;
const ROWS: u16 = 44;
const PX: f32 = 17.0;

struct Fonts {
    regular: Font,
    bold: Font,
    fallback: Vec<Font>,
}

fn load(path: &Path) -> Font {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Font::from_bytes(bytes, FontSettings::default()).expect("a font")
}

fn app(away: bool) -> App {
    let mut a = App::new(Config::default(), demo::machines(away));
    a.demo = true;
    a.fixed_now = Some(demo::now());
    a.width = COLS;
    a.rebuild();
    for card in &mut a.board.cards {
        if let Some(detail) = &mut card.detail {
            detail.organization = Some("example-studio".into());
            detail.tenant = (card.session.as_deref() == Some("web")).then(|| "shop".into());
            detail.project = Some(card.session.clone().unwrap_or_else(|| "platform".into()));
            detail.created = Some("2026-10-01".into());
            detail.refreshed_at = Some(
                if card.stale_days.unwrap_or(0) >= 14 {
                    "2026-10-04"
                } else {
                    "2026-10-21"
                }
                .into(),
            );
        }
    }
    a
}

fn select(a: &mut App, reference: &str) {
    if let Some(c) = a.board.cards.iter().find(|c| c.reference == reference) {
        a.sel = Selection {
            col: c.stage.index(),
            key: Some(c.key.clone()),
        };
    }
}

fn render(a: &App) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    t.draw(|f| ui::draw(f, a)).unwrap();
    t.backend().buffer().clone()
}

fn rgb(c: Color, fallback: Color) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Reset => rgb(fallback, fallback),
        _ => rgb(fallback, fallback),
    }
}

fn blend(img: &mut RgbImage, x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 || alpha <= 0.0 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    for (channel, c) in p.0.iter_mut().zip(color) {
        *channel = (*channel as f32 * (1.0 - alpha) + c as f32 * alpha).round() as u8;
    }
}

/// Box-drawing characters drawn as lines, so borders join without gaps at any line height.
fn draw_box(
    img: &mut RgbImage,
    ch: char,
    x: i32,
    y: i32,
    cw: i32,
    chh: i32,
    color: [u8; 3],
) -> bool {
    let thick = matches!(ch, '━' | '┃' | '┏' | '┓' | '┗' | '┛');
    let t = if thick { 2 } else { 1 };
    let (cx, cy) = (x + cw / 2, y + chh / 2);
    let (left, right, up, down) = match ch {
        '─' | '━' => (true, true, false, false),
        '│' | '┃' => (false, false, true, true),
        '┌' | '┏' | '╭' => (false, true, false, true),
        '┐' | '┓' | '╮' => (true, false, false, true),
        '└' | '┗' | '╰' => (false, true, true, false),
        '┘' | '┛' | '╯' => (true, false, true, false),
        _ => return false,
    };
    let rounded = matches!(ch, '╭' | '╮' | '╰' | '╯');
    let r = if rounded { (cw / 2).min(chh / 2) } else { 0 };
    let hline = |img: &mut RgbImage, x0: i32, x1: i32| {
        for xx in x0..x1 {
            for k in 0..t {
                blend(img, xx, cy - t / 2 + k, color, 1.0);
            }
        }
    };
    let vline = |img: &mut RgbImage, y0: i32, y1: i32| {
        for yy in y0..y1 {
            for k in 0..t {
                blend(img, cx - t / 2 + k, yy, color, 1.0);
            }
        }
    };
    if left {
        hline(img, x, if rounded { cx - r + 1 } else { cx + t });
    }
    if right {
        hline(img, if rounded { cx + r } else { cx - t / 2 }, x + cw);
    }
    if up {
        vline(img, y, if rounded { cy - r + 1 } else { cy + t });
    }
    if down {
        vline(img, if rounded { cy + r } else { cy - t / 2 }, y + chh);
    }
    if rounded {
        // A quarter circle joining the two half lines, anti-aliased by distance.
        let (ox, oy) = match ch {
            '╭' => (cx + r, cy + r),
            '╮' => (cx - r, cy + r),
            '╰' => (cx + r, cy - r),
            _ => (cx - r, cy - r),
        };
        for yy in (cy - r - 1)..=(cy + r + 1) {
            for xx in (cx - r - 1)..=(cx + r + 1) {
                let in_quadrant = match ch {
                    '╭' => xx <= ox && yy <= oy,
                    '╮' => xx >= ox && yy <= oy,
                    '╰' => xx <= ox && yy >= oy,
                    _ => xx >= ox && yy >= oy,
                };
                if !in_quadrant {
                    continue;
                }
                let d = (((xx - ox) * (xx - ox) + (yy - oy) * (yy - oy)) as f32).sqrt();
                let cover = (1.0 - (d - r as f32).abs()).clamp(0.0, 1.0);
                blend(img, xx, yy, color, cover);
            }
        }
    }
    true
}

fn rasterize(buf: &Buffer, fonts: &Fonts, out: &Path) {
    let lm = fonts
        .regular
        .horizontal_line_metrics(PX)
        .expect("line metrics");
    let cw = fonts.regular.metrics('M', PX).advance_width.round() as i32;
    let chh = (lm.new_line_size * 1.08).round() as i32;
    let ascent = lm.ascent + (chh as f32 - lm.new_line_size) / 2.0;
    let pad = 18;
    let w = COLS as i32 * cw + pad * 2;
    let h = ROWS as i32 * chh + pad * 2;
    let bg0 = rgb(theme::BG, theme::BG);
    let mut img = RgbImage::from_pixel(w as u32, h as u32, Rgb(bg0));
    for y in 0..ROWS {
        for x in 0..COLS {
            let cell = &buf[(x, y)];
            let (px, py) = (pad + x as i32 * cw, pad + y as i32 * chh);
            let bg = rgb(cell.bg, theme::BG);
            for yy in py..py + chh {
                for xx in px..px + cw {
                    blend(&mut img, xx, yy, bg, 1.0);
                }
            }
            let fg = rgb(cell.fg, theme::FG);
            let sym = cell.symbol();
            let Some(ch) = sym.chars().next() else {
                continue;
            };
            if ch == ' ' || draw_box(&mut img, ch, px, py, cw, chh, fg) {
                continue;
            }
            let bold = cell.modifier.contains(Modifier::BOLD);
            let primary = if bold { &fonts.bold } else { &fonts.regular };
            let font = std::iter::once(primary)
                .chain(fonts.fallback.iter())
                .find(|f| f.lookup_glyph_index(ch) != 0)
                .unwrap_or(primary);
            let (m, bitmap) = font.rasterize(ch, PX);
            let gx = px + m.xmin + ((cw - m.advance_width.round() as i32) / 2).max(0);
            let gy = py + ascent.round() as i32 - m.height as i32 - m.ymin;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = bitmap[row * m.width + col] as f32 / 255.0;
                    blend(&mut img, gx + col as i32, gy + row as i32, fg, a);
                }
            }
        }
    }
    img.save(out).expect("png written");
    println!("{}", out.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().expect("an output folder"));
    let fonts = Fonts {
        regular: load(Path::new(&args[1])),
        bold: load(Path::new(&args[2])),
        fallback: args[3..].iter().map(|p| load(Path::new(p))).collect(),
    };
    std::fs::create_dir_all(&out).unwrap();

    let board = app(false);
    rasterize(&render(&board), &fonts, &out.join("board.png"));

    let mut backlog_card = app(false);
    select(&mut backlog_card, "#21");
    rasterize(
        &render(&backlog_card),
        &fonts,
        &out.join("board-backlog-card.png"),
    );

    let mut filter = app(false);
    filter.mode = Mode::Filter {
        query: String::new(),
        sel: 0,
        back: Box::new(Mode::Board),
    };
    rasterize(&render(&filter), &fonts, &out.join("filter.png"));

    let mut peek = app(false);
    select(&mut peek, "#41");
    peek.mode = Mode::Peek {
        scroll: 0,
        back: Box::new(Mode::Board),
    };
    rasterize(&render(&peek), &fonts, &out.join("peek.png"));

    let mut road = app(true);
    select(&mut road, "#41");
    rasterize(&render(&road), &fonts, &out.join("reconnecting.png"));

    let mut machines = app(true);
    machines.mode = Mode::Machines { sel: 1 };
    rasterize(&render(&machines), &fonts, &out.join("machines.png"));

    let mut keys = app(false);
    keys.mode = Mode::Keys {
        filter: String::new(),
        typing: false,
        scroll: 0,
    };
    rasterize(&render(&keys), &fonts, &out.join("keys.png"));
    let mut machine = app(false);
    machine.machine = Some("build-box".into());
    machine.rebuild();
    machine.tab = 1;
    rasterize(&render(&machine), &fonts, &out.join("machine-sessions.png"));

    let mut backlog = app(false);
    backlog.mode = Mode::Backlog { sel: 0 };
    rasterize(&render(&backlog), &fonts, &out.join("backlog.png"));

    let mut misc = app(false);
    misc.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('i'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    rasterize(&render(&misc), &fonts, &out.join("misc.png"));

    let mut palette = app(false);
    select(&mut palette, "#21");
    palette.mode = Mode::Palette {
        back: Box::new(Mode::Board),
        target: palette.sel.key.clone(),
    };
    rasterize(&render(&palette), &fonts, &out.join("commands.png"));

    let mut compose = app(false);
    compose.mode = Mode::Compose {
        draft: Draft {
            words: "Show each game’s trade-offs before choosing one, so I can pick without opening every store page.\n\nNot urgent.".into(),
            machine: 0,
            session: "web".into(),
        },
        back: Box::new(Mode::Board),
    };
    rasterize(&render(&compose), &fonts, &out.join("new-intent.png"));

    let mut confirmation = app(false);
    confirmation.mode = Mode::Confirm { machine: 0,
        request: serde_json::json!({"action":"dispatch","id":"40","override":true}),
        title: "Start #40 in web?".into(),
        message: "web has 3 active workspaces. The usual limit is 3. Starting this makes 4; the maximum is 5.".into(),
        back: Box::new(Mode::Backlog { sel: 0 }),
        target: None,
        queue: false,
        ready_at: None,
    };
    rasterize(
        &render(&confirmation),
        &fonts,
        &out.join("dispatch-confirmation.png"),
    );

    let mut dismissed = app(false);
    if let Some(card) = dismissed
        .board
        .cards
        .iter_mut()
        .find(|c| c.stage == kerdr::model::Stage::Backlog)
    {
        if let Some(detail) = &mut card.detail {
            detail.state = Some("wont_do".into());
            detail.disposition = Some("wont_do".into());
        }
        card.state = Some("wont_do".into());
    }
    dismissed.wont_do = true;
    dismissed.mode = Mode::Backlog { sel: 0 };
    rasterize(&render(&dismissed), &fonts, &out.join("wont-do.png"));
    let mut provider = app(false);
    for (index, card) in provider.board.cards.iter_mut().enumerate() {
        for agent in &mut card.agents {
            agent.agent = if index % 2 == 0 { "codex" } else { "claude" }.into();
        }
    }
    provider.provider = Some("codex".into());
    if let Some(card) = provider
        .board
        .cards
        .iter()
        .find(|card| provider.card_visible(card))
    {
        provider.sel = Selection {
            col: card.stage.index(),
            key: Some(card.key.clone()),
        };
    }
    rasterize(&render(&provider), &fonts, &out.join("provider.png"));
    let mut empty = app(false);
    empty.project = Some("no-matching-project".into());
    rasterize(&render(&empty), &fonts, &out.join("empty.png"));
}
