//! Drawing every screen: the board, and over or instead of it the peek, search, keys menu,
//! machines and backlog. Layout follows the design canvas.

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{
    App, Background, Draft, Hit, Kickoff, Mode, Search, SearchFilter, Tone, match_range,
};
use crate::board::{Scope, SessionRow};
use crate::herdr;
use crate::model::{Attention, Card, Priority, Stage, Verdict};
use crate::poll::Conn;
use crate::theme::*;

#[path = "account_ui.rs"]
mod account_ui;
#[path = "card_ui.rs"]
mod card_ui;

pub fn draw(f: &mut Frame, app: &App) {
    draw_screen(f, app);
    crate::palette::recolor_active(f.buffer_mut());
}

fn draw_screen(f: &mut Frame, app: &App) {
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
    if app.failing() {
        failure(f, area, app);
    } else if !matches!(app.mode, Mode::Board) {
        action_status(f, area, app);
    }
}

/// A change that didn't happen (intent 0416): a red box over whatever is up, every failure that
/// came together on a line of its own, until a key is pressed once it has been seen.
fn failure(f: &mut Frame, area: Rect, app: &App) {
    let Some((text, _)) = app.message() else {
        return;
    };
    let w = area.width.saturating_sub(4).min(100);
    let inner = w.saturating_sub(4) as usize;
    // The oldest few failures; a key takes them down and the rest come up next.
    let all: Vec<&str> = text.lines().collect();
    let mut lines = vec![Line::from(Span::styled("Not done".to_string(), bold(RED)))];
    for line in all.iter().take(crate::app::FAIL_PAGE) {
        lines.extend(wrapped(line, inner, fg(FG), Some(("✗ ", RED))));
    }
    let more = all.len().saturating_sub(crate::app::FAIL_PAGE);
    // Only once a key can: until then a key on its way mustn't take it down, and nothing should
    // promise otherwise (intent 0491).
    let closes = !matches!(&app.message, Some((_, _, at)) if at.elapsed() < crate::app::FAIL_SEEN);
    lines.push(Line::from(Span::styled(
        match (more > 0, closes) {
            (true, true) => {
                format!("The board shows them as they really are. Any key shows {more} more.")
            }
            (true, false) => "The board shows them as they really are.".to_string(),
            (false, true) => "The board shows it as it really is. Any key closes this.".to_string(),
            (false, false) => "The board shows it as it really is.".to_string(),
        },
        fg(DIM),
    )));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.bottom().saturating_sub(h + 2),
        width: w,
        height: h,
    };
    dialog(f, rect, RED);
    f.render_widget(Paragraph::new(lines), pad(rect, 2, 1));
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
    let tone_color = |tone: Tone| match tone {
        Tone::Info => FG2,
        Tone::Good => GREEN,
        Tone::Warn => YELLOW,
        Tone::Fail => RED,
    };
    // While an action runs, anything said since it started shows (a refusal, a ⏎ waiting its
    // turn, its own working line); otherwise what it's doing and for how long (intent 0411).
    let working;
    let (text, color) = if app.busy {
        if let Some((text, tone)) = app.message_since_busy() {
            (text, tone_color(tone))
        } else if let Some(line) = app.busy_line() {
            // On an account screen its keys wait for the answer; elsewhere the board is free.
            working = if matches!(app.mode, Mode::Accounts { .. }) {
                format!("{line} · saving; this screen's keys wait for it, Esc leaves it")
            } else {
                format!("{line} · the board is yours meanwhile")
            };
            (working.as_str(), BLUE)
        } else {
            ("Action in progress", BLUE)
        }
    } else if let Some((text, tone)) = app.message() {
        (text, tone_color(tone))
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
        Mode::Filter {
            query,
            sel,
            typing,
            back,
        } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            filter_view(f, area, app, query, *sel, *typing);
        }
        Mode::Confirm {
            title,
            message,
            back,
            request,
            queue,
            ready_at,
            choices,
            enter_queues,
            ..
        } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            confirm(
                f,
                area,
                Question {
                    title,
                    message,
                    action: request["action"].as_str(),
                    queue: *queue,
                    choices,
                    enter_queues: *enter_queues,
                    too_soon: ready_at.is_some_and(|at| Instant::now() < at),
                },
            );
        }
        Mode::Kickoff { form, back, .. } => {
            draw_mode(f, area, app, back);
            dim_behind(f);
            kickoff(f, area, form);
        }
        Mode::Machines { sel } => machines_view(f, area, app, *sel),
        Mode::Backlog { sel } => backlog_view(f, area, app, *sel),
        Mode::Misc { sel } => misc_view(f, area, app, *sel),
        Mode::Done { sel } => done_view(f, area, app, *sel),
        Mode::Card {
            card,
            focus,
            opened,
            ..
        } => card_ui::draw(f, area, app, card, focus, opened),
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
    key_hints_held(hints, &[])
}

/// The same, with the keys in `held` drawn faint: a guard won't let them answer yet, so a key
/// pressed now is seen to do nothing, rather than looking like a hang (intent 0491).
fn key_hints_held(hints: &[(&str, &str)], held: &[&str]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (k, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        if held.contains(k) {
            spans.push(Span::styled(k.to_string(), fg(FAINT)));
            spans.push(Span::styled(format!(" {what}"), fg(FAINT)));
        } else {
            spans.push(Span::styled(k.to_string(), bold(FG)));
            spans.push(Span::styled(format!(" {what}"), fg(DIM)));
        }
    }
    spans
}

/// The keys for the selected backlog card, led by its number, so the bottom line says what
/// can be done to the card you're on. Empty when the selected card isn't waiting work.
fn card_hints(app: &App) -> Vec<Span<'static>> {
    let Some(card) = app.backlog_card_for_keys() else {
        return started_hints(app);
    };
    let mut spans = card_badge(card, YELLOW);
    // On the board ⏎ starts it too (intent 0404); the backlog screen's ⏎ peeks.
    let start = if matches!(app.mode, Mode::Board) && app.actions_on() {
        "d ⏎"
    } else {
        "d"
    };
    let waiting = [
        (start, "start it"),
        ("e", "edit its text"),
        ("y", "still want it"),
        ("x", "won’t do"),
        ("C", "done"),
        ("J K", "move down / up"),
        ("t B", "to the top / bottom"),
    ];
    spans.extend(key_hints(if card.dismissed() {
        &[("u", "bring it back")]
    } else {
        &waiting
    }));
    spans.push(Span::styled("   │   ", fg(BORDER)));
    spans
}

/// What gets a card that needs you, and has no workspace open, somewhere to answer: what does work
/// for it, never only that it has none (intent 0591).
pub(crate) fn where_to_answer(app: &App, c: &Card) -> String {
    if !app.actions_on() {
        return "It has no workspace open, and this board has no [actions] command to start one for it.".into();
    }
    if c.outside_herdr().is_some() && !c.done() {
        return "It runs in a terminal outside herdr: ⏎ moves it into herdr to answer in.".into();
    }
    if c.dismissed() {
        // A backend that brings it back as it starts it (intent 0591, K1-06).
        if app.does_steps(c) {
            return "It's in won't do: ⏎ asks to bring it back and start it.".into();
        }
        return "It's in won't do: u brings it back, then ⏎ starts it.".into();
    }
    if c.can_reopen_done() {
        return "It's done: ⏎ asks to reopen it to answer in.".into();
    }
    if c.can_reopen() {
        return "Its workspace closed: ⏎ reopens it to answer in.".into();
    }
    if c.stage == Stage::Backlog {
        let questions = c.detail.as_ref().is_some_and(|d| d.kickoff.is_some());
        return if questions {
            "d asks its questions in the Kickoff box, then starts it.".into()
        } else {
            "⏎ starts it, opening a workspace for it.".into()
        };
    }
    "⏎ opens a workspace for it to answer in.".into()
}

/// Work already started with no workspace open, selected on the board: ⏎ reopens it, and C and
/// x end it without starting anything (intent 0289). Else what done_hints says.
fn started_hints(app: &App) -> Vec<Span<'static>> {
    let Some(card) = app
        .started_card_for_keys()
        .filter(|_| matches!(app.mode, Mode::Board) && app.actions_on())
    else {
        return running_hints(app);
    };
    let mut spans = card_badge(card, if card.no_agent() { YELLOW } else { CYAN });
    let mut hints: Vec<(&str, String)> = vec![];
    if card.outside_herdr().is_some() && !card.done() {
        // ⏎ moves its conversation into herdr, and goes in (intent 0591).
        hints.push(("⏎", "move it into herdr".into()));
    } else if card.reopenable() {
        hints.push(("⏎", "reopen it".into()));
    }
    if let Some(m) = move_hint(app, card) {
        hints.push(("s", m));
    }
    hints.extend([("C", "done".into()), ("x", "won’t do".into())]);
    with_inside(app, card, &mut hints);
    spans.extend(owned_hints(&hints));
    spans.push(Span::styled("   │   ", fg(BORDER)));
    spans
}

/// The account move s offers for a card, as its hint says it: the card's own agent, off the
/// account it's on (intent 0321, F-0294). None when s has no agent there to move.
fn move_hint(app: &App, card: &Card) -> Option<String> {
    use crate::app::run_state::{RunState, StartHow};
    // What s does depends on where the card's agent stands (intent 0580).
    match app.run_state(card) {
        RunState::Running(agent) if agent.account.is_empty() => {
            Some("move its agent to another account".into())
        }
        RunState::Running(agent) => Some(format!("move its agent off {}", agent.account)),
        RunState::Stopped(start) if start.restore => Some("bring it back on an account".into()),
        RunState::Stopped(start) if start.outside => {
            Some("move it into herdr on an account".into())
        }
        RunState::Stopped(start) if start.how == StartHow::Reopen => {
            Some("resume it on an account".into())
        }
        RunState::Stopped(_) => Some("start it on an account".into()),
        // Its machine's accounts are on their way: s waits for them (intent 0591).
        RunState::NoAccounts(_) => Some("start it on an account".into()),
        RunState::Unavailable(_) => None,
    }
}

/// Work an agent is at, selected on the board: ⏎ goes in, s moves its agent to another account
/// (saying which it's on), and X cancels it, stopping that agent (intent 0321). Else what
/// done_hints says.
fn running_hints(app: &App) -> Vec<Span<'static>> {
    let Some(card) = app
        .running_card_for_keys()
        .filter(|_| matches!(app.mode, Mode::Board))
    else {
        return done_hints(app);
    };
    let mut spans = card_badge(card, CYAN);
    let moving = move_hint(app, card);
    let mut hints: Vec<(&str, String)> = vec![("⏎", "go in".into())];
    if let Some(m) = moving {
        hints.push(("s", m));
    }
    if app.actions_on() {
        hints.push(("X", "cancel".into()));
    }
    with_inside(app, card, &mut hints);
    spans.extend(owned_hints(&hints));
    spans.push(Span::styled("   │   ", fg(BORDER)));
    spans
}

/// A done card selected on the board: what ⏎ does for it, which is to ask to reopen it.
fn done_hints(app: &App) -> Vec<Span<'static>> {
    let Some(card) = app
        .selected()
        .filter(|c| matches!(app.mode, Mode::Board) && c.can_reopen_done() && app.actions_on())
    else {
        return vec![];
    };
    let mut spans = card_badge(card, GREEN);
    let mut hints: Vec<(&str, String)> = vec![("⏎", "ask to reopen it".into())];
    if let Some(m) = move_hint(app, card) {
        hints.push(("s", m));
    }
    with_inside(app, card, &mut hints);
    spans.extend(owned_hints(&hints));
    spans.push(Span::styled("   │   ", fg(BORDER)));
    spans
}

/// `o`'s hint for a card: what's inside it, or the card out of view that it sits in.
fn inside_hint(app: &App, card: &Card) -> Option<String> {
    let i = app.card_index(&card.key)?;
    if let Some(r) = app.held_elsewhere(i)
        && app.board.up.get(i).copied().flatten().is_some()
    {
        return Some(format!("opens {r}"));
    }
    let n = app.pieces(i).len();
    (n > 0).then(|| format!("what's inside ({n})"))
}

/// The hints a card holding work adds: ⏎ is its own agent, and o opens what's inside.
fn with_inside(app: &App, card: &Card, hints: &mut Vec<(&str, String)>) {
    if let Some(h) = inside_hint(app, card) {
        let holds = !app.tabs_inside(card).is_empty() || h.starts_with("what");
        for (k, v) in hints.iter_mut() {
            if *k == "⏎" && v == "go in" && holds {
                *v = "its own agent".into();
            }
        }
        let at = hints
            .iter()
            .position(|(k, _)| *k == "⏎")
            .map_or(0, |p| p + 1);
        hints.insert(at, ("o", h));
    }
}

fn owned_hints(hints: &[(&str, String)]) -> Vec<Span<'static>> {
    let refs: Vec<(&str, &str)> = hints.iter().map(|(k, v)| (*k, v.as_str())).collect();
    key_hints(&refs)
}

fn card_badge(card: &Card, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {} ", card.reference),
            Style::default()
                .bg(color)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ]
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
    // What was said takes the line first; this lasts as long as the ⏎ does (intent 0491).
    let remembered = remembered_line(app);
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
        Constraint::Length(u16::from(
            pending.is_some() || message.is_some() || remembered.is_some(),
        )),
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
        // Machines still giving their first read: the board is loading, not empty (intent 0445).
        let reading: Vec<String> = app
            .machines
            .iter()
            .filter(|m| matches!(m.conn, crate::poll::Conn::Connecting))
            .map(|m| m.name())
            .collect();
        let first = if reading.is_empty() {
            "No work cards in this view.".to_string()
        } else {
            format!("Reading {}…", reading.join(", "))
        };
        let lines = vec![
            Line::styled(first, bold(FG2)),
            Line::raw(""),
            Line::styled(
                if app.owner_split() {
                    if app.owner_pick.is_none() {
                        "{ } choose an owner, then [ ] choose a team"
                    } else {
                        "[ ] choose a team    { } change owner"
                    }
                } else if app.machine.is_none() {
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
    } else if let Some((text, tone)) = message.filter(|(_, t)| *t != Tone::Fail) {
        let color = match tone {
            Tone::Info => FG2,
            Tone::Good => GREEN,
            Tone::Warn | Tone::Fail => YELLOW,
        };
        put(
            f.buffer_mut(),
            rows[4].x,
            rows[4].y,
            rows[4].width,
            vec![Span::styled(text.to_string(), fg(color))],
        );
    } else if let Some(line) = remembered {
        put(f.buffer_mut(), rows[4].x, rows[4].y, rows[4].width, line);
    }
    let backlog_hints = card_hints(app);
    let card_moves = matches!(app.mode, Mode::Board)
        && app
            .running_card_for_keys()
            .is_some_and(|c| app.move_agent(c).is_some());
    let mut hints = if accounts {
        let mut h = account_ui::new_intents(app);
        h.extend(key_hints(if card_moves {
            &[("A", "accounts"), ("S", "models")][..]
        } else {
            &[("s", "move"), ("A", "accounts"), ("S", "models")][..]
        }));
        h
    } else {
        card_hints(app)
    };
    if accounts {
        hints.push(Span::raw("   "));
        hints.extend(backlog_hints);
    }
    // With a backlog card's keys in front, the rest gives up what the menu already lists. A
    // column the board leaves out names the key for its own screen.
    let done_shown = app.visible_columns().last().copied().unwrap_or(false);
    let mut full = vec![
        ("c", "new intent"),
        ("⏎", "jump in"),
        ("space", "peek"),
        ("n N", "next that needs you"),
        ("/", "search everything"),
        ("f", "filter"),
        ("b", "backlog"),
    ];
    if !done_shown {
        full.push(("D", "done"));
    }
    full.extend([("ctrl+space", "menu"), ("?", "keys"), ("q", "quit")]);
    hints.extend(key_hints(if hints.is_empty() {
        &full
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

/// Work under way in a scope, and how much of it needs you, under f's filters.
fn tab_counts(app: &App, scope: &Scope) -> (usize, usize) {
    let mine = app
        .board
        .cards
        .iter()
        .filter(|c| scope.contains(c) && app.card_matches_filters(c) && c.stage != Stage::Misc);
    let (mut open, mut needs) = (0, 0);
    for c in mine {
        if c.stage.is_working() {
            open += 1;
        }
        if c.stage != Stage::Maintain && c.needs_you() {
            needs += 1;
        }
    }
    (open, needs)
}

/// An owner's or a team's tab on the owner split (intent 0595): its name, how much is under way,
/// and how much needs you. A team with nothing open is dimmed, there to start work in.
fn owner_tab(app: &App, selected: bool, label: String, scope: &Scope) -> Vec<Span<'static>> {
    let sel_style = Style::default()
        .bg(CYAN)
        .fg(BG)
        .add_modifier(Modifier::BOLD);
    let (open, needs) = tab_counts(app, scope);
    let base = if selected {
        sel_style
    } else if open == 0 {
        Style::default().bg(RAISED).fg(DIM)
    } else {
        Style::default().bg(RAISED).fg(FG)
    };
    let mut spans = vec![Span::styled(format!(" {label}"), base)];
    spans.push(Span::styled(
        format!(" {open}"),
        if selected { base } else { base.fg(DIM) },
    ));
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

fn tab_spans(app: &App, i: usize, scope: &Scope) -> Vec<Span<'static>> {
    let selected = i == app.tab;
    let sel_style = Style::default()
        .bg(CYAN)
        .fg(BG)
        .add_modifier(Modifier::BOLD);
    match scope {
        // On all of Kerdr the row lists owners; on an owner, its first tab is All.
        Scope::Owner(o) if app.owner_pick.is_some() => {
            owner_tab(app, selected, "All".into(), scope)
        }
        Scope::Owner(o) => owner_tab(app, selected, app.owner_short(o), scope),
        Scope::Team(o, t) => owner_tab(app, selected, app.team_name(o, t.as_deref()), scope),
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
            // The last good state's age, quietly, while a drop or a failing feed is short
            // (intent 0591).
            let now = app.now();
            let age = m
                .and_then(|m| {
                    let quiet_feed =
                        m.conn.is_live() && m.feed_error.is_some() && m.feed_problem(now).is_none();
                    let since = match m.conn {
                        Conn::Reconnecting { .. } => m.last_ok,
                        _ if quiet_feed => m.feed_taken,
                        _ => None,
                    };
                    since.map(|t| format!(" · cards {} old", ago(t, now)))
                })
                .unwrap_or_default();
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
            if !age.is_empty() {
                spans.push(Span::styled(age, fg(DIM)));
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

/// What the bar's first box says: the machine, on the machine split; on the owner split all of
/// Kerdr's work, or the owner, with its code and how full its main session is on the machine its
/// work runs on (`Tinyhost · TNY 1/3`).
fn level_label(app: &App) -> String {
    if !app.owner_split() {
        return app.machine.as_deref().unwrap_or("All machines").to_string();
    }
    let Some(o) = app.owner_pick.as_deref() else {
        return "All work".into();
    };
    let Some(info) = app.owner_info(o) else {
        return app.owner_short(o);
    };
    let mut label = info.label().to_string();
    if let Some(code) = info.code.as_deref() {
        label.push_str(&format!(" · {code}"));
        let machine = info
            .agent_machine
            .as_deref()
            .and_then(|m| app.machine_index(m))
            .map(|mi| app.machines[mi].name());
        if let Some(row) = app.board.sessions.iter().find(|r| {
            r.session == code && r.running && machine.as_deref().is_none_or(|m| r.machine == m)
        }) {
            label.push_str(&format!(" {}", row.slots.label()));
        }
    }
    label
}

fn header(buf: &mut Buffer, rect: Rect, app: &App) {
    let machine = level_label(app);
    let label = format!(" {} ▾ ", trunc(&machine, (rect.width as usize / 3).max(8)));
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
        .or_else(|| app.owner_machine())
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
    // The current account leads, and stays when the usage figures don't fit (intent 0455).
    let current = usage_machine
        .map(|mi| account_ui::current_tag(app, mi))
        .unwrap_or_default();
    let mut usage = usage_machine
        .map(|mi| account_ui::usage(app, mi))
        .unwrap_or_default();
    if !current.is_empty() {
        let mut both = current.clone();
        both.push(Span::raw(" "));
        both.extend(usage);
        usage = both;
    }
    if app.machine.is_none()
        && app.owner_pick.is_none()
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
    // When usage won't fit, room is kept for the current account's code, so the tabs shown
    // around the selected one leave it space (intent 0455).
    let short: Vec<Span<'static>> = usage_machine
        .map(|mi| account_ui::current_short(app, mi))
        .unwrap_or_default();
    let short_w: usize = short.iter().map(|s| width(&s.content)).sum();
    let reserve = if rect.width as usize > fixed + usage_w + 18 {
        usage_w
    } else if short_w > 0 {
        short_w + 2
    } else {
        0
    };
    let avail = (rect.width as usize).saturating_sub(fixed + reserve);
    // The level's own tab is the box in front, but for an owner's All tab, which is in the row.
    let skip = if app.owner_split() && app.owner_pick.is_some() {
        0
    } else {
        1
    };
    let tabs: Vec<_> = app
        .tabs
        .iter()
        .enumerate()
        .skip(skip)
        .map(|(i, scope)| tab_spans(app, i, scope))
        .collect();
    let widths: Vec<usize> = tabs
        .iter()
        .map(|t| t.iter().map(|s| width(&s.content)).sum())
        .collect();
    let selected = app
        .tab
        .saturating_sub(skip)
        .min(tabs.len().saturating_sub(1));
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
        right.push(Span::styled(
            if app.owner_split() {
                "{ } owner · [ ] team"
            } else {
                "{ } machine · [ ] session"
            },
            fg(DIM),
        ));
    }
    right.push(Span::styled(format!(" · {}", clock(app.now())), fg(DIM)));
    let lw: usize = left.iter().map(|s| width(&s.content)).sum();
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let cw: usize = current.iter().map(|s| width(&s.content)).sum();
    if lw + rw + 2 <= rect.width as usize {
        put_lr(buf, rect, rect.y, left, right);
    } else if cw > 0 && lw + cw + 2 <= rect.width as usize {
        put_lr(buf, rect, rect.y, left, current);
    } else if short_w > 0 {
        // The code alone, at any width; the tabs were laid out leaving room for it.
        put_lr(buf, rect, rect.y, left, short);
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
        || app.machine_filter.is_some()
        || app.project.is_some()
        || app.provider.is_some()
        || app.background != Background::All
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
    if let Some(m) = &app.machine_filter {
        shown.push(format!("on {m}"));
    }
    if app.provider.is_some() {
        shown.push(app.provider_label());
    }
    if app.background != Background::All {
        shown.push(app.background.label().to_string());
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
        let what = app
            .busy_line()
            .unwrap_or_else(|| "action in progress".to_string());
        vec![Span::styled(format!("◐ {what}"), fg(BLUE))]
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
    let troubled = app.troubled_machines();
    let reading = read_troubles(app);
    let Some(&i) = troubled.first() else {
        return reading_banner(reading);
    };
    let m = &app.machines[i];
    let name = m.name();
    // Every card of it the board shows, whatever its column: all of them are dimmed.
    let cards = app
        .board
        .cards
        .iter()
        .filter(|c| c.machine == name && app.card_visible(c))
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
                    format!(
                        "⟳ {name} dropped {} ago: {}",
                        ago(*since, now),
                        m.conn.cause().unwrap_or("it can't be reached")
                    ),
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
                    match cards {
                        0 => format!("   its cards are as they were at {t}"),
                        1 => format!("   its 1 card is dimmed, as it was at {t}"),
                        n => format!("   its {n} cards are dimmed, as they were at {t}"),
                    },
                    fg(DIM),
                ));
            }
            v
        }
        Conn::Unreachable {
            since, retry_at, ..
        } => {
            let mut v = vec![
                Span::styled(format!("○ can't reach {name}"), bold(FG2)),
                Span::styled(
                    format!(
                        "   no answer since {} ({})",
                        clock(*since),
                        m.conn.cause().unwrap_or("it doesn't answer")
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
    // The rest, its machines' feed or herdr reads that keep failing too: named here, said on M.
    let more: Vec<String> = troubled[1..]
        .iter()
        .map(|&j| app.machines[j].name())
        .chain(reading.into_iter().map(|(name, ..)| name))
        .collect();
    left.extend(more_span(&more));
    let right = match &m.conn {
        Conn::HostKey { .. } | Conn::Auth { .. } => key_hints(&[("M", "then ⏎ answers it")]),
        Conn::HerdrMissing | Conn::HerdrTooOld { .. } => key_hints(&[("M", "then u sets it up")]),
        _ => key_hints(&[("r", "retry now"), ("M", "machines")]),
    };
    Some((left, right))
}

/// The machines that answer, but whose feed or herdr reads have kept failing past the quiet
/// spell (intent 0591): each one's name, why in plain words (the feed's own reason first), and
/// since when.
fn read_troubles(app: &App) -> Vec<(String, String, chrono::DateTime<chrono::Local>)> {
    let now = app.now();
    app.machines
        .iter()
        .filter(|m| m.is_shown() && m.conn.is_live())
        .filter_map(|m| {
            let (why, since) = m.lasting_problem(now)?;
            Some((m.name(), why, since))
        })
        .collect()
}

/// "+2 more: laptop, mac" for the banner, naming the other machines in trouble; nothing for none.
fn more_span(more: &[String]) -> Option<Span<'static>> {
    (!more.is_empty()).then(|| {
        Span::styled(
            format!("   +{} more: {}", more.len(), more.join(", ")),
            fg(DIM),
        )
    })
}

/// With every machine answering: one line about the first whose feed or herdr reads have kept
/// failing past the quiet spell (intent 0591), said once, in plain words, the feed's own reason
/// first. Its rows and cards are marked out of date meanwhile; this says why.
fn reading_banner(
    troubled: Vec<(String, String, chrono::DateTime<chrono::Local>)>,
) -> Option<(Vec<Span<'static>>, Vec<Span<'static>>)> {
    let (name, why, since) = troubled.first()?;
    let mut left = vec![
        Span::styled(format!("● {name}: {why}"), bold(YELLOW)),
        Span::styled(
            format!(
                "   since {}; what's shown of it is from its last good read",
                clock(*since)
            ),
            fg(DIM),
        ),
    ];
    let more: Vec<String> = troubled[1..]
        .iter()
        .map(|(name, ..)| name.clone())
        .collect();
    left.extend(more_span(&more));
    Some((left, key_hints(&[("M", "machines")])))
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

/// What a remembered ⏎ on a starting card says for as long as it lasts: messages go after 8 s, and a
/// start can run for minutes before herdr takes the screen (intent 0491).
fn remembered_line(app: &App) -> Option<Vec<Span<'static>>> {
    let card = app.card(app.enter_when_open.as_deref()?)?;
    // In followed work's own words: a move into herdr isn't a start (intent 0591).
    let doing = app.follow_doing(card).unwrap_or("is starting");
    Some(vec![
        Span::styled(format!("{} ", card.reference), bold(CYAN)),
        Span::styled(format!("{doing}. "), fg(FG)),
        Span::styled(
            "Kerdr goes in when its workspace opens · esc stops waiting",
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
            if s.running > 0 { fg(GREEN) } else { fg(DIM) },
        ),
        Span::raw("   "),
    ];
    if s.waiting > 0 {
        left.push(Span::styled(format!("◇ {} waiting", s.waiting), fg(BLUE)));
        left.push(Span::raw("   "));
    }
    left.extend([Span::styled(
        format!("■ {} blocked", s.blocked),
        if s.blocked > 0 { bold(RED) } else { fg(DIM) },
    )]);
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
    let layout = app.layout();
    let vis = app.visible_in(&layout);
    let cols = app.columns_in(&layout);
    let done = layout.done();
    let band_cols = |b: &crate::stages::Band| -> Vec<usize> {
        (b.first..b.first + b.len).filter(|&c| vis[c]).collect()
    };
    // The widest open band sets the columns' width, as the one band always did (intent 0595).
    let widest = layout
        .bands
        .iter()
        .filter(|b| !b.folded)
        .map(|b| band_cols(b).len())
        .max()
        .unwrap_or(0);
    let sides = usize::from(vis[0]) + usize::from(vis[done]);
    let n = ((sides + widest) as u16).max(1);
    let gap = if rect.width > 160 { 2 } else { 1 };
    let col_w = (rect.width.saturating_sub(gap * (n - 1))) / n;
    let x_of = |k: u16| rect.x + k * (col_w + gap);
    let full = |x: u16| Rect {
        x,
        y: rect.y,
        width: col_w,
        height: rect.height,
    };
    if vis[0] {
        column(f, full(x_of(0)), app, &layout.columns[0], &cols[0]);
    }
    if vis[done] {
        column(
            f,
            full(x_of(n - 1)),
            app,
            &layout.columns[done],
            &cols[done],
        );
    }
    // Between them, the bands, stacked: a folded one is a line; the open ones share the rest.
    let first = u16::from(vis[0]);
    let mid_x = x_of(first);
    let mid_w = if widest == 0 {
        0
    } else {
        x_of(first + widest as u16 - 1) + col_w - mid_x
    };
    let banded = layout.banded();
    let folded = layout.bands.iter().filter(|b| b.folded).count() as u16;
    let room = rect.height.saturating_sub(folded);
    // Each open band asks for what its fullest column needs; when they don't all fit, each gets
    // its share of the room, never less than a few cards' worth.
    let wants: Vec<u16> = layout
        .bands
        .iter()
        .map(|b| {
            if b.folded {
                return 1;
            }
            let most = band_cols(b)
                .iter()
                .map(|&c| cols[c].len())
                .max()
                .unwrap_or(0) as u16;
            u16::from(banded) + 2 + most.max(1) * 5 + 1
        })
        .collect();
    let open_want: u16 = layout
        .bands
        .iter()
        .zip(&wants)
        .filter(|(b, _)| !b.folded)
        .map(|(_, w)| *w)
        .sum();
    let heights: Vec<u16> = layout
        .bands
        .iter()
        .zip(&wants)
        .map(|(b, &w)| {
            if b.folded {
                1
            } else if open_want <= room {
                w
            } else {
                (u32::from(room) * u32::from(w) / u32::from(open_want.max(1))) as u16
            }
            .max(if b.folded {
                1
            } else {
                crate::stages::BAND_ROWS.min(room)
            })
        })
        .collect();
    let last_open = layout.bands.iter().rposition(|b| !b.folded);
    let mut y = rect.y;
    for (bi, band) in layout.bands.iter().enumerate() {
        if y >= rect.bottom() {
            break;
        }
        let line = Rect {
            x: mid_x,
            y,
            width: mid_w,
            height: 1,
        };
        if band.folded {
            band_title(f.buffer_mut(), line, band, true);
            y += 1;
            continue;
        }
        // The last open band takes what's left over, less the folded bands below it.
        let below = layout.bands[bi + 1..].iter().filter(|b| b.folded).count() as u16;
        let open_below = layout.bands[bi + 1..].iter().filter(|b| !b.folded).count() as u16;
        let h = if Some(bi) == last_open {
            rect.bottom().saturating_sub(y + below)
        } else {
            // Leave the open bands below room for a card each.
            heights[bi]
                .min(
                    rect.bottom()
                        .saturating_sub(y + below + open_below * crate::stages::BAND_ROWS),
                )
                .max(crate::stages::BAND_ROWS.min(rect.bottom().saturating_sub(y)))
        };
        let mut top = y;
        if banded {
            band_title(f.buffer_mut(), line, band, false);
            top += 1;
        }
        let shown = band_cols(band);
        let m = (shown.len() as u16).max(1);
        let w = (mid_w.saturating_sub(gap * (m - 1))) / m;
        for (k, &ci) in shown.iter().enumerate() {
            let r = Rect {
                x: mid_x + k as u16 * (w + gap),
                y: top,
                width: w,
                height: (y + h).saturating_sub(top),
            };
            column(f, r, app, &layout.columns[ci], &cols[ci]);
        }
        y += h;
    }
}

/// A band's title on a board that mixes stage lists: whose list it is and who's on it, and, folded,
/// how much is in it.
fn band_title(buf: &mut Buffer, rect: Rect, band: &crate::stages::Band, folded: bool) {
    let mut left = vec![Span::styled(if folded { "▸ " } else { "▾ " }, fg(CYAN))];
    left.push(Span::styled(band.title.clone(), bold(FG2)));
    let mut right = vec![];
    if band.open == 0 {
        right.push(Span::styled("nothing under way", fg(DIM)));
    } else {
        right.push(Span::styled(format!("{} under way", band.open), fg(FG2)));
        if band.needs > 0 {
            right.push(Span::styled(format!(" ●{}", band.needs), bold(ORANGE)));
        }
        if folded {
            right.push(Span::styled(" · j k open it", fg(DIM)));
        }
    }
    put_lr(buf, rect, rect.y, left, right);
}

fn column(f: &mut Frame, rect: Rect, app: &App, col: &crate::stages::Column, items: &[usize]) {
    let stage = col.stage;
    let sub = match stage {
        Stage::Maintain => "done".to_string(),
        Stage::Backlog => "your order".to_string(),
        _ if col.unknown => "unknown stage".to_string(),
        _ if col.retired => "retired".to_string(),
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
                Style::default()
                    .fg(if col.unknown { YELLOW } else { FG2 })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", items.len()), bold(FG)),
        ],
        if items.is_empty() && !col.unknown {
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
        let (mut glyph, mut gcolor, mut meta, mut mcolor) =
            list_glyph(c, now, body.width as usize / 2);
        if app.starts_when_written(&c.key) {
            meta = "starts once written up".into();
        }
        // A card holding work says so with ▸; a done one with work still open says how much,
        // yellow only when some of it needs you. One whose card is out of view names that card.
        let holds = app.holds(i);
        if holds.open > 0 && c.stage == Stage::Maintain {
            meta = format!("{} open", holds.open);
            mcolor = if holds.needs > 0 { YELLOW } else { FG2 };
        }
        // Not started, but holding a piece that waits on you.
        if c.stage == Stage::Backlog && holds.needs > 0 && !c.dismissed() {
            (glyph, gcolor) = (Attention::NeedsYou.glyph(), YELLOW);
            (meta, mcolor) = (Attention::NeedsYou.label().into(), YELLOW);
        }
        if let Some(r) = app.held_elsewhere(i) {
            meta = format!("in {r}");
            mcolor = CYAN;
        }
        // A question waits on it for you (intent 0591).
        if let Some(line) = app.waiting_line_for(&c.key) {
            (glyph, gcolor) = (Attention::NeedsYou.glyph(), YELLOW);
            (meta, mcolor) = (line.to_string(), YELLOW);
        }
        let marker = if holds.count > 0 { "▸" } else { "" };
        let tag = app
            .whose_tag(c)
            .map(|t| format!(" {t}"))
            .unwrap_or_default();
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
        // On a narrow column the note on the right gives way before the card's number does: the
        // glyph's colour still says it.
        if width(glyph) + 1 + width(&c.reference) + width(marker) + 1 + width(&meta)
            > body.width as usize
        {
            meta.clear();
        }
        put_lr(
            buf,
            line_rect,
            y,
            vec![
                Span::styled(format!("{glyph} "), fg(gcolor)),
                Span::styled(c.reference.clone(), text_style),
                Span::styled(marker, fg(CYAN)),
                Span::styled(tag, fg(CYAN)),
                Span::styled(format!(" {}", c.title), text_style),
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

/// The most of a write-up's reason any card shows: a few words, as the feed promises.
const WRITE_UP_WHY_MOST: usize = 60;

/// What a card says of an intent open in more than one workspace, from its feed's `open_in`: two
/// agents may be at it. None for one place or none, as older feeds say (intent 0591).
fn open_in_line(c: &Card) -> Option<String> {
    const NAMED_MOST: usize = 3;
    let places = &c.detail.as_ref()?.open_in;
    if places.len() < 2 {
        return None;
    }
    let mut named: Vec<String> = places
        .iter()
        .take(NAMED_MOST)
        .map(|p| {
            trunc(
                &p.chars().filter(|ch| !ch.is_control()).collect::<String>(),
                40,
            )
        })
        .collect();
    if places.len() > NAMED_MOST {
        named.push(format!("{} more", places.len() - NAMED_MOST));
    }
    Some(format!(
        "open in {} workspaces ({}), so two agents may be at it",
        places.len(),
        named.join(", ")
    ))
}

/// Why a card's write-up failed, in its feed's words on one line and cut to fit; None when the
/// feed doesn't say, as older feeds don't (intent 0591).
fn write_up_why(c: &Card) -> Option<String> {
    let why = c.detail.as_ref()?.intake_why.as_deref()?;
    let why: String = why
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|ch| !ch.is_control())
        .collect();
    (!why.is_empty()).then(|| trunc(&why, WRITE_UP_WHY_MOST))
}

/// What a list row says of a card whose write-up failed, in at most `room` columns when it gives
/// a reason: without room for any of that, the plain words, which never shrink.
fn not_written_up(c: &Card, room: usize) -> String {
    const PLAIN: &str = "not written up";
    match write_up_why(c) {
        // ": ", and at least one character of it before the "…".
        Some(why) if room >= width(PLAIN) + 4 => trunc(&format!("{PLAIN}: {why}"), room),
        _ => PLAIN.into(),
    }
}

/// A list row's glyph, and its note on the right, which takes at most `room` columns.
fn list_glyph(
    c: &Card,
    now: chrono::DateTime<chrono::Local>,
    room: usize,
) -> (
    &'static str,
    ratatui::style::Color,
    String,
    ratatui::style::Color,
) {
    // A start or a reopen on its way (intents 0416 and 0445): shown the moment it's sent,
    // whatever the column, a start on another machine too. Any change, a new intent's words
    // included, being sent again by itself says so, quietly (intent 0591).
    match c.detail.as_ref().and_then(|d| d.status.as_deref()) {
        Some(crate::resolve::RETRYING_SAVE) => {
            return ("◐", BLUE, "saving, trying again…".into(), BLUE);
        }
        Some(crate::resolve::RETRYING) => return ("◐", BLUE, "trying again…".into(), BLUE),
        Some(s) if s.starts_with("Starting") => return ("◐", BLUE, "starting up".into(), BLUE),
        Some("Reopening…") => return ("◐", BLUE, "reopening".into(), BLUE),
        _ => {}
    }
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
        // A new intent still being saved (intent 0445).
        _ if c.intake() == Some(crate::resolve::SAVING) => ("◐", BLUE, "saving…".into(), BLUE),
        _ if c.intake() == Some("pending") => ("✎", PURPLE, "writing it up".into(), PURPLE),
        _ if c.intake() == Some("failed") => ("✎", YELLOW, not_written_up(c, room), YELLOW),
        // Not started, but waiting on you all the same: questions asked before it starts.
        _ if c.needs_you() && !c.dismissed() => (
            Attention::NeedsYou.glyph(),
            YELLOW,
            Attention::NeedsYou.label().into(),
            YELLOW,
        ),
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
    note: (String, ratatui::style::Color),
    age: (String, ratatui::style::Color),
    /// An account code is never cut: the age gives way to it instead.
    whole: bool,
}

fn card_lines(app: &App, c: &Card, inner_w: usize) -> CardLines {
    let now = app.now();
    let cached = c.cached_at.is_some();
    let mut head = vec![Span::styled(
        c.reference.clone(),
        bold(if cached { FG2 } else { FG }),
    )];
    if let Some(tag) = app.whose_tag(c) {
        head.push(Span::styled(format!(" {tag}"), fg(CYAN)));
    }
    if c.priority == Priority::High {
        head.push(Span::styled(" ▲", fg(YELLOW)));
    }
    if c.background {
        head.push(Span::styled(" bg", fg(PURPLE)));
    }
    if let Some(r) = app.card_index(&c.key).and_then(|i| app.held_elsewhere(i)) {
        head.push(Span::styled(" in ", fg(DIM)));
        head.push(Span::styled(r, fg(CYAN)));
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
        // ⇄: handed over from another program, when there's room beside the code for it.
        note = account
            .map(|a| {
                if a.handed_from.is_some() && width(&a.account) + 2 <= inner_w {
                    format!("{} ⇄", a.account)
                } else {
                    a.account.clone()
                }
            })
            .unwrap_or_default();
    }
    // Open work its agent left, or whose workspace closed under it: say so where it shows
    // (intent 0289). C marks it done and x sets it aside, each starting nothing.
    // Its conversation runs in a terminal outside herdr, so no workspace shows it (intent 0411).
    let note = if let Some(line) = app.waiting_line_for(&c.key) {
        // A question waits on it for you: it says so, as asking you, never as a failure (intent
        // 0591).
        (line.to_string(), YELLOW)
    } else if let Some(outside) = c.outside_herdr().filter(|_| !cached) {
        (format!("outside herdr · {}", outside.short()), CYAN)
    } else if c.no_agent() && !cached {
        ("no agent".to_string(), YELLOW)
    } else {
        (note, DIM)
    };
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
        whole: modern && note.1 == DIM && !note.0.is_empty(),
        note,
        age,
    }
}

fn cards(f: &mut Frame, body: Rect, app: &App, items: &[usize], selected: Option<&str>) {
    let inner_w = body.width.saturating_sub(4) as usize;
    let compact = body.height < 18;
    let holds: std::collections::HashMap<usize, crate::family::Holds> =
        items.iter().map(|&i| (i, app.holds(i))).collect();
    let all: Vec<(usize, CardLines)> = items
        .iter()
        .map(|&i| (i, card_lines(app, &app.board.cards[i], inner_w)))
        .collect();
    let height = |i: usize, cl: &CardLines| -> u16 {
        let title = if compact {
            1
        } else {
            cl.title.len().max(1) as u16
        };
        // A card holding work has one more line: what's inside it.
        let tray = u16::from(holds.get(&i).is_some_and(|h| h.count > 0));
        2 + 1 + title + 1 + tray
    };
    let sel_pos = all
        .iter()
        .position(|(i, _)| Some(app.board.cards[*i].key.as_str()) == selected);
    // Scroll so the selected card is in view.
    let mut start = 0;
    if let Some(p) = sel_pos {
        loop {
            let used: u16 = all[start..=p].iter().map(|(i, cl)| height(*i, cl)).sum();
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
        let h = height(*i, cl);
        let held = &holds[i];
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
        // A box means it's selected or waiting on you, and nothing else does (intent 0401).
        let on_you = crate::family::on_you(c, held);
        let rect = Rect {
            x: body.x,
            y,
            width: body.width,
            height: h,
        };
        if is_sel || on_you.is_some() {
            let border = match on_you {
                _ if is_sel => CYAN,
                Some(crate::family::OnYou::Asking) => YELLOW,
                Some(crate::family::OnYou::Blocked) => RED,
                _ => DIM,
            };
            let (fill, kind) = if is_sel {
                (SEL, BorderType::Thick)
            } else {
                (PANEL, BorderType::Rounded)
            };
            panel(f, rect, border, fill, kind);
        }
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
        let crowded = width(&cl.note.0) + 1 + width(&cl.age.0) > inner.width as usize;
        let age = if cl.whole && crowded {
            String::new()
        } else {
            cl.age.0.clone()
        };
        put_lr(
            buf,
            inner,
            note_y,
            vec![Span::styled(cl.note.0.clone(), fg(cl.note.1))],
            vec![Span::styled(age, fg(cl.age.1))],
        );
        if held.count > 0 {
            // The card's line: how many inside, a few marks, and what needs you, named. It uses
            // the card's whole inside width, one column in from its border.
            let w = rect.width.saturating_sub(2);
            let line = held.line(w as usize);
            let used: usize = line.iter().map(|(t, _)| width(t)).sum();
            let gap = u16::from(used < w as usize);
            put(
                buf,
                rect.x + 1 + gap,
                note_y + 1,
                w - gap,
                card_ui::holds_spans(line),
            );
        }
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
        (
            "s",
            "move this agent to another account, keeping its conversation",
        ),
        ("S", "the list new agents start on"),
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
    // What's on its way for it says so first: a save, a start or a reopen (intent 0445).
    if let Some(line) = app.starting_line(c) {
        facts.push(line);
    } else if c.workspace_id.is_none() {
        facts.push(match c.stage {
            Stage::Backlog if c.intake() == Some("pending") => {
                "✎ being written up from your words; d starts it once that's done".into()
            }
            Stage::Backlog if c.intake() == Some("failed") => match write_up_why(c) {
                Some(why) => {
                    format!("✎ not written up ({why}): the agent that starts it writes it up")
                }
                None => "✎ not written up: the agent that starts it writes it up".into(),
            },
            Stage::Backlog => "not started".into(),
            Stage::Maintain => "closed".into(),
            _ if app.workspaces_stale(c) => "workspace not known: herdr didn't answer".into(),
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
    if let Some(line) = open_in_line(c) {
        facts.push(line);
    }
    if let Some(t) = c.cached_at {
        facts.push(format!("as last seen at {}", clock(t)));
    }
    lines.extend(wrapped(&facts.join(" · "), w, fg(DIM), None));

    if let Some(agent) = app.account_agent(c).cloned().or_else(|| app.move_agent(c)) {
        lines.push(Line::raw(""));
        lines.push(section("ACCOUNT"));
        let providers = app.providers(c.machine_index);
        let account = app.account(c.machine_index, &agent.account);
        let state = account
            .map(|a| {
                format!(
                    "{} · {} · {}",
                    providers.label(&a.provider),
                    app.account_numbers(a),
                    a.status_label()
                )
            })
            .unwrap_or_default();
        lines.extend(wrapped(
            &if agent.account.is_empty() {
                "No account recorded for this agent yet: s moves it to one and records it."
                    .to_string()
            } else {
                format!(
                    "{}  {state} · {}",
                    agent.account,
                    match agent.how.as_str() {
                        "manual" => "moved by hand",
                        "next" => "next in the list",
                        "handed_over" => "handed over to a fresh agent",
                        "current" => "started on the current account",
                        _ => "follows the list",
                    }
                )
            },
            w,
            fg(FG2),
            None,
        ));
        // Handed over from another program: from which account and program, why and when.
        if let Some(from) = &agent.handed_from {
            let now_runs = app.agent_program(c.machine_index, &agent).or_else(|| {
                account
                    .filter(|a| !a.runs.is_empty())
                    .map(|a| providers.program_label(&a.runs))
            });
            let when = from
                .at
                .as_deref()
                .map(|t| format!(" at {}", crate::accounts::moment(t, now)))
                .unwrap_or_default();
            // A program the feed doesn't name is left out of the sentence rather than guessed.
            let fresh = now_runs
                .as_deref()
                .map_or("A fresh agent".into(), |p| format!("A fresh {p} agent"));
            let stays = now_runs
                .as_deref()
                .map_or("It stays".into(), |p| format!("It stays on {p}"));
            lines.extend(wrapped(
                &format!(
                    "Handed over from {} ({}){when}: {}{}. {fresh} picked up from the log.",
                    from.account,
                    providers.program_label(&from.program),
                    from.reason(),
                    if from.why == "sign_in" {
                        " (⏎ on it in A signs it in again)"
                    } else {
                        ""
                    }
                ),
                w,
                fg(YELLOW),
                None,
            ));
            lines.extend(wrapped(
                &format!("{stays} until {} runs out or the work ends.", agent.account),
                w,
                fg(DIM),
                None,
            ));
        }
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
    // The card's own list's stages, between Backlog and Done (intent 0595).
    let rail = app.rail(c);
    let pos = rail.iter().position(|s| *s == c.stage).unwrap_or(0);
    let cell = (w / rail.len().max(1)).max(8);
    let mut names = Vec::new();
    let mut notes = Vec::new();
    for (k, s) in rail
        .iter()
        .copied()
        .enumerate()
        .filter(|_| c.stage != Stage::Misc)
    {
        let (glyph, style) = if k == pos {
            (
                "●",
                bold(if c.attention == Attention::NeedsYou {
                    ORANGE
                } else {
                    CYAN
                }),
            )
        } else if k < pos {
            ("✓", fg(GREEN))
        } else {
            ("○", fg(DIM))
        };
        let name_style = if k == pos {
            style
        } else if k < pos {
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
                d.timeline.iter().rev().find(|t| {
                    t.stage == s.id()
                        || (s.list().is_none_or(|l| l == crate::model::STANDARD)
                            && Stage::parse(&t.stage) == Some(s))
                })
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
            "Answer in its pane: ⏎ jumps in.".to_string()
        } else {
            where_to_answer(app, c)
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
    // Work whose workspace closed says what ⏎ does for it: reopen it, resuming its agent. Done
    // work says ⏎ asks first, since a yes opens it again.
    let program = c
        .resume_agent()
        .map(|id| app.providers(c.machine_index).program_label(id));
    let reopen = match (c.can_reopen_done(), program) {
        (false, Some(agent)) => format!("reopen it, resuming its {agent} conversation"),
        (false, None) => "reopen its workspace".to_string(),
        (true, Some(agent)) => format!("ask to reopen it, resuming its {agent} conversation"),
        (true, None) => "ask to reopen it".to_string(),
    };
    // ⏎ starts waiting work too, as on the board, but not in a peek from the backlog screen,
    // whose ⏎ peeks (intent 0404).
    let start = match &app.mode {
        Mode::Peek { back, .. } if !matches!(**back, Mode::Backlog { .. }) && app.actions_on() => {
            "d ⏎"
        }
        _ => "d",
    };
    let mut hints = match app.backlog_card_for_keys() {
        Some(c) if c.dismissed() => vec![("u", "bring it back")],
        Some(_) => vec![
            (start, "start it in its session"),
            ("e", "edit its text"),
            ("y", "still want it"),
            ("x", "won’t do"),
            ("C", "done"),
            ("J K", "move down / up"),
            ("t B", "to the top / bottom"),
        ],
        None if c.reopenable() && app.actions_on() => vec![("⏎", reopen.as_str())],
        None => vec![("⏎", "jump in")],
    };
    // Started work with no workspace ends without an agent too (intent 0289).
    if app.started_card_for_keys().is_some() && app.actions_on() {
        hints.extend([("C", "done"), ("x", "won’t do")]);
    }
    // Work an agent is at: its agent to another account, or cancelled (intent 0321).
    let moving = move_hint(app, c);
    if app.running_card_for_keys().is_some() {
        if let Some(m) = &moving {
            hints.push(("s", m.as_str()));
        }
        if app.actions_on() {
            hints.push(("X", "cancel"));
        }
    } else if let Some(m) = &moving {
        // Nothing running it: s starts it on an account you pick (intent 0580).
        hints.push(("s", m.as_str()));
    }
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
        centered(area, 76, if one_column { 25 } else { 16 }, Some(1))
    };
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let card = app.card_for_keys();
    let this_card = card
        .map(|c| format!("{} {}", c.reference, c.title))
        .unwrap_or_else(|| "select a card first".into());
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
    let started = app.backlog_card_for_keys().is_none() && card.is_some();
    let running = app.backlog_card_for_keys().is_none()
        && app.started_card_for_keys().is_none()
        && app.running_card_for_keys().is_some();
    let card_keys: &[(&str, &str)] = if restore {
        &[("u", "bring it back")]
    } else if running {
        &[
            ("s", "move its agent to another account, or start it on one"),
            ("X", "cancel: stop its agent, won’t do"),
        ]
    } else if started {
        &[
            ("s", "start it again on an account you pick"),
            ("C", "done, starting nothing"),
            ("x", "won’t do"),
        ]
    } else {
        &[
            ("d", "start it"),
            ("e", "edit its text"),
            ("y", "still want it"),
            ("x", "won’t do"),
            ("C", "done"),
            ("J K", "move down / up"),
            ("t B", "move to the top / bottom"),
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
                ("w D", "won’t do / done work"),
                ("i", "workspaces with no intent"),
                ("f", "filter by who, agents or bg"),
                ("a", "agents: all, then each one"),
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
        field_lines(&draft.words, text.width.max(1) as usize, rows, true)
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
    let filed_to = draft
        .filed
        .tenant
        .clone()
        .or(draft.filed.organization.clone());
    let where_line = match (&filed_to, app.owner_split()) {
        // On the owner split, whose backlog it's in (intent 0595).
        (Some(owner), _) => {
            let name = app
                .owner_info(owner)
                .map(|o| o.label().to_string())
                .unwrap_or_else(|| owner.clone());
            let team = draft
                .filed
                .team
                .as_deref()
                .map(|t| format!("{}'s backlog", app.team_name(owner, Some(t))))
                .unwrap_or_else(|| "backlog, under No team".into());
            vec![
                Span::styled("It goes in ", fg(FG2)),
                Span::styled(name, bold(FG)),
                Span::styled(format!("'s {team}, until you start it."), fg(FG2)),
            ]
        }
        (None, true) => vec![Span::styled(
            "It goes in the backlog, filed under whoever your words say it's for.",
            fg(FG2),
        )],
        (None, false) => vec![
            Span::styled("It goes in the backlog of ", fg(FG2)),
            Span::styled(draft.session.clone(), bold(FG)),
            Span::styled(" on ", fg(FG2)),
            Span::styled(machine, bold(FG)),
            Span::styled(
                ", the session you're looking at, until you start it.",
                fg(FG2),
            ),
        ],
    };
    put(buf, inner.x, y, inner.width, fit(where_line, w));
    let said = match &draft.unfiled {
        Some(note) => note.clone(),
        None => "Say \"start it now\", or name another session or who it's for, and the agent that writes it up does that instead.".to_string(),
    };
    let tone = if draft.unfiled.is_some() { YELLOW } else { DIM };
    for (n, line) in wrap(&said, w, 2).into_iter().enumerate() {
        put(
            buf,
            inner.x,
            y + 1 + n as u16,
            inner.width,
            vec![Span::styled(line, fg(tone))],
        );
    }
    // Nothing of his holds the box: an account action, or an earlier intent still saving, runs
    // on its own (intent 0445).
    let hints = key_hints(&[
        ("⏎", "send"),
        ("shift+⏎ / alt+⏎", "new line"),
        ("ctrl+v", "image"),
        ("ctrl+u", "clear"),
        ("esc", "keep for later"),
    ]);
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(hints, w),
    );
}

/// The filter list (f): who the work is for, or one of their projects.
/// One filter's chips, the one in force drawn selected.
fn chips(label: &str, options: &[(&str, bool)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(format!("{label:<12}"), bold(FG2))];
    for (name, on) in options {
        spans.push(Span::styled(
            format!(" {name} "),
            if *on {
                Style::default()
                    .bg(BORDER)
                    .fg(FG)
                    .add_modifier(Modifier::BOLD)
            } else {
                fg(FG2)
            },
        ));
        spans.push(Span::raw(" "));
    }
    spans
}

fn filter_view(f: &mut Frame, area: Rect, app: &App, query: &str, sel: usize, typing: bool) {
    let rows = app.filter_matches(query);
    let list_h = (rows.len() as u16)
        .min(area.height.saturating_sub(16))
        .max(1);
    let rect = centered(area, 84, list_h + 12, Some(4));
    dialog(f, rect, CYAN);
    let inner = pad(rect, 3, 1);
    let w = inner.width as usize;
    let buf = f.buffer_mut();
    put_lr(
        buf,
        inner,
        inner.y,
        vec![
            Span::styled("FILTERS", bold(CYAN)),
            Span::styled("   what the board shows", fg(FG2)),
        ],
        vec![Span::styled("esc close", fg(DIM))],
    );
    put_lr(
        buf,
        inner,
        inner.y + 2,
        vec![Span::styled("WHO IT’S FOR", bold(FG2))],
        if typing {
            vec![]
        } else {
            key_hints(&[("/", "search")])
        },
    );
    put(
        buf,
        inner.x,
        inner.y + 3,
        inner.width,
        if typing {
            vec![
                Span::styled("› ", bold(CYAN)),
                Span::styled(query.to_string(), bold(FG)),
                Span::styled("▏", fg(CYAN)),
                Span::styled("      esc stops searching", fg(DIM)),
            ]
        } else {
            vec![
                Span::styled("› ", bold(CYAN)),
                Span::styled("press / to search", fg(DIM)),
            ]
        },
    );
    let top = inner.y + 5;
    let agents_y = inner.bottom().saturating_sub(4);
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
        let on = app.filter_row_on(row);
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
        if y >= agents_y.saturating_sub(1) {
            break;
        }
        if n == sel {
            buf.set_style(line, Style::default().bg(SEL));
        }
        put_lr(buf, line, y, left, vec![Span::styled(counts, fg(FG2))]);
    }
    // Every program the feeds name, then any other herdr reports, each in its feed's words.
    let provider = app.provider.as_deref();
    let names = app.all_providers();
    let mut labels = vec![("all".to_string(), provider.is_none())];
    labels.extend(
        app.agent_programs()
            .into_iter()
            .map(|id| (names.program_label(&id), provider == Some(id.as_str()))),
    );
    let agents: Vec<(&str, bool)> = labels.iter().map(|(l, on)| (l.as_str(), *on)).collect();
    put_lr(
        buf,
        Rect {
            y: agents_y,
            height: 1,
            ..inner
        },
        agents_y,
        chips("AGENTS", &agents),
        key_hints(&[("tab", "here"), ("a", "anywhere")]),
    );
    let background: Vec<(&str, bool)> = Background::ALL
        .iter()
        .map(|b| (b.label(), *b == app.background))
        .collect();
    let mut bg = chips("BACKGROUND", &background);
    bg.push(Span::styled(" cards marked ", fg(DIM)));
    bg.push(Span::styled("bg", fg(PURPLE)));
    put_lr(
        buf,
        Rect {
            y: agents_y + 1,
            height: 1,
            ..inner
        },
        agents_y + 1,
        bg,
        key_hints(&[("b", "cycles")]),
    );
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(
            if typing {
                key_hints(&[
                    ("↑ ↓", "move"),
                    ("⏎", "show only this"),
                    ("esc", "stop searching"),
                ])
            } else {
                key_hints(&[
                    ("↑↓ j k", "move"),
                    ("⏎", "show only this"),
                    ("/", "search"),
                    ("F", "clear all"),
                    ("esc", "close"),
                ])
            },
            w,
        ),
    );
}

/// A question as its box shows it.
struct Question<'a> {
    title: &'a str,
    message: &'a str,
    action: Option<&'a str>,
    /// A full session's w, from a backend that offers no choices.
    queue: bool,
    /// The answers its backend offers beyond yes and no (intent 0591).
    choices: &'a [crate::actions::Choice],
    /// At the most its session holds: ⏎ queues it, and there's no yes.
    enter_queues: bool,
    too_soon: bool,
}

/// The keys a question answers to, each with what it does, in the order its box lists them: yes,
/// the backend's other answers (or a full session's w), then no.
fn question_keys(q: &Question) -> Vec<(String, String)> {
    let yes = match q.action {
        Some("dispatch") => "start anyway",
        Some("reopen") => "go ahead",
        Some("cancel") => "stop it",
        _ => "yes",
    };
    let mut keys: Vec<(String, String)> = vec![];
    if !q.enter_queues {
        keys.push(("Enter / y".into(), yes.into()));
    }
    let picks = crate::actions::choice_keys(q.choices);
    for (choice, key) in q.choices.iter().zip(picks) {
        let Some(key) = key else { continue };
        let key = if q.enter_queues && choice.id == "queue" {
            format!("Enter / {key}")
        } else {
            key.to_string()
        };
        keys.push((key, choice.label.clone()));
    }
    if q.queue {
        let key = if q.enter_queues { "Enter / w" } else { "w" };
        keys.push((key.into(), "queue it for a free slot".into()));
    }
    // On X's question, "cancel" would read as the yes (intent 0321).
    let no = match q.action {
        Some("cancel") => "keep it running",
        _ if !q.choices.is_empty() => "leave it",
        _ => "cancel",
    };
    keys.push(("Esc / n".into(), no.into()));
    keys
}

/// A question, whole (intent 0591): the box grows to hold all of what its backend said, and its
/// answers, one to a line when it offers more than yes and no, as the design's canvas shows them.
fn confirm(f: &mut Frame, area: Rect, q: Question) {
    let width = 86u16.min(area.width.saturating_sub(2));
    let w = width.saturating_sub(6).max(1) as usize;
    let mut lines = wrapped(q.title, w, bold(YELLOW), None);
    lines.push(Line::raw(""));
    lines.extend(wrapped(q.message, w, fg(FG), None));
    if q.action == Some("dispatch") && q.choices.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(wrapped(
            if q.enter_queues {
                "Starting more work takes another slot, and this session has none left: ⏎ queues it until one frees."
            } else {
                "Starting more work takes another slot."
            },
            w,
            fg(DIM),
            None,
        ));
    }
    let keys = question_keys(&q);
    let held: Vec<&str> = if q.too_soon {
        keys.iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| !k.starts_with("Esc"))
            .collect()
    } else {
        vec![]
    };
    // More than yes and no: one answer to a line. Otherwise the one line it always had.
    let listed = keys.len() > 2;
    let key_rows = if listed { keys.len() as u16 } else { 1 };
    // Its border 2, a blank above the keys 1.
    let height = (lines.len() as u16 + key_rows + 3).max(16);
    let rect = centered(area, width, height, None);
    dialog(f, rect, YELLOW);
    let inner = pad(rect, 3, 1);
    let body = Rect {
        height: inner.height.saturating_sub(key_rows + 1),
        ..inner
    };
    f.render_widget(Paragraph::new(lines), body);
    let first = inner.bottom().saturating_sub(key_rows);
    if listed {
        for (row, (k, what)) in keys.iter().enumerate() {
            put(
                f.buffer_mut(),
                inner.x,
                first + row as u16,
                inner.width,
                fit(
                    key_hints_held(&[(k.as_str(), what.as_str())], &held),
                    inner.width as usize,
                ),
            );
        }
    } else {
        let pairs: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        put(
            f.buffer_mut(),
            inner.x,
            first,
            inner.width,
            fit(key_hints_held(&pairs, &held), inner.width as usize),
        );
    }
}

/// The Kickoff Questions box (intent 0409): what it is, then one question, whole. On a screen too
/// small for all of it, its text scrolls, and ⏎ shows the rest before it can take the recommendation.
fn kickoff(f: &mut Frame, area: Rect, form: &Kickoff) {
    let question = form.question();
    let many = form.questions.len() > 1;
    let box_width = 100.min(area.width.saturating_sub(2));
    let w = box_width.saturating_sub(8).max(1) as usize;
    let mut above = wrapped(
        if many {
            "Answer these and it runs on its own from start to finish. It starts after the last one."
        } else {
            "Answer this and it runs on its own from start to finish. It starts right after."
        },
        w,
        fg(FG2),
        None,
    );
    above.push(Line::raw(""));
    above.push(Line::styled(
        if many {
            "Kickoff Questions"
        } else {
            "Kickoff Question"
        },
        bold(YELLOW),
    ));
    above.extend(wrapped(&question.question, w, bold(FG), None));
    if !question.background.is_empty() {
        above.extend(wrapped(&question.background, w, fg(DIM), None));
    }
    if form.recommends() {
        above.push(Line::raw(""));
        let label = "Recommended";
        if width(label) + 2 + width(&question.pick) <= w {
            above.push(Line::from(vec![
                Span::styled(label, bold(CYAN)),
                Span::raw("  "),
                Span::styled(question.pick.clone(), bold(FG)),
            ]));
        } else {
            above.push(Line::styled(label, bold(CYAN)));
            above.extend(wrapped(&question.pick, w, bold(FG), None));
        }
        if !question.why.is_empty() {
            above.extend(wrapped(&question.why, w, fg(DIM), None));
        }
    }
    if let Some(note) = &form.note {
        above.push(Line::raw(""));
        above.extend(wrapped(note, w, fg(YELLOW), None));
    }
    // Its border 2, its title row and a blank 2, the answer box and a blank each side 6, and two
    // rows of keys.
    let height = above.len() as u16 + 12;
    let rect = centered(area, box_width, height, None);
    dialog(f, rect, PURPLE);
    let inner = pad(rect, 3, 1);
    let of = if many {
        format!("{} of {}", form.at + 1, form.questions.len())
    } else {
        String::new()
    };
    put_lr(
        f.buffer_mut(),
        inner,
        inner.y,
        vec![
            Span::styled(
                " BACKGROUND TASK ",
                Style::default()
                    .bg(PURPLE)
                    .fg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(form.reference.clone(), bold(FG)),
            Span::raw(" "),
            Span::styled(form.title.clone(), fg(FG2)),
        ],
        vec![Span::styled(of, fg(DIM))],
    );
    let keys = 2;
    let answer_rows = 4;
    let answer_y = inner.bottom().saturating_sub(keys + 1 + answer_rows);
    let top = inner.y + 2;
    let room = answer_y.saturating_sub(top + 1);
    let most = (above.len() as u16).saturating_sub(room);
    form.scroll_most.set(most);
    form.scroll.set(form.scroll.get().min(most));
    f.render_widget(
        Paragraph::new(above).scroll((form.scroll.get(), 0)),
        Rect {
            y: top,
            height: room,
            ..inner
        },
    );
    let typed = form.typed();
    let boxed = Rect {
        y: answer_y,
        height: answer_rows,
        ..inner
    };
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(fg(if typed.is_empty() { BORDER } else { CYAN }))
            .style(Style::default().bg(BG)),
        boxed,
    );
    let field = pad(boxed, 2, 1);
    let shown: Vec<Line> = if typed.is_empty() {
        vec![Line::from(vec![
            Span::styled("▏", fg(CYAN)),
            Span::styled(
                if form.recommends() {
                    "Type your own answer, or press ⏎ to take the recommendation."
                } else {
                    "Type your answer."
                },
                fg(FAINT),
            ),
        ])]
    } else {
        field_lines(
            typed,
            field.width.max(1) as usize,
            field.height.max(1) as usize,
            true,
        )
        .into_iter()
        .map(Line::raw)
        .collect()
    };
    f.render_widget(Paragraph::new(shown), field);
    let enter = match (form.is_last(), typed.trim().is_empty()) {
        // ⏎ never takes what you couldn't see.
        (_, true) if form.recommends() && form.scroll.get() < most => "show the rest of it",
        (false, true) => "take the recommendation",
        (false, false) => "keep your answer",
        (true, true) => "take the recommendation and start it",
        (true, false) => "send your answers and start it",
    };
    let back = format!("back to question {}", form.at);
    let mut first = vec![("⏎", enter)];
    if form.at > 0 {
        first.push(("↑", back.as_str()));
    }
    if most > 0 {
        first.push(("pgdn pgup", "the rest of it"));
    }
    let mut second = Vec::new();
    if form.along.is_some() {
        second.push(("tab", "start it with me along instead"));
    }
    second.push((
        "esc",
        if form.reached > 0 {
            "keep these answers and leave it for later"
        } else {
            "leave it for later"
        },
    ));
    let too_soon: &[&str] = if Instant::now() < form.ready_at {
        &["⏎"]
    } else {
        &[]
    };
    let buf = f.buffer_mut();
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(2),
        inner.width,
        fit(key_hints_held(&first, too_soon), w),
    );
    put(
        buf,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        fit(key_hints(&second), w),
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
                "working" => GREEN,
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
                "on work nobody started: start it, as d does",
                "with actions on",
            ),
            (
                "⏎",
                "on work whose workspace closed: reopen it, resuming its agent's conversation",
                "with actions on",
            ),
            (
                "⏎",
                "on done work: asks, then opens it again and resumes its agent's conversation",
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
            ("/", "search cards, workspaces and sessions", "on the board"),
            (
                "/",
                "search only the list you're in: esc clears it, ⏎ does what ⏎ does there",
                "backlog, won't do, misc",
            ),
        ],
    ),
    (
        "WORK INSIDE A CARD",
        &[
            (
                "o",
                "open the card's own screen: itself, then what's inside",
                "board, backlog, a peek",
            ),
            (
                "⏎",
                "on a card holding work: its own agent, in its own tab",
                "never another's tab",
            ),
            (
                "n  N",
                "stop on work inside a card too, opening its screen on it",
                "",
            ),
            (
                "j k ⏎",
                "on its screen: move, then go where that row runs",
                "⏎ on a ▸ group opens it",
            ),
            (
                "space  esc",
                "on its screen: peek at the row / back, on the card",
                "d e y x C X s act on the row",
            ),
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
            (
                "R",
                "read every machine now, retrying any not answering",
                "",
            ),
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
                "move the selected card's agent to another account",
                "keeps its conversation; only that agent",
            ),
            (
                "S",
                "the viewed session owner's ordered models",
                "for agents that start next; moves none running",
            ),
            (
                "c C",
                "in accounts: make one current, or clear it",
                "new sessions everywhere start on it first",
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
            (
                "e",
                "edit its text, the words that started it, in your editor",
                ":wq saves, :cq leaves it",
            ),
            ("y", "still want it: its age starts again", ""),
            (
                "x",
                "won’t do",
                "asks first; backlog, or work with no agent",
            ),
            (
                "C",
                "done: closes it, starting nothing",
                "asks first; work with no agent",
            ),
            (
                "X",
                "cancel: stop its agent, close its workspace, won’t do",
                "asks first; work an agent is at",
            ),
            ("u", "bring it back from won’t do", ""),
            ("J  K", "move it down / up your order", "10J moves ten"),
            ("t", "move it to the top of your order", ""),
            ("B", "move it to the bottom of your order", ""),
        ],
    ),
    (
        "SHOW",
        &[
            ("b", "the backlog, in your order", ""),
            ("w", "the won’t do list", ""),
            ("D", "done work, where ⏎ asks to reopen it", ""),
            ("i", "workspaces with no intent (misc)", ""),
            (
                "f",
                "filters: one owner, tenant or project, agents, background",
                "F shows everything",
            ),
            (
                "a",
                "agents: all, then each program the feeds name, then others",
                "",
            ),
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
    (
        "space",
        "next card that needs you, without the board",
        "work inside cards too, straight to its tab",
    ),
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
    // This machine, hidden: off the board, still read (intent 0591, K1-10).
    if m.machine.hidden && !matches!(m.conn, Conn::Hidden) {
        return vec![
            "You hid its cards with x. Kerdr still reads it, for its accounts and the actions it runs; x shows them again.".into(),
        ];
    }
    match &m.conn {
        Conn::Live { latency } => {
            let mut lines = vec![format!(
                "Answering. Read every {}s{}; the last read took {} ms.",
                if m.machine.is_local() { app.cfg.machines.local_every } else { app.cfg.machines.remote_every },
                if m.machine.is_local() { "" } else { " over SSH" },
                latency.as_millis()
            )];
            // Its feed, and herdr as Kerdr reads it, each in its own words: a herdr read that
            // keeps failing is never called the feed's failure, and the feed's reason comes first
            // (intent 0591).
            let feed_says: Vec<&str> = m.read_failures_lasting(now).iter().map(|f| f.why.as_str()).collect();
            lines.push(match (&m.machine.feed, m.feed_problem(now)) {
                (Some(_), Some(e)) => format!("Its card feed failed ({e}), so it shows the last cards it sent, with herdr's live view."),
                (Some(cmd), None) if !feed_says.is_empty() => {
                    format!("Cards come from its feed ({cmd}), which says: {}.", feed_says.join("; "))
                }
                (Some(cmd), None) => format!("Cards come from its feed: {cmd}"),
                (None, _) => "No card feed: every herdr workspace with an agent is a card.".into(),
            });
            if feed_says.is_empty()
                && let Some(why) = m.sessions_problem(now)
            {
                lines.push(format!(
                    "Kerdr's own read of herdr there: {}. Its last list of sessions stands, marked out of date.",
                    why.trim_end_matches('.')
                ));
            }
            lines
        }
        Conn::Connecting => vec!["Reaching it for the first time.".into()],
        // Why, in Kerdr's words: SSH's own stay in the error log (intent 0591).
        Conn::Reconnecting { since, attempt, retry_at, .. } => vec![
            format!(
                "Lost the connection {} ago{}. Trying again in {}s (try {}). The wait grows to 15 seconds.",
                ago(*since, now),
                m.conn.cause().map(|c| format!(" ({c})")).unwrap_or_default(),
                (*retry_at - now).num_seconds().max(0),
                attempt + 1
            ),
            format!(
                "Its cards stay on the board, dimmed{}. ⏎ on one waits for the connection, then jumps in.",
                as_of.map(|t| format!(", as they were at {t}")).unwrap_or_default()
            ),
        ],
        Conn::Unreachable { since, retry_at, .. } => vec![
            format!(
                "No answer since {}{}: asleep, off, or off the network.",
                clock(*since),
                m.conn.cause().map(|c| format!(" ({c})")).unwrap_or_default()
            ),
            format!("Kerdr tries again in {}s, then every 15 seconds; r tries now. ⏎ and d on its cards wait for it, then carry on.", (*retry_at - now).num_seconds().max(0)),
        ],
        // What SSH says of the key is kept as it said it: you compare it to approve it.
        Conn::HostKey { detail } => vec![
            format!("SSH asked whether to trust this machine's host key ({detail})."),
            "Kerdr never answers that for you: ⏎ runs ssh here, so you can check the key and answer it yourself.".into(),
        ],
        Conn::Auth { .. } => vec![
            "SSH wants a password, a passphrase or another key (what it said is in Kerdr's error log).".into(),
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
            _ if m.machine.hidden => hints = vec![("x", "show its cards again")],
            _ => hints.insert(0, ("⏎", "its board")),
        }
        if !m.machine.hidden && !matches!(m.conn, Conn::Hidden) {
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

/// The search line of a list view (`/` there, intent 0402): what it's narrowed to and how many
/// of the view's cards that leaves, or None with no search.
fn view_search_line(app: &App, shown: usize, total: usize) -> Option<Vec<Span<'static>>> {
    let s = app.view_search.as_ref()?;
    let mut line = vec![
        Span::styled("/ ", bold(CYAN)),
        Span::styled(s.query.clone(), bold(FG)),
    ];
    if s.typing {
        line.push(Span::styled("▏", fg(CYAN)));
    }
    line.push(Span::styled(
        if s.query.trim().is_empty() {
            "   type to search this list".to_string()
        } else {
            format!("   {shown} of {total}")
        },
        fg(DIM),
    ));
    Some(line)
}

/// The bottom line of a list view while its search is being typed.
fn view_search_hints(enter: &'static str) -> Vec<Span<'static>> {
    key_hints(&[("↑ ↓", "select"), ("⏎", enter), ("esc", "clear the search")])
}

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
    let total = app
        .board
        .cards
        .iter()
        .filter(|c| c.stage == Stage::Misc && app.card_visible(c))
        .count();
    put(
        f.buffer_mut(),
        rows[3].x,
        rows[3].y,
        rows[3].width,
        vec![
            Span::styled(format!("MISC  {total}"), bold(CYAN)),
            Span::styled("   Conversations without an intent", fg(FG2)),
        ],
    );
    if let Some(line) = view_search_line(app, items.len(), total) {
        put(
            f.buffer_mut(),
            rows[3].x,
            rows[3].y + 1,
            rows[3].width,
            line,
        );
    }
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
                if app
                    .view_search
                    .as_ref()
                    .is_some_and(|s| !s.query.trim().is_empty())
                {
                    "No miscellaneous conversations match this search."
                } else {
                    "No miscellaneous conversations match these filters."
                },
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
            match &app.view_search {
                Some(s) if s.typing => view_search_hints("jump in"),
                Some(_) => key_hints(&[
                    ("⏎", "jump in"),
                    ("j k", "select"),
                    ("space", "peek"),
                    ("/", "change the search"),
                    ("esc", "clear the search"),
                ]),
                None => key_hints(&[
                    ("⏎", "jump in"),
                    ("j k", "select"),
                    ("space", "peek"),
                    ("/", "search this list"),
                    ("[ ]", "session"),
                    ("{ }", "machine"),
                    ("c", "new intent"),
                    ("f", "filter"),
                    ("esc", "board"),
                ]),
            },
            rows[5].width as usize,
        ),
    );
}

/// The Done screen (`D`, intent 0444): finished work, listed as the Maintain column lists it, a
/// done card with work still open inside first, saying how much.
fn done_view(f: &mut Frame, area: Rect, app: &App, sel: usize) {
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
    let items = app.done_cards();
    let total = app.columns().last().map_or(0, Vec::len);
    put(
        f.buffer_mut(),
        rows[3].x,
        rows[3].y,
        rows[3].width,
        vec![
            Span::styled(format!("DONE  {total}"), bold(GREEN)),
            Span::styled("   Finished work · ⏎ asks to reopen it", fg(FG2)),
        ],
    );
    if let Some(line) = view_search_line(app, items.len(), total) {
        put(
            f.buffer_mut(),
            rows[3].x,
            rows[3].y + 1,
            rows[3].width,
            line,
        );
    }
    if items.is_empty() {
        put(
            f.buffer_mut(),
            rows[4].x,
            rows[4].y,
            rows[4].width,
            vec![Span::styled(
                if app
                    .view_search
                    .as_ref()
                    .is_some_and(|s| !s.query.trim().is_empty())
                {
                    "No done work matches this search."
                } else {
                    "No done work in this view."
                },
                fg(DIM),
            )],
        );
    } else {
        let selected = items
            .get(sel.min(items.len() - 1))
            .map(|&i| app.board.cards[i].key.as_str());
        list_rows(f.buffer_mut(), rows[4], app, &items, selected);
    }
    let card = app
        .sel
        .key
        .as_deref()
        .and_then(|k| app.card(k))
        .filter(|c| c.stage == Stage::Maintain);
    let mut hints: Vec<(&str, String)> = Vec::new();
    match &app.view_search {
        Some(s) if s.typing => {}
        _ => {
            if card.is_some_and(|c| c.can_reopen_done()) && app.actions_on() {
                hints.push(("⏎", "ask to reopen it".into()));
            }
            if let Some(c) = card {
                with_inside(app, c, &mut hints);
            }
        }
    }
    let rest: &[(&str, &str)] = match &app.view_search {
        Some(s) if s.typing => &[],
        Some(_) => &[
            ("j k", "select"),
            ("space", "peek"),
            ("/", "change the search"),
            ("esc", "clear the search"),
        ],
        None => &[
            ("j k", "select"),
            ("space", "peek"),
            ("/", "search this list"),
            ("[ ]", "session"),
            ("{ }", "machine"),
            ("f", "filter"),
            ("D esc", "board"),
        ],
    };
    let mut line = match &app.view_search {
        Some(s) if s.typing => view_search_hints("ask to reopen it"),
        _ => owned_hints(&hints),
    };
    if !hints.is_empty() && !rest.is_empty() {
        line.push(Span::raw("   "));
    }
    line.extend(key_hints(rest));
    put(
        f.buffer_mut(),
        rows[5].x,
        rows[5].y,
        rows[5].width,
        fit(line, rows[5].width as usize),
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
    // Each card keeps its place in the whole order while a search narrows the list.
    let order = app.backlog_order();
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
                        format!("{} set aside · u restores to backlog", order.len())
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

    let mut lines: Vec<(Option<usize>, Vec<Span<'static>>, Vec<Span<'static>>)> =
        view_search_line(app, items.len(), order.len())
            .map(|line| (None, line, vec![]))
            .into_iter()
            .collect();
    lines.push((
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
    ));
    for (k, &i) in items.iter().enumerate() {
        let c = &app.board.cards[i];
        let place = order.iter().position(|&j| j == i).unwrap_or(k) + 1;
        let (glyph, gc, meta, meta_color) = list_glyph(c, now, list.width as usize / 2);
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
                Span::styled(format!("{place:>3} {glyph} "), fg(gc)),
                Span::styled(format!("{:<6}", c.reference), bold(FG)),
                Span::styled(c.title.clone(), fg(FG)),
            ],
            if c.intake().is_some() || meta_color == BLUE {
                let meta = if app.starts_when_written(&c.key) {
                    format!("{meta} · starts once done")
                } else {
                    meta
                };
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
                if app
                    .view_search
                    .as_ref()
                    .is_some_and(|s| !s.query.trim().is_empty())
                {
                    "Nothing here matches this search."
                } else if app.wont_do {
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
                let why = write_up_why(c)
                    .map(|why| format!(" ({why})"))
                    .unwrap_or_default();
                out.extend(wrapped(
                    &format!("✎ No agent could write this up{why}, so it keeps your words and their first ones as its title. The agent that starts it writes it up."),
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
                            "{} is at its usual limit. Starting it with d asks before opening another slot.",
                            r.session
                        ),
                    }
                }
                Some(r) => match r.slots.free() {
                    Some(n) => format!("{} has room: {n} free.", r.session),
                    None => format!("{} has no limit.", r.session),
                },
                // Its backend starts the session first (intent 0591).
                None => match c.session.as_deref() {
                    Some(s) => format!(
                        "{s} isn't running on {}: d starts it there first, then the work in it.",
                        c.machine
                    ),
                    None => "d starts it in its owner's session, starting that session first if it isn't running.".into(),
                },
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
            match &app.view_search {
                Some(s) if s.typing => view_search_hints("peek"),
                Some(_) => key_hints(if app.wont_do {
                    &[
                        ("j k", "move"),
                        ("u", "bring it back"),
                        ("⏎", "peek"),
                        ("/", "change the search"),
                        ("esc", "clear the search"),
                    ]
                } else {
                    &[
                        ("j k", "move"),
                        ("d", "start it"),
                        ("e", "edit its text"),
                        ("y", "still want it"),
                        ("x", "won’t do"),
                        ("C", "done"),
                        ("J K", "move down / up"),
                        ("⏎", "peek"),
                        ("/", "change the search"),
                        ("esc", "clear the search"),
                    ]
                }),
                None => key_hints(if app.wont_do {
                    &[
                        ("j k", "move"),
                        ("u", "bring it back"),
                        ("⏎", "peek"),
                        ("/", "search this list"),
                        ("w", "the backlog"),
                        ("f", "filter"),
                        ("esc", "board"),
                    ]
                } else {
                    &[
                        ("j k", "move"),
                        ("d", "start it"),
                        ("e", "edit its text"),
                        ("y", "still want it"),
                        ("x", "won’t do"),
                        ("C", "done"),
                        ("J K", "move down / up · 10J ten places"),
                        ("t B", "to the top / bottom"),
                        ("c", "new intent"),
                        ("⏎", "peek"),
                        ("/", "search this list"),
                        ("w", "won’t do list"),
                        ("f", "filter"),
                        ("esc", "board"),
                    ]
                }),
            },
            rows[4].width as usize,
        ),
    );
}

/// The whole screen as text, for other modules' tests.
#[cfg(test)]
pub(crate) fn tests_render(a: &App, w: u16, h: u16) -> String {
    let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, a)).unwrap();
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Search, SearchFilter, Selection};
    use crate::config::Config;
    use chrono::TimeZone;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn demo(away: bool) -> App {
        let mut a = App::new(Config::machine_split(), crate::demo::machines(away));
        a.demo = true;
        a.fixed_now = Some(crate::demo::now());
        a.rebuild();
        a
    }

    /// A Build card with no workspace, on machine `i`, as a read after the last rebuild left it.
    fn card_with_no_workspace(a: &mut App, i: usize) -> String {
        let card = a
            .board
            .cards
            .iter_mut()
            .find(|c| c.stage == Stage::Build && c.detail.is_some())
            .unwrap();
        card.machine_index = i;
        card.workspace_id = None;
        card.session = None;
        card.key.clone()
    }

    fn peek_at(a: &mut App, key: &str) -> String {
        let c = a.board.cards.iter().find(|c| c.key == key).unwrap();
        a.sel = crate::app::Selection {
            col: c.stage.index(),
            key: Some(c.key.clone()),
        };
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        rendered(a, 176, 44)
    }

    /// The demo with every machine answering, but for the ones a test puts in trouble.
    fn all_answering(away: bool) -> App {
        let mut a = demo(away);
        for m in a.machines.iter_mut() {
            if m.is_shown() && !matches!(m.conn, Conn::Reconnecting { .. }) {
                m.conn = Conn::Live {
                    latency: Default::default(),
                };
            }
        }
        a
    }

    #[test]
    fn a_read_failure_that_lasts_is_said_once_on_the_board_in_plain_words() {
        let mut a = all_answering(false);
        let i = a.machines.iter().position(|m| m.feed.is_some()).unwrap();
        let name = a.machines[i].name();
        let now = a.now();
        let reason = "herdr on this machine didn't answer within a few seconds";
        // herdr fails every call: the feed says so, and Kerdr's own read of its list fails too.
        let herdr_failing = |a: &mut App, secs: i64| {
            let since = now - chrono::Duration::seconds(secs);
            let m = &mut a.machines[i];
            let feed = m.feed.as_mut().unwrap();
            feed.read_failures =
                serde_json::from_value(serde_json::json!([{"part":"herdr","why":reason}])).unwrap();
            let key = feed.read_failures[0].key();
            m.read_failing_since = [(key, since)].into_iter().collect();
            m.sessions_failing = Some((
                "herdr answered its list of sessions with something Kerdr can't read (what it printed is in Kerdr's error log).".into(),
                since,
            ));
        };
        // A few seconds of it: the board stays quiet, its rows only marked.
        herdr_failing(&mut a, 10);
        let screen = rendered(&a, 200, 44);
        assert!(!screen.contains(reason), "{screen}");
        // Past the quiet spell: said once, in the feed's words, never herdr's or as the feed's
        // own failure.
        herdr_failing(&mut a, 45);
        let screen = rendered(&a, 200, 44);
        assert!(screen.contains(&format!("{name}: {reason}")), "{screen}");
        assert_eq!(screen.matches(reason).count(), 1, "{screen}");
        assert!(!screen.contains("can't read"), "{screen}");
        assert!(!screen.contains("feed failed"), "{screen}");
        // M's detail too: the feed's reason, and no feed failure.
        let explained = machine_explain(&a, i).join(" ");
        assert!(explained.contains(reason), "{explained}");
        assert!(!explained.contains("feed failed"), "{explained}");
        // Without the feed's reason, Kerdr's own words for herdr's list, still not the feed's.
        a.machines[i].feed.as_mut().unwrap().read_failures.clear();
        let screen = rendered(&a, 200, 44);
        assert!(
            screen.contains(&format!("{name}: herdr answered its list of sessions")),
            "{screen}"
        );
        let explained = machine_explain(&a, i).join(" ");
        assert!(!explained.contains("feed failed"), "{explained}");
        assert!(
            explained.contains("Kerdr's own read of herdr"),
            "{explained}"
        );

        // The feed prints nothing: quiet for one read, said once it lasts.
        let mut a = all_answering(false);
        let feed_failing = |a: &mut App, reads: u32, secs: i64| {
            let m = &mut a.machines[i];
            m.feed_error = Some(crate::poll::FEED_EMPTY.into());
            m.feed_failures = reads;
            m.feed_failing_since = Some(now - chrono::Duration::seconds(secs));
        };
        feed_failing(&mut a, 1, 5);
        let screen = rendered(&a, 200, 44);
        assert!(!screen.contains(crate::poll::FEED_EMPTY), "{screen}");
        feed_failing(&mut a, 3, 42);
        let screen = rendered(&a, 200, 44);
        assert!(
            screen.contains(&format!("{name}: its card feed failed: it printed nothing")),
            "{screen}"
        );
    }

    #[test]
    fn a_read_failure_that_lasts_is_named_beside_another_machine_s_connection_trouble() {
        // One machine waits for its host key to be approved while another's feed has failed for
        // 45 s: the first is the warning, and the second is still named (Codex's look).
        let mut a = all_answering(false);
        let gpu = a.machine_index("gpu-rig").unwrap();
        a.machines[gpu].conn = Conn::HostKey {
            detail: "Host key verification failed.".into(),
        };
        let i = a.machines.iter().position(|m| m.feed.is_some()).unwrap();
        assert_ne!(i, gpu);
        let name = a.machines[i].name();
        let now = a.now();
        let feed_failing = |a: &mut App, reads: u32, secs: i64| {
            let m = &mut a.machines[i];
            m.feed_error = Some(crate::poll::FEED_EMPTY.into());
            m.feed_failures = reads;
            m.feed_failing_since = Some(now - chrono::Duration::seconds(secs));
        };
        feed_failing(&mut a, 3, 45);
        let screen = rendered(&a, 200, 44);
        assert!(screen.contains("gpu-rig needs you"), "{screen}");
        assert!(screen.contains(&format!("+1 more: {name}")), "{screen}");
        // A moment's failure isn't named, as it isn't said anywhere yet.
        feed_failing(&mut a, 1, 5);
        let screen = rendered(&a, 200, 44);
        assert!(screen.contains("gpu-rig needs you"), "{screen}");
        assert!(!screen.contains("more:"), "{screen}");
        // Two machines with read trouble alone: the first said, the other named.
        let mut a = all_answering(false);
        for (k, m) in a
            .machines
            .iter_mut()
            .enumerate()
            .filter(|(_, m)| m.is_shown())
        {
            m.feed_error = Some(crate::poll::FEED_EMPTY.into());
            m.feed_failures = 3;
            m.feed_failing_since = Some(now - chrono::Duration::seconds(45 + k as i64));
        }
        let shown = a.machines.iter().filter(|m| m.is_shown()).count();
        let screen = rendered(&a, 200, 44);
        assert!(
            screen.contains(&format!("+{} more: ", shown - 1)),
            "{screen}"
        );
    }

    #[test]
    fn the_machines_screen_says_why_in_kerdr_s_words_and_keeps_a_host_key_as_ssh_said_it() {
        let mut a = all_answering(true);
        let i = a.machine_index("build-box").unwrap();
        let now = a.now();
        let said = |a: &App| machine_explain(a, i).join(" ");
        let raw = "ssh: connect to host build-box port 22: No route to host";
        a.machines[i].conn = Conn::Reconnecting {
            since: now - chrono::Duration::seconds(40),
            attempt: 2,
            retry_at: now + chrono::Duration::seconds(8),
            detail: raw.into(),
        };
        let text = said(&a);
        assert!(text.contains("ago (there's no route to it)."), "{text}");
        assert!(!text.contains("No route to host"), "{text}");
        a.machines[i].conn = Conn::Unreachable {
            since: now - chrono::Duration::minutes(3),
            retry_at: now + chrono::Duration::seconds(10),
            detail: raw.into(),
        };
        let text = said(&a);
        assert!(text.contains("(there's no route to it): asleep"), "{text}");
        assert!(!text.contains("No route to host"), "{text}");
        // What SSH says that Kerdr doesn't know: none of it, and no empty brackets.
        a.machines[i].conn = Conn::Reconnecting {
            since: now - chrono::Duration::seconds(40),
            attempt: 2,
            retry_at: now + chrono::Duration::seconds(8),
            detail: "mux_client_request_session: session request failed".into(),
        };
        let text = said(&a);
        assert!(!text.contains("mux_client"), "{text}");
        assert!(!text.contains("()"), "{text}");
        assert!(text.contains("ago. Trying again"), "{text}");
        a.machines[i].conn = Conn::Auth {
            detail: "drew@build-box: Permission denied (publickey,password).".into(),
        };
        let text = said(&a);
        assert!(!text.contains("Permission denied"), "{text}");
        assert!(text.contains("Kerdr's error log"), "{text}");
        // A host key's details stay exactly as SSH gave them: you compare them to approve it.
        let key =
            "The ECDSA host key for build-box has changed: SHA256:2fQ0mY0sVb5rQe9x8kFjT1LhW7cZ";
        a.machines[i].conn = Conn::HostKey { detail: key.into() };
        let text = said(&a);
        assert!(text.contains(key), "{text}");
    }

    #[test]
    fn a_dropped_machine_s_banner_counts_every_card_it_dims_and_says_why_in_plain_words() {
        let mut a = all_answering(true);
        let i = a.machine_index("build-box").unwrap();
        let raw = "ssh: connect to host build-box port 22: No route to host";
        if let Conn::Reconnecting { detail, .. } = &mut a.machines[i].conn {
            *detail = raw.into();
        }
        // Backlog cards of its own, as on the run that found it: dimmed like the rest.
        let backlog: Vec<usize> = (0..a.board.cards.len())
            .filter(|&k| a.board.cards[k].stage == Stage::Backlog)
            .take(3)
            .collect();
        assert_eq!(backlog.len(), 3);
        for k in backlog {
            let c = &mut a.board.cards[k];
            c.machine = "build-box".into();
            c.machine_index = i;
        }
        let shown = a
            .board
            .cards
            .iter()
            .filter(|c| c.machine == "build-box" && a.card_visible(c))
            .count();
        assert!(shown >= 3);
        let screen = rendered(&a, 200, 44);
        assert!(
            screen.contains(&format!("its {shown} cards are dimmed")),
            "{screen}"
        );
        assert!(
            screen.contains("build-box dropped 40s ago: there's no route to it"),
            "{screen}"
        );
        assert!(!screen.contains("No route to host"), "{screen}");
        // Past two minutes it can't be reached: its cause in the same words, never SSH's.
        let now = a.now();
        a.machines[i].conn = Conn::Unreachable {
            since: now - chrono::Duration::minutes(3),
            retry_at: now + chrono::Duration::seconds(10),
            detail: raw.into(),
        };
        let screen = rendered(&a, 200, 44);
        assert!(screen.contains("can't reach build-box"), "{screen}");
        assert!(screen.contains("(there's no route to it)"), "{screen}");
        assert!(!screen.contains("No route to host"), "{screen}");
    }

    #[test]
    fn a_feed_that_couldnt_read_herdr_marks_its_workspaces_stale_quietly() {
        let mut a = demo(false);
        let i = a.machines.iter().position(|m| m.feed.is_some()).unwrap();
        let name = a.machines[i].name();
        let rows = |a: &App| {
            a.board
                .sessions
                .iter()
                .filter(|r| r.machine == name)
                .map(|r| (r.session.clone(), r.cached, r.error.clone()))
                .collect::<Vec<_>>()
        };
        // Its feed says nothing failed: exactly as before.
        assert!(rows(&a).iter().all(|(_, cached, _)| !cached));
        let key = card_with_no_workspace(&mut a, i);
        assert!(peek_at(&mut a, &key).contains("no workspace open"));
        let c = a.card(&key).unwrap().clone();
        assert_eq!(
            crate::ui::card_ui::place_for_test(&a, &c).0,
            "no workspace open"
        );

        let failing = |a: &mut App, list: serde_json::Value| {
            a.machines[i].feed.as_mut().unwrap().read_failures =
                serde_json::from_value(list).unwrap();
            a.mode = Mode::Board;
            a.rebuild();
        };
        // herdr itself: every session's workspaces, marked at once, and no words until it lasts.
        failing(
            &mut a,
            serde_json::json!([{"part":"herdr","why":"herdr on this machine didn't answer within a few seconds"}]),
        );
        let seen = rows(&a);
        assert!(seen.iter().all(|(_, cached, _)| *cached), "{seen:?}");
        assert!(seen.iter().all(|(_, _, error)| error.is_none()), "{seen:?}");
        let key = card_with_no_workspace(&mut a, i);
        let peek = peek_at(&mut a, &key);
        assert!(peek.contains("workspace not known"), "{peek}");
        assert!(!peek.contains("no workspace open"), "{peek}");
        let c = a.card(&key).unwrap().clone();
        assert_eq!(
            crate::ui::card_ui::place_for_test(&a, &c).0,
            "workspace not known"
        );

        // One session: that session's alone.
        let first = seen[0].0.clone();
        failing(
            &mut a,
            serde_json::json!([{"part":"session","session":first,"why":"it didn't list its tabs in time"}]),
        );
        for (session, cached, _) in rows(&a) {
            assert_eq!(cached, session == first, "{session}");
        }
    }

    #[test]
    fn a_card_open_in_several_workspaces_says_so_and_any_other_looks_as_before() {
        let mut a = demo(false);
        let i = a.machines.iter().position(|m| m.feed.is_some()).unwrap();
        let key = card_with_no_workspace(&mut a, i);
        let set = |a: &mut App, places: &[&str]| {
            let c = a.board.cards.iter_mut().find(|c| c.key == key).unwrap();
            c.detail.as_mut().unwrap().open_in = places.iter().map(|p| p.to_string()).collect();
            c.clone()
        };
        // None, or the one place the card already names: nothing more said.
        assert_eq!(open_in_line(&set(&mut a, &[])), None);
        assert_eq!(open_in_line(&set(&mut a, &["web/w1"])), None);
        let peek = peek_at(&mut a, &key);
        assert!(!peek.contains("workspaces ("), "{peek}");
        // More than one: every place, within reason.
        assert_eq!(
            open_in_line(&set(&mut a, &["web/w1", "ops/w3"])).as_deref(),
            Some("open in 2 workspaces (web/w1, ops/w3), so two agents may be at it")
        );
        let peek = peek_at(&mut a, &key);
        assert!(
            peek.contains("open in 2 workspaces (web/w1, ops/w3)"),
            "{peek}"
        );
        assert_eq!(
            open_in_line(&set(&mut a, &["a/1", "b/2", "c/3", "d/4", "e/5"])).as_deref(),
            Some("open in 5 workspaces (a/1, b/2, c/3, 2 more), so two agents may be at it")
        );
        // Only ever plain text, however a feed writes it.
        let long = "x".repeat(200);
        let line = open_in_line(&set(&mut a, &["web/\u{1b}[31mw1", &long])).unwrap();
        assert!(!line.contains('\u{1b}'), "{line}");
        assert!(line.len() < 120, "{line}");
    }

    #[test]
    fn a_failed_write_up_says_why_when_its_feed_does_and_looks_as_before_when_it_doesnt() {
        let a = demo(false);
        let now = a.now();
        let key = a
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Backlog && !c.dismissed())
            .unwrap()
            .key
            .clone();
        let mut a = a;
        // The backlog's own screen, where a row has room to spare.
        a.mode = Mode::Backlog { sel: 0 };
        let set = |a: &mut App, intake: &str, why: Option<&str>| {
            for c in a.board.cards.iter_mut().filter(|c| c.key == key) {
                let d = c.detail.as_mut().unwrap();
                d.intake = Some(intake.into());
                d.intake_why = why.map(str::to_string);
            }
            a.board.cards.iter().find(|c| c.key == key).unwrap().clone()
        };
        // A feed that doesn't say: exactly as before, whatever the room.
        let c = set(&mut a, "failed", None);
        for room in [0, 17, 30, usize::MAX] {
            let row = list_glyph(&c, now, room);
            assert_eq!(
                (row.0, row.1, row.2.as_str(), row.3),
                ("✎", YELLOW, "not written up", YELLOW)
            );
        }
        let screen = crate::ui::tests_render(&a, 200, 50);
        assert!(screen.contains("not written up"), "{screen}");
        assert!(!screen.contains("not written up:"), "{screen}");
        // One that does: its words, in the same colours.
        let c = set(&mut a, "failed", Some("no account was free"));
        let row = list_glyph(&c, now, usize::MAX);
        assert_eq!(
            (row.0, row.1, row.2.as_str(), row.3),
            ("✎", YELLOW, "not written up: no account was free", YELLOW)
        );
        let screen = crate::ui::tests_render(&a, 200, 50);
        assert!(
            screen.contains("not written up: no account was free"),
            "{screen}"
        );
        // Cut to the room there is; with no room for any of it, the plain words.
        let cut = list_glyph(&c, now, 24).2;
        assert_eq!(cut, "not written up: no acco…");
        assert_eq!(width(&cut), 24);
        assert_eq!(list_glyph(&c, now, 17).2, "not written up");
        // However long a feed makes it, a row never gets more than a few words.
        let long = "x".repeat(500);
        let c = set(&mut a, "failed", Some(&long));
        let row = list_glyph(&c, now, usize::MAX).2;
        assert_eq!(width(&row), width("not written up: ") + WRITE_UP_WHY_MOST);
        assert!(row.ends_with('…'), "{row}");
        // One line, and blank is no reason at all.
        let c = set(&mut a, "failed", Some(" no account\nwas\tfree "));
        assert_eq!(
            list_glyph(&c, now, usize::MAX).2,
            "not written up: no account was free"
        );
        let c = set(&mut a, "failed", Some("  \n "));
        assert_eq!(list_glyph(&c, now, usize::MAX).2, "not written up");
        // Only a failed write-up says it.
        let c = set(&mut a, "pending", Some("no account was free"));
        assert_eq!(list_glyph(&c, now, usize::MAX).2, "writing it up");
    }

    #[test]
    fn a_backlog_row_waiting_on_you_says_so_in_yellow() {
        let a = demo(false);
        let mut c = a
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Backlog && !c.dismissed())
            .unwrap()
            .clone();
        let quiet = list_glyph(&c, a.now(), usize::MAX);
        assert_ne!(quiet.2, "needs you");
        c.attention = Attention::NeedsYou;
        let (glyph, color, meta, mcolor) = list_glyph(&c, a.now(), usize::MAX);
        assert_eq!(
            (glyph, color, meta.as_str(), mcolor),
            ("●", YELLOW, "needs you", YELLOW)
        );
    }

    fn modes(a: &App) -> Vec<Mode> {
        let holder = a
            .board
            .cards
            .iter()
            .find(|c| c.reference == "#41")
            .map(|c| c.key.clone())
            .unwrap_or_default();
        let mut modes = vec![
            Mode::Board,
            Mode::Card {
                card: holder.clone(),
                focus: String::new(),
                opened: vec![],
                back: Box::new(Mode::Board),
            },
            Mode::Card {
                card: holder,
                focus: "group:done".into(),
                opened: vec![],
                back: Box::new(Mode::Board),
            },
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
                typing: true,
                back: Box::new(Mode::Board),
            },
            Mode::Confirm {
                plain: false,
                choices: vec![],
                enter_queues: false,
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

    /// Intent 0327: the accounts screen names the machine Enter signs in on, the sign-in box names
    /// it in its title, says which code it wants, and offers a pasted token only where one works.
    #[test]
    fn sign_in_says_which_machine_and_which_code() {
        use crate::app::account_controls::AccountScreen as S;
        let mut a = demo(false);
        a.open_accounts();
        let list = a.mode.clone();
        let text = rendered(&a, 176, 44);
        // KBZ_C is out on the laptop, not ready: Enter signs in there, and says so.
        assert!(text.contains("⏎ sign in on laptop · t test"), "{text}");
        assert!(
            text.contains("j k move · ⏎ sign in on laptop · t test · e rename"),
            "{text}"
        );
        // At 80 columns the panel is hidden, so the bottom line names every machine Enter signs in.
        for f in a.machines[1].feed.iter_mut() {
            f.accounts
                .iter_mut()
                .find(|x| x.code == "KBZ_C")
                .unwrap()
                .status = "sign_in".into();
        }
        let narrow = rendered(&a, 80, 24);
        assert!(
            narrow.contains("⏎ sign in on laptop, then build-box"),
            "{narrow}"
        );
        for f in a.machines[1].feed.iter_mut() {
            f.accounts
                .iter_mut()
                .find(|x| x.code == "KBZ_C")
                .unwrap()
                .status = "out".into();
        }
        if let Mode::Accounts {
            screen: S::List { sel, .. },
            ..
        } = &mut a.mode
        {
            *sel = 1;
        }
        let text = rendered(&a, 176, 44);
        assert!(
            text.contains("⏎ ready on laptop: choose where to sign in"),
            "{text}"
        );
        let screens = crate::demo::account_screens(&a);
        let find = |f: &dyn Fn(&S) -> bool| {
            screens
                .iter()
                .find(|m| matches!(m, Mode::Accounts { screen, .. } if f(screen)))
                .unwrap()
                .clone()
        };
        a.mode = find(&|s| matches!(s, S::Where { .. }));
        let text = rendered(&a, 176, 44);
        assert!(text.contains("Sign ARK_C in where?"), "{text}");
        assert!(text.contains("ARK_C is already signed in and ready on build-box"));
        assert!(text.contains("laptop                 ✕ sign in"));
        assert!(text.contains("⏎ sign in there"));
        a.mode = find(&|s| matches!(s, S::Signin(x) if x.note.is_some()));
        let text = rendered(&a, 176, 44);
        assert!(text.contains("Sign KBZ_C in on laptop"), "{text}");
        assert!(!text.contains("on each machine"));
        assert!(text.contains("Once you allow it, the page shows a code"));
        assert!(text.contains("code from the page"));
        assert!(text.contains("short code a sign-in page emails you"));
        assert!(!text.contains("claude.ai"), "{text}");
        assert!(
            text.contains("⏎ send code    esc stop"),
            "a saved sign-in offers no token: {text}"
        );
        assert!(!text.contains("p token"));
        a.mode = find(&|s| matches!(s, S::Signin(x) if x.paste && x.copy.is_none()));
        let text = rendered(&a, 176, 44);
        assert!(
            text.contains("Sign ARK_C in on laptop, then build-box"),
            "{text}"
        );
        assert!(
            text.contains("⏎ send code    p token    esc stop"),
            "{text}"
        );
        a.mode = find(&|s| matches!(s, S::Signin(x) if x.failed && !x.paste));
        let text = rendered(&a, 176, 44);
        assert!(text.contains("r try again    esc later"), "{text}");
        // At 80×24, a three-machine sign-in with ark-account's whole message still shows the code
        // field and its note while a code is typed, rather than the machine row it would follow.
        let mut typing = find(&|s| matches!(s, S::Signin(x) if x.note.is_some()));
        if let Mode::Accounts {
            screen: S::Signin(x),
            ..
        } = &mut typing
        {
            x.machines = vec![0, 1, 2];
            x.message = "Authorize as you@example.com on laptop. The page signs in whichever account the browser is signed in to claude.ai as: if it names another, use Switch account at its foot.".into();
        }
        a.mode = typing;
        let text = rendered(&a, 80, 24);
        assert!(text.contains("code from the page"), "{text}");
        assert!(text.contains("That looks like the short code"), "{text}");
        for mode in [list, a.mode.clone()] {
            a.mode = mode;
            for (w, h) in [(1, 1), (10, 3), (30, 8), (60, 15), (80, 24), (120, 40)] {
                let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                t.draw(|f| draw(f, &a)).unwrap();
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
            .find(|x| x.code == "ARK_C")
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
        assert!(
            text.contains("Sign ARK_C in on build-box: Tenants"),
            "{text}"
        );
        assert!(text.contains("Only the Tenants copy changes."));
        assert!(text.contains("esc before it finishes leaves this one as it was"));
        // Once it has finished, Esc only closes: the dialog no longer says it would undo anything.
        if let Mode::Accounts {
            screen: crate::app::account_controls::AccountScreen::Signin(s),
            ..
        } = &mut a.mode
        {
            s.current = 1;
            s.ticket = None;
        }
        for (w, h) in [(1, 1), (30, 8), (80, 24), (176, 44)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| draw(f, &a)).unwrap();
        }
        let text = rendered(&a, 176, 44);
        assert!(text.contains("The Tenants copy is signed in."), "{text}");
        assert!(text.contains("Signed in.    esc accounts"));
        assert!(!text.contains("leaves this one as it was"));
    }

    #[test]
    fn a_remembered_enter_keeps_saying_so_after_the_words_have_gone() {
        // Messages go after 8 s, and herdr can take the screen minutes later (intent 0491).
        let mut a = crate::app::tests::demo_app();
        let key = a.board.cards[0].key.clone();
        let reference = a.board.cards[0].reference.clone();
        let line = "Kerdr goes in when its workspace opens";
        assert!(!rendered(&a, 160, 40).contains(line));
        a.enter_when_open = Some(key);
        let screen = rendered(&a, 160, 40);
        assert!(screen.contains(line), "{screen}");
        assert!(screen.contains(&reference), "{screen}");
        // Anything said meanwhile comes first.
        a.say("Something else", Tone::Info);
        let screen = rendered(&a, 160, 40);
        assert!(screen.contains("Something else"), "{screen}");
        assert!(!screen.contains(line), "{screen}");
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

    /// The demo on the owner split (intent 0595), as `kerdr --demo` shows it.
    fn owner_demo() -> App {
        let mut a = App::new(Config::default(), crate::demo::machines(false));
        a.demo = true;
        a.fixed_now = Some(crate::demo::now());
        a.width = 176;
        a.height = 44;
        a.rebuild();
        a
    }

    #[test]
    fn the_owner_split_bar_lists_owners_then_an_owners_teams() {
        let mut a = owner_demo();
        let top = row(&a, 176, 44, 0);
        assert!(top.contains("All work ▾"), "{top}");
        for code in ["ARK", "SHP", "PLT"] {
            assert!(top.contains(code), "{top}");
        }
        assert!(
            !top.contains("laptop ▾") && !top.contains("All machines"),
            "{top}"
        );
        a.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('}'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        a.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('}'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        let top = row(&a, 176, 44, 0);
        assert!(top.contains("Shop · SHP"), "{top}");
        for tab in ["All 8", "Dev 4", "Marketing 3", "No team 1"] {
            assert!(top.contains(tab), "{top}");
        }
        let screen = rendered(&a, 176, 44);
        assert!(
            screen.contains("Shop Marketing's stages"),
            "a band for each list"
        );
        assert!(screen.contains("Standard stages · Dev, No team"));
        assert!(screen.contains("EXECUTE") && screen.contains("LEARN"));
        // On Marketing's tab, its own stages and no bands.
        a.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('3'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        let screen = rendered(&a, 176, 44);
        assert!(!screen.contains("Standard stages") && !screen.contains("DESIGN"));
        assert!(screen.contains("EXECUTE"));
    }

    #[test]
    fn the_owner_split_hint_names_owner_and_team_keys() {
        let mut a = owner_demo();
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.accounts.clear();
            }
        }
        a.rebuild();
        let top = row(&a, 176, 44, 0);
        assert!(top.contains("{ } owner · [ ] team"), "{top}");
    }

    /// F-0294 and intent 0321: a card an agent is at says on the board itself that s moves its
    /// agent, off the account it's on, and X cancels it; its peek says what s does.
    #[test]
    fn a_card_an_agent_is_at_names_s_and_x_on_the_board_and_in_its_peek() {
        let mut a = demo(false);
        a.cfg.actions.command = Some("fixture".into());
        let card = a
            .board
            .cards
            .iter()
            .find(|c| c.reference == "#44")
            .unwrap()
            .clone();
        assert!(card.cancellable(), "{card:?}");
        let account = a.move_agent(&card).unwrap().account;
        a.sel = crate::app::Selection {
            col: card.stage.index(),
            key: Some(card.key.clone()),
        };
        let screen = rendered(&a, 176, 44);
        let bottom = &screen[screen.len() - 176..];
        assert!(
            bottom.contains(&format!(
                "#44   ⏎ go in   s move its agent off {account}   X cancel"
            )),
            "{bottom}"
        );
        // At 80 columns the card's own keys still show whole.
        let narrow = rendered(&a, 80, 24);
        assert!(
            narrow[narrow.len() - 80..]
                .contains(&format!("s move its agent off {account}   X cancel")),
            "80 columns"
        );
        // The generic "s move" isn't said twice.
        assert_eq!(bottom.matches(" s ").count(), 1, "{bottom}");
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let peek = rendered(&a, 176, 44);
        assert!(
            peek.contains("s move this agent to another account, keeping its conversation"),
            "peek"
        );
        assert!(
            peek.contains(&format!("s move its agent off {account}")),
            "peek"
        );
        assert!(peek.contains("X cancel"), "peek");
        // X's question: its buttons say stop it or keep it running, never a bare "cancel".
        a.mode = Mode::Board;
        a.on_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('X'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(a.mode, Mode::Confirm { .. }), "{:?}", a.message());
        let asked = rendered(&a, 176, 44);
        assert!(
            asked.contains("Enter / y stop it   Esc / n keep it running"),
            "question"
        );
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
            owner.models = vec!["KBZ_C".into(), "ARK_C".into()];
        }
        assert!(rendered(&a, 176, 44).contains("new in web: Claude ARK_C"));
        for account in &mut a.machines[mi].feed.as_mut().unwrap().accounts {
            account.status = "out".into();
        }
        assert!(rendered(&a, 176, 44).contains("new in web: Claude KBZ_C"));
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

    /// Modes outside the board, where the action line is drawn over the screen.
    fn screens_over_the_board(a: &App) -> Vec<Mode> {
        let card = a.board.cards[0].key.clone();
        vec![
            Mode::Peek {
                scroll: 0,
                back: Box::new(Mode::Board),
            },
            Mode::Backlog { sel: 0 },
            Mode::Misc { sel: 0 },
            Mode::Card {
                card: card.clone(),
                focus: card,
                opened: vec![],
                back: Box::new(Mode::Board),
            },
        ]
    }

    #[test]
    fn newer_message_shows_over_busy_line() {
        let mut a = demo(false);
        for mode in screens_over_the_board(&a) {
            a.mode = mode;
            a.busy = true;
            a.busy_label = "Reopening #39…".into();
            a.busy_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
            a.say(
                "#41 opens once #39's answer is in (Esc drops it).",
                Tone::Info,
            );
            let text = rendered(&a, 176, 44);
            assert!(text.contains("#41 opens once #39's answer is in"), "{text}");
            // A message from before the action started gives way to the action's own line.
            a.message = Some((
                "Something said earlier".into(),
                Tone::Info,
                std::time::Instant::now() - std::time::Duration::from_secs(2),
            ));
            let text = rendered(&a, 176, 44);
            assert!(
                text.contains("Reopening #39… 1 s · the board is yours meanwhile"),
                "{text}"
            );
        }
    }

    #[test]
    fn busy_line_names_the_work_and_how_long() {
        let mut a = demo(false);
        a.busy = true;
        a.busy_label = "Reopening #39…".into();
        a.busy_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(20));
        a.message = None;
        for mode in screens_over_the_board(&a) {
            a.mode = mode;
            let text = rendered(&a, 176, 44);
            assert!(
                text.contains("Reopening #39… 20 s · the board is yours meanwhile"),
                "{text}"
            );
        }
        // On the board, its header says the same.
        a.mode = Mode::Board;
        assert!(rendered(&a, 176, 44).contains("◐ Reopening #39… 20 s"));
    }

    #[test]
    fn the_board_says_it_is_opening_a_card_while_herdr_gets_ready() {
        let mut a = demo(false);
        a.say(
            crate::jump::opening("41-checkout-expired-cards", "LAB-Server", false),
            Tone::Info,
        );
        assert!(rendered(&a, 176, 44).contains("Opening 41-checkout-expired-cards on LAB-Server…"));
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

    /// A box means selected or waiting on you: an agent at work or waiting on something else has
    /// none, and what waits on you comes first in its column (intent 0401).
    #[test]
    fn only_the_selected_card_and_cards_waiting_on_you_have_boxes() {
        let mut a = demo(false);
        let build: Vec<usize> = a.columns()[Stage::Build.index()].clone();
        assert!(build.len() >= 3, "the demo's Build column is too short");
        for &i in &build {
            a.board.cards[i].attention = Attention::Running;
            a.board.cards[i].cached_at = None;
            if let Some(d) = a.board.cards[i].detail.as_mut() {
                d.no_agent = false;
            }
        }
        let (asking, quiet) = (*build.last().unwrap(), build[build.len() - 2]);
        a.board.cards[asking].attention = Attention::NeedsYou;
        a.board.cards[quiet].attention = Attention::Idle;
        // Waiting on something else: no box either.
        let elsewhere = build[0];
        a.board.cards[elsewhere].attention = Attention::Waiting;
        a.board.holds.clear();
        let order = a.columns()[Stage::Build.index()].clone();
        assert_eq!(&order[..2], &[asking, quiet]);

        let area = Rect::new(0, 0, 40, 60);
        let corners = |a: &App, items: &[usize], selected: Option<&str>, want: &str| {
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
            terminal
                .draw(|f| cards(f, area, a, items, selected))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .filter(|c| c.symbol() == want)
                .map(|c| c.fg)
                .collect::<Vec<_>>()
        };
        // Yellow first, then an easy-to-see grey, and nothing else.
        assert_eq!(corners(&a, &order, None, "╭"), vec![YELLOW, DIM]);
        assert!(corners(&a, &order, None, "┏").is_empty());

        // A selected card that isn't waiting on you has a box while it's selected.
        let key = a.board.cards[elsewhere].key.clone();
        assert_eq!(corners(&a, &order, Some(&key), "┏"), vec![CYAN]);
        assert_eq!(corners(&a, &order, Some(&key), "╭"), vec![YELLOW, DIM]);

        // As last seen from a machine out of reach: no box, and after what's live.
        a.board.cards[asking].cached_at = Some(chrono::Local::now());
        let order = a.columns()[Stage::Build.index()].clone();
        assert_eq!(order[0], quiet);
        assert_eq!(*order.last().unwrap(), asking);
        assert_eq!(corners(&a, &order, None, "╭"), vec![DIM]);
    }

    /// A grey piece inside a card at work makes the card grey, unless it hasn't started yet.
    #[test]
    fn a_quiet_piece_inside_a_card_at_work_puts_the_card_on_you() {
        use crate::family::{Holds, OnYou, on_you};
        let a = demo(false);
        let mut parent = a.board.cards[a.columns()[Stage::Build.index()][0]].clone();
        parent.attention = Attention::Running;
        parent.cached_at = None;
        if let Some(d) = parent.detail.as_mut() {
            d.no_agent = false;
        }
        let mut piece = parent.clone();
        assert_eq!(on_you(&parent, &Holds::of([&piece])), None);
        piece.attention = Attention::Idle;
        assert_eq!(on_you(&parent, &Holds::of([&piece])), Some(OnYou::Quiet));
        piece.stage = Stage::Backlog;
        assert_eq!(on_you(&parent, &Holds::of([&piece])), None);
    }

    /// j and k walk the column in the order it's shown, and the selection stays on its card when
    /// that card's turn comes up to the top (intent 0401).
    #[test]
    fn j_and_k_follow_a_column_as_what_waits_on_you_moves_up() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut a = demo(false);
        let build: Vec<usize> = a.columns()[Stage::Build.index()].clone();
        for &i in &build {
            a.board.cards[i].attention = Attention::Running;
            a.board.cards[i].cached_at = None;
            if let Some(d) = a.board.cards[i].detail.as_mut() {
                d.no_agent = false;
            }
        }
        a.board.holds.clear();
        let last = *build.last().unwrap();
        let key = a.board.cards[last].key.clone();
        a.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key.clone()),
        };
        a.board.cards[last].attention = Attention::Idle;
        let order = a.columns()[Stage::Build.index()].clone();
        assert_eq!(order[0], last);
        assert_eq!(a.selected().map(|c| c.key.clone()), Some(key));
        a.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(
            a.selected().map(|c| c.key.clone()),
            Some(a.board.cards[order[1]].key.clone())
        );
        a.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(
            a.selected().map(|c| c.key.clone()),
            Some(a.board.cards[order[0]].key.clone())
        );
    }

    /// Intent 0289: work its agent left open says "no agent" on its card, and the bottom line,
    /// a peek, the menu and the keys list name C and x for it.
    #[test]
    fn work_running_outside_herdr_says_where_on_its_card() {
        let mut a = demo(false);
        a.cfg.actions.command = Some("test-adapter".into());
        let i = a
            .board
            .cards
            .iter()
            .position(|c| c.detail.is_some() && c.stage == Stage::Build)
            .unwrap();
        let card = &mut a.board.cards[i];
        card.workspace_id = None;
        card.session = None;
        card.agents.clear();
        card.detail.as_mut().unwrap().outside_herdr = Some(crate::feed::OutsideHerdr {
            terminal: Some("foot".into()),
            since: None,
        });
        let (key, reference) = (card.key.clone(), card.reference.clone());
        let board = rendered(&a, 220, 44);
        assert!(board.contains("outside herdr · foot"), "{board}");
        // ⏎ moves it into herdr (intent 0591), and its hint says so rather than "reopen it".
        a.sel = crate::app::Selection {
            col: Stage::Build.index(),
            key: Some(key),
        };
        let board = rendered(&a, 220, 44);
        assert!(
            board.contains(&format!(" {reference}   ⏎ move it into herdr")),
            "{board}"
        );
        assert!(!board.contains("⏎ reopen it"), "{board}");
    }

    /// Intent 0591: a card that needs you with no workspace open says what gets it one, never only
    /// that it has none.
    #[test]
    fn work_that_needs_you_with_no_workspace_says_what_gets_it_one() {
        let mut a = demo(false);
        let card = a
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Backlog && c.detail.is_some() && !c.dismissed())
            .unwrap()
            .clone();
        assert!(where_to_answer(&a, &card).contains("no [actions] command"));
        a.cfg.actions.command = Some("test-adapter".into());
        let mut questions = card.clone();
        questions.detail.as_mut().unwrap().kickoff =
            serde_json::from_value(serde_json::json!({"asked":"t","questions":[]})).ok();
        assert!(where_to_answer(&a, &questions).starts_with("d asks its questions"));
        let mut none = card.clone();
        none.detail.as_mut().unwrap().kickoff = None;
        assert!(where_to_answer(&a, &none).starts_with("⏎ starts it"));
        let mut aside = none.clone();
        aside.detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(where_to_answer(&a, &aside).contains("u brings it back"));
    }

    #[test]
    fn work_with_no_agent_says_so_and_names_c_and_x() {
        let mut a = demo(false);
        a.cfg.actions.command = Some("test-adapter".into());
        let i = a
            .board
            .cards
            .iter()
            .position(|c| c.detail.is_some() && c.stage == Stage::Build)
            .unwrap();
        let card = &mut a.board.cards[i];
        card.workspace_id = None;
        card.session = None;
        card.agents.clear();
        card.attention = Attention::Idle;
        card.detail.as_mut().unwrap().no_agent = true;
        let reference = card.reference.clone();
        a.sel = crate::app::Selection {
            col: Stage::Build.index(),
            key: Some(card.key.clone()),
        };
        let board = rendered(&a, 220, 44);
        assert!(board.contains("no agent"), "{board}");
        assert!(
            board.contains(&format!(
                " {reference}   ⏎ reopen it   s resume it on an account   C done   x won’t do"
            )),
            "{board}"
        );
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let peek = rendered(&a, 176, 44);
        assert!(peek.contains("C done   x won’t do"), "{peek}");
        a.mode = Mode::Palette {
            back: Box::new(Mode::Board),
            target: Some(a.board.cards[i].key.clone()),
        };
        let menu = rendered(&a, 176, 44);
        assert!(menu.contains("done, starting nothing"), "{menu}");
        assert!(WORK_KEYS.iter().any(|(_, rows)| {
            rows.iter()
                .any(|(k, what, _)| *k == "C" && what.starts_with("done: closes it"))
        }));
        // A workspace on it again: no mark, and C and x aren't offered.
        a.mode = Mode::Board;
        a.board.cards[i].workspace_id = Some("w9".into());
        a.board.cards[i].session = Some("web".into());
        let board = rendered(&a, 220, 44);
        assert!(
            !board.contains("no agent") && !board.contains("C done"),
            "{board}"
        );
    }

    fn kickoff_form() -> crate::app::Kickoff {
        let response = crate::actions::Response {
            confirmation: true,
            confirm: Some("0123456789abcdef".into()),
            asked: Some("fedcba9876543210".into()),
            questions: vec![
                crate::actions::Question {
                    num: "1".into(),
                    question: "Whose work is this, Arkship's or Kibbelz's?".into(),
                    background: format!(
                        "It decides which machine runs it. {}And the last words.",
                        "It matters. ".repeat(12)
                    ),
                    pick: "Kibbelz".into(),
                    why: "it sounds like your personal site (a guess)".into(),
                },
                crate::actions::Question {
                    num: "2".into(),
                    question: "Where should it be served?".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let request = serde_json::json!({"action":"dispatch","id":"0449"});
        crate::app::Kickoff::from_answer(
            0,
            &request,
            &response,
            "#449".into(),
            "Robb Morgan site".into(),
            &[],
        )
        .unwrap()
    }

    /// The colour `text` is drawn in, wherever it first shows on the screen.
    fn colour_of(a: &App, w: u16, h: u16, text: &str) -> ratatui::style::Color {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, a)).unwrap();
        let buf = t.backend().buffer();
        let cells: Vec<&str> = buf.content.iter().map(|c| c.symbol()).collect();
        let needle: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        let at = cells
            .windows(needle.len())
            .position(|run| run.iter().zip(&needle).all(|(a, b)| *a == b))
            .unwrap_or_else(|| panic!("{text} isn't on screen"));
        buf.content[at].fg
    }

    #[test]
    fn a_questions_keys_are_faint_until_they_can_answer_it() {
        // A guard that swallows a key looks like a hang unless it shows (intent 0491).
        use std::time::Duration;
        let mut a = App::new(Config::default(), crate::demo::machines(false));
        a.mode = Mode::Confirm {
            machine: 0,
            request: serde_json::json!({"action": "reopen", "id": "1"}),
            title: "Reopen #1 in web?".into(),
            message: "It's done.".into(),
            back: Box::new(Mode::Board),
            target: None,
            queue: true,
            ready_at: Some(Instant::now() + Duration::from_secs(60)),
            plain: false,
            choices: vec![],
            enter_queues: false,
        };
        assert_eq!(colour_of(&a, 120, 40, "Enter / y"), FAINT);
        assert_eq!(colour_of(&a, 120, 40, "w queue"), FAINT);
        // What never needs the guard reads as usual meanwhile.
        assert_eq!(colour_of(&a, 120, 40, "Esc / n"), FG);
        if let Mode::Confirm { ready_at, .. } = &mut a.mode {
            *ready_at = Some(Instant::now());
        }
        assert_eq!(colour_of(&a, 120, 40, "Enter / y"), FG);
        assert_eq!(colour_of(&a, 120, 40, "w queue"), FG);
    }

    #[test]
    fn a_kickoff_questions_enter_is_faint_until_it_can_take_it() {
        use std::time::Duration;
        let mut a = App::new(Config::default(), crate::demo::machines(false));
        let mut form = kickoff_form();
        form.ready_at = Instant::now() + Duration::from_secs(60);
        a.mode = Mode::Kickoff {
            form: form.clone(),
            back: Box::new(Mode::Board),
            target: None,
        };
        assert_eq!(colour_of(&a, 120, 50, "⏎ take"), FAINT);
        assert_eq!(colour_of(&a, 120, 50, "esc leave"), FG);
        form.ready_at = Instant::now();
        a.mode = Mode::Kickoff {
            form,
            back: Box::new(Mode::Board),
            target: None,
        };
        assert_eq!(colour_of(&a, 120, 50, "⏎ take"), FG);
    }

    #[test]
    fn a_failure_box_says_a_key_closes_it_only_once_one_can() {
        let mut a = App::new(Config::default(), crate::demo::machines(false));
        a.fail("It didn't work.");
        let early = rendered(&a, 120, 40);
        assert!(early.contains("as it really is"), "{early}");
        assert!(!early.contains("Any key"), "{early}");
        let back = |a: &mut App| {
            if let Some((_, _, at)) = &mut a.message {
                *at -= crate::app::FAIL_SEEN;
            }
        };
        back(&mut a);
        assert!(rendered(&a, 120, 40).contains("Any key closes this."));
        // With more behind it, the same.
        a.message = None;
        a.fail("One\nTwo\nThree\nFour");
        assert!(!rendered(&a, 120, 40).contains("Any key"));
        back(&mut a);
        assert!(rendered(&a, 120, 40).contains("Any key shows 1 more."));
    }

    #[test]
    fn kickoff_questions_say_it_is_background_work_and_show_each_question_whole() {
        let mut a = demo(false);
        let form = kickoff_form();
        a.mode = Mode::Kickoff {
            form: form.clone(),
            back: Box::new(Mode::Board),
            target: None,
        };
        let text = rendered(&a, 176, 44);
        for want in [
            "BACKGROUND TASK",
            "#449",
            "Robb Morgan site",
            "1 of 2",
            "runs on its own",
            "Kickoff Questions",
            "Whose work is this, Arkship's or Kibbelz's?",
            "And the last words.",
            "Recommended",
            "Kibbelz",
            "it sounds like your personal site (a guess)",
            "take the recommendation",
            "start it with me along",
            "leave it for later",
        ] {
            assert!(text.contains(want), "{want} missing: {text}");
        }
        assert!(!text.contains("I'd say"), "{text}");

        // The second question: where you are, a way back, and no recommendation to take.
        let mut second = form.clone();
        second.at = 1;
        a.mode = Mode::Kickoff {
            form: second.clone(),
            back: Box::new(Mode::Board),
            target: None,
        };
        let text = rendered(&a, 176, 44);
        assert!(
            text.contains("2 of 2") && text.contains("back to question 1"),
            "{text}"
        );
        assert!(
            text.contains("Type your answer.") && !text.contains("Recommended"),
            "{text}"
        );
        second.drafts[1] = "On the laptop this one time".into();
        a.mode = Mode::Kickoff {
            form: second,
            back: Box::new(Mode::Board),
            target: None,
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("On the laptop this one time"), "{text}");
        assert!(text.contains("send your answers and start it"), "{text}");
    }

    #[test]
    fn kickoff_questions_keep_their_answer_box_and_keys_on_a_small_screen() {
        let mut a = demo(false);
        a.mode = Mode::Kickoff {
            form: kickoff_form(),
            back: Box::new(Mode::Board),
            target: None,
        };
        let text = rendered(&a, 80, 24);
        for want in [
            "BACKGROUND TASK",
            "Type your own answer",
            "esc",
            "Recommended  Kibbelz",
            "take the recommendation",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        // Too small for all of it: the rest is a key away, and ⏎ shows it before it can take it.
        let text = rendered(&a, 60, 20);
        for want in [
            "BACKGROUND TASK",
            "Type your own answer",
            "esc",
            "pgdn",
            "show the rest of it",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!text.contains("Recommended"), "{text}");
        let Mode::Kickoff { form, .. } = &a.mode else {
            panic!()
        };
        form.scroll.set(u16::MAX);
        let text = rendered(&a, 60, 20);
        for want in [
            "Recommended  Kibbelz",
            "it sounds like your",
            "take the recommendation",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
    }

    #[test]
    fn a_long_recommendation_can_be_read_whole_on_a_small_screen() {
        let mut form = kickoff_form();
        form.questions[0].pick =
            "A pick that is much too long to fit beside its label on this screen".into();
        form.questions[0].why = format!("{}FINALE", "A reason that goes on. ".repeat(12));
        let mut a = demo(false);
        a.mode = Mode::Kickoff {
            form,
            back: Box::new(Mode::Board),
            target: None,
        };
        let Mode::Kickoff { form, .. } = &a.mode else {
            panic!()
        };
        rendered(&a, 60, 20);
        form.scroll.set(u16::MAX);
        let text = rendered(&a, 60, 20);
        assert!(text.contains("FINALE"), "{text}");
        form.scroll.set(form.scroll.get().saturating_sub(4));
        let text = rendered(&a, 60, 20);
        assert!(!text.contains("FINALE"), "{text}");
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
                text.contains("agents: all, then each one"),
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
                "move to the top / bottom",
                "new intent",
                "the backlog",
                "won’t do / done work",
                "filter by who, agents or bg",
                "agents: all, then each one",
                "machines",
                "every key",
                "without Ctrl+Space",
            ] {
                assert!(text.contains(expected), "{expected} missing at {w}x{h}");
            }
        }
    }

    #[test]
    fn the_filter_screen_names_each_filter_and_the_board_names_the_background_one() {
        let mut a = demo(false);
        a.mode = Mode::Filter {
            query: String::new(),
            sel: 0,
            typing: false,
            back: Box::new(Mode::Board),
        };
        for (w, h) in [(100, 30), (176, 44)] {
            let text = rendered(&a, w, h);
            for expected in [
                "FILTERS",
                "WHO IT’S FOR",
                "press / to search",
                "AGENTS",
                "BACKGROUND",
                "bg only",
                "no bg",
                "b cycles",
            ] {
                assert!(text.contains(expected), "{expected} missing at {w}x{h}");
            }
        }
        a.mode = Mode::Filter {
            query: "ark".into(),
            sel: 0,
            typing: true,
            back: Box::new(Mode::Board),
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("esc stops searching"), "{text}");
        a.mode = Mode::Board;
        assert!(!rendered(&a, 176, 44).contains("bg only"));
        a.background = Background::Only;
        let text = rendered(&a, 176, 44);
        assert!(text.contains("showing bg only"), "{text}");
        a.background = Background::Hide;
        assert!(rendered(&a, 176, 44).contains("showing no bg"));
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
        // With intent actions ⏎ starts it too, so the peek and the board name it beside d; the
        // backlog screen's ⏎ peeks, so neither its line nor its peek's does (intent 0404).
        a.cfg.actions.command = Some("test-adapter".into());
        let text = rendered(&a, 176, 44);
        assert!(text.contains("d ⏎ start it in its session"), "{text}");
        a.mode = Mode::Board;
        let text = rendered(&a, 176, 44);
        assert!(text.contains("d ⏎ start it"), "{text}");
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Backlog { sel: 0 }),
        };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("d start it in its session"), "{text}");
        assert!(!text.contains("d ⏎"), "{text}");
        a.mode = Mode::Backlog { sel: 0 };
        let text = rendered(&a, 176, 44);
        assert!(text.contains("⏎ peek"), "{text}");
        assert!(!text.contains("d ⏎"), "{text}");
    }

    #[test]
    fn a_done_card_says_enter_asks_to_reopen_it_on_the_board_in_a_peek_and_in_help() {
        let mut a = demo(false);
        a.cfg.actions.command = Some("test-adapter".into());
        let i = a
            .board
            .cards
            .iter()
            .position(|c| c.can_reopen_done())
            .expect("a done card in the demo");
        a.board.cards[i].detail.as_mut().unwrap().resume = Some(crate::feed::Resume {
            agent: Some("claude".into()),
            conversation: Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41".into()),
        });
        a.sel = crate::app::Selection {
            col: Stage::Maintain.index(),
            key: Some(a.board.cards[i].key.clone()),
        };
        let reference = a.board.cards[i].reference.clone();
        let board = rendered(&a, 220, 44);
        assert!(
            board.contains(&format!(" {reference}   ⏎ ask to reopen it")),
            "{board}"
        );
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let peek = rendered(&a, 176, 44);
        assert!(
            peek.contains("⏎ ask to reopen it, resuming its Claude Code conversation"),
            "{peek}"
        );
        // Without actions, nothing offers what Enter can't do.
        a.cfg.actions.command = None;
        assert!(!rendered(&a, 176, 44).contains("ask to reopen"));
        a.mode = Mode::Board;
        assert!(!rendered(&a, 220, 44).contains("ask to reopen"));
        assert!(BOARD_KEYS.iter().any(|(_, rows)| {
            rows.iter()
                .any(|(k, what, _)| *k == "⏎" && what.starts_with("on done work: asks"))
        }));
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

    /// Each machine's feed holds only the usage its own sessions last saw. Gives ARK_C an old
    /// reading on the laptop and a new one on the build box, and KBZ_C the other way round.
    fn split_readings(a: &mut App) {
        let at = |h, m| {
            chrono::Local
                .with_ymd_and_hms(2026, 10, 21, h, m, 0)
                .single()
                .unwrap()
                .to_rfc3339()
        };
        let reading = |seen: String, five: f64, week: f64| crate::accounts::Usage {
            five: Some(five),
            five_at: Some(at(21, 0)),
            week: Some(week),
            week_at: Some(at(23, 0)),
            seen: Some(seen),
            ..Default::default()
        };
        for m in &mut a.machines {
            let laptop = m.name() == "laptop";
            let Some(f) = &mut m.feed else { continue };
            for acc in &mut f.accounts {
                acc.usage = Some(match (acc.code.as_str(), laptop) {
                    ("ARK_C", true) => reading(at(11, 44), 0.0, 0.0),
                    ("ARK_C", false) => reading(at(17, 45), 65.0, 19.0),
                    ("KBZ_C", true) => reading(at(17, 44), 17.0, 87.0),
                    _ => reading(at(9, 0), 2.0, 83.0),
                });
                acc.status = "ready".into();
            }
        }
    }

    fn top_line(a: &App, w: u16) -> String {
        rendered(a, w, 44).chars().take(w as usize).collect()
    }

    #[test]
    fn usage_is_each_accounts_freshest_reading_on_every_machine_tab() {
        let mut a = demo(false);
        split_readings(&mut a);
        for machine in [None, Some("laptop"), Some("build-box")] {
            a.machine = machine.map(str::to_string);
            a.rebuild();
            let top = top_line(&a, 176);
            assert!(top.contains("ARK_C 65│19"), "{machine:?}: {top}");
            assert!(top.contains("KBZ_C 17│87"), "{machine:?}: {top}");
            a.open_accounts();
            let screen = rendered(&a, 176, 44);
            assert!(screen.contains("65│19  ready"), "{machine:?}: {screen}");
            assert!(screen.contains("17│87  ready"), "{machine:?}: {screen}");
            assert!(!screen.contains("0│0"), "{machine:?}: {screen}");
            a.mode = Mode::Board;
        }
    }

    #[test]
    fn tied_readings_show_the_same_figures_on_every_tab() {
        let mut a = demo(false);
        for m in &mut a.machines {
            let laptop = m.name() == "laptop";
            let Some(f) = &mut m.feed else { continue };
            for acc in f.accounts.iter_mut().filter(|acc| acc.code == "ARK_C") {
                acc.usage = Some(crate::accounts::Usage {
                    five: Some(if laptop { 65.0 } else { 1.0 }),
                    week: Some(19.0),
                    seen: Some("2026-10-21T17:40:00-05:00".into()),
                    ..Default::default()
                });
            }
        }
        for machine in [None, Some("laptop"), Some("build-box")] {
            a.machine = machine.map(str::to_string);
            a.rebuild();
            let top = top_line(&a, 176);
            assert!(top.contains("ARK_C 65│19"), "{machine:?}: {top}");
        }
    }

    #[test]
    fn a_reading_past_its_reset_time_says_so_instead_of_showing_old_figures() {
        let mut a = demo(false);
        let at = |h, m| {
            chrono::Local
                .with_ymd_and_hms(2026, 10, 21, h, m, 0)
                .single()
                .unwrap()
                .to_rfc3339()
        };
        let (five_at, week_at) = (at(16, 40), at(17, 0));
        for m in &mut a.machines {
            let Some(f) = &mut m.feed else { continue };
            // The Codex account keeps its own weekly-only reading.
            for acc in f.accounts.iter_mut().filter(|acc| acc.code != "KBZ_X") {
                acc.usage = Some(crate::accounts::Usage {
                    five: Some(40.0),
                    five_at: Some(five_at.clone()),
                    week: Some(19.0),
                    week_at: Some(if acc.code == "ARK_C" {
                        week_at.clone()
                    } else {
                        "2026-10-24T09:00:00-05:00".into()
                    }),
                    seen: Some(at(11, 44)),
                    ..Default::default()
                });
            }
        }
        a.machine = Some("laptop".into());
        a.rebuild();
        let top = top_line(&a, 176);
        assert!(top.contains("ARK_C —│—"), "{top}");
        assert!(top.contains("KBZ_C —│19"), "{top}");
        a.open_accounts();
        let five = format!(
            "five-hour window reset {}; use since unknown",
            crate::accounts::reset_time(&five_at)
        );
        let week = format!(
            "weekly window reset {}; use since unknown",
            crate::accounts::reset_time(&week_at)
        );
        // KBZ_C is out in the demo and ARK_C ready; both explain their resets in full at every width.
        for w in [80, 120, 176] {
            let screen = rendered(&a, w, 44);
            assert!(screen.contains("—│—  ready"), "{w}: {screen}");
            let out = format!("—│19  {}", crate::accounts::pause_glyph());
            assert!(screen.contains(&out), "{w}: {screen}");
            assert_eq!(screen.matches(&five).count(), 2, "{w}: {screen}");
            assert_eq!(screen.matches(&week).count(), 1, "{w}: {screen}");
        }
    }

    /// Intent 0356: Codex accounts beside Claude's, every name from the feed's providers.
    #[test]
    fn the_top_bar_groups_accounts_by_provider_with_the_windows_each_has() {
        let mut a = demo(false);
        a.machine = Some("laptop".into());
        a.rebuild();
        let top = top_line(&a, 176);
        assert!(top.contains("Claude KBZ_C 86│100"), "{top}");
        assert!(top.contains("· ARK_C 12│30"), "{top}");
        // Codex comes after Claude, as the feed lists them, with its one weekly figure.
        let (claude, codex) = (
            top.find("Claude").unwrap(),
            top.find("Codex KBZ_X 7").unwrap(),
        );
        assert!(claude < codex, "{top}");
    }

    #[test]
    fn a_handed_over_card_shows_its_mark_and_its_peek_says_from_where_why_and_when() {
        let mut a = demo(false);
        a.machine = Some("laptop".into());
        a.rebuild();
        assert!(rendered(&a, 176, 44).contains("KBZ_X ⇄"));
        // Narrow, the whole code stays and the mark and the age give way.
        let narrow = rendered(&a, 80, 30);
        assert!(narrow.contains("KBZ_X"), "{narrow}");
        assert!(!narrow.contains("KBZ_X ⇄"), "{narrow}");
        let c = a.board.cards.iter().find(|c| c.reference == "#47").unwrap();
        a.sel = Selection {
            col: c.stage.index(),
            key: Some(c.key.clone()),
        };
        a.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        let peek = rendered(&a, 176, 44);
        assert!(peek.contains("KBZ_X Codex · 7 this week · ready"), "{peek}");
        assert!(
            peek.contains("Handed over from KBZ_C (Claude Code) at"),
            "{peek}"
        );
        assert!(
            peek.contains("KBZ_C ran out. A fresh Codex agent"),
            "{peek}"
        );
        assert!(
            peek.contains("It stays on Codex until KBZ_X runs out"),
            "{peek}"
        );
        // A program the feed doesn't name is left out of the sentences, not shown blank.
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                for g in f.agents.iter_mut().filter(|g| g.handed_from.is_some()) {
                    g.program.clear();
                }
                for acc in f.accounts.iter_mut().filter(|acc| acc.code == "KBZ_X") {
                    acc.runs.clear();
                }
            }
        }
        let peek = rendered(&a, 176, 44);
        assert!(
            peek.contains("A fresh agent picked up from the log."),
            "{peek}"
        );
        assert!(peek.contains("It stays until KBZ_X runs out"), "{peek}");
        assert!(!peek.contains("fresh  agent"), "{peek}");
    }

    #[test]
    fn a_six_letter_code_is_never_cut_on_its_card() {
        let mut a = demo(false);
        a.machine = Some("laptop".into());
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                for agent in f.agents.iter_mut().filter(|g| g.account == "ARK_C") {
                    agent.account = "ANET_C".into();
                }
            }
        }
        a.rebuild();
        for w in [80, 100, 176] {
            let screen = rendered(&a, w, 30);
            assert!(screen.contains("ANET_C"), "{w}: {screen}");
            assert!(!screen.contains("ANET_…"), "{w}: {screen}");
        }
    }

    #[test]
    fn a_feed_without_providers_names_accounts_as_before() {
        let mut a = demo(false);
        a.machine = Some("laptop".into());
        for m in &mut a.machines {
            if let Some(f) = &mut m.feed {
                f.providers.clear();
                f.accounts.retain(|acc| acc.provider == "claude");
                for acc in &mut f.accounts {
                    acc.provider_label = None;
                    acc.program_label = None;
                }
            }
        }
        a.rebuild();
        let top = top_line(&a, 176);
        assert!(top.contains("Claude KBZ_C 86│100"), "{top}");
        a.open_accounts();
        let screen = rendered(&a, 176, 44);
        assert!(screen.contains("Claude · saved sign-in"), "{screen}");
        assert!(screen.contains("Claude · token per machine"), "{screen}");
    }

    #[test]
    fn a_row_shows_a_start_or_reopen_on_its_way_whatever_its_column() {
        let a = crate::app::App::new(
            crate::config::Config::default(),
            crate::demo::machines(false),
        );
        let mut done = a
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Maintain && c.detail.is_some())
            .cloned()
            .expect("a done card in the demo");
        done.detail.as_mut().unwrap().status = Some("Reopening…".into());
        assert_eq!(list_glyph(&done, a.now(), usize::MAX).2, "reopening");
        done.detail.as_mut().unwrap().status = Some("Starting…".into());
        assert_eq!(list_glyph(&done, a.now(), usize::MAX).2, "starting up");
        done.detail.as_mut().unwrap().status = Some("Starting on arknet-one…".into());
        assert_eq!(list_glyph(&done, a.now(), usize::MAX).2, "starting up");
    }

    #[test]
    fn a_row_says_when_its_change_or_its_new_intent_is_being_sent_again() {
        use crate::app::tests::{demo_app, press};
        let mut a = demo_app();
        a.cfg.actions.command = Some("test-adapter".into());
        let again = crate::actions::Resend::Retry {
            wait: std::time::Duration::from_secs(5),
            message: "#4's record is locked by a pull under way.".into(),
        };
        let backlog: Vec<Card> = a
            .board
            .cards
            .iter()
            .filter(|c| c.stage == Stage::Backlog && !c.dismissed() && c.intake().is_none())
            .filter(|c| c.detail.is_some())
            .take(2)
            .cloned()
            .collect();
        // A refresh refused for something passing, sent again by itself (intent 0591).
        let (refreshed, started) = (&backlog[0], &backlog[1]);
        let id = |c: &Card| c.detail.as_ref().unwrap().id.clone();
        a.queue_action(
            refreshed.machine_index,
            serde_json::json!({"action": "refresh", "id": id(refreshed)}),
        );
        let (n, ..) = a.resolver.ready()[0].clone();
        a.resending(Some(n), refreshed.machine_index, &again);
        let row = |a: &App, key: &str| list_glyph(a.card(key).unwrap(), a.now(), usize::MAX);
        let (glyph, color, words, _) = row(&a, &refreshed.key);
        assert_eq!((glyph, color, words.as_str()), ("◐", BLUE, "trying again…"));
        a.mode = Mode::Backlog { sel: 0 };
        let screen = tests_render(&a, 200, 50);
        assert!(screen.contains("trying again…"), "{screen}");
        // A new intent's words: not just "saving…".
        a.mode = Mode::Board;
        press(&mut a, 'c');
        for c in "Back up the NAS".chars() {
            press(&mut a, c);
        }
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let crate::app::Action::Mutate { machine, request } =
            a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        a.queue_action(machine, request);
        let (n, ..) = a.resolver.ready()[0].clone();
        a.resending(Some(n), machine, &again);
        let saving = a.backlog_card_for_keys().cloned().expect("its card");
        assert_eq!(row(&a, &saving.key).2, "saving, trying again…");
        let screen = tests_render(&a, 200, 50);
        assert!(screen.contains("saving, trying again…"), "{screen}");
        // A start's place says so too.
        a.mode = Mode::Board;
        a.queue_action(
            started.machine_index,
            serde_json::json!({"action": "dispatch", "id": id(started)}),
        );
        let (n, ..) = a.resolver.ready()[0].clone();
        a.resending(Some(n), started.machine_index, &again);
        let c = a.card(&started.key).unwrap().clone();
        let place = crate::ui::card_ui::place_for_test(&a, &c).0;
        assert!(place.starts_with("◐ starting up, trying again…"), "{place}");
    }

    /// Intent 0455: a current account shows on the top bar, even when its usage doesn't fit, and on
    /// the accounts screen; a machine where it can't start sessions says why, in another colour.
    #[test]
    fn the_current_account_shows_on_the_top_bar_and_the_accounts_screen() {
        let mut a = demo(false);
        let plain = rendered(&a, 176, 44);
        assert!(
            !plain.contains("★"),
            "nothing new without a current account"
        );
        for m in a.machines.iter_mut() {
            let name = m.name();
            let Some(feed) = m.feed.as_mut() else {
                continue;
            };
            feed.current = Some(Some(crate::accounts::Current {
                account: "KBZ_X".into(),
                name: "Personal Codex".into(),
                since: Some("2026-10-21T16:00:00-05:00".into()),
                status: "ready".into(),
                until: None,
                machine: Some(name),
            }));
        }
        let board = rendered(&a, 176, 44);
        assert!(board.contains("★ current KBZ_X"), "{board}");
        let narrow = rendered(&a, 80, 24);
        assert!(
            narrow.contains("★ KBZ_X") || narrow.contains("★ current KBZ_X"),
            "{narrow}"
        );
        a.open_accounts();
        let text = rendered(&a, 176, 44);
        assert!(text.contains("CURRENT ACCOUNT"), "{text}");
        assert!(
            text.contains("new sessions start on it, ahead of their lists"),
            "{text}"
        );
        assert!(text.contains("laptop: new sessions start on it"), "{text}");
        assert!(text.contains("Personal Codex   Codex"), "{text}");
        assert!(text.contains("★ current"), "{text}");
        // Out on the build box: that machine's line says so, and its lists start new work there.
        let c = a.machines[1]
            .feed
            .as_mut()
            .unwrap()
            .current
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap();
        c.status = "sign_in".into();
        let text = rendered(&a, 176, 44);
        assert!(
            text.contains("build-box: no sign-in there, so new sessions use their lists"),
            "{text}"
        );
    }

    /// A feed from before the current account, with no `current` or a null one, reads as none.
    #[test]
    fn a_feed_without_a_current_account_reads_as_none() {
        let old: crate::feed::Feed = serde_json::from_str(r#"{"kerdr_feed":1}"#).unwrap();
        assert!(old.current_account().is_none() && !old.knows_current());
        // Written back (a cached feed), an older one stays older.
        let again: crate::feed::Feed =
            serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        assert!(!again.knows_current());
        let null: crate::feed::Feed =
            serde_json::from_str(r#"{"kerdr_feed":1,"current":null}"#).unwrap();
        assert!(null.current_account().is_none() && null.knows_current());
        let set: crate::feed::Feed = serde_json::from_str(
            r#"{"kerdr_feed":1,"current":{"account":"KBZ_C","name":null,"status":"out","until":null}}"#,
        )
        .unwrap();
        let c = set.current_account().unwrap();
        assert_eq!((c.account.as_str(), c.name.as_str()), ("KBZ_C", ""));
        assert_eq!(c.held_back("here").as_deref(), Some("out until reset"));
    }

    /// Sets the same current account on every machine's feed.
    fn set_current(a: &mut App, code: &str, status: &str) {
        for m in a.machines.iter_mut() {
            let Some(feed) = m.feed.as_mut() else {
                continue;
            };
            feed.current = Some(Some(crate::accounts::Current {
                account: code.into(),
                status: status.into(),
                until: Some("2026-10-22T09:00:00-05:00".into()),
                ..Default::default()
            }));
        }
    }

    /// One row of the screen, as text.
    fn row(a: &App, w: u16, h: u16, y: usize) -> String {
        rendered(a, w, h)
            .chars()
            .skip(y * w as usize)
            .take(w as usize)
            .collect()
    }

    /// Codex's review: an older feed shows the accounts screen as before, with no block or keys.
    #[test]
    fn an_older_feed_shows_the_accounts_screen_as_before() {
        let mut a = demo(false);
        a.open_accounts();
        let text = rendered(&a, 176, 44);
        assert!(!text.contains("CURRENT ACCOUNT"), "{text}");
        assert!(!text.contains("make current"), "{text}");
        assert!(!text.contains("make it current with c"), "{text}");
        // A backend that has it, none set: the block says so, and c is offered.
        for m in a.machines.iter_mut() {
            if let Some(f) = m.feed.as_mut() {
                f.current = Some(None);
            }
        }
        let text = rendered(&a, 176, 44);
        assert!(text.contains("CURRENT ACCOUNT"), "{text}");
        assert!(text.contains("c make current"), "{text}");
    }

    /// Codex's review: the current account outlasts usage and the title at any width, and keeps
    /// the selected session's tab in sight.
    #[test]
    fn the_current_account_stays_in_sight_on_narrow_screens() {
        let mut a = demo(false);
        set_current(&mut a, "KBZ_X", "out");
        for w in [36, 60, 80] {
            let top = row(&a, w, 24, 0);
            assert!(top.contains("★") && top.contains("KBZ_X"), "{w}: {top}");
        }
        a.open_accounts();
        for w in [50, 80, 120] {
            let top = row(&a, w, 30, 1);
            assert!(top.contains("★") && top.contains("KBZ_X"), "{w}: {top}");
        }
        let wide = row(&a, 176, 44, 1);
        assert!(wide.contains("★ current KBZ_X · out until"), "{wide}");
        a.mode = Mode::Board;
        a.machine = Some(a.this_machine());
        a.rebuild();
        a.tab = a.tabs.len().saturating_sub(1);
        let Some(Scope::Session(_, session)) = a.tabs.last().cloned() else {
            panic!("the demo has session tabs")
        };
        for w in [44, 60, 80] {
            let top = row(&a, w, 24, 0);
            assert!(
                top.contains(&session) && top.contains("KBZ_X"),
                "{w}: {top}"
            );
        }
    }

    /// Codex's review: an account in no list that is current says new sessions start on it.
    #[test]
    fn the_current_account_in_no_list_says_it_still_runs() {
        let mut a = demo(false);
        for m in a.machines.iter_mut() {
            for o in m.feed.iter_mut().flat_map(|f| f.owners.iter_mut()) {
                o.models.retain(|c| c != "KBZ_X");
            }
        }
        set_current(&mut a, "KBZ_X", "ready");
        a.open_accounts();
        let rows = match &a.mode {
            Mode::Accounts {
                screen: crate::app::account_controls::AccountScreen::List { rows, .. },
                ..
            } => rows.clone(),
            _ => panic!("accounts list"),
        };
        let at = rows
            .iter()
            .position(|r| {
                matches!(r, crate::app::account_controls::Row::Account(c)
                    | crate::app::account_controls::Row::Copy(c, _) if c == "KBZ_X")
            })
            .unwrap();
        if let Mode::Accounts {
            screen: crate::app::account_controls::AccountScreen::List { sel, .. },
            ..
        } = &mut a.mode
        {
            *sel = at;
        }
        let text = rendered(&a, 176, 44);
        assert!(text.contains("In no list, but it's the current"), "{text}");
    }
}
