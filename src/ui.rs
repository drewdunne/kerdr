//! Drawing every screen: the board, and over or instead of it the peek, search, keys menu,
//! machines and backlog. Layout follows the design canvas.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Hit, IntentForm, Mode, Search, SearchFilter, Tone, match_range};
use crate::board::{Scope, SessionRow};
use crate::herdr;
use crate::model::{Attention, Card, Priority, Stage, Verdict};
use crate::poll::Conn;
use crate::theme::*;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 32 || area.height < 10 {
        let text = trunc("Kerdr needs a bigger window.", area.width as usize);
        let y = area.y + area.height / 2;
        put(
            f.buffer_mut(),
            area.x,
            y,
            area.width,
            vec![Span::styled(text, fg(FG2))],
        );
        return;
    }
    draw_mode(f, area, app, &app.mode);
    if !matches!(app.mode, Mode::Board) {
        action_status(f, area, app);
    }
}

fn action_status(f: &mut Frame, area: Rect, app: &App) {
    let (text, color) = if app.busy {
        ("Action in progress · navigation stays available", BLUE)
    } else if let Some((text, tone)) = app.message() {
        (
            text,
            match tone {
                Tone::Info => FG2,
                Tone::Good => GREEN,
                Tone::Warn => YELLOW,
            },
        )
    } else {
        return;
    };
    let w = area.width.saturating_sub(4).min(120);
    let lines = wrapped(text, w.saturating_sub(4) as usize, fg(color), None);
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.bottom().saturating_sub(h + 1),
        width: w,
        height: h,
    };
    dialog(f, rect, color);
    f.render_widget(Paragraph::new(lines), pad(rect, 2, 1));
}

fn draw_mode(f: &mut Frame, area: Rect, app: &App, mode: &Mode) {
    match mode {
        Mode::Peek { back, scroll } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            peek(f, area, app, *scroll);
        }
        Mode::Palette { back } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            palette(f, area);
        }
        Mode::Compose { form, back } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            compose(f, area, app, form);
        }
        Mode::Confirm {
            title,
            message,
            back,
            ..
        } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            confirm(f, area, title, message);
        }
        Mode::Machines { sel } => machines_view(f, area, app, *sel),
        Mode::Backlog { sel } => backlog_view(f, area, app, *sel),
        Mode::Misc { sel } => misc_view(f, area, app, *sel),
        Mode::Search(s) => {
            board_view(f, area, app);
            dim_behind(f);
            search(f, area, app, s);
        }
        Mode::Keys {
            filter,
            typing,
            scroll,
        } => {
            board_view(f, area, app);
            dim_behind(f);
            keys(f, area, app, filter, *typing, *scroll);
        }
        Mode::Board => board_view(f, area, app),
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, w: u16, spans: Vec<Span<'static>>) {
    if w == 0 {
        return;
    }
    buf.set_line(x, y, &Line::from(spans), w);
}

/// Left text and right text on one row, the left cut to make room.
fn put_lr(
    buf: &mut Buffer,
    rect: Rect,
    y: u16,
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
) {
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let avail = rect.width as usize;
    let lw = avail.saturating_sub(if rw > 0 { rw + 1 } else { 0 });
    put(buf, rect.x, y, lw as u16, fit(left, lw));
    if rw > 0 && rw <= avail {
        put(buf, rect.x + (avail - rw) as u16, y, rw as u16, right);
    }
}

/// Cuts a run of spans to a width, ending the last one with … when cut.
fn fit(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let w = width(&s.content);
        if used + w <= max {
            used += w;
            out.push(s);
        } else {
            let room = max.saturating_sub(used);
            if room > 0 {
                out.push(Span::styled(trunc(&s.content, room), s.style));
            }
            break;
        }
    }
    out
}

fn key_hints(hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (k, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(k.to_string(), bold(FG)));
        spans.push(Span::styled(format!(" {what}"), fg(DIM)));
    }
    spans
}

fn pad(rect: Rect, h: u16, v: u16) -> Rect {
    Rect {
        x: rect.x + h,
        y: rect.y + v,
        width: rect.width.saturating_sub(h * 2),
        height: rect.height.saturating_sub(v * 2),
    }
}

fn centered(area: Rect, w: u16, h: u16, top: Option<u16>) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(1));
    let x = area.x + (area.width - w) / 2;
    let y = top
        .map(|t| area.y + t.min(area.height.saturating_sub(h)))
        .unwrap_or(area.y + (area.height - h) / 2);
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn dim_behind(f: &mut Frame) {
    let area = f.area();
    let buf = f.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(FAINT);
                cell.set_bg(BG);
                cell.modifier = Modifier::empty();
            }
        }
    }
}

fn dialog(f: &mut Frame, rect: Rect, border: ratatui::style::Color) {
    f.render_widget(Clear, rect);
    panel(f, rect, border, PANEL, BorderType::Rounded);
}

/// An outline drawn on the page's background with only the inside filled, so the line isn't
/// framed by a lighter band.
fn panel(
    f: &mut Frame,
    rect: Rect,
    border: ratatui::style::Color,
    fill: ratatui::style::Color,
    kind: BorderType,
) {
    f.render_widget(
        Block::bordered()
            .border_type(kind)
            .border_style(Style::default().fg(border).bg(BG))
            .style(Style::default().bg(BG).fg(FG)),
        rect,
    );
    if rect.width > 2 && rect.height > 2 {
        f.buffer_mut()
            .set_style(pad(rect, 1, 1), Style::default().bg(fill));
    }
}

fn conn_color(c: &Conn) -> ratatui::style::Color {
    match c {
        Conn::Live { .. } => GREEN,
        Conn::Connecting => BLUE,
        Conn::Reconnecting { .. } => YELLOW,
        Conn::Unreachable { .. } => DIM,
        Conn::HostKey { .. } | Conn::Auth { .. } => ORANGE,
        Conn::HerdrMissing | Conn::HerdrTooOld { .. } => RED,
        Conn::Hidden => FAINT,
    }
}

// ─── The board ──────────────────────────────────────────────────────────────────────────────

fn board_view(f: &mut Frame, area: Rect, app: &App) {
    let inner = pad(area, 1, 0);
    let banner = banner(app);
    let pending = pending_line(app);
    let message = app.message();
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(u16::from(banner.is_some())),
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(u16::from(pending.is_some() || message.is_some())),
        Constraint::Length(1),
    ])
    .split(inner);
    header(
        f.buffer_mut(),
        Rect {
            height: 1,
            ..rows[0]
        },
        app,
    );
    filters(
        f.buffer_mut(),
        Rect {
            y: rows[0].y + 1,
            height: 1,
            ..rows[0]
        },
        app,
    );
    if let Some((left, right)) = banner {
        put_lr(f.buffer_mut(), rows[1], rows[1].y, left, right);
    }
    strip(f, rows[2], app);
    columns(f, rows[3], app);
    if app
        .columns()
        .iter()
        .zip(app.visible_columns())
        .all(|(cards, visible)| !visible || cards.is_empty())
    {
        let rect = Rect {
            y: rows[3].y.saturating_add(3),
            height: rows[3].height.saturating_sub(3),
            ..rows[3]
        };
        let lines = vec![
            Line::styled("No work cards in this view.", bold(FG2)),
            Line::raw(""),
            Line::styled(
                if app.machine.is_none() {
                    "{ } choose a machine, then [ ] choose a session"
                } else {
                    "[ ] choose session    { } change machine"
                },
                fg(CYAN),
            ),
            Line::styled("a provider    / search    i Misc", fg(CYAN)),
            Line::styled("Ctrl+Space commands and work filters", fg(DIM)),
        ];
        f.render_widget(
            Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
            rect,
        );
    }
    if let Some(line) = pending {
        put(f.buffer_mut(), rows[4].x, rows[4].y, rows[4].width, line);
    } else if let Some((text, tone)) = message {
        let color = match tone {
            Tone::Info => FG2,
            Tone::Good => GREEN,
            Tone::Warn => YELLOW,
        };
        put(
            f.buffer_mut(),
            rows[4].x,
            rows[4].y,
            rows[4].width,
            vec![Span::styled(text.to_string(), fg(color))],
        );
    }
    let hints = key_hints(&[
        ("a", "provider"),
        ("Ctrl+Space", "commands"),
        ("h j k l", "move"),
        ("⏎", "jump in"),
        ("m", "peek"),
        ("n N", "next that needs you"),
        ("/", "search"),
        ("[ ]", "tab"),
        ("{ }", "machine"),
        ("b", "backlog"),
        ("i", "misc"),
        ("M", "machines"),
        ("?", "keys"),
        ("q", "quit"),
    ]);
    put(
        f.buffer_mut(),
        rows[5].x,
        rows[5].y,
        rows[5].width,
        fit(hints, rows[5].width as usize),
    );
}

fn tab_spans(app: &App, i: usize, scope: &Scope) -> Vec<Span<'static>> {
    let selected = i == app.tab;
    let sel_style = Style::default()
        .bg(CYAN)
        .fg(BG)
        .add_modifier(Modifier::BOLD);
    match scope {
        Scope::All => {
            let n = app
                .board
                .cards
                .iter()
                .filter(|c| !matches!(c.stage, Stage::Backlog | Stage::Maintain | Stage::Misc))
                .count();
            let style = if selected {
                sel_style
            } else {
                Style::default().bg(RAISED).fg(FG)
            };
            vec![Span::styled(format!(" All {n} "), style)]
        }
        Scope::Machine(name) => {
            let m = app.machine_index(name).map(|i| &app.machines[i]);
            let conn = m.map(|m| m.conn.clone()).unwrap_or(Conn::Connecting);
            let mut spans = vec![Span::styled(" │ ", fg(BORDER))];
            let (gstyle, nstyle) = if selected {
                (sel_style, sel_style)
            } else {
                (bold(conn_color(&conn)), bold(FG))
            };
            spans.push(Span::styled(format!("{} ", conn.glyph()), gstyle));
            spans.push(Span::styled(name.clone(), nstyle));
            if !conn.is_live() && !matches!(conn, Conn::Connecting) {
                let short = match &conn {
                    Conn::Reconnecting { .. } => "reconnecting",
                    Conn::Unreachable { .. } => "can't reach",
                    Conn::HostKey { .. } | Conn::Auth { .. } => "needs you",
                    Conn::HerdrMissing => "no herdr",
                    Conn::HerdrTooOld { .. } => "herdr too old",
                    _ => "",
                };
                spans.push(Span::styled(format!(" {short}"), fg(conn_color(&conn))));
            }
            spans.push(Span::raw(" "));
            spans
        }
        Scope::Session(m, s) => {
            let row = app
                .board
                .sessions
                .iter()
                .find(|r| &r.machine == m && &r.session == s);
            let (slots, _, running, cached) = row
                .map(|r| (r.slots.label(), r.needs, r.running, r.cached))
                .unwrap_or_default();
            let needs = app
                .board
                .cards
                .iter()
                .filter(|card| {
                    card.machine == *m
                        && card.session.as_deref() == Some(s.as_str())
                        && app.card_matches_filters(card)
                        && card.stage != Stage::Misc
                        && card.needs_you()
                })
                .count();
            let base = if selected {
                sel_style
            } else if !running || cached {
                Style::default().bg(RAISED).fg(DIM)
            } else {
                Style::default().bg(RAISED).fg(FG)
            };
            let mut spans = vec![Span::styled(format!(" {s}"), base)];
            if running {
                spans.push(Span::styled(
                    format!(" {slots}"),
                    if selected { base } else { base.fg(DIM) },
                ));
            } else {
                spans.push(Span::styled(" ◦", base));
            }
            if needs > 0 {
                spans.push(Span::styled(
                    format!(" ●{needs}"),
                    if selected {
                        base
                    } else {
                        base.fg(ORANGE).add_modifier(Modifier::BOLD)
                    },
                ));
            }
            spans.push(Span::styled(" ", base));
            spans.push(Span::raw(" "));
            spans
        }
    }
}

fn header(buf: &mut Buffer, rect: Rect, app: &App) {
    let machine = app.machine.as_deref().unwrap_or("All machines");
    let label = format!(" {} ▾ ", trunc(machine, (rect.width as usize / 3).max(8)));
    let mut left = vec![
        Span::styled(" kerdr ", bold(CYAN)),
        Span::raw(" "),
        Span::styled(
            label,
            Style::default()
                .bg(CYAN)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let fixed: usize = left.iter().map(|s| width(&s.content)).sum();
    let avail = (rect.width as usize).saturating_sub(fixed);
    let tabs: Vec<_> = app
        .tabs
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, scope)| tab_spans(app, i, scope))
        .collect();
    let widths: Vec<usize> = tabs
        .iter()
        .map(|t| t.iter().map(|s| width(&s.content)).sum())
        .collect();
    let selected = app.tab.saturating_sub(1).min(tabs.len().saturating_sub(1));
    let mut start = 0;
    while start < selected && widths[start..=selected].iter().sum::<usize>() + 2 > avail {
        start += 1;
    }
    if start > 0 {
        left.push(Span::styled("‹ ", fg(DIM)));
    }
    let mut used = usize::from(start > 0) * 2;
    let mut end = start;
    while end < tabs.len() && used + widths[end] <= avail {
        left.extend(tabs[end].clone());
        used += widths[end];
        end += 1;
    }
    if end < tabs.len() {
        left.push(Span::styled("›", fg(DIM)));
    }
    let right = vec![Span::styled(
        format!("{{ }} machine · [ ] session · {}", clock(app.now())),
        fg(DIM),
    )];
    let lw: usize = left.iter().map(|s| width(&s.content)).sum();
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    if lw + rw + 2 <= rect.width as usize {
        put_lr(buf, rect, rect.y, left, right);
    } else {
        put(
            buf,
            rect.x,
            rect.y,
            rect.width,
            fit(left, rect.width as usize),
        );
    }
}

fn filters(buf: &mut Buffer, rect: Rect, app: &App) {
    let mut left = vec![
        Span::styled(format!(" a provider: {}", app.provider_label()), fg(CYAN)),
        Span::styled(format!("   owner: {}", app.owner_label()), fg(CYAN)),
        Span::styled(format!("   project: {}", app.project_label()), fg(CYAN)),
    ];
    if app.wont_do {
        left.push(Span::styled("   WON’T DO", bold(YELLOW)));
    }
    let right = if app.busy {
        vec![Span::styled("◐ action in progress", fg(BLUE))]
    } else if !app.count.is_empty() {
        vec![Span::styled(format!("count: {}", app.count), fg(CYAN))]
    } else if rect.width >= 90 {
        key_hints(&[("Ctrl+Space", "commands")])
    } else {
        vec![]
    };
    put_lr(buf, rect, rect.y, left, right);
}

/// One line about the machine in the most trouble, the way the design's banner reads.
fn banner(app: &App) -> Option<(Vec<Span<'static>>, Vec<Span<'static>>)> {
    let now = app.now();
    let rank = |c: &Conn| match c {
        Conn::HostKey { .. } | Conn::Auth { .. } => 0,
        Conn::Reconnecting { .. } => 1,
        Conn::Unreachable { .. } => 2,
        Conn::HerdrTooOld { .. } | Conn::HerdrMissing => 3,
        _ => 9,
    };
    let mut troubled: Vec<usize> = (0..app.machines.len())
        .filter(|&i| rank(&app.machines[i].conn) < 9)
        .collect();
    troubled.sort_by_key(|&i| rank(&app.machines[i].conn));
    let &i = troubled.first()?;
    let m = &app.machines[i];
    let name = m.name();
    let cards = app
        .board
        .cards
        .iter()
        .filter(|c| {
            c.machine == name && !matches!(c.stage, Stage::Backlog | Stage::Maintain | Stage::Misc)
        })
        .count();
    let as_of = m.last_ok.map(clock);
    let color = conn_color(&m.conn);
    let mut left = match &m.conn {
        Conn::Reconnecting {
            since,
            attempt,
            retry_at,
            ..
        } => {
            let mut v = vec![
                Span::styled(
                    format!("⟳ {name} dropped {} ago", ago(*since, now)),
                    bold(color),
                ),
                Span::styled(
                    format!(
                        "   trying again in {}s (try {})",
                        (*retry_at - now).num_seconds().max(0),
                        attempt + 1
                    ),
                    fg(FG),
                ),
            ];
            if let Some(t) = as_of {
                v.push(Span::styled(
                    format!("   its {cards} cards are dimmed, as they were at {t}"),
                    fg(DIM),
                ));
            }
            v
        }
        Conn::Unreachable {
            since,
            retry_at,
            detail,
        } => {
            let mut v = vec![
                Span::styled(format!("○ can't reach {name}"), bold(FG2)),
                Span::styled(
                    format!(
                        "   no answer since {} ({})",
                        clock(*since),
                        trunc(detail, 40)
                    ),
                    fg(FG),
                ),
                Span::styled(
                    format!("   next try in {}s", (*retry_at - now).num_seconds().max(0)),
                    fg(DIM),
                ),
            ];
            if let Some(t) = as_of {
                v.push(Span::styled(format!("   its cards as of {t}"), fg(DIM)));
            }
            v
        }
        Conn::HostKey { .. } => vec![
            Span::styled(format!("● {name} needs you"), bold(color)),
            Span::styled(
                "   SSH asks whether to trust its host key; Kerdr never answers that for you",
                fg(FG),
            ),
        ],
        Conn::Auth { .. } => vec![
            Span::styled(format!("● {name} needs you"), bold(color)),
            Span::styled("   SSH wants a password, a passphrase or a key", fg(FG)),
        ],
        Conn::HerdrTooOld { version } => vec![
            Span::styled(
                format!("■ {name}: herdr {version} is too old for Kerdr"),
                bold(color),
            ),
            Span::styled(
                format!(
                    "   it needs {} or newer",
                    herdr::version_string(herdr::MIN_VERSION)
                ),
                fg(FG),
            ),
        ],
        Conn::HerdrMissing => vec![Span::styled(
            format!("■ {name}: herdr isn't installed there"),
            bold(color),
        )],
        _ => return None,
    };
    if troubled.len() > 1 {
        left.push(Span::styled(
            format!("   +{} more", troubled.len() - 1),
            fg(DIM),
        ));
    }
    let right = match &m.conn {
        Conn::HostKey { .. } | Conn::Auth { .. } => key_hints(&[("M", "then ⏎ answers it")]),
        Conn::HerdrMissing | Conn::HerdrTooOld { .. } => key_hints(&[("M", "then u sets it up")]),
        _ => key_hints(&[("r", "retry now"), ("M", "machines")]),
    };
    Some((left, right))
}

fn pending_line(app: &App) -> Option<Vec<Span<'static>>> {
    let key = app.pending_jump.as_deref()?;
    let card = app.card(key)?;
    let conn = app
        .machines
        .get(card.machine_index)
        .map(|_| card.machine_index)
        .map(|i| app.machines[i].conn.short(app.now()))
        .unwrap_or_default();
    Some(vec![
        Span::styled(format!("{} ", card.reference), bold(CYAN)),
        Span::styled(format!("is on {}, which is {conn}. ", card.machine), fg(FG)),
        Span::styled(
            "Kerdr jumps in when it answers · esc stops waiting",
            fg(DIM),
        ),
    ])
}

fn strip(f: &mut Frame, rect: Rect, app: &App) {
    let s = app.summary();
    panel(f, rect, BORDER, PANEL, BorderType::Rounded);
    let inner = pad(rect, 2, 1);
    let sep = || Span::styled("  │  ", fg(BORDER));
    let needs_word = if s.needs_you == 1 { "needs" } else { "need" };
    let mut left = vec![
        Span::styled(
            format!("● {} {needs_word} you", s.needs_you),
            if s.needs_you > 0 {
                bold(ORANGE)
            } else {
                fg(DIM)
            },
        ),
        Span::raw("   "),
        Span::styled(
            format!("◐ {} running", s.running),
            if s.running > 0 { fg(BLUE) } else { fg(DIM) },
        ),
        Span::raw("   "),
        Span::styled(
            format!("■ {} blocked", s.blocked),
            if s.blocked > 0 { bold(RED) } else { fg(DIM) },
        ),
    ];
    if s.cached > 0 {
        let since = s
            .cached_since
            .map(|t| format!(" as of {}", clock(t)))
            .unwrap_or_default();
        left.push(Span::raw("   "));
        left.push(Span::styled(format!("⟳ {}{since}", s.cached), fg(YELLOW)));
    }
    if s.slots_limit > 0 {
        left.push(sep());
        left.push(Span::styled(
            format!("slots {}/{}", s.slots_used, s.slots_limit),
            bold(FG),
        ));
        if !s.full_sessions.is_empty() {
            let names = match s.full_sessions.len() {
                1 => s.full_sessions[0].clone(),
                2 => format!("{} and {}", s.full_sessions[0], s.full_sessions[1]),
                n => format!("{} and {} more", s.full_sessions[..2].join(", "), n - 2),
            };
            left.push(Span::styled(format!(" {names} full"), fg(DIM)));
        }
    }
    if s.backlog > 0 {
        left.push(sep());
        left.push(Span::styled(format!("backlog {}", s.backlog), bold(FG)));
        let mut parts = Vec::new();
        if s.queued > 0 {
            parts.push(format!("{} queued", s.queued));
        }
        if s.shelved > 0 {
            parts.push(format!("{} shelved", s.shelved));
        }
        if !parts.is_empty() {
            left.push(Span::styled(format!(" {}", parts.join(", ")), fg(DIM)));
        }
    }
    let right = match &s.verdict {
        Some(v @ Verdict::FinishFirst(_)) => {
            vec![Span::styled(
                format!(" {} ", v.text()),
                Style::default()
                    .bg(ORANGE)
                    .fg(BG)
                    .add_modifier(Modifier::BOLD),
            )]
        }
        Some(v @ Verdict::Full) => {
            vec![Span::styled(
                format!(" {} ", v.text()),
                Style::default().bg(RED).fg(BG).add_modifier(Modifier::BOLD),
            )]
        }
        Some(v @ Verdict::Room { .. }) => {
            vec![Span::styled(format!(" {} ", v.text()), bold(GREEN))]
        }
        Some(v @ Verdict::Empty) => vec![Span::styled(v.text(), fg(DIM))],
        None => vec![],
    };
    put_lr(f.buffer_mut(), inner, inner.y, left, right);
}

fn columns(f: &mut Frame, rect: Rect, app: &App) {
    let vis = app.visible_columns();
    let stages: Vec<Stage> = Stage::ALL
        .iter()
        .copied()
        .filter(|s| vis[s.index()])
        .collect();
    let n = stages.len() as u16;
    let gap = if rect.width > 160 { 2 } else { 1 };
    let col_w = (rect.width.saturating_sub(gap * (n - 1))) / n;
    let cols = app.columns();
    for (k, stage) in stages.iter().enumerate() {
        let x = rect.x + k as u16 * (col_w + gap);
        let r = Rect {
            x,
            y: rect.y,
            width: col_w,
            height: rect.height,
        };
        column(f, r, app, *stage, &cols[stage.index()]);
    }
}

fn column(f: &mut Frame, rect: Rect, app: &App, stage: Stage, items: &[usize]) {
    let sub = match stage {
        Stage::Maintain => "done".to_string(),
        Stage::Backlog => "your order".to_string(),
        _ => String::new(),
    };
    let buf = f.buffer_mut();
    put_lr(
        buf,
        rect,
        rect.y,
        vec![
            Span::styled(
                stage.name().to_uppercase(),
                Style::default().fg(FG2).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", items.len()), bold(FG)),
        ],
        if items.is_empty() {
            vec![]
        } else {
            vec![Span::styled(sub, fg(DIM))]
        },
    );
    put(
        buf,
        rect.x,
        rect.y + 1,
        rect.width,
        vec![Span::styled("─".repeat(rect.width as usize), fg(BORDER))],
    );
    let body = Rect {
        x: rect.x,
        y: rect.y + 2,
        width: rect.width,
        height: rect.height.saturating_sub(2),
    };
    let selected = app.sel.key.as_deref();
    if stage.is_list() {
        list_rows(f.buffer_mut(), body, app, items, selected);
    } else {
        cards(f, body, app, items, selected);
    }
}

fn list_rows(buf: &mut Buffer, body: Rect, app: &App, items: &[usize], selected: Option<&str>) {
    let now = app.now();
    let rows = body.height as usize;
    let sel_pos = items
        .iter()
        .position(|&i| Some(app.board.cards[i].key.as_str()) == selected);
    let need_more = items.len() > rows;
    let visible = if need_more {
        rows.saturating_sub(1)
    } else {
        rows
    };
    let start = match sel_pos {
        Some(p) if p >= visible => p + 1 - visible,
        _ => 0,
    };
    for (row, &i) in items.iter().enumerate().skip(start).take(visible) {
        let c = &app.board.cards[i];
        let y = body.y + (row - start) as u16;
        let (glyph, gcolor, meta, mcolor) = list_glyph(c, now);
        let is_sel = Some(c.key.as_str()) == selected;
        let text_style = if is_sel {
            bold(FG)
        } else if c.cached_at.is_some() {
            fg(DIM)
        } else {
            fg(FG2)
        };
        let line_rect = Rect {
            x: body.x,
            y,
            width: body.width,
            height: 1,
        };
        if is_sel {
            buf.set_style(line_rect, Style::default().bg(SEL));
        }
        put_lr(
            buf,
            line_rect,
            y,
            vec![
                Span::styled(format!("{glyph} "), fg(gcolor)),
                Span::styled(format!("{} {}", c.reference, c.title), text_style),
            ],
            vec![Span::styled(meta, fg(mcolor))],
        );
    }
    if need_more {
        let more = items.len() - visible - start;
        if more > 0 {
            let hint = if items.first().map(|&i| app.board.cards[i].stage) == Some(Stage::Backlog) {
                format!("+{more} more · b opens the backlog")
            } else {
                format!("+{more} more · / finds them")
            };
            put(
                buf,
                body.x,
                (body.y + body.height).saturating_sub(1),
                body.width,
                vec![Span::styled(trunc(&hint, body.width as usize), fg(DIM))],
            );
        }
    }
}

fn list_glyph(
    c: &Card,
    now: chrono::DateTime<chrono::Local>,
) -> (
    &'static str,
    ratatui::style::Color,
    String,
    ratatui::style::Color,
) {
    match c.stage {
        Stage::Maintain => {
            let when = c
                .detail
                .as_ref()
                .and_then(|d| d.closed.as_deref())
                .and_then(|d| {
                    chrono::NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d").ok()
                })
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .and_then(|d| d.and_local_timezone(chrono::Local).single())
                .map(|t| day(t, now))
                .unwrap_or_default();
            if c.workspace_id.is_some() {
                ("✓", GREEN, "still open".into(), YELLOW)
            } else {
                ("✓", GREEN, when, DIM)
            }
        }
        _ => {
            let (glyph, color) = if c.dismissed() {
                ("×", DIM)
            } else if c.priority == Priority::High {
                ("▲", YELLOW)
            } else if c.state.as_deref() == Some("shelved") {
                ("‖", FG2)
            } else if c
                .detail
                .as_ref()
                .is_some_and(|d| d.waits_on.iter().any(|w| w.open != Some(false)))
            {
                ("◇", BLUE)
            } else {
                ("·", DIM)
            };
            let age = c
                .stale_days
                .map(|n| {
                    if n == 0 {
                        "today".into()
                    } else {
                        format!("{n}d")
                    }
                })
                .unwrap_or_else(|| "—".into());
            (
                glyph,
                color,
                age,
                if c.stale_days.unwrap_or(0) >= 14 {
                    YELLOW
                } else {
                    DIM
                },
            )
        }
    }
}

struct CardLines {
    head: Vec<Span<'static>>,
    badge: Vec<Span<'static>>,
    title: Vec<String>,
    note: String,
    age: (String, ratatui::style::Color),
}

fn card_lines(app: &App, c: &Card, inner_w: usize) -> CardLines {
    let now = app.now();
    let cached = c.cached_at.is_some();
    let mut head = vec![Span::styled(
        c.reference.clone(),
        bold(if cached { FG2 } else { FG }),
    )];
    if c.priority == Priority::High {
        head.push(Span::styled(" ▲", fg(YELLOW)));
    }
    if c.background {
        head.push(Span::styled(" bg", fg(PURPLE)));
    }
    let color = if cached {
        DIM
    } else {
        attention_color(c.attention)
    };
    let head_w: usize = head.iter().map(|s| width(&s.content)).sum();
    let full = format!("{} {}", c.attention.glyph(), c.attention.label());
    let label = if head_w + 1 + width(&full) <= inner_w {
        full
    } else {
        c.attention.glyph().to_string()
    };
    let badge = vec![Span::styled(
        label,
        if c.attention == Attention::NeedsYou && !cached {
            bold(color)
        } else {
            fg(color)
        },
    )];
    let title = wrap(&c.title, inner_w, 2);
    let mut note = String::new();
    if let Some((met, total)) = c.progress {
        note.push_str(&format!("{met}/{total} · "));
    }
    if !matches!(app.scope(), Scope::Session(..))
        && let Some(s) = &c.session
    {
        note.push_str(s);
        note.push_str(" · ");
    }
    note.push_str(&c.why);
    // A record that gives only a date says nothing about the hour, so it shows as a day.
    let date_only = c
        .detail
        .as_ref()
        .and_then(|d| d.updated.as_deref())
        .is_some_and(|u| !u.contains('T') && !u.contains(':'));
    let age = match (c.cached_at, c.updated) {
        (Some(t), _) => (clock(t), YELLOW),
        (None, Some(t)) if date_only => (day(t, now), DIM),
        (None, Some(t)) => (ago(t, now), DIM),
        _ => (String::new(), DIM),
    };
    CardLines {
        head,
        badge,
        title,
        note,
        age,
    }
}

fn cards(f: &mut Frame, body: Rect, app: &App, items: &[usize], selected: Option<&str>) {
    let inner_w = body.width.saturating_sub(4) as usize;
    let compact = body.height < 18;
    let all: Vec<(usize, CardLines)> = items
        .iter()
        .map(|&i| (i, card_lines(app, &app.board.cards[i], inner_w)))
        .collect();
    let height = |cl: &CardLines| -> u16 {
        let title = if compact {
            1
        } else {
            cl.title.len().max(1) as u16
        };
        2 + 1 + title + 1
    };
    let sel_pos = all
        .iter()
        .position(|(i, _)| Some(app.board.cards[*i].key.as_str()) == selected);
    // Scroll so the selected card is in view.
    let mut start = 0;
    if let Some(p) = sel_pos {
        loop {
            let used: u16 = all[start..=p].iter().map(|(_, cl)| height(cl)).sum();
            if used < body.height || start == p {
                break;
            }
            start += 1;
        }
    }
    let mut y = body.y;
    if start > 0 {
        put(
            f.buffer_mut(),
            body.x,
            y,
            body.width,
            vec![Span::styled(format!("↑ {start} more"), fg(DIM))],
        );
        y += 1;
    }
    for (k, (i, cl)) in all.iter().enumerate().skip(start) {
        let h = height(cl);
        let remaining = (body.y + body.height).saturating_sub(y);
        if h > remaining || (k + 1 < all.len() && h + 1 > remaining) {
            let more = all.len() - k;
            put(
                f.buffer_mut(),
                body.x,
                y,
                body.width,
                vec![Span::styled(format!("+{more} more ↓"), fg(DIM))],
            );
            break;
        }
        let c = &app.board.cards[*i];
        let is_sel = Some(c.key.as_str()) == selected;
        let cached = c.cached_at.is_some();
        let border = if is_sel {
            CYAN
        } else if c.attention == Attention::NeedsYou && !cached {
            ORANGE
        } else if c.attention == Attention::Blocked && !cached {
            RED
        } else {
            BORDER
        };
        let fill = if is_sel {
            SEL
        } else if cached {
            BG
        } else {
            PANEL
        };
        let rect = Rect {
            x: body.x,
            y,
            width: body.width,
            height: h,
        };
        let kind = if is_sel {
            BorderType::Thick
        } else {
            BorderType::Rounded
        };
        panel(f, rect, border, fill, kind);
        let inner = pad(rect, 2, 1);
        let buf = f.buffer_mut();
        put_lr(buf, inner, inner.y, cl.head.clone(), cl.badge.clone());
        let title_rows = if compact { 1 } else { cl.title.len().max(1) };
        for (t, line) in cl.title.iter().take(title_rows).enumerate() {
            let text = if compact {
                trunc(&cl.title.join(" "), inner.width as usize)
            } else {
                line.clone()
            };
            put(
                buf,
                inner.x,
                inner.y + 1 + t as u16,
                inner.width,
                vec![Span::styled(text, fg(if cached { DIM } else { FG }))],
            );
        }
        let note_y = inner.y + 1 + title_rows as u16;
        put_lr(
            buf,
            inner,
            note_y,
            vec![Span::styled(cl.note.clone(), fg(DIM))],
            vec![Span::styled(cl.age.0.clone(), fg(cl.age.1))],
        );
        y += h;
    }
}

// ─── Peek ───────────────────────────────────────────────────────────────────────────────────

fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default().fg(FG2).add_modifier(Modifier::BOLD),
    ))
}

fn wrapped(
    text: &str,
    w: usize,
    style: Style,
    prefix: Option<(&str, ratatui::style::Color)>,
) -> Vec<Line<'static>> {
    let pw = prefix.map(|(p, _)| width(p)).unwrap_or(0);
    wrap(text, w.saturating_sub(pw).max(10), 50)
        .into_iter()
        .map(|l| {
            let mut spans = Vec::new();
            if let Some((p, c)) = prefix {
                spans.push(Span::styled(p.to_string(), fg(c)));
            }
            spans.push(Span::styled(l, style));
            Line::from(spans)
        })
        .collect()
}

fn peek(f: &mut Frame, area: Rect, app: &App, scroll: u16) {
    let Some(c) = app.selected() else { return };
    let now = app.now();
    let box_w = 112.min(area.width.saturating_sub(2));
    let w = box_w.saturating_sub(6) as usize;
    let mut lines: Vec<Line> = Vec::new();

    let color = attention_color(c.attention);
    let mut badge = format!("{} {}", c.attention.glyph(), c.attention.label());
    if let Some(t) = c.updated
        && c.attention == Attention::NeedsYou
    {
        badge.push_str(&format!(" · {}", ago(t, now)));
    }
    let title = format!("{}  {}", c.reference, c.title);
    let tw = w.saturating_sub(width(&badge) + 2);
    let pad_n = w.saturating_sub(width(&trunc(&title, tw)) + width(&badge));
    lines.push(Line::from(vec![
        Span::styled(trunc(&title, tw), bold(FG)),
        Span::raw(" ".repeat(pad_n)),
        Span::styled(badge, bold(color)),
    ]));

    let mut place = vec![c.machine.clone()];
    if let Some(s) = &c.session {
        place.push(s.clone());
    }
    if let Some(l) = &c.workspace_label {
        place.push(l.clone());
    }
    let mut facts = vec![place.join(" › ")];
    facts.push(format!(
        "owner: {} · project: {}",
        c.owner_label(),
        if c.project().is_empty() {
            "unassigned"
        } else {
            c.project()
        }
    ));
    if let Some(d) = &c.detail {
        if let Some(created) = &d.created {
            facts.push(format!("created: {created}"));
        }
        if let Some(refreshed) = &d.refreshed_at {
            facts.push(format!("last affirmed: {refreshed}"));
        } else if let Some(created) = &d.created {
            facts.push(format!("last affirmed: {created} (created)"));
        }
    }
    if c.workspace_id.is_none() {
        facts.push(match c.stage {
            Stage::Backlog => "not started".into(),
            Stage::Maintain => "closed".into(),
            _ => "no workspace open".into(),
        });
    }
    for a in &c.agents {
        let what = match a.status.as_str() {
            "done" => "finished its turn, not yet seen",
            "idle" => "waiting for input",
            "working" => "working",
            "blocked" => "asking you something",
            _ => "status unknown",
        };
        facts.push(format!("{} {what}", a.agent));
    }
    if let Some(t) = c.cached_at {
        facts.push(format!("as last seen at {}", clock(t)));
    }
    lines.extend(wrapped(&facts.join(" · "), w, fg(DIM), None));

    if let Some(story) = c.detail.as_ref().and_then(|d| d.story.clone()) {
        lines.push(Line::raw(""));
        lines.extend(wrapped(&story, w, fg(FG2), None));
    }

    // The lifecycle rail: where it's been, where it is, what's left.
    lines.push(Line::raw(""));
    let cell = (w / 7).max(8);
    let mut names = Vec::new();
    let mut notes = Vec::new();
    for s in Stage::ALL.into_iter().filter(|_| c.stage != Stage::Misc) {
        let (glyph, style) = if s == c.stage {
            (
                "●",
                bold(if c.attention == Attention::NeedsYou {
                    ORANGE
                } else {
                    CYAN
                }),
            )
        } else if s < c.stage {
            ("✓", fg(GREEN))
        } else {
            ("○", fg(DIM))
        };
        let name_style = if s == c.stage {
            style
        } else if s < c.stage {
            fg(FG)
        } else {
            fg(DIM)
        };
        let label = format!("{glyph} {}", s.name());
        names.push(Span::styled(
            format!("{:<cell$}", trunc(&label, cell - 1)),
            name_style,
        ));
        let note = c
            .detail
            .as_ref()
            .and_then(|d| {
                d.timeline
                    .iter()
                    .rev()
                    .find(|t| Stage::parse(&t.stage) == Some(s))
            })
            .map(|t| t.note.clone().or(t.date.clone()).unwrap_or_default())
            .unwrap_or_default();
        notes.push(Span::styled(
            format!("{:<cell$}", trunc(&note, cell - 1)),
            fg(DIM),
        ));
    }
    lines.push(Line::from(names));
    if notes.iter().any(|s| !s.content.trim().is_empty()) {
        lines.push(Line::from(notes));
    }

    let detail = c.detail.as_ref();
    if c.attention == Attention::NeedsYou {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("▌ ", fg(ORANGE)),
            Span::styled("What it needs from you", bold(ORANGE)),
        ]));
        let needs = detail
            .and_then(|d| d.needs.clone())
            .unwrap_or_else(|| c.why.clone());
        lines.extend(wrapped(&needs, w, fg(FG), Some(("▌ ", ORANGE))));
        let how = if c.is_open_workspace() {
            "Answer in its pane: ⏎ jumps in."
        } else {
            "It has no workspace open to answer in."
        };
        lines.push(Line::from(vec![
            Span::styled("▌ ", fg(ORANGE)),
            Span::styled(how, fg(DIM)),
        ]));
    }
    if let Some(d) = detail {
        if !d.blockers.is_empty() {
            lines.push(Line::raw(""));
            lines.push(section("BLOCKED BY"));
            for b in &d.blockers {
                lines.extend(wrapped(b, w, fg(FG), Some(("■ ", RED))));
            }
        }
        if !d.criteria.is_empty() {
            lines.push(Line::raw(""));
            let mut head = vec![Span::styled(
                "DONE WHEN",
                Style::default().fg(FG2).add_modifier(Modifier::BOLD),
            )];
            if let Some((met, total)) = c.progress {
                head.push(Span::styled(
                    format!("   {met} of {total} met   "),
                    bold(FG),
                ));
                for k in 0..total {
                    head.push(Span::styled(
                        if k < met { "▰" } else { "▱" },
                        fg(if k < met { GREEN } else { BORDER }),
                    ));
                }
            } else {
                head.push(Span::styled("   not ticked off yet", fg(DIM)));
            }
            lines.push(Line::from(head));
            for cr in &d.criteria {
                let (g, col) = match cr.met {
                    Some(true) => ("✓ ", GREEN),
                    Some(false) => ("○ ", DIM),
                    None => ("· ", DIM),
                };
                lines.extend(wrapped(
                    &cr.text,
                    w,
                    fg(if cr.met == Some(true) { FG } else { FG2 }),
                    Some((g, col)),
                ));
            }
        }
        if let Some(msg) = &d.message {
            lines.push(Line::raw(""));
            let mut head = vec![Span::styled(
                "ITS LAST UPDATE",
                Style::default().fg(FG2).add_modifier(Modifier::BOLD),
            )];
            if let Some(status) = &d.status {
                head.push(Span::styled(format!("   {status}"), fg(DIM)));
            }
            lines.push(Line::from(head));
            lines.extend(wrapped(msg, w, fg(FG), Some(("│ ", BORDER))));
        }
        if !d.waits_on.is_empty() {
            lines.push(Line::raw(""));
            lines.push(section("WAITS ON"));
            for wo in &d.waits_on {
                let state = match wo.open {
                    Some(false) => "done",
                    _ => "open",
                };
                let text = format!(
                    "{} {} ({state})",
                    wo.reference.clone().unwrap_or_default(),
                    wo.title.clone().unwrap_or_default()
                );
                lines.extend(wrapped(&text, w, fg(FG2), Some(("◇ ", BLUE))));
            }
        }
        if let Some(result) = &d.result {
            lines.push(Line::raw(""));
            lines.push(section("WHAT IT DELIVERED"));
            lines.extend(wrapped(result, w, fg(FG), Some(("✓ ", GREEN))));
        }
        if !d.log.is_empty() {
            lines.push(Line::raw(""));
            lines.push(section("LATELY, FROM ITS LOG"));
            for e in d.log.iter().take(5) {
                let date = e.date.clone().unwrap_or_default();
                let short = date.get(5..).unwrap_or(&date).to_string();
                let prefix = format!("{short:<6} ");
                for (k, l) in wrap(&e.text, w.saturating_sub(7), 3)
                    .into_iter()
                    .enumerate()
                {
                    let p = if k == 0 {
                        prefix.clone()
                    } else {
                        " ".repeat(7)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(p, fg(DIM)),
                        Span::styled(l, fg(FG2)),
                    ]));
                }
            }
        }
        if let Some(words) = &d.words {
            lines.push(Line::raw(""));
            lines.push(section("HOW IT STARTED"));
            lines.extend(wrapped(words, w, fg(DIM), Some(("“ ", DIM))));
        }
    } else if !c.agents.is_empty() {
        lines.push(Line::raw(""));
        lines.push(section("ITS AGENTS"));
        for a in &c.agents {
            let text = format!(
                "{} · {} · {}",
                a.agent,
                a.status,
                a.title.clone().unwrap_or_default()
            );
            lines.extend(wrapped(&text, w, fg(FG2), Some(("· ", DIM))));
        }
        lines.push(Line::raw(""));
        lines.extend(wrapped(
            "Misc conversation: no intent record is linked to this workspace. It joins the work board when a record claims it.",
            w,
            fg(DIM),
            None,
        ));
    }

    let want = lines.len() as u16 + 5;
    let rect = centered(area, box_w, want.min(area.height.saturating_sub(2)), None);
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let max_scroll = (lines.len() as u16).saturating_sub(body.height);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll.min(max_scroll), 0)),
        body,
    );
    let footer_y = (inner.y + inner.height).saturating_sub(1);
    let buf = f.buffer_mut();
    put(
        buf,
        inner.x,
        footer_y.saturating_sub(1),
        inner.width,
        vec![Span::styled("─".repeat(w), fg(BORDER))],
    );
    let mut hints = vec![("⏎", "jump in"), ("n N", "next that needs you")];
    if max_scroll > 0 {
        hints.push(("j k", "scroll"));
    }
    hints.push(("esc", "back"));
    put(
        buf,
        inner.x,
        footer_y,
        inner.width,
        fit(key_hints(&hints), w),
    );
}

// ─── Work commands ─────────────────────────────────────────────────────────────────────────

fn palette(f: &mut Frame, area: Rect) {
    let rect = centered(area, 68, 20, Some(4));
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let mut lines = vec![
        Line::from(Span::styled("COMMANDS", bold(CYAN))),
        Line::raw(""),
    ];
    for (key, label) in [
        ("n", "New intent"),
        ("d", "Dispatch selected work"),
        ("f", "Affirm: keep this work"),
        ("x", "Move to won’t do"),
        ("u", "Restore to backlog"),
        ("t", "Change owner / tenant"),
        ("p", "Change project"),
        ("a", "Provider: all / Claude / Codex / others"),
        ("w", "Show / hide won’t do"),
        ("b", "Backlog: review and reorder"),
        ("m", "Misc: conversations without an intent"),
        ("g", "Go to top"),
        ("?", "All keys"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!(" {key:<4}"), bold(CYAN)),
            Span::styled(label, fg(FG)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Press a key · Esc closes",
        fg(DIM),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn field_lines(value: &str, max_width: usize, max_lines: usize, active: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for ch in value.chars().chain(active.then_some('▏')) {
        if ch == '\n' {
            lines.push(std::mem::take(&mut line));
            continue;
        }
        let mut next = line.clone();
        next.push(ch);
        if width(&next) > max_width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        line.push(ch);
    }
    lines.push(line);
    if lines.len() > max_lines {
        if active {
            lines.drain(..lines.len() - max_lines);
        } else {
            lines.truncate(max_lines);
        }
    }
    lines
}

fn compose(f: &mut Frame, area: Rect, app: &App, form: &IntentForm) {
    let rect = centered(area, 94, 27, None);
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let target = app
        .machines
        .get(form.machine)
        .map(|m| m.name())
        .unwrap_or_else(|| "no machine".into());
    let fields = [
        ("Title", form.title.as_str()),
        ("Request", form.words.as_str()),
        ("Organization", form.organization.as_str()),
        ("Tenant", form.tenant.as_str()),
        ("Project", form.project.as_str()),
        ("Machine", target.as_str()),
        ("Session", form.session.as_str()),
    ];
    let mut lines = vec![
        Line::from(Span::styled("NEW INTENT", bold(CYAN))),
        Line::from(Span::styled(
            "Write the outcome you want. Leave Tenant empty for organization work.",
            fg(DIM),
        )),
        Line::raw(""),
    ];
    let mut focused_line = 0;
    for (index, (label, value)) in fields.iter().enumerate() {
        let active = form.field == index;
        if active {
            focused_line = lines.len();
        }
        let name = format!("{}{label:<13}", if active { "› " } else { "  " });
        let body = if value.is_empty() {
            match index {
                3 => "(organization work)",
                4 => "(optional)",
                _ => "…",
            }
        } else {
            value
        };
        let parts = field_lines(
            body,
            w.saturating_sub(15).max(1),
            if index == 1 { 3 } else { 1 },
            active,
        );
        for (row, part) in parts.into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    if row == 0 {
                        name.clone()
                    } else {
                        " ".repeat(15)
                    },
                    if active { bold(CYAN) } else { fg(DIM) },
                ),
                Span::styled(
                    part,
                    if active {
                        Style::default().fg(FG).bg(SEL)
                    } else {
                        fg(FG2)
                    },
                ),
            ]));
        }
        lines.push(Line::raw(""));
    }
    if form.field == 7 {
        focused_line = lines.len();
    }
    lines.push(Line::from(Span::styled(
        "  Create intent  ",
        if form.field == 7 {
            Style::default()
                .bg(CYAN)
                .fg(BG)
                .add_modifier(Modifier::BOLD)
        } else {
            bold(CYAN)
        },
    )));
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let scroll = focused_line.saturating_sub(body.height.saturating_sub(1) as usize) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    let hints = key_hints(&[
        ("Tab / Shift+Tab", "field"),
        ("← →", "machine"),
        ("Ctrl+Enter", "create"),
        ("Esc", "cancel"),
    ]);
    put(
        f.buffer_mut(),
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(hints, w),
    );
}

fn confirm(f: &mut Frame, area: Rect, title: &str, message: &str) {
    let rect = centered(area, 86, 16, None);
    dialog(f, rect, YELLOW);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let mut lines = wrapped(title, w, bold(YELLOW), None);
    lines.push(Line::raw(""));
    lines.extend(wrapped(message, w, fg(FG), None));
    lines.push(Line::raw(""));
    lines.extend(wrapped(
        "Starting more work takes another slot. At the maximum, finish or shelve one first.",
        w,
        fg(DIM),
        None,
    ));
    let body = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    f.render_widget(Paragraph::new(lines), body);
    put(
        f.buffer_mut(),
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(
            key_hints(&[("Enter / y", "start anyway"), ("Esc / n", "cancel")]),
            w,
        ),
    );
}

// ─── Search ─────────────────────────────────────────────────────────────────────────────────

fn search(f: &mut Frame, area: Rect, app: &App, s: &Search) {
    let rect = centered(area, 110, 34, Some(3));
    dialog(f, rect, CYAN);
    let inner = pad(rect, 2, 1);
    let hits = app.search_hits(s);
    let buf = f.buffer_mut();
    // The query.
    let count = if s.query.is_empty() {
        String::new()
    } else {
        format!("{} matches", hits.len())
    };
    put_lr(
        buf,
        inner,
        inner.y,
        vec![
            Span::styled("/ ", bold(CYAN)),
            Span::styled(s.query.clone(), bold(FG)),
            Span::styled("▏", fg(CYAN)),
            Span::styled(
                if s.query.is_empty() {
                    " cards, workspaces and sessions on every machine"
                } else {
                    ""
                },
                fg(DIM),
            ),
        ],
        vec![Span::styled(count, fg(DIM))],
    );
    // The filters.
    let mut chips = Vec::new();
    for filter in SearchFilter::ALL {
        let style = if filter == s.filter {
            Style::default()
                .bg(CYAN)
                .fg(BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(RAISED).fg(FG2)
        };
        chips.push(Span::styled(format!(" {} ", filter.label()), style));
        chips.push(Span::raw(" "));
    }
    let shown = app.machines.iter().filter(|m| m.is_shown()).count();
    let away: Vec<String> = app
        .machines
        .iter()
        .filter(|m| m.is_shown() && !m.conn.is_live())
        .map(|m| m.name())
        .collect();
    let scope_text = if away.is_empty() {
        format!("{shown} machines")
    } else {
        format!("{shown} machines; as last seen: {}", away.join(", "))
    };
    put_lr(
        buf,
        inner,
        inner.y + 2,
        chips,
        vec![Span::styled(trunc(&scope_text, 48), fg(DIM))],
    );

    let list = Rect {
        x: inner.x,
        y: inner.y + 4,
        width: inner.width,
        height: inner.height.saturating_sub(6),
    };
    let mut rows: Vec<(Option<usize>, Vec<Span<'static>>, Vec<Span<'static>>)> = Vec::new();
    let mut last_group = None;
    for (k, (hit, _)) in hits.iter().enumerate() {
        let group = match hit {
            Hit::Card(_) => "CARDS",
            Hit::Workspace(_) => "WORKSPACES",
            Hit::Session(_) => "SESSIONS",
        };
        if last_group != Some(group) {
            if last_group.is_some() {
                rows.push((None, vec![], vec![]));
            }
            let sub = match hit {
                Hit::Card(_) => "",
                Hit::Workspace(_) => "any herdr workspace, on a card or not",
                Hit::Session(_) => "every machine's sessions",
            };
            rows.push((
                None,
                vec![
                    Span::styled(group, Style::default().fg(FG2).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("   {sub}"), fg(DIM)),
                ],
                vec![],
            ));
            last_group = Some(group);
        }
        let (left, right) = hit_row(app, *hit, &s.query);
        rows.push((Some(k), left, right));
    }
    if hits.is_empty() && !s.query.is_empty() {
        rows.push((
            None,
            vec![Span::styled("Nothing matches.", fg(DIM))],
            vec![],
        ));
    }
    let sel_row = rows
        .iter()
        .position(|(k, _, _)| *k == Some(s.sel))
        .unwrap_or(0);
    let h = list.height as usize;
    let start = if sel_row >= h { sel_row + 1 - h } else { 0 };
    for (r, (k, left, right)) in rows.iter().enumerate().skip(start).take(h) {
        let y = list.y + (r - start) as u16;
        let line = Rect {
            x: list.x,
            y,
            width: list.width,
            height: 1,
        };
        if *k == Some(s.sel) {
            buf.set_style(line, Style::default().bg(SEL));
        }
        put_lr(buf, line, y, left.clone(), right.clone());
    }
    let footer_y = (inner.y + inner.height).saturating_sub(1);
    put(
        buf,
        inner.x,
        footer_y.saturating_sub(1),
        inner.width,
        vec![Span::styled("─".repeat(inner.width as usize), fg(BORDER))],
    );
    put(
        buf,
        inner.x,
        footer_y,
        inner.width,
        fit(
            key_hints(&[
                ("⏎", "jump in"),
                ("ctrl+o", "peek"),
                ("↑ ↓", "move"),
                ("tab", "narrow it"),
                ("esc", "back to the board"),
            ]),
            inner.width as usize,
        ),
    );
}

fn highlight(text: &str, query: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    match match_range(query, text).filter(|&(a, b)| a <= b && b <= chars.len()) {
        Some((a, b)) => {
            vec![
                Span::styled(chars[..a].iter().collect::<String>(), base),
                Span::styled(chars[a..b].iter().collect::<String>(), bold(CYAN)),
                Span::styled(chars[b..].iter().collect::<String>(), base),
            ]
        }
        None => vec![Span::styled(text.to_string(), base)],
    }
}

fn hit_row(app: &App, hit: Hit, query: &str) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    match hit {
        Hit::Card(i) => {
            let c = &app.board.cards[i];
            let color = attention_color(c.attention);
            let mut left = vec![
                Span::styled(format!("{} ", c.attention.glyph()), fg(color)),
                Span::styled(format!("{:<8}", trunc(&c.reference, 8)), bold(FG)),
            ];
            left.extend(highlight(&c.title, query, fg(FG)));
            let mut meta = vec![c.stage.id().to_string()];
            if let Some(s) = &c.session {
                meta.push(s.clone());
            }
            meta.push(c.attention.label().to_string());
            (left, vec![Span::styled(meta.join(" · "), fg(color))])
        }
        Hit::Workspace(i) => {
            let w = &app.board.workspaces[i];
            let color = match w.status.as_str() {
                "working" => BLUE,
                "blocked" | "done" => ORANGE,
                _ => DIM,
            };
            let mut left = vec![
                Span::styled("○ ", fg(color)),
                Span::styled(format!("{:<8}", ""), fg(DIM)),
            ];
            left.extend(highlight(
                &format!("{} › {}", w.session, w.label),
                query,
                fg(FG),
            ));
            let mut right = vec![Span::styled(
                format!("{} · {}", w.machine, w.status),
                fg(DIM),
            )];
            if let Some(n) = &w.note {
                right.push(Span::styled(format!(" · {n}"), fg(YELLOW)));
            }
            (left, right)
        }
        Hit::Session(i) => {
            let r: &SessionRow = &app.board.sessions[i];
            let (g, color) = if r.running {
                ("●", GREEN)
            } else {
                ("◦", DIM)
            };
            let mut left = vec![
                Span::styled(format!("{g} "), fg(color)),
                Span::styled(format!("{:<8}", ""), fg(DIM)),
            ];
            left.extend(highlight(
                &format!("{} on {}", r.session, r.machine),
                query,
                fg(FG),
            ));
            let state = if r.running {
                match r.slots.limit {
                    Some(l) => format!("running · {} of {l} slots", r.slots.used),
                    None => format!("running · {} agents", r.slots.used),
                }
            } else {
                "not running".to_string()
            };
            (left, vec![Span::styled(state, fg(DIM))])
        }
    }
}

// ─── Keys ───────────────────────────────────────────────────────────────────────────────────

/// A key, what it does, and what else does the same.
pub type KeyRow = (&'static str, &'static str, &'static str);

pub const BOARD_KEYS: &[(&str, &[KeyRow])] = &[
    (
        "WORK COMMANDS",
        &[
            ("Ctrl+Space", "open commands, then press a key below", ""),
            ("leader n", "compose a new intent", "Ctrl+Enter creates"),
            (
                "a / leader a",
                "filter by reported agent provider",
                "all / Claude / Codex / others",
            ),
            ("leader d", "dispatch selected work", "d in backlog"),
            (
                "leader f",
                "affirm: still want selected work",
                "y in backlog",
            ),
            ("leader x/u", "move to won’t do / restore", "x/u in backlog"),
            ("leader t/p", "cycle owner or project filter", ""),
            ("w", "toggle won’t-do view", "leader w too"),
            (
                "10K / 10J",
                "move backlog item ten positions",
                "K/J move one",
            ),
        ],
    ),
    (
        "CARDS",
        &[
            ("h j k l", "move between columns and cards", "arrows too"),
            (
                "⏎",
                "jump in: herdr, full screen, on the card's workspace",
                "",
            ),
            (
                "m",
                "peek: its stage, what it needs, its last update",
                "space too",
            ),
            (
                "n  N",
                "next / previous card that needs you",
                "ctrl+alt+j/k too",
            ),
            ("/", "search the current machine and work filters", ""),
            ("b", "the backlog", ""),
            ("i", "misc: conversations without an intent", "leader m too"),
        ],
    ),
    (
        "SESSIONS AND MACHINES",
        &[
            (
                "[  ]",
                "previous / next session of chosen machine",
                "ctrl+alt+h/l, tab too",
            ),
            ("{  }", "previous / next machine", "ctrl+alt+shift+h/l too"),
            ("0", "all-machine overview", ""),
            ("1..9", "session tab 1 to 9", ""),
            ("M", "machines: every connection and its state", ""),
        ],
    ),
    (
        "WHEN A MACHINE IS IN TROUBLE",
        &[
            ("r", "retry its machine now", "skips the wait"),
            ("R", "retry every machine now", ""),
            (
                "M ⏎",
                "answer SSH's prompt for a machine that needs you",
                "",
            ),
            ("M u", "run herdr's own setup there, which asks first", ""),
            ("M x", "hide a machine, or show it again", ""),
        ],
    ),
    (
        "KERDR",
        &[
            ("?", "this menu", ""),
            ("q", "quit Kerdr; every session keeps running", ""),
        ],
    ),
];

/// Keys inside herdr, after herdr's prefix.
pub const HERDR_KEYS: &[KeyRow] = &[
    ("q", "back to the board", "herdr's own detach"),
    ("space", "next card that needs you, without the board", ""),
    ("backspace", "back to the board, as listed in herdr", ""),
    (
        "?",
        "herdr's keybinds menu, with Kerdr's two under custom",
        "",
    ),
];

fn keys(f: &mut Frame, area: Rect, app: &App, filter: &str, typing: bool, scroll: u16) {
    let rect = centered(area, 132, 42, None);
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let q = filter.to_lowercase();
    let matches = |k: &str, d: &str, a: &str| {
        q.is_empty() || format!("{k} {d} {a}").to_lowercase().contains(&q)
    };
    let buf = f.buffer_mut();
    put_lr(
        buf,
        inner,
        inner.y,
        vec![
            Span::styled("kerdr keys", bold(CYAN)),
            Span::styled(
                "   Ctrl+Space opens work commands. Inside herdr, use herdr’s own prefix.",
                fg(DIM),
            ),
        ],
        vec![Span::styled("esc close", fg(DIM))],
    );
    let prompt = if typing || !filter.is_empty() {
        vec![
            Span::styled("/ ", bold(CYAN)),
            Span::styled(filter.to_string(), bold(FG)),
            Span::styled(if typing { "▏" } else { "" }, fg(CYAN)),
        ]
    } else {
        vec![
            Span::styled("/ ", bold(CYAN)),
            Span::styled("press / to filter by what it does or by its key", fg(DIM)),
        ]
    };
    put(buf, inner.x, inner.y + 2, inner.width, prompt);

    let col_w = (inner.width.saturating_sub(4)) / 2;
    let left = Rect {
        x: inner.x,
        y: inner.y + 4,
        width: col_w,
        height: inner.height.saturating_sub(6),
    };
    let right = Rect {
        x: inner.x + col_w + 4,
        y: inner.y + 4,
        width: col_w,
        height: inner.height.saturating_sub(6),
    };
    let entry = |k: &str, d: &str, a: &str, key_w: usize, w: u16| -> Vec<Vec<Span<'static>>> {
        let text = if a.is_empty() {
            d.to_string()
        } else {
            format!("{d} · {a}")
        };
        wrap(&text, (w as usize).saturating_sub(key_w), 3)
            .into_iter()
            .enumerate()
            .map(|(n, l)| {
                let key = if n == 0 {
                    format!("{k:<key_w$}")
                } else {
                    " ".repeat(key_w)
                };
                vec![Span::styled(key, bold(FG)), Span::styled(l, fg(FG2))]
            })
            .collect()
    };
    let heading = |s: &str| {
        vec![Span::styled(
            s.to_string(),
            Style::default().fg(FG2).add_modifier(Modifier::BOLD),
        )]
    };

    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    for (group, items) in BOARD_KEYS {
        let shown: Vec<_> = items.iter().filter(|(k, d, a)| matches(k, d, a)).collect();
        if shown.is_empty() {
            continue;
        }
        lines.push(heading(group));
        for (k, d, a) in shown {
            lines.extend(entry(k, d, a, 13, left.width));
        }
        lines.push(vec![]);
    }
    for (r, spans) in lines
        .iter()
        .skip(scroll as usize)
        .take(left.height as usize)
        .enumerate()
    {
        put(buf, left.x, left.y + r as u16, left.width, spans.clone());
    }

    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let herdr_rows: Vec<(String, &str, &str)> = HERDR_KEYS
        .iter()
        .map(|(k, d, a)| (format!("{} {k}", app.prefix), *d, *a))
        .filter(|(k, d, a)| matches(k, d, a))
        .collect();
    if !herdr_rows.is_empty() {
        out.push(heading("INSIDE HERDR"));
        for (k, d, a) in &herdr_rows {
            out.extend(entry(k, d, a, 22, right.width));
        }
        out.push(vec![]);
        for l in wrap(
            "Everything else inside is herdr's, unchanged: tabs, panes, agents. The space and backspace keys are Kerdr's: kerdr herdr-keys --install adds them to herdr's config on each machine, so they work over SSH too.",
            right.width as usize,
            4,
        ) {
            out.push(vec![Span::styled(l, fg(DIM))]);
        }
    }
    if q.is_empty() {
        out.push(vec![]);
        out.push(heading("WORK COMMANDS"));
        for l in wrap(
            "Press Ctrl+Space, then a command key, to create, dispatch or review work and change filters. Movement and search still work directly. Inside herdr, its prefix brings you back to Kerdr.",
            right.width as usize,
            6,
        ) {
            out.push(vec![Span::styled(l, fg(DIM))]);
        }
        out.push(vec![]);
        out.push(heading("WHILE A MACHINE RECONNECTS"));
        out.extend(entry(
            "⏎",
            "waits for the machine, then jumps in",
            "",
            22,
            right.width,
        ));
        out.extend(entry("esc", "stops waiting", "", 22, right.width));
    }
    for (r, spans) in out.iter().take(right.height as usize).enumerate() {
        put(buf, right.x, right.y + r as u16, right.width, spans.clone());
    }
    let footer_y = (inner.y + inner.height).saturating_sub(1);
    put(
        buf,
        inner.x,
        footer_y,
        inner.width,
        key_hints(&[("/", "search"), ("j k", "scroll"), ("esc or ?", "close")]),
    );
}

// ─── Machines ───────────────────────────────────────────────────────────────────────────────

fn conn_count(app: &App) -> Vec<Span<'static>> {
    let mut live = 0;
    let mut reconnecting = 0;
    let mut needs = 0;
    let mut old = 0;
    let mut away = 0;
    for m in &app.machines {
        match m.conn {
            Conn::Live { .. } => live += 1,
            Conn::Reconnecting { .. } | Conn::Connecting => reconnecting += 1,
            Conn::HostKey { .. } | Conn::Auth { .. } => needs += 1,
            Conn::HerdrMissing | Conn::HerdrTooOld { .. } => old += 1,
            Conn::Unreachable { .. } => away += 1,
            Conn::Hidden => {}
        }
    }
    let n = app.machines.len();
    let mut spans = vec![Span::styled(
        format!("{n} machine{}: ", if n == 1 { "" } else { "s" }),
        fg(DIM),
    )];
    let mut parts: Vec<Span<'static>> = Vec::new();
    for (count, text, color) in [
        (live, "live", GREEN),
        (reconnecting, "reconnecting", YELLOW),
        (needs, "needs you", ORANGE),
        (old, "needs herdr set up", RED),
        (away, "can't reach", DIM),
    ] {
        if count > 0 {
            if !parts.is_empty() {
                parts.push(Span::styled(", ", fg(DIM)));
            }
            parts.push(Span::styled(format!("{count} {text}"), fg(color)));
        }
    }
    spans.extend(parts);
    spans
}

fn machine_explain(app: &App, i: usize) -> Vec<String> {
    let m = &app.machines[i];
    let now = app.now();
    let as_of = m.last_ok.map(clock);
    match &m.conn {
        Conn::Live { latency } => vec![
            format!(
                "Answering. Read every {}s{}; the last read took {} ms.",
                if m.machine.is_local() { app.cfg.machines.local_every } else { app.cfg.machines.remote_every },
                if m.machine.is_local() { "" } else { " over SSH" },
                latency.as_millis()
            ),
            match (&m.machine.feed, &m.feed_error) {
                (Some(_), Some(e)) => format!("Its card feed failed ({e}), so it shows the last cards it sent, with herdr's live view."),
                (Some(cmd), None) => format!("Cards come from its feed: {cmd}"),
                (None, _) => "No card feed: every herdr workspace with an agent is a card.".into(),
            },
        ],
        Conn::Connecting => vec!["Reaching it for the first time.".into()],
        Conn::Reconnecting { since, attempt, retry_at, detail } => vec![
            format!(
                "Lost the connection {} ago ({}). Trying again in {}s (try {}). The wait grows to two minutes, like herdr's own reconnects.",
                ago(*since, now),
                detail,
                (*retry_at - now).num_seconds().max(0),
                attempt + 1
            ),
            format!(
                "Its cards stay on the board, dimmed{}. ⏎ on one waits for the connection, then jumps in.",
                as_of.map(|t| format!(", as they were at {t}")).unwrap_or_default()
            ),
        ],
        Conn::Unreachable { since, retry_at, detail } => vec![
            format!("No answer since {} ({detail}): asleep, off, or off the network.", clock(*since)),
            format!("Kerdr tries again in {}s, then every two minutes; r tries now.", (*retry_at - now).num_seconds().max(0)),
        ],
        Conn::HostKey { detail } => vec![
            format!("SSH asked whether to trust this machine's host key ({detail})."),
            "Kerdr never answers that for you: ⏎ runs ssh here, so you can check the key and answer it yourself.".into(),
        ],
        Conn::Auth { detail } => vec![
            format!("SSH wants a password, a passphrase or another key ({detail})."),
            "⏎ runs ssh here so you can answer it. For a key with a passphrase, ssh-add keeps it answered.".into(),
        ],
        Conn::HerdrMissing => vec![
            "herdr isn't installed there, or isn't on the PATH an SSH command gets.".into(),
            "u runs herdr's own setup over SSH (herdr --remote), which asks before installing anything.".into(),
        ],
        Conn::HerdrTooOld { version } => vec![
            format!(
                "herdr {version} there is older than Kerdr needs ({}).",
                herdr::version_string(herdr::MIN_VERSION)
            ),
            "u runs herdr's own setup, which asks before it updates or restarts anything.".into(),
        ],
        Conn::Hidden => vec!["You hid it with x. Kerdr doesn't read it until x shows it again.".into()],
    }
}

const LEGEND: &[(&str, ratatui::style::Color, &str, &str)] = &[
    (
        "●",
        GREEN,
        "live",
        "Answering. Read every 2s here, every 4s over SSH.",
    ),
    (
        "◌",
        BLUE,
        "connecting",
        "First contact. Sessions fill in as each one answers.",
    ),
    (
        "⟳",
        YELLOW,
        "reconnecting",
        "Dropped. Retries with longer waits, up to two minutes. Cards stay, dimmed.",
    ),
    (
        "○",
        DIM,
        "can't reach",
        "No answer for two minutes: asleep, off, or off the network.",
    ),
    (
        "●",
        ORANGE,
        "needs you",
        "SSH wants a password, a passphrase or a host key. ⏎ asks you here.",
    ),
    (
        "■",
        RED,
        "herdr too old",
        "Missing, or older than 0.9.1. u runs herdr's own setup, asking first.",
    ),
    (
        "⊘",
        FAINT,
        "hidden",
        "You hid it with x. It isn't read until x again.",
    ),
];

fn machines_view(f: &mut Frame, area: Rect, app: &App, sel: usize) {
    let inner = pad(area, 1, 0);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let now = app.now();
    {
        let buf = f.buffer_mut();
        let mut left = vec![
            Span::styled(" kerdr ", bold(CYAN)),
            Span::styled("› ", fg(DIM)),
            Span::styled("machines   ", bold(FG)),
        ];
        left.extend(conn_count(app));
        put_lr(
            buf,
            rows[0],
            rows[0].y,
            left,
            vec![
                Span::styled("this machine: ", fg(DIM)),
                Span::styled(app.this_machine(), fg(FG)),
                Span::styled(format!(" · {}", clock(now)), fg(DIM)),
            ],
        );
    }
    let main = rows[2];
    let detail_w = (main.width * 2 / 5).clamp(40, 64);
    let list = Rect {
        width: main.width.saturating_sub(detail_w + 2),
        ..main
    };
    let detail = Rect {
        x: main.x + list.width + 2,
        width: detail_w,
        ..main
    };
    let order = app.machine_rows();
    let sel = sel.min(order.len().saturating_sub(1));

    let mut y = list.y;
    for (k, &i) in order.iter().enumerate() {
        let m = &app.machines[i];
        let sessions: Vec<&SessionRow> = app
            .board
            .sessions
            .iter()
            .filter(|r| r.machine == m.name())
            .collect();
        let h = if sessions.is_empty() { 4 } else { 5 };
        if y + h > list.y + list.height {
            break;
        }
        let rect = Rect {
            x: list.x,
            y,
            width: list.width,
            height: h,
        };
        let is_sel = k == sel;
        let kind = if is_sel {
            BorderType::Thick
        } else {
            BorderType::Rounded
        };
        panel(
            f,
            rect,
            if is_sel { CYAN } else { BORDER },
            if is_sel { SEL } else { PANEL },
            kind,
        );
        let inner = pad(rect, 2, 1);
        let color = conn_color(&m.conn);
        let how = match (
            &m.machine.ssh,
            m.data.as_ref().and_then(|d| d.version.clone()),
        ) {
            (None, Some(v)) => format!("this machine · herdr {v}"),
            (None, None) => "this machine".into(),
            (Some(s), Some(v)) => format!("ssh {s} · herdr {v}"),
            (Some(s), None) => format!("ssh {s}"),
        };
        let buf = f.buffer_mut();
        put_lr(
            buf,
            inner,
            inner.y,
            vec![
                Span::styled(format!("{} ", m.conn.glyph()), bold(color)),
                Span::styled(m.name(), bold(FG)),
                Span::styled(format!("   {how}"), fg(DIM)),
            ],
            vec![Span::styled(m.conn.short(now), bold(color))],
        );
        let line = match &m.conn {
            Conn::Live { .. } => {
                let cards = app
                    .board
                    .cards
                    .iter()
                    .filter(|c| {
                        c.machine == m.name()
                            && !matches!(c.stage, Stage::Backlog | Stage::Maintain | Stage::Misc)
                    })
                    .count();
                format!(
                    "{} session{}, {cards} card{} under way.",
                    sessions.len(),
                    if sessions.len() == 1 { "" } else { "s" },
                    if cards == 1 { "" } else { "s" }
                )
            }
            _ => machine_explain(app, i)
                .into_iter()
                .next()
                .unwrap_or_default(),
        };
        put(
            buf,
            inner.x,
            inner.y + 1,
            inner.width,
            vec![Span::styled(trunc(&line, inner.width as usize), fg(FG2))],
        );
        if !sessions.is_empty() {
            let mut chips = Vec::new();
            for r in sessions {
                let (g, gc) = if !r.running {
                    ("◦", DIM)
                } else if r.cached {
                    ("●", DIM)
                } else {
                    ("●", GREEN)
                };
                chips.push(Span::styled(
                    format!(" {g} "),
                    Style::default().bg(RAISED).fg(gc),
                ));
                chips.push(Span::styled(
                    r.session.clone(),
                    Style::default()
                        .bg(RAISED)
                        .fg(if r.cached { DIM } else { FG }),
                ));
                let slots = if r.running {
                    format!(" {}", r.slots.label())
                } else {
                    " not running".into()
                };
                chips.push(Span::styled(slots, Style::default().bg(RAISED).fg(DIM)));
                if r.needs > 0 {
                    chips.push(Span::styled(
                        format!(" ●{}", r.needs),
                        Style::default()
                            .bg(RAISED)
                            .fg(ORANGE)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                chips.push(Span::styled(" ", Style::default().bg(RAISED)));
                chips.push(Span::raw(" "));
            }
            put(
                buf,
                inner.x,
                inner.y + 2,
                inner.width,
                fit(chips, inner.width as usize),
            );
        }
        y += h + 1;
    }

    // The selected machine, and every state a machine can be in.
    if let Some(&i) = order.get(sel) {
        let m = &app.machines[i];
        dialog(f, detail, CYAN);
        let d = pad(detail, 2, 1);
        let w = d.width as usize;
        let color = conn_color(&m.conn);
        let mut lines: Vec<Line> = vec![Line::from(vec![
            Span::styled(m.name(), bold(FG)),
            Span::raw("   "),
            Span::styled(
                format!("{} {}", m.conn.glyph(), m.conn.short(now)),
                bold(color),
            ),
        ])];
        let mut facts = Vec::new();
        facts.push(match &m.machine.ssh {
            Some(s) => format!("ssh {s}"),
            None => "this machine".into(),
        });
        if let Some(v) = m.data.as_ref().and_then(|d| d.version.clone()) {
            facts.push(format!("herdr {v}"));
        }
        if let Some(data) = &m.data {
            facts.push(format!("{} sessions", data.sessions.len()));
        }
        lines.extend(wrapped(&facts.join(" · "), w, fg(DIM), None));
        lines.push(Line::raw(""));
        for p in machine_explain(app, i) {
            lines.extend(wrapped(&p, w, fg(FG), None));
            lines.push(Line::raw(""));
        }
        let mut hints: Vec<(&str, &str)> = vec![("r", "retry now")];
        match &m.conn {
            Conn::HostKey { .. } | Conn::Auth { .. } => hints.insert(0, ("⏎", "answer it here")),
            Conn::HerdrMissing | Conn::HerdrTooOld { .. } => hints.insert(0, ("u", "set up herdr")),
            Conn::Hidden => hints = vec![("x", "show it again")],
            _ => hints.insert(0, ("⏎", "its board")),
        }
        if !m.machine.is_local() && !matches!(m.conn, Conn::Hidden) {
            hints.push(("x", "hide"));
        }
        lines.push(Line::from(fit(key_hints(&hints), w)));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("─".repeat(w), fg(BORDER))));
        lines.push(section("EVERY STATE A MACHINE CAN BE IN"));
        for (g, c, name, what) in LEGEND {
            let what = if *name == "live" {
                format!(
                    "Answering. Read every {}s here, every {}s over SSH.",
                    app.cfg.machines.local_every, app.cfg.machines.remote_every
                )
            } else {
                what.to_string()
            };
            let what = what.as_str();
            let first = format!("{:<14}", name);
            for (k, l) in wrap(what, w.saturating_sub(18), 2).into_iter().enumerate() {
                if k == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{g} "), bold(*c)),
                        Span::styled(first.clone(), bold(FG)),
                        Span::styled(l, fg(FG2)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(16)),
                        Span::styled(l, fg(FG2)),
                    ]));
                }
            }
        }
        lines.push(Line::raw(""));
        lines.extend(wrapped(
            "Sessions: ● running · ◦ not running · a dimmed ● is as last seen.",
            w,
            fg(DIM),
            None,
        ));
        f.render_widget(Paragraph::new(lines), d);
    }

    let buf = f.buffer_mut();
    put(
        buf,
        rows[4].x,
        rows[4].y,
        rows[4].width,
        fit(
            key_hints(&[
                ("j k", "move"),
                ("⏎", "its board, or answer its prompt"),
                ("r", "retry now"),
                ("R", "retry all"),
                ("u", "set up herdr there"),
                ("x", "hide or show"),
                ("esc", "back to the board"),
            ]),
            rows[4].width as usize,
        ),
    );
    if let Some((text, _)) = app.message() {
        put(
            buf,
            rows[3].x,
            rows[3].y,
            rows[3].width,
            vec![Span::styled(text.to_string(), fg(FG2))],
        );
    }
}

// ─── Backlog ────────────────────────────────────────────────────────────────────────────────

fn misc_view(f: &mut Frame, area: Rect, app: &App, sel: usize) {
    let inner = pad(area, 1, 0);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    header(f.buffer_mut(), rows[0], app);
    filters(f.buffer_mut(), rows[1], app);
    strip(f, rows[2], app);
    let items = app.misc_cards();
    put(
        f.buffer_mut(),
        rows[3].x,
        rows[3].y,
        rows[3].width,
        vec![
            Span::styled(format!("MISC  {}", items.len()), bold(CYAN)),
            Span::styled("   Conversations without an intent", fg(FG2)),
        ],
    );
    let selected = sel.min(items.len().saturating_sub(1));
    let h = rows[4].height as usize / 2;
    let start = selected.saturating_sub(h.saturating_sub(1));
    if items.is_empty() {
        put(
            f.buffer_mut(),
            rows[4].x,
            rows[4].y,
            rows[4].width,
            vec![Span::styled(
                "No miscellaneous conversations match these filters.",
                fg(DIM),
            )],
        );
    }
    for (row, &i) in items.iter().enumerate().skip(start).take(h) {
        let c = &app.board.cards[i];
        let y = rows[4].y + ((row - start) * 2) as u16;
        let rect = Rect {
            x: rows[4].x,
            y,
            width: rows[4].width,
            height: 2,
        };
        if row == selected {
            f.buffer_mut().set_style(rect, Style::default().bg(SEL));
        }
        let color = attention_color(c.attention);
        put_lr(
            f.buffer_mut(),
            rect,
            y,
            vec![
                Span::styled(format!(" {} ", c.attention.glyph()), fg(color)),
                Span::styled(c.title.clone(), bold(FG)),
            ],
            vec![Span::styled(c.attention.label(), fg(color))],
        );
        let mut place = format!(
            "   {} › {} › {}",
            c.machine,
            c.session.as_deref().unwrap_or(""),
            c.workspace_label.as_deref().unwrap_or("")
        );
        if let Some(at) = c.cached_at {
            place.push_str(&format!(" · cached {}", clock(at)));
        }
        put(
            f.buffer_mut(),
            rect.x,
            y + 1,
            rect.width,
            vec![Span::styled(place, fg(DIM))],
        );
    }
    put(
        f.buffer_mut(),
        rows[5].x,
        rows[5].y,
        rows[5].width,
        fit(
            key_hints(&[
                ("⏎", "jump in"),
                ("j k", "select"),
                ("m", "peek"),
                ("[ ]", "session"),
                ("{ }", "machine"),
                ("Esc", "board"),
            ]),
            rows[5].width as usize,
        ),
    );
}

fn backlog_view(f: &mut Frame, area: Rect, app: &App, sel: usize) {
    let inner = pad(area, 1, 0);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let items = app.backlog_cards();
    let s = app.summary();
    let now = app.now();
    {
        let buf = f.buffer_mut();
        let free: Vec<String> = app
            .board
            .sessions
            .iter()
            .filter(|r| app.scope().contains_session(r) && r.running)
            .filter_map(|r| {
                r.slots
                    .free()
                    .filter(|&n| n > 0)
                    .map(|n| format!("{} {n}", r.session))
            })
            .collect();
        let mut right = Vec::new();
        if !free.is_empty() {
            right.push(Span::styled("slots free: ", fg(DIM)));
            right.push(Span::styled(free.join(", "), fg(GREEN)));
        }
        if !s.full_sessions.is_empty() {
            right.push(Span::styled(
                format!("   {} full", s.full_sessions.join(" and ")),
                fg(ORANGE),
            ));
        }
        put_lr(
            buf,
            rows[0],
            rows[0].y,
            vec![
                Span::styled(" kerdr ", bold(CYAN)),
                Span::styled("› ", fg(DIM)),
                Span::styled(
                    if app.wont_do {
                        "won’t do   "
                    } else {
                        "backlog   "
                    },
                    bold(FG),
                ),
                Span::styled(
                    if app.wont_do {
                        format!("{} set aside · u restores to backlog", items.len())
                    } else {
                        format!(
                            "{} waiting to start: {} queued, {} shelved",
                            s.backlog, s.queued, s.shelved
                        )
                    },
                    fg(DIM),
                ),
            ],
            right,
        );
    }
    filters(f.buffer_mut(), rows[1], app);
    let main = rows[2];
    let detail_w = (main.width * 2 / 5).clamp(40, 64);
    let list = Rect {
        width: main.width.saturating_sub(detail_w + 2),
        ..main
    };
    let detail = Rect {
        x: main.x + list.width + 2,
        width: detail_w,
        ..main
    };
    let sel = sel.min(items.len().saturating_sub(1));

    let mut lines: Vec<(Option<usize>, Vec<Span<'static>>, Vec<Span<'static>>)> = vec![(
        None,
        vec![Span::styled(
            if app.wont_do {
                "WON’T DO"
            } else {
                "YOUR ORDER"
            },
            bold(FG2),
        )],
        vec![Span::styled("LAST AFFIRMED", fg(DIM))],
    )];
    for (k, &i) in items.iter().enumerate() {
        let c = &app.board.cards[i];
        let (glyph, gc, _, _) = list_glyph(c, now);
        let age = c
            .stale_days
            .map(|days| {
                if days == 0 {
                    "today".into()
                } else {
                    format!("{days}d")
                }
            })
            .unwrap_or_else(|| "unknown".into());
        let age = if !c.dismissed() && c.stale_days.unwrap_or(0) >= 14 {
            format!("{age} · review")
        } else {
            age
        };
        lines.push((
            Some(k),
            vec![
                Span::styled(format!("{:>3} {glyph} ", k + 1), fg(gc)),
                Span::styled(format!("{:<6}", c.reference), bold(FG)),
                Span::styled(c.title.clone(), fg(FG)),
            ],
            vec![Span::styled(
                age,
                fg(if c.stale_days.unwrap_or(0) >= 14 {
                    YELLOW
                } else {
                    DIM
                }),
            )],
        ));
    }
    if items.is_empty() {
        lines.push((
            None,
            vec![Span::styled(
                if app.wont_do {
                    "No work set aside here."
                } else {
                    "Nothing waiting to start here."
                },
                fg(DIM),
            )],
            vec![],
        ));
    }
    let sel_row = lines
        .iter()
        .position(|(k, _, _)| *k == Some(sel))
        .unwrap_or(0);
    let h = list.height as usize;
    let start = if sel_row >= h { sel_row + 1 - h } else { 0 };
    {
        let buf = f.buffer_mut();
        for (r, (k, left, right)) in lines.iter().enumerate().skip(start).take(h) {
            let y = list.y + (r - start) as u16;
            let line = Rect {
                x: list.x,
                y,
                width: list.width,
                height: 1,
            };
            if *k == Some(sel) {
                buf.set_style(line, Style::default().bg(SEL));
            }
            put_lr(buf, line, y, left.clone(), right.clone());
        }
    }

    if let Some(&i) = items.get(sel) {
        let c = &app.board.cards[i];
        dialog(f, detail, CYAN);
        let d = pad(detail, 2, 1);
        let w = d.width as usize;
        let mut out: Vec<Line> = Vec::new();
        let stale = c
            .stale_days
            .filter(|&n| n >= 14)
            .map(|n| format!("last affirmed {n}d ago"))
            .unwrap_or_default();
        out.push(Line::from(vec![
            Span::styled(c.reference.clone(), bold(FG)),
            Span::raw(" ".repeat(w.saturating_sub(width(&c.reference) + width(&stale)))),
            Span::styled(stale, fg(YELLOW)),
        ]));
        out.extend(wrapped(&c.title, w, bold(FG), None));
        out.push(Line::raw(""));
        let state = if c.dismissed() {
            "set aside".into()
        } else {
            c.state.clone().unwrap_or_else(|| "queued".into())
        };
        let place = match &c.session {
            Some(s) => format!("{state} in {s} on {}", c.machine),
            None => format!("{state} on {}", c.machine),
        };
        out.extend(wrapped(&place, w, fg(FG2), None));
        out.extend(wrapped(
            &format!(
                "{} / {}",
                c.owner_label(),
                if c.project().is_empty() {
                    "unassigned"
                } else {
                    c.project()
                }
            ),
            w,
            fg(CYAN),
            None,
        ));
        out.extend(wrapped(
            &format!(
                "last affirmed: {}",
                c.detail
                    .as_ref()
                    .and_then(|d| d.refreshed_at.as_deref().or(d.created.as_deref()))
                    .unwrap_or("unknown")
            ),
            w,
            fg(DIM),
            None,
        ));
        if let Some(d) = &c.detail {
            for wo in d.waits_on.iter().filter(|w| w.open != Some(false)) {
                out.extend(wrapped(
                    &format!(
                        "waits on {} {}",
                        wo.reference.clone().unwrap_or_default(),
                        wo.title.clone().unwrap_or_default()
                    ),
                    w,
                    fg(BLUE),
                    None,
                ));
            }
            if let Some(story) = &d.story {
                out.push(Line::raw(""));
                out.extend(wrapped(story, w, fg(FG2), None));
            }
        }
        if !c.dismissed()
            && c.stale_days.unwrap_or(0) >= 14
            && c.state.as_deref() != Some("shelved")
        {
            out.push(Line::raw(""));
            out.push(Line::from(vec![
                Span::styled("▌ ", fg(YELLOW)),
                Span::styled("Still want this?", bold(YELLOW)),
            ]));
            let stale_n = items
                .iter()
                .filter(|&&j| app.board.cards[j].stale_days.unwrap_or(0) >= 14)
                .count();
            out.extend(wrapped(
                &format!("{stale_n} items need review. Affirm the work you still want, or move it to won’t do. You can restore it later."),
                w,
                fg(FG),
                Some(("▌ ", YELLOW)),
            ));
        }
        out.push(Line::raw(""));
        out.push(section(if c.dismissed() {
            "RESTORE TO BACKLOG"
        } else {
            "STARTING IT"
        }));
        let home = c.session.as_deref().and_then(|s| {
            app.board
                .sessions
                .iter()
                .find(|r| r.session == s && r.machine == c.machine)
        });
        let advice = if c.dismissed() {
            "Press u to restore this work to the queued backlog and affirm it again. It starts only when you dispatch it.".into()
        } else {
            match home {
                Some(r) if r.slots.is_full() => {
                    let other = app
                        .board
                        .sessions
                        .iter()
                        .filter(|o| o.running && !o.slots.is_full() && o.slots.limit.is_some())
                        .max_by_key(|o| o.slots.free().unwrap_or(0));
                    match other {
                        Some(o) => format!(
                            "{} is at its usual limit of {}. Dispatch asks before opening another slot. Or use {}, which has {} free.",
                            r.session,
                            r.slots.limit.unwrap_or(0),
                            o.session,
                            o.slots.free().unwrap_or(0)
                        ),
                        None => format!(
                            "{} is at its usual limit. Dispatch asks before opening another slot. At the maximum, finish or shelve one first.",
                            r.session
                        ),
                    }
                }
                Some(r) => match r.slots.free() {
                    Some(n) => format!("{} has room: {n} free.", r.session),
                    None => format!("{} has no limit.", r.session),
                },
                None => "Its session isn't running, or isn't on a machine Kerdr can see.".into(),
            }
        };
        out.extend(wrapped(&advice, w, fg(FG), None));
        f.render_widget(Paragraph::new(out), d);
    }

    let buf = f.buffer_mut();
    put(
        buf,
        rows[4].x,
        rows[4].y,
        rows[4].width,
        fit(
            key_hints(&[
                ("j k", "move"),
                ("10K / 10J", "reorder"),
                ("y", "affirm"),
                ("d", "dispatch"),
                ("x", "won’t do"),
                ("u", "restore"),
                ("w", "view won’t do"),
                ("⏎", "peek"),
                ("esc", "board"),
            ]),
            rows[4].width as usize,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Search, SearchFilter, Selection};
    use crate::config::Config;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn demo(away: bool) -> App {
        let mut a = App::new(Config::default(), crate::demo::machines(away));
        a.demo = true;
        a.fixed_now = Some(crate::demo::now());
        a.rebuild();
        a
    }

    fn modes(a: &App) -> Vec<Mode> {
        vec![
            Mode::Board,
            Mode::Peek {
                scroll: 0,
                back: Box::new(Mode::Board),
            },
            Mode::Peek {
                scroll: 40,
                back: Box::new(Mode::Backlog { sel: 0 }),
            },
            Mode::Search(Search {
                query: "e".into(),
                filter: SearchFilter::Everything,
                sel: 3,
            }),
            Mode::Search(Search {
                query: String::new(),
                filter: SearchFilter::DoneToo,
                sel: 0,
            }),
            Mode::Keys {
                filter: String::new(),
                typing: false,
                scroll: 0,
            },
            Mode::Keys {
                filter: "machine".into(),
                typing: true,
                scroll: 9,
            },
            Mode::Machines {
                sel: a.machines.len() + 3,
            },
            Mode::Backlog { sel: 99 },
            Mode::Misc { sel: 99 },
            Mode::Palette {
                back: Box::new(Mode::Backlog { sel: 0 }),
            },
            Mode::Compose {
                form: IntentForm {
                    title: "New useful work".into(),
                    words: "A clear outcome".into(),
                    field: 7,
                    ..Default::default()
                },
                back: Box::new(Mode::Board),
            },
            Mode::Confirm {
                machine: 0,
                request: serde_json::json!({}),
                title: "Start this work?".into(),
                message: "Session web has 3 of 3 workspaces.".into(),
                back: Box::new(Mode::Board),
            },
        ]
    }

    #[test]
    fn every_screen_draws_at_every_size() {
        for away in [false, true] {
            let mut a = demo(away);
            if let Some(c) = a.board.cards.iter().find(|c| c.reference == "#41") {
                a.sel = Selection {
                    col: c.stage.index(),
                    key: Some(c.key.clone()),
                };
            }
            for (w, h) in [
                (1, 1),
                (10, 3),
                (30, 8),
                (40, 10),
                (60, 15),
                (80, 24),
                (99, 30),
                (120, 40),
                (200, 60),
            ] {
                a.width = w;
                for mode in modes(&a) {
                    a.mode = mode;
                    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                    t.draw(|f| draw(f, &a)).unwrap();
                }
            }
        }
    }

    fn rendered(a: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, a)).unwrap();
        t.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn header_keeps_machine_visible_when_a_session_is_selected() {
        let mut a = demo(false);
        let home = a.this_machine();
        a.machine = Some(home.clone());
        a.rebuild();
        a.tab = a.tabs.len().saturating_sub(1);
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| draw(f, &a)).unwrap();
        let first: String = (0..100)
            .map(|x| t.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(first.contains(&home), "machine selector vanished: {first}");
        assert!(
            first.contains("web"),
            "selected machine session missing: {first}"
        );
        assert!(
            !first.contains("api"),
            "other machine session leaked: {first}"
        );
        assert!(
            !first.contains("build-box"),
            "other machine appeared in session strip: {first}"
        );
    }

    #[test]
    fn overview_header_explains_machine_and_session_keys() {
        let a = demo(false);
        let text = rendered(&a, 176, 44);
        assert!(
            text.contains("Ctrl+Space"),
            "leader commands must be discoverable"
        );
        assert!(
            text.to_lowercase().contains("owner: all"),
            "owner scope must remain visible"
        );
    }

    #[test]
    fn composer_shows_user_text_and_target_before_submission() {
        let mut a = demo(false);
        a.mode = Mode::Compose {
            form: IntentForm {
                title: "Compare the shortlist".into(),
                words: "Show trade-offs before choosing".into(),
                organization: "Arkship".into(),
                tenant: "pick4me".into(),
                project: "picker".into(),
                machine: 0,
                session: "web".into(),
                field: 0,
            },
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        for expected in [
            "NEW INTENT",
            "Compare the shortlist",
            "Show trade-offs before choosing",
            "pick4me",
            "picker",
            "laptop",
            "web",
            "Ctrl+Enter",
        ] {
            assert!(text.contains(expected), "composer hid {expected}");
        }
    }

    #[test]
    fn confirmation_preserves_reason_and_cancel_action() {
        let mut a = demo(false);
        a.mode = Mode::Confirm {
            machine: 0,
            request: serde_json::json!({}),
            title: "Start shortlist review?".into(),
            message: "web has 3 active workspaces; the usual limit is 3.".into(),
            back: Box::new(Mode::Backlog { sel: 0 }),
        };
        let text = rendered(&a, 176, 44);
        for expected in [
            "Start shortlist review?",
            "3 active workspaces",
            "start anyway",
            "cancel",
        ] {
            assert!(text.contains(expected), "confirmation hid {expected}");
        }
    }

    #[test]
    fn action_failures_remain_visible_over_backlog_and_dialogs() {
        let mut a = demo(false);
        for mode in [
            Mode::Backlog { sel: 0 },
            Mode::Compose {
                form: IntentForm::default(),
                back: Box::new(Mode::Board),
            },
            Mode::Peek {
                scroll: 0,
                back: Box::new(Mode::Board),
            },
        ] {
            a.mode = mode;
            a.message = Some((
                "Could not create: tracker unavailable".into(),
                Tone::Warn,
                std::time::Instant::now(),
            ));
            let text = rendered(&a, 176, 44);
            assert!(
                text.contains("tracker unavailable"),
                "failure hidden by modal"
            );
        }
        a.message = None;
        a.busy = true;
        assert!(rendered(&a, 176, 44).contains("Action in progress"));
    }

    #[test]
    fn dismissed_work_explains_restore_instead_of_dispatch() {
        let mut a = demo(false);
        let card = a
            .board
            .cards
            .iter_mut()
            .find(|c| c.stage == Stage::Backlog)
            .unwrap();
        card.detail.as_mut().unwrap().disposition = Some("wont_do".into());
        a.wont_do = true;
        a.mode = Mode::Backlog { sel: 0 };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("RESTORE TO BACKLOG"));
        assert!(!text.contains("STARTING IT"));
        assert!(text.contains("owner:"));
    }

    #[test]
    fn composer_keeps_the_end_of_multiline_input_visible() {
        let mut a = demo(false);
        a.mode = Mode::Compose {
            form: IntentForm {
                title: "A new task".into(),
                words: format!("{}\nLatest input", "Earlier input with context ".repeat(30)),
                field: 1,
                ..Default::default()
            },
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("Latest input"));
        assert!(text.contains("Create intent"));
        assert!(!text.contains("Create & start"));
    }

    #[test]
    fn provider_scope_and_command_are_visible_at_normal_and_narrow_widths() {
        let mut a = demo(false);
        a.provider = Some("codex".into());
        for width in [60, 100, 176] {
            let text = rendered(&a, width, 30);
            assert!(
                text.contains("provider: Codex"),
                "provider hidden at {width}"
            );
            a.mode = Mode::Palette {
                back: Box::new(Mode::Board),
            };
            let text = rendered(&a, width, 30);
            assert!(
                text.contains("Provider"),
                "provider command hidden at {width}"
            );
            a.mode = Mode::Board;
        }
    }

    #[test]
    fn filtered_session_badges_keep_real_slots_without_unmatched_attention() {
        let mut a = demo(false);
        a.machine = Some("laptop".into());
        a.rebuild();
        for card in &mut a.board.cards {
            for agent in &mut card.agents {
                agent.agent = "codex".into();
            }
        }
        a.provider = Some("claude".into());
        let mut t = Terminal::new(TestBackend::new(176, 44)).unwrap();
        t.draw(|f| draw(f, &a)).unwrap();
        let first: String = (0..176)
            .map(|x| t.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(
            first.contains("web 3/3"),
            "real occupancy disappeared: {first}"
        );
        assert!(
            !first.contains("●2"),
            "unmatched provider attention leaked: {first}"
        );
    }

    #[test]
    fn highlighting_never_slices_past_the_text() {
        let spans = highlight("İstanbul office move", "move", Style::default());
        let joined: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "İstanbul office move");
    }

    #[test]
    fn an_empty_board_draws() {
        let mut a = App::new(Config::default(), vec![]);
        for mode in [
            Mode::Board,
            Mode::Machines { sel: 0 },
            Mode::Backlog { sel: 0 },
        ] {
            a.mode = mode;
            let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
            t.draw(|f| draw(f, &a)).unwrap();
        }
    }

    #[test]
    fn empty_board_shows_machine_session_provider_and_misc_navigation() {
        let mut app = demo(false);
        app.project = Some("no-matching-project".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        for hint in ["No work cards", "[ ]", "{ }", "provider", "Misc"] {
            assert!(text.contains(hint), "missing {hint}: {text}");
        }
    }
}
