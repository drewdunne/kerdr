//! Drawing every screen: the board, and over or instead of it the peek, search, keys menu,
//! machines and backlog. Layout follows the design canvas.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, Draft, Hit, Mode, Search, SearchFilter, Tone, match_range};
use crate::board::{Scope, SessionRow};
use crate::herdr;
use crate::model::{Attention, Card, Priority, Stage, Verdict};
use crate::poll::Conn;
use crate::theme::*;

#[path = "account_ui.rs"]
mod account_ui;

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
    if app.busy
        && app
            .account_pending
            .as_ref()
            .is_some_and(|p| p.request["action"] == "account-signin")
    {
        return;
    }
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
        Mode::Accounts { screen, back } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            account_ui::draw(f, area, app, screen);
        }
        Mode::Peek { back, scroll } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            peek(f, area, app, *scroll);
        }
        Mode::Palette { back, .. } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            palette(f, area, app);
        }
        Mode::Compose { draft, back } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            compose(f, area, app, draft);
        }
        Mode::Filter { query, sel, back } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            filter_view(f, area, app, query, *sel);
        }
        Mode::Confirm {
            title,
            message,
            back,
            request,
            queue,
            ..
        } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            confirm(f, area, title, message, request["action"].as_str(), *queue);
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
    if w == 0 || y >= buf.area.bottom() || x >= buf.area.right() {
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

/// The keys for the selected backlog card, led by its number, so the bottom line says what
/// can be done to the card you're on. Empty when the selected card isn't waiting work.
fn card_hints(app: &App) -> Vec<Span<'static>> {
    let Some(card) = app.backlog_card_for_keys() else {
        return vec![];
    };
    let mut spans = vec![
        Span::styled(
            format!(" {} ", card.reference),
            Style::default()
                .bg(YELLOW)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    spans.extend(key_hints(if card.dismissed() {
        &[("u", "bring it back")]
    } else {
        &[
            ("d", "start it"),
            ("y", "still want it"),
            ("x", "won’t do"),
            ("J K", "move down / up"),
            ("t", "to the top"),
        ]
    }));
    spans.push(Span::styled("   │   ", fg(BORDER)));
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
    let filter_line = filtered(app);
    let accounts = app
        .machines
        .iter()
        .any(|m| m.is_shown() && m.feed.as_ref().is_some_and(|f| !f.accounts.is_empty()));
    let rows = Layout::vertical([
        Constraint::Length(1 + u16::from(filter_line)),
        Constraint::Length(u16::from(banner.is_some())),
        Constraint::Length(if accounts { 1 } else { 3 }),
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
    if filter_line {
        filters(
            f.buffer_mut(),
            Rect {
                y: rows[0].y + 1,
                height: 1,
                ..rows[0]
            },
            app,
        );
    }
    if let Some((left, right)) = banner {
        put_lr(f.buffer_mut(), rows[1], rows[1].y, left, right);
    }
    if !accounts {
        strip(f, rows[2], app);
    }
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
            Line::styled("f filter    a agents    / search    i misc", fg(CYAN)),
            Line::styled(
                "c makes a new intent · Ctrl+Space lists every command",
                fg(DIM),
            ),
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
    let backlog_hints = card_hints(app);
    let mut hints = if accounts {
        let mut h = account_ui::new_intents(app);
        h.extend(key_hints(&[
            ("s", "move"),
            ("A", "accounts"),
            ("S", "models"),
        ]));
        h
    } else {
        card_hints(app)
    };
    if accounts {
        hints.push(Span::raw("   "));
        hints.extend(backlog_hints);
    }
    // With a backlog card's keys in front, the rest gives up what the menu already lists.
    hints.extend(key_hints(if hints.is_empty() {
        &[
            ("c", "new intent"),
            ("⏎", "jump in"),
            ("space", "peek"),
            ("n N", "next that needs you"),
            ("/", "search"),
            ("f", "filter"),
            ("b", "backlog"),
            ("ctrl+space", "menu"),
            ("?", "keys"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("c", "new intent"),
            ("n N", "next that needs you"),
            ("/", "search"),
            ("f", "filter"),
            ("ctrl+space", "menu"),
            ("?", "keys"),
        ]
    }));
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
    let usage_machine = app
        .machine
        .as_ref()
        .and_then(|name| app.machine_index(name))
        .or_else(|| {
            app.machines.iter().position(|m| {
                m.is_shown()
                    && m.machine.is_local()
                    && m.feed.as_ref().is_some_and(|f| !f.accounts.is_empty())
            })
        })
        .or_else(|| {
            app.machines.iter().position(|m| {
                m.is_shown() && m.feed.as_ref().is_some_and(|f| !f.accounts.is_empty())
            })
        });
    let mut usage = usage_machine
        .map(|mi| account_ui::usage(app, mi))
        .unwrap_or_default();
    if app.machine.is_none()
        && !usage.is_empty()
        && let Some(mi) = usage_machine
    {
        usage.insert(
            0,
            Span::styled(format!("{} ", app.machines[mi].name()), fg(DIM)),
        );
    }
    let usage_w = if usage.is_empty() {
        0
    } else {
        usage.iter().map(|s| width(&s.content)).sum::<usize>() + 9
    };
    let reserve = if rect.width as usize > fixed + usage_w + 18 {
        usage_w
    } else {
        0
    };
    let avail = (rect.width as usize).saturating_sub(fixed + reserve);
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
    let mut right = usage;
    if right.is_empty() {
        right.push(Span::styled("{ } machine · [ ] session", fg(DIM)));
    }
    right.push(Span::styled(format!(" · {}", clock(app.now())), fg(DIM)));
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

/// Whether the board shows only part of the work, so its header names what.
fn filtered(app: &App) -> bool {
    app.owner.is_some()
        || app.project.is_some()
        || app.provider.is_some()
        || app.wont_do
        || app.busy
        || !app.count.is_empty()
}

fn filters(buf: &mut Buffer, rect: Rect, app: &App) {
    let mut shown = Vec::new();
    if app.owner.is_some() {
        shown.push(app.owner_label());
    }
    if let Some(p) = &app.project {
        shown.push(p.clone());
    }
    if app.provider.is_some() {
        shown.push(app.provider_label());
    }
    let mut left = if shown.is_empty() {
        vec![Span::styled(" everything shown · f filters", fg(DIM))]
    } else {
        vec![
            Span::styled(" showing ", fg(DIM)),
            Span::styled(shown.join(" · "), bold(CYAN)),
            Span::styled(" · ", fg(DIM)),
            Span::styled("f", bold(FG)),
            Span::styled(" change · ", fg(DIM)),
            Span::styled("F", bold(FG)),
            Span::styled(" show everything", fg(DIM)),
        ]
    };
    if app.wont_do {
        left.push(Span::styled("   WON’T DO", bold(YELLOW)));
    }
    let right = if app.busy {
        vec![Span::styled("◐ action in progress", fg(BLUE))]
    } else if !app.count.is_empty() {
        vec![Span::styled(format!("count: {}", app.count), fg(CYAN))]
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
    let pending = app.pending_jump.as_ref()?;
    let card = app.card(&pending.key)?;
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
        _ if c.intake() == Some("pending") => ("✎", PURPLE, "writing it up".into(), PURPLE),
        _ if c.intake() == Some("failed") => ("✎", YELLOW, "not written up".into(), YELLOW),
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
    let account = app.account_agent(c);
    let modern = app
        .account_feed(c.machine_index)
        .is_some_and(|f| !f.accounts.is_empty());
    let full = format!("{} {}", c.attention.glyph(), c.attention.label());
    let label = if modern {
        if c.attention == Attention::Waiting && c.why == "waiting for usage" {
            crate::accounts::pause_glyph()
        } else {
            match c.attention {
                Attention::NeedsYou => "▲",
                Attention::Running => "●",
                Attention::Blocked => "✕",
                _ => "■",
            }
        }
        .to_string()
    } else if head_w + 1 + width(&full) <= inner_w {
        full
    } else {
        c.attention.glyph().to_string()
    };
    let color = if modern && c.attention == Attention::Waiting && c.why == "waiting for usage" {
        RED
    } else if modern && c.attention == Attention::NeedsYou {
        YELLOW
    } else {
        color
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
    if modern {
        note = account.map(|a| a.account.clone()).unwrap_or_default();
    }
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
        } else if (c.attention == Attention::Waiting && c.why == "waiting for usage") && !cached {
            RED
        } else if c.attention == Attention::NeedsYou && !cached {
            YELLOW
        } else if c.attention == Attention::Running
            && !cached
            && app
                .account_feed(c.machine_index)
                .is_some_and(|f| !f.accounts.is_empty())
        {
            BLUE
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

fn line_for_account_keys() -> Line<'static> {
    Line::from(key_hints(&[
        ("s", "move account"),
        ("S", "models"),
        ("A", "accounts"),
    ]))
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

    if let Some(agent) = app.account_agent(c) {
        lines.push(Line::raw(""));
        lines.push(section("ACCOUNT"));
        let account = app
            .account_feed(c.machine_index)
            .and_then(|f| f.accounts.iter().find(|a| a.code == agent.account));
        let state = account
            .map(|a| {
                format!(
                    "{} · {} · {}",
                    crate::accounts::provider_name(&a.provider),
                    a.numbers(),
                    a.status_label()
                )
            })
            .unwrap_or_default();
        lines.extend(wrapped(
            &format!(
                "{}  {state} · {}",
                agent.account,
                match agent.how.as_str() {
                    "manual" => "moved by hand",
                    "next" => "next in the list",
                    "handed_over" => "handed over to a fresh agent",
                    _ => "follows the list",
                }
            ),
            w,
            fg(FG2),
            None,
        ));
        if let Some(problem) = agent.problem.as_deref().filter(|p| !p.trim().is_empty()) {
            lines.extend(wrapped(problem, w, fg(RED), None));
        }
        lines.push(line_for_account_keys());
    }
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
        } else if c.can_reopen() && app.actions_on() {
            "Its workspace closed: ⏎ reopens it to answer in."
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
    // Waiting work has nothing to jump into: its line names the keys that start or change it.
    // Work whose workspace closed says what ⏎ does for it: reopen it, resuming its agent.
    let reopen = match c.resume_agent() {
        Some(agent) => format!("reopen it, resuming its {agent} conversation"),
        None => "reopen its workspace".to_string(),
    };
    let mut hints = match app.backlog_card_for_keys() {
        Some(c) if c.dismissed() => vec![("u", "bring it back")],
        Some(_) => vec![
            ("d", "start it in its session"),
            ("y", "still want it"),
            ("x", "won’t do"),
            ("J K", "move down / up"),
            ("t", "to the top"),
        ],
        None if c.can_reopen() && app.actions_on() => vec![("⏎", reopen.as_str())],
        None => vec![("⏎", "jump in")],
    };
    hints.push(("n N", "next that needs you"));
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

/// One column of the Ctrl+Space menu: its name, a line under it, and its keys.
type MenuGroup<'a> = (&'a str, String, &'a [(&'a str, &'a str)]);

fn palette(f: &mut Frame, area: Rect, app: &App) {
    // Three columns with a line under each heading on a wide screen; two compact columns on an
    // ordinary one (80×24); one when even that won't fit.
    let wide = area.width >= 116;
    let one_column = !wide && area.width.saturating_sub(2).min(76) < 70;
    let rect = if wide {
        centered(area, 112, 16, Some(6))
    } else {
        centered(area, 76, if one_column { 24 } else { 16 }, Some(1))
    };
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let card = app.backlog_card_for_keys();
    let this_card = card
        .map(|c| format!("{} {}", c.reference, c.title))
        .unwrap_or_else(|| "select a backlog card first".into());
    let buf = f.buffer_mut();
    let mut title = vec![
        Span::styled("CTRL+SPACE", bold(CYAN)),
        Span::styled("   press a key", fg(FG2)),
    ];
    if !wide {
        title.push(Span::styled(format!("   {this_card}"), fg(DIM)));
    }
    put_lr(
        buf,
        inner,
        inner.y,
        title,
        vec![Span::styled("esc close", fg(DIM))],
    );
    let restore = card.is_some_and(|c| c.dismissed());
    let card_keys: &[(&str, &str)] = if restore {
        &[("u", "bring it back")]
    } else {
        &[
            ("d", "start it"),
            ("y", "still want it"),
            ("x", "won’t do"),
            ("J K", "move down / up"),
            ("t", "move to the top"),
        ]
    };
    let groups: [MenuGroup; 3] = [
        ("THIS CARD", this_card, card_keys),
        ("NEW", "in your own words".into(), &[("c", "new intent")]),
        (
            "SHOW",
            "what the board lists".into(),
            &[
                ("b", "the backlog"),
                ("w", "won’t do"),
                ("i", "workspaces with no intent"),
                ("f", "filter by who or project"),
                ("a", "agents: all, Claude, Codex"),
                ("M", "machines"),
                ("?", "every key"),
            ],
        ),
    ];
    let columns: u16 = if wide {
        3
    } else if one_column {
        1
    } else {
        2
    };
    let col_w = inner.width.saturating_sub(4 * (columns - 1)) / columns;
    let top = inner.y + 2;
    let mut y = top;
    for (n, (name, sub, keys)) in groups.iter().enumerate() {
        // Wide: a column each. Two columns: THIS CARD and NEW share the first.
        let col = match columns {
            3 => n as u16,
            2 => u16::from(n == 2),
            _ => 0,
        };
        if (columns == 3) || (columns == 2 && n != 1) {
            y = top;
        } else if n > 0 {
            y += 1;
        }
        let x = inner.x + col * (col_w + 4);
        let faint = card.is_none() && n == 0;
        put(
            buf,
            x,
            y,
            col_w,
            vec![Span::styled(name.to_string(), bold(FG2))],
        );
        y += 1;
        if wide {
            put(
                buf,
                x,
                y,
                col_w,
                vec![Span::styled(
                    trunc(sub, col_w as usize),
                    fg(if faint { FAINT } else { DIM }),
                )],
            );
            y += 2;
        }
        for (key, what) in keys.iter() {
            put(
                buf,
                x,
                y,
                col_w,
                vec![
                    Span::styled(format!("{key:<5}"), bold(CYAN)),
                    Span::styled(what.to_string(), fg(if faint { FAINT } else { FG })),
                ],
            );
            y += 1;
        }
    }
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(
            vec![Span::styled(
                if inner.width >= 62 {
                    "Every letter here also works on its own, without Ctrl+Space."
                } else {
                    "Each letter works without Ctrl+Space too."
                },
                fg(DIM),
            )],
            inner.width as usize,
        ),
    );
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
            if ch == ' ' {
                // A wrapped line never starts with the space it broke at.
                continue;
            }
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

/// One line of the new-intent box, its attached images' tags set apart from the words.
fn tagged(line: String, tags: &[String]) -> Line<'static> {
    let mut spans = vec![];
    let mut rest = line.as_str();
    while let Some((at, tag)) = tags
        .iter()
        .filter_map(|t| rest.find(t.as_str()).map(|at| (at, t)))
        .min_by_key(|(at, _)| *at)
    {
        if at > 0 {
            spans.push(Span::styled(rest[..at].to_string(), fg(FG)));
        }
        spans.push(Span::styled(tag.clone(), bold(CYAN)));
        rest = &rest[at + tag.len()..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), fg(FG)));
    }
    Line::from(spans)
}

fn compose(f: &mut Frame, area: Rect, app: &App, draft: &Draft) {
    let rect = centered(area, 100, 24, Some(4));
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let buf = f.buffer_mut();
    put_lr(
        buf,
        inner,
        inner.y,
        vec![
            Span::styled("NEW INTENT", bold(CYAN)),
            Span::styled(
                "   Say what you want, the way you would to an agent.",
                fg(FG2),
            ),
        ],
        vec![Span::styled("esc cancel", fg(DIM))],
    );
    let boxed = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: inner.height.saturating_sub(8),
    };
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(fg(BORDER))
            .style(Style::default().bg(BG)),
        boxed,
    );
    let text = pad(boxed, 2, 1);
    let rows = text.height.max(1) as usize;
    let lines: Vec<Line> = if draft.words.is_empty() {
        vec![Line::from(vec![
            Span::styled("▏", fg(CYAN)),
            Span::styled(
                "For example: the laptop's Wi-Fi drops after sleep; make it reconnect on its own.",
                fg(FAINT),
            ),
        ])]
    } else {
        let tags: Vec<String> = draft
            .images
            .iter()
            .map(|i| crate::images::tag(i.tag))
            .collect();
        field_lines(&draft.words, text.width.max(1) as usize, rows, !app.busy)
            .into_iter()
            .map(|l| tagged(l, &tags))
            .collect()
    };
    f.render_widget(Paragraph::new(lines), text);
    let machine = app
        .machines
        .get(draft.machine)
        .map(|m| m.name())
        .unwrap_or_else(|| "this machine".into());
    let buf = f.buffer_mut();
    let y = boxed.bottom() + 1;
    let where_line = vec![
        Span::styled("It goes in the backlog of ", fg(FG2)),
        Span::styled(draft.session.clone(), bold(FG)),
        Span::styled(" on ", fg(FG2)),
        Span::styled(machine, bold(FG)),
        Span::styled(
            ", the session you're looking at, until you start it.",
            fg(FG2),
        ),
    ];
    put(buf, inner.x, y, inner.width, fit(where_line, w));
    for (n, line) in wrap(
        "Say \"start it now\", or name another session or who it's for, and the agent that writes it up does that instead.",
        w,
        2,
    )
    .into_iter()
    .enumerate()
    {
        put(
            buf,
            inner.x,
            y + 1 + n as u16,
            inner.width,
            vec![Span::styled(line, fg(DIM))],
        );
    }
    let hints = if app.busy {
        vec![Span::styled("◐ saving it…", fg(BLUE))]
    } else {
        key_hints(&[
            ("⏎", "send"),
            ("shift+⏎ / alt+⏎", "new line"),
            ("ctrl+v", "image"),
            ("ctrl+u", "clear"),
            ("esc", "keep for later"),
        ])
    };
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(hints, w),
    );
}

/// The filter list (f): who the work is for, or one of their projects.
fn filter_view(f: &mut Frame, area: Rect, app: &App, query: &str, sel: usize) {
    let rows = app.filter_matches(query);
    let list_h = (rows.len() as u16)
        .min(area.height.saturating_sub(14))
        .max(1);
    let rect = centered(area, 84, list_h + 10, Some(4));
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let buf = f.buffer_mut();
    put_lr(
        buf,
        inner,
        inner.y,
        vec![
            Span::styled("SHOW ONLY", bold(CYAN)),
            Span::styled("   who it's for, or one project", fg(FG2)),
        ],
        vec![Span::styled("esc close", fg(DIM))],
    );
    put(
        buf,
        inner.x,
        inner.y + 2,
        inner.width,
        if query.is_empty() {
            vec![
                Span::styled("› ", bold(CYAN)),
                Span::styled("type to narrow", fg(DIM)),
            ]
        } else {
            vec![
                Span::styled("› ", bold(CYAN)),
                Span::styled(query.to_string(), bold(FG)),
                Span::styled("▏", fg(CYAN)),
            ]
        },
    );
    let top = inner.y + 4;
    let sel = sel.min(rows.len().saturating_sub(1));
    let skip = sel.saturating_sub(list_h.saturating_sub(1) as usize);
    if rows.is_empty() {
        put(
            buf,
            inner.x,
            top,
            inner.width,
            vec![Span::styled("Nothing matches.", fg(DIM))],
        );
    }
    for (n, row) in rows.iter().enumerate().skip(skip).take(list_h as usize) {
        let y = top + (n - skip) as u16;
        let on = row.owner == app.owner && row.project == app.project;
        let counts = format!("{:>3} open {:>4} backlog", row.open, row.backlog);
        let left = vec![
            Span::styled(if on { "✓ " } else { "  " }, bold(CYAN)),
            Span::raw("  ".repeat(row.depth as usize)),
            Span::styled(
                row.label.clone(),
                if n == sel || on { bold(FG) } else { fg(FG) },
            ),
            Span::styled(
                if row.kind.is_empty() {
                    String::new()
                } else {
                    format!("  {}", row.kind)
                },
                fg(DIM),
            ),
        ];
        let line = Rect {
            y,
            height: 1,
            ..inner
        };
        if y >= inner.bottom().saturating_sub(3) {
            break;
        }
        if n == sel {
            buf.set_style(line, Style::default().bg(SEL));
        }
        put_lr(buf, line, y, left, vec![Span::styled(counts, fg(FG2))]);
    }
    let agents_y = inner.bottom().saturating_sub(3);
    let mut agents = vec![Span::styled("AGENTS   ", bold(FG2))];
    for (name, value) in [
        ("all", None),
        ("Claude", Some("claude")),
        ("Codex", Some("codex")),
    ] {
        let on = app.provider.as_deref() == value;
        agents.push(Span::styled(
            format!(" {name} "),
            if on {
                Style::default().bg(SEL).fg(FG).add_modifier(Modifier::BOLD)
            } else {
                fg(FG2)
            },
        ));
        agents.push(Span::raw(" "));
    }
    if let Some(other) = app
        .provider
        .as_deref()
        .filter(|p| !matches!(*p, "claude" | "codex"))
    {
        agents.push(Span::styled(
            format!(" {other} "),
            Style::default().bg(SEL).fg(FG).add_modifier(Modifier::BOLD),
        ));
    }
    put_lr(
        buf,
        Rect {
            y: agents_y,
            height: 1,
            ..inner
        },
        agents_y,
        agents,
        key_hints(&[("tab", "switches here"), ("a", "anywhere")]),
    );
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(
            key_hints(&[
                ("↑ ↓", "move"),
                ("⏎", "show only this"),
                ("All work", "shows everything"),
                ("esc", "close"),
            ]),
            w,
        ),
    );
}

fn confirm(
    f: &mut Frame,
    area: Rect,
    title: &str,
    message: &str,
    action: Option<&str>,
    queue: bool,
) {
    let rect = centered(area, 86, 16, None);
    dialog(f, rect, YELLOW);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let mut lines = wrapped(title, w, bold(YELLOW), None);
    lines.push(Line::raw(""));
    lines.extend(wrapped(message, w, fg(FG), None));
    if action == Some("dispatch") {
        lines.push(Line::raw(""));
        lines.extend(wrapped(
            "Starting more work takes another slot. At the maximum, finish or shelve one first.",
            w,
            fg(DIM),
            None,
        ));
    }
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
            key_hints(
                &[
                    (
                        "Enter / y",
                        match action {
                            Some("dispatch") => "start anyway",
                            Some("reopen") => "go ahead",
                            _ => "yes",
                        },
                    ),
                    ("w", "queue it for a free slot"),
                    ("Esc / n", "cancel"),
                ][..]
                    .iter()
                    .filter(|(k, _)| queue || *k != "w")
                    .copied()
                    .collect::<Vec<_>>(),
            ),
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
        "MOVE AND LOOK",
        &[
            ("h j k l", "move between columns and cards", "arrows too"),
            ("g  G", "first / last card in the column", ""),
            (
                "⏎",
                "jump in: herdr, full screen, on the card's workspace",
                "",
            ),
            (
                "⏎",
                "on work whose workspace closed: reopen it, resuming its agent's conversation",
                "with actions on",
            ),
            (
                "space",
                "peek: its stage, what it needs, its last update",
                "m too",
            ),
            (
                "n  N",
                "next / previous card that needs you",
                "ctrl+alt+j/k too",
            ),
            ("/", "search cards, workspaces and sessions", ""),
        ],
    ),
    (
        "SESSIONS AND MACHINES",
        &[
            (
                "[  ]",
                "previous / next session on this machine",
                "tab, ctrl+alt+h/l too",
            ),
            ("{  }", "previous / next machine", "ctrl+alt+shift+h/l too"),
            ("0", "every session on every machine", ""),
            ("1..9", "session 1 to 9", ""),
            ("M", "machines and how each connection is doing", ""),
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
];

/// The right-hand column: making and changing work, what the board shows, and the menu.
pub const WORK_KEYS: &[(&str, &[KeyRow])] = &[
    (
        "ACCOUNTS",
        &[
            (
                "A",
                "accounts and tenant model lists",
                "tab changes machine",
            ),
            (
                "s",
                "move the selected agent to another account",
                "asks only when needed",
            ),
            (
                "S",
                "the viewed session’s ordered models",
                "space moves waiting sessions",
            ),
        ],
    ),
    (
        "WORK · the same keys on the board, the backlog and a peek",
        &[
            ("c", "new intent, in your own words", ""),
            (
                "d",
                "start the selected backlog intent",
                "asks first past the usual limit",
            ),
            ("y", "still want it: its age starts again", ""),
            ("x", "won’t do", "asks first"),
            ("u", "bring it back from won’t do", ""),
            ("J  K", "move it down / up your order", "10J moves ten"),
            ("t", "move it to the top of your order", ""),
        ],
    ),
    (
        "SHOW",
        &[
            ("b", "the backlog, in your order", ""),
            ("w", "the won’t do list", ""),
            ("i", "workspaces with no intent (misc)", ""),
            (
                "f",
                "show only one owner, tenant or project",
                "F shows everything",
            ),
            ("a", "agents: all, Claude, Codex, then others", ""),
        ],
    ),
    (
        "MENU",
        &[
            ("ctrl+space", "a menu of the work and show keys", ""),
            ("?", "this list", ""),
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
                "   On the board every key is Kerdr's. Inside herdr every key is herdr's, plus the ones below.",
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
    for (group, items) in WORK_KEYS {
        let shown: Vec<_> = items.iter().filter(|(k, d, a)| matches(k, d, a)).collect();
        if shown.is_empty() {
            continue;
        }
        out.push(heading(group));
        for (k, d, a) in shown {
            out.extend(entry(k, d, a, 13, right.width));
        }
        out.push(vec![]);
    }
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
            "Everything else inside is herdr's, unchanged. The space and backspace keys are Kerdr's: kerdr herdr-keys --install adds them to herdr's config on each machine.",
            right.width as usize,
            3,
        ) {
            out.push(vec![Span::styled(l, fg(DIM))]);
        }
    }
    for (r, spans) in out
        .iter()
        .skip(scroll as usize)
        .take(right.height as usize)
        .enumerate()
    {
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
                "Lost the connection {} ago ({}). Trying again in {}s (try {}). The wait grows to 15 seconds.",
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
            format!("Kerdr tries again in {}s, then every 15 seconds; r tries now.", (*retry_at - now).num_seconds().max(0)),
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
        "Dropped. Retries with longer waits, up to 15 seconds. Cards stay, dimmed.",
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
                ("space", "peek"),
                ("[ ]", "session"),
                ("{ }", "machine"),
                ("c", "new intent"),
                ("f", "filter"),
                ("esc", "board"),
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
        let (glyph, gc, meta, meta_color) = list_glyph(c, now);
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
            if c.intake().is_some() {
                vec![Span::styled(meta, fg(meta_color))]
            } else {
                vec![Span::styled(
                    age,
                    fg(if c.stale_days.unwrap_or(0) >= 14 {
                        YELLOW
                    } else {
                        DIM
                    }),
                )]
            },
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
        match c.intake() {
            Some("pending") => {
                out.push(Line::raw(""));
                out.extend(wrapped(
                    "✎ An agent is writing this up from your words: its title, who it's for and its triage. It stays here until you start it.",
                    w,
                    fg(PURPLE),
                    None,
                ));
            }
            Some("failed") => {
                out.push(Line::raw(""));
                out.extend(wrapped(
                    "✎ No agent could write this up, so it keeps your words and their first ones as its title. The agent that starts it writes it up.",
                    w,
                    fg(YELLOW),
                    None,
                ));
            }
            _ => {}
        }
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
                &format!("{stale_n} items need review. y for the work you still want, x for won’t do; u brings it back later."),
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
                            "{} is at its usual limit of {}. Starting it with d asks before opening another slot. Or use {}, which has {} free.",
                            r.session,
                            r.slots.limit.unwrap_or(0),
                            o.session,
                            o.slots.free().unwrap_or(0)
                        ),
                        None => format!(
                            "{} is at its usual limit. Starting it with d asks before opening another slot. At the maximum, finish or shelve one first.",
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
            key_hints(if app.wont_do {
                &[
                    ("j k", "move"),
                    ("u", "bring it back"),
                    ("⏎", "peek"),
                    ("w", "the backlog"),
                    ("f", "filter"),
                    ("esc", "board"),
                ]
            } else {
                &[
                    ("j k", "move"),
                    ("d", "start it"),
                    ("y", "still want it"),
                    ("x", "won’t do"),
                    ("J K", "move down / up · 10J ten places"),
                    ("t", "to the top"),
                    ("c", "new intent"),
                    ("⏎", "peek"),
                    ("w", "won’t do list"),
                    ("f", "filter"),
                    ("esc", "board"),
                ]
            }),
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
        let mut modes = vec![
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
                target: None,
            },
            Mode::Compose {
                draft: Draft {
                    words: "A clear outcome".into(),
                    ..Default::default()
                },
                back: Box::new(Mode::Board),
            },
            Mode::Filter {
                query: "zzz".into(),
                sel: 99,
                back: Box::new(Mode::Board),
            },
            Mode::Confirm {
                machine: 0,
                request: serde_json::json!({"action":"dispatch"}),
                title: "Start this work?".into(),
                message: "Session web has 3 of 3 workspaces.".into(),
                back: Box::new(Mode::Board),
                target: None,
                queue: false,
                ready_at: None,
            },
        ];
        modes.extend(crate::demo::account_screens(a));
        modes
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

    #[test]
    fn every_copy_state_draws_at_every_size_and_says_what_it_is() {
        let mut a = demo(false);
        let copies = crate::demo::account_screens(&a)[1].clone();
        a.mode = copies;
        let small = rendered(&a, 80, 24);
        for words in [
            "Your agents",
            "✓ stored",
            "Tenants",
            "✕ not stored",
            "– not checked",
        ] {
            assert!(small.contains(words), "{words} missing at 80×24");
        }
        let large = rendered(&a, 176, 44);
        for words in [
            "held by root",
            "✓ works · ",
            "Tenants on build-box",
            "⏎ sign in this copy",
        ] {
            assert!(large.contains(words), "{words} missing at 176×44");
        }
        let states = [
            serde_json::json!({"id":"agents","label":"Your agents","stored":true,"works":false,"checked":"2026-10-21T17:40:00-05:00","note":"refused by Claude"}),
            serde_json::json!({"id":"tenants","label":"A rather long label for a copy","stored":true,"works":null,"checked":null,"note":null}),
            serde_json::json!({"id":"third","label":null,"stored":false,"works":true,"checked":"not a time","note":""}),
        ];
        let ark = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .accounts
            .iter_mut()
            .find(|x| x.code == "ARK")
            .unwrap();
        ark.copies = serde_json::from_value(serde_json::Value::from(states.to_vec())).unwrap();
        let screens = crate::demo::account_screens(&a);
        for sel in 0..a.account_rows(1).len() {
            for (w, h) in [
                (1, 1),
                (10, 3),
                (30, 8),
                (60, 15),
                (80, 24),
                (120, 40),
                (200, 60),
            ] {
                a.width = w;
                a.mode = screens[1].clone();
                if let Mode::Accounts {
                    screen: crate::app::account_controls::AccountScreen::List { sel: s, .. },
                    ..
                } = &mut a.mode
                {
                    *s = sel;
                }
                let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                t.draw(|f| draw(f, &a)).unwrap();
            }
        }
        a.mode = screens[1].clone();
        let text = rendered(&a, 176, 44);
        assert!(text.contains("✕ doesn't work · "), "{text}");
        assert!(text.contains("refused by Claude"));
        assert!(text.contains("A rather long label…"));
        assert!(text.contains("third"), "an unlabelled copy shows its id");
        let signin = crate::demo::account_screens(&a)
            .into_iter()
            .find(|m| {
                matches!(m, Mode::Accounts {
                    screen: crate::app::account_controls::AccountScreen::Signin(s), ..
                } if s.copy.is_some())
            })
            .unwrap();
        a.mode = signin;
        let text = rendered(&a, 176, 44);
        assert!(text.contains("Sign ARK in on build-box: Tenants"), "{text}");
        assert!(text.contains("Only the Tenants copy changes."));
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
    fn new_intents_footer_skips_unavailable_accounts_then_falls_back_to_first() {
        let mut a = demo(false);
        a.machine = Some(a.this_machine());
        a.rebuild();
        a.tab = a
            .tabs
            .iter()
            .position(|scope| matches!(scope, Scope::Session(_, session) if session == "web"))
            .unwrap();
        let mi = a.machine_index(&a.this_machine()).unwrap();
        for owner in &mut a.machines[mi].feed.as_mut().unwrap().owners {
            owner.models = vec!["KBZ".into(), "ARK".into()];
        }
        assert!(rendered(&a, 176, 44).contains("new in web: Claude ARK"));
        for account in &mut a.machines[mi].feed.as_mut().unwrap().accounts {
            account.status = "out".into();
        }
        assert!(rendered(&a, 176, 44).contains("new in web: Claude KBZ"));
    }

    #[test]
    fn overview_header_explains_machine_and_session_keys() {
        let mut a = demo(false);
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.accounts.clear();
            }
        }
        let text = rendered(&a, 176, 44);
        let mut terminal = Terminal::new(TestBackend::new(176, 44)).unwrap();
        terminal.draw(|f| draw(f, &a)).unwrap();
        let header: String = (0..176)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(
            header
                .trim_end()
                .ends_with(&format!("{{ }} machine · [ ] session · {}", clock(a.now()))),
            "{header}"
        );
        assert!(
            text.contains("ctrl+space menu"),
            "the menu must be discoverable"
        );
        assert!(text.contains("{ } machine · [ ] session"));
        assert!(
            !text.contains("owner:") && !text.contains("showing"),
            "an unfiltered board spends no line on filters"
        );
    }

    #[test]
    fn new_intent_box_takes_prose_and_says_where_it_goes() {
        let mut a = demo(false);
        a.mode = Mode::Compose {
            draft: Draft {
                words: "Show trade-offs before choosing".into(),
                machine: 0,
                session: "web".into(),
                ..Default::default()
            },
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        for expected in [
            "NEW INTENT",
            "Show trade-offs before choosing",
            "backlog of web",
            "start it now",
            "send",
            "new line",
        ] {
            assert!(text.contains(expected), "the box hid {expected}");
        }
        for gone in ["Title", "Organization", "Tenant", "Create intent"] {
            assert!(!text.contains(gone), "the old form's {gone} is back");
        }
    }

    #[test]
    fn action_failures_remain_visible_over_backlog_and_dialogs() {
        let mut a = demo(false);
        for mode in [
            Mode::Backlog { sel: 0 },
            Mode::Compose {
                draft: Draft::default(),
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
        assert!(text.contains("WON’T DO"));
        assert!(text.contains("u bring it back"));
    }

    #[test]
    fn composer_keeps_the_end_of_multiline_input_visible() {
        let mut a = demo(false);
        a.mode = Mode::Compose {
            draft: Draft {
                words: format!("{}\nLatest input", "Earlier input with context ".repeat(80)),
                ..Default::default()
            },
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("Latest input"));
    }

    #[test]
    fn provider_scope_and_command_are_visible_at_normal_and_narrow_widths() {
        let mut a = demo(false);
        a.provider = Some("codex".into());
        for width in [60, 100, 176] {
            let text = rendered(&a, width, 30);
            assert!(text.contains("showing Codex"), "provider hidden at {width}");
            a.mode = Mode::Palette {
                back: Box::new(Mode::Board),
                target: None,
            };
            let text = rendered(&a, width, 30);
            assert!(
                text.contains("agents: all, Claude, Codex"),
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
    fn the_menu_shows_every_command_in_an_ordinary_window() {
        let mut a = demo(false);
        a.mode = Mode::Palette {
            back: Box::new(Mode::Board),
            target: None,
        };
        for (w, h) in [(80, 24), (100, 30), (60, 30), (176, 44)] {
            let text = rendered(&a, w, h);
            for expected in [
                "start it",
                "move to the top",
                "new intent",
                "the backlog",
                "filter by who or project",
                "agents: all, Claude, Codex",
                "machines",
                "every key",
                "without Ctrl+Space",
            ] {
                assert!(text.contains(expected), "{expected} missing at {w}x{h}");
            }
        }
    }

    #[test]
    fn a_peek_at_waiting_work_names_the_key_that_starts_it() {
        let mut a = demo(false);
        let key = a
            .board
            .cards
            .iter()
            .find(|c| crate::app::waiting_work(c))
            .unwrap()
            .key
            .clone();
        a.sel = crate::app::Selection {
            col: Stage::Backlog.index(),
            key: Some(key),
        };
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("d start it in its session"), "{text}");
        assert!(!text.contains("⏎ jump in"));
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
        for hint in ["No work cards", "[ ]", "{ }", "a agents", "i misc"] {
            assert!(text.contains(hint), "missing {hint}: {text}");
        }
    }
}
