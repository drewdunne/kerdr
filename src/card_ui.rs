//! A card's own screen (`o`): the card itself, then what's inside it by group, and the selected
//! row's details beside the list, or below it on a narrow screen.

use super::*;
use crate::app::inside::Row;
use crate::family::{self, Group, Mark};

pub fn mark_color(m: Mark) -> Color {
    match m {
        Mark::NeedsYou | Mark::NoAgent => YELLOW,
        Mark::Running | Mark::Done => GREEN,
        Mark::Waiting | Mark::WaitsOn => BLUE,
        Mark::NotStarted | Mark::WontDo => DIM,
    }
}

/// The marks and words of a card's line on the board, as spans.
pub fn holds_spans(parts: Vec<(String, Option<Mark>)>) -> Vec<Span<'static>> {
    parts
        .into_iter()
        .map(|(text, mark)| match mark {
            Some(Mark::NeedsYou) if text.starts_with(' ') => Span::styled(text, bold(YELLOW)),
            Some(m) => Span::styled(text, fg(mark_color(m))),
            None => Span::styled(text, fg(FG2)),
        })
        .collect()
}

/// `w2:t3` → `3`: how a tab is named for people.
fn tab_number(tab: &str) -> String {
    let last = tab.rsplit(':').next().unwrap_or(tab);
    last.trim_start_matches('t').to_string()
}

fn closed_on(c: &Card) -> Option<String> {
    let d = c.detail.as_ref()?.closed.as_deref()?;
    Some(d.get(5..10).unwrap_or(d).to_string())
}

#[cfg(test)]
pub(super) fn place_for_test(app: &App, c: &Card) -> (String, Color) {
    place(app, c, c, true)
}

/// Where a row's work runs, for its WHERE IT RUNS column, and the colour it says it in.
fn place(app: &App, c: &Card, holder: &Card, itself: bool) -> (String, Color) {
    if c.dismissed() {
        return ("won’t do".into(), DIM);
    }
    // A start or reopen on its way, before its workspace or tab shows: what it is, not what
    // broken work would say (intent 0445).
    if !c.is_open_workspace()
        && let Some(job) = c.detail.as_ref().and_then(|d| app.resolver.starting(&d.id))
    {
        let secs = job.asked.elapsed().as_secs();
        let what = if job.request["action"] == "reopen" {
            "reopening".to_string()
        } else if !itself
            && c.link
                .as_ref()
                .is_some_and(|(_, l)| *l == crate::model::Link::Part)
        {
            format!("starting in a tab of {}", holder.reference)
        } else {
            "starting up".to_string()
        };
        return (format!("◐ {what}… {secs} s"), CYAN);
    }
    if family::finished(c) {
        return match closed_on(c) {
            Some(d) => (format!("closed {d}"), DIM),
            None => ("closed".into(), DIM),
        };
    }
    if let Some(outside) = c.outside_herdr() {
        return (
            format!("running in {}, outside herdr", outside.place()),
            CYAN,
        );
    }
    if c.no_agent() {
        return ("no agent at it".into(), YELLOW);
    }
    if c.stage == Stage::Backlog {
        let waits = c.detail.as_ref().and_then(|d| {
            d.waits_on
                .iter()
                .find(|w| w.open != Some(false))
                .and_then(|w| w.reference.clone())
        });
        return match waits {
            Some(r) => (format!("waits on {r}"), BLUE),
            None => ("not started".into(), DIM),
        };
    }
    if !c.is_open_workspace() {
        return (
            if app.workspaces_stale(c) {
                "workspace not known"
            } else {
                "no workspace open"
            }
            .into(),
            DIM,
        );
    }
    if itself {
        return match &c.own_tab {
            Some(t) => (format!("tab {} · its own agent", tab_number(t)), FG2),
            None => (
                c.workspace_label
                    .clone()
                    .unwrap_or_else(|| "its workspace".into()),
                FG2,
            ),
        };
    }
    let same_ws = c.machine_index == holder.machine_index
        && c.session == holder.session
        && c.workspace_id == holder.workspace_id;
    let session = c.session.clone().unwrap_or_default();
    if same_ws {
        match &c.tab_id {
            Some(t) => (format!("tab {}", tab_number(t)), FG2),
            None => ("its workspace".into(), FG2),
        }
    } else if c.machine_index != holder.machine_index {
        (format!("{} › {session}", c.machine), CYAN)
    } else {
        let _ = app;
        (
            format!(
                "{session} › {}",
                c.workspace_label.clone().unwrap_or_default()
            ),
            FG2,
        )
    }
}

fn stage_word(c: &Card) -> &'static str {
    if c.dismissed() {
        "won’t do"
    } else if family::finished(c) {
        "done"
    } else {
        c.stage.id()
    }
}

/// What a row is to the card: "the card itself", "part", "follow-up · bg".
fn what(c: &Card, itself: bool) -> Vec<Span<'static>> {
    let mut out = vec![Span::styled(
        if itself {
            "the card itself".to_string()
        } else {
            c.link
                .as_ref()
                .map(|(_, l)| l.label().to_string())
                .unwrap_or_else(|| "part".into())
        },
        fg(DIM),
    )];
    if c.background && !itself {
        out.push(Span::styled(" · ", fg(DIM)));
        out.push(Span::styled("bg", fg(PURPLE)));
    }
    out
}

struct Cols {
    name: usize,
    what: usize,
    place: usize,
    stage: usize,
}

fn cols(w: usize) -> Cols {
    let stage = if w >= 60 { 8 } else { 0 };
    let (what, place) = if w >= 100 {
        (18, 30)
    } else if w >= 64 {
        (12, 18)
    } else {
        (0, 0)
    };
    Cols {
        name: w.saturating_sub(stage + what + place),
        what,
        place,
        stage,
    }
}

/// One table row: its name part, then the columns, each cut to its width.
#[allow(clippy::too_many_arguments)]
fn table(
    buf: &mut Buffer,
    rect: Rect,
    y: u16,
    c: &Cols,
    name: Vec<Span<'static>>,
    what: Vec<Span<'static>>,
    place: (String, Color),
    stage: (String, Color),
) {
    let mut x = rect.x;
    put(
        buf,
        x,
        y,
        c.name as u16,
        fit(name, c.name.saturating_sub(2)),
    );
    x += c.name as u16;
    if c.what > 0 {
        put(buf, x, y, c.what as u16, fit(what, c.what - 2));
        x += c.what as u16;
    }
    if c.place > 0 {
        put(
            buf,
            x,
            y,
            c.place as u16,
            vec![Span::styled(trunc(&place.0, c.place - 2), fg(place.1))],
        );
        x += c.place as u16;
    }
    if c.stage > 0 {
        let text = trunc(&stage.0, c.stage);
        let pad = c.stage.saturating_sub(width(&text)) as u16;
        put(
            buf,
            x + pad,
            y,
            c.stage as u16,
            vec![Span::styled(text, fg(stage.1))],
        );
    }
}

/// How the header names what a card holds: "4 inside: 1 needs you, 1 under way, 2 not started".
fn inside_words(app: &App, i: usize) -> String {
    let pieces = app.pieces(i);
    if pieces.is_empty() {
        return "nothing inside yet".into();
    }
    let mut counts: Vec<(Group, usize)> = vec![];
    for g in Group::ALL {
        let n = pieces
            .iter()
            .filter(|&&p| family::group(&app.board.cards[p]) == g)
            .count();
        if n > 0 {
            counts.push((g, n));
        }
    }
    if counts.len() == 1 && counts[0].0 == Group::Done {
        return format!("{} inside, all done", pieces.len());
    }
    let parts: Vec<String> = counts.iter().map(|(g, n)| g.words(*n)).collect();
    format!("{} inside: {}", pieces.len(), parts.join(", "))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, key: &str, focus: &str, opened: &[Group]) {
    let Some(i) = app.card_index(key) else { return };
    let card = &app.board.cards[i];
    let inner = pad(area, 1, 0);
    let rows_area = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let now = app.now();
    let done = family::finished(card);
    {
        let buf = f.buffer_mut();
        let mut left = vec![
            Span::styled("kerdr ", bold(CYAN)),
            Span::styled("› ", fg(DIM)),
            Span::styled(format!("{} {}", card.reference, card.title), bold(FG)),
            Span::raw("   "),
        ];
        if done {
            left.push(Span::styled("done · ", fg(GREEN)));
        }
        left.push(Span::styled(inside_words(app, i), fg(DIM)));
        put_lr(
            buf,
            rows_area[0],
            rows_area[0].y,
            left,
            vec![Span::styled(
                format!("{} · {}", app.this_machine(), clock(now)),
                fg(DIM),
            )],
        );
        let holds = app.holds(i);
        let sub = if done {
            let when = closed_on(card)
                .map(|d| format!(" (closed {d})"))
                .unwrap_or_default();
            if holds.open > 0 {
                format!(
                    "Its own work is done{when}. It stays at the top of Maintain until what's inside is done or set aside."
                )
            } else {
                format!("Its own work is done{when}.")
            }
        } else {
            let (p, _) = place(app, card, card, true);
            format!(
                "Its own agent: {} · {} · {p}, on {}",
                card.why,
                stage_word(card),
                card.machine
            )
        };
        put(
            buf,
            rows_area[1].x,
            rows_area[1].y,
            rows_area[1].width,
            vec![Span::styled(
                trunc(&sub, rows_area[1].width as usize),
                fg(DIM),
            )],
        );
    }

    let main = rows_area[2];
    let side = main.width >= 118;
    let (list, detail) = if side {
        let dw = (main.width * 2 / 5).clamp(40, 66);
        (
            Rect {
                width: main.width.saturating_sub(dw + 2),
                ..main
            },
            Rect {
                x: main.x + main.width.saturating_sub(dw),
                width: dw,
                ..main
            },
        )
    } else {
        let lh = (main.height * 11 / 20).max(4);
        (
            Rect { height: lh, ..main },
            Rect {
                y: main.y + lh + 1,
                height: main.height.saturating_sub(lh + 1),
                ..main
            },
        )
    };
    let rows = app.card_rows(i, opened);
    let c = cols(list.width as usize);
    {
        let buf = f.buffer_mut();
        let head = Cols {
            ..cols(list.width as usize)
        };
        table(
            buf,
            list,
            list.y,
            &head,
            vec![Span::styled("THE CARD AND WHAT'S INSIDE", bold(FG2))],
            vec![Span::styled("WHAT IT IS", bold(FG2))],
            ("WHERE IT RUNS".into(), FG2),
            ("STAGE".into(), FG2),
        );
        // Bold column titles for the two plain-text columns too.
        for x in list.x..list.right() {
            if let Some(cell) = buf.cell_mut((x, list.y))
                && cell.fg == FG2
            {
                cell.modifier = Modifier::BOLD;
            }
        }
    }
    let body = Rect {
        y: list.y + 1,
        height: list.height.saturating_sub(1),
        ..list
    };
    // None when the selected piece has left the card: nothing is highlighted.
    let found = rows
        .iter()
        .position(|r| r.focus(app).as_deref() == Some(focus));
    let sel_row = found.unwrap_or(usize::MAX);
    let h = body.height as usize;
    let mut start = 0;
    if rows.len() > h {
        // Room for the "above" and "below" lines.
        let room = h.saturating_sub(2).max(1);
        if found.is_some() && sel_row >= room {
            start = (sel_row + 1 - room).min(rows.len().saturating_sub(room));
        }
    }
    let hidden = |r: &[Row]| r.iter().filter(|r| matches!(r, Row::Piece { .. })).count();
    let mut y = body.y;
    let buf = f.buffer_mut();
    if start > 0 {
        put(
            buf,
            body.x,
            y,
            body.width,
            vec![Span::styled(
                format!("↑ {} above", hidden(&rows[..start]).max(1)),
                fg(DIM),
            )],
        );
        y += 1;
    }
    let bottom = body.y + body.height;
    for (n, row) in rows.iter().enumerate().skip(start) {
        let last_line = y + 1 == bottom;
        if y >= bottom || (last_line && n + 1 < rows.len()) {
            put(
                buf,
                body.x,
                y.min(bottom.saturating_sub(1)),
                body.width,
                vec![Span::styled(
                    format!("↓ {} below", hidden(&rows[n..]).max(1)),
                    fg(DIM),
                )],
            );
            break;
        }
        let selected = n == sel_row;
        let line = Rect {
            x: body.x,
            y,
            width: body.width,
            height: 1,
        };
        if selected && !matches!(row, Row::Blank) {
            buf.set_style(line, Style::default().bg(SEL));
        }
        let text = |s: Style| {
            if selected {
                s.add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        match row {
            Row::Blank => {}
            Row::Itself(k) => {
                let me = &app.board.cards[*k];
                let m = family::mark(me);
                table(
                    buf,
                    line,
                    y,
                    &c,
                    vec![
                        Span::styled(format!("{} ", m.glyph()), fg(mark_color(m))),
                        Span::styled(format!("{:<5} ", me.reference), text(bold(FG))),
                        Span::styled(me.title.clone(), text(fg(FG))),
                    ],
                    what(me, true),
                    place(app, me, me, true),
                    (stage_word(me).into(), DIM),
                );
            }
            Row::Heading {
                group,
                members,
                folded,
            } => {
                let mut spans = vec![
                    Span::styled(group.title(), bold(FG2)),
                    Span::styled(format!("  {}", members.len()), bold(FG)),
                ];
                if *folded {
                    spans.push(Span::styled(" ▸   ", fg(CYAN)));
                    let first = &app.board.cards[members[0]];
                    let mut gist = format!("{} {}", first.reference, first.title);
                    if members.len() > 1 {
                        gist.push_str(&format!(", and {} more", members.len() - 1));
                    }
                    spans.push(Span::styled(gist, fg(DIM)));
                    if selected {
                        spans.push(Span::styled(" · ⏎ opens the list", fg(DIM)));
                    }
                }
                put(buf, line.x, y, line.width, fit(spans, line.width as usize));
            }
            Row::Piece { index, depth } => {
                let p = &app.board.cards[*index];
                let m = family::mark(p);
                let mut name = vec![
                    Span::raw("  ".repeat(*depth)),
                    Span::styled(format!("{} ", m.glyph()), fg(mark_color(m))),
                    Span::styled(format!("{:<5} ", p.reference), text(bold(FG))),
                    Span::styled(
                        p.title.clone(),
                        text(fg(if family::finished(p) { FG2 } else { FG })),
                    ),
                ];
                if app.board.down.get(*index).is_some_and(|d| !d.is_empty()) {
                    name.insert(3, Span::styled("▸", fg(CYAN)));
                }
                table(
                    buf,
                    line,
                    y,
                    &c,
                    name,
                    what(p, false),
                    place(app, p, card, false),
                    (stage_word(p).into(), DIM),
                );
            }
        }
        y += 1;
    }

    details(f, detail, app, card, &rows, focus, side);
    footer(f, rows_area[4], app, card, focus, found.is_some());
    let _ = rows_area[3];
}

fn details(
    f: &mut Frame,
    rect: Rect,
    app: &App,
    holder: &Card,
    rows: &[Row],
    focus: &str,
    side: bool,
) {
    if rect.height < 3 || rect.width < 20 {
        return;
    }
    if side {
        dialog(f, rect, CYAN);
    } else {
        put(
            f.buffer_mut(),
            rect.x,
            rect.y.saturating_sub(1),
            rect.width,
            vec![Span::styled("─".repeat(rect.width as usize), fg(BORDER))],
        );
    }
    let d = if side { pad(rect, 2, 1) } else { rect };
    let w = d.width as usize;
    let now = app.now();
    let mut out: Vec<Line> = Vec::new();
    if !rows.iter().any(|r| r.focus(app).as_deref() == Some(focus)) {
        out.extend(wrapped(
            "The row you were on has left this card: it closed out of its feed, or moved to another. Nothing is selected until j or k chooses a row.",
            w,
            fg(DIM),
            None,
        ));
        f.render_widget(Paragraph::new(out), d);
        return;
    }
    if let Some(id) = focus.strip_prefix("group:") {
        if let Some(Row::Heading { group, members, .. }) = rows
            .iter()
            .find(|r| matches!(r, Row::Heading { group, .. } if group.id() == id))
        {
            out.push(Line::from(vec![
                Span::styled(group.title(), bold(FG2)),
                Span::styled(format!("  {}", members.len()), bold(FG)),
            ]));
            for &k in members.iter().take(40) {
                let p = &app.board.cards[k];
                let m = family::mark(p);
                out.push(Line::from(fit(
                    vec![
                        Span::styled(format!("{} ", m.glyph()), fg(mark_color(m))),
                        Span::styled(format!("{} ", p.reference), bold(FG)),
                        Span::styled(p.title.clone(), fg(FG2)),
                    ],
                    w,
                )));
            }
            out.push(Line::raw(""));
            out.push(Line::styled("⏎ opens the list here.", fg(DIM)));
        }
        f.render_widget(Paragraph::new(out), d);
        return;
    }
    let c = if focus.is_empty() {
        holder
    } else {
        match app.card(focus) {
            Some(c) => c,
            None => return,
        }
    };
    let itself = focus.is_empty();
    let needs = family::needs(c);
    let (badge, bcolor) = if c.dismissed() {
        ("× won’t do".to_string(), DIM)
    } else if family::finished(c) {
        (
            closed_on(c)
                .map(|d| format!("✓ closed {d}"))
                .unwrap_or_else(|| "✓ done".into()),
            GREEN,
        )
    } else if c.no_agent() {
        ("○ no agent at it".to_string(), YELLOW)
    } else {
        let mut b = format!("● {}", c.attention.label());
        if let Some(t) = c.updated {
            b.push_str(&format!(" · {}", day(t, now)));
        }
        (
            b,
            if needs {
                ORANGE
            } else {
                attention_color(c.attention)
            },
        )
    };
    let mut head = vec![Span::styled(c.reference.clone(), bold(FG))];
    if c.background {
        head.push(Span::styled(" bg", fg(PURPLE)));
    }
    let hw: usize = head.iter().map(|s| width(&s.content)).sum();
    head.push(Span::raw(" ".repeat(w.saturating_sub(hw + width(&badge)))));
    head.push(Span::styled(badge, bold(bcolor)));
    out.push(Line::from(head));
    out.extend(wrapped(&c.title, w, bold(FG), None));
    let mut facts = vec![];
    if itself {
        facts.push("the card itself".to_string());
    } else if let Some((r, l)) = &c.link {
        facts.push(format!("{} of {r}", l.label()));
    }
    let (p, _) = place(app, c, holder, itself);
    // A change on its way that its place doesn't already say (intent 0445).
    if !p.starts_with('◐')
        && let Some(line) = app.starting_line(c)
    {
        facts.push(line);
    }
    facts.push(p);
    if c.machine_index != holder.machine_index || itself {
        facts.push(format!("on {}", c.machine));
    }
    // On the owner split, the owner by its name on the list, and its team (intent 0595).
    match app.owner_info(&c.owner).filter(|_| app.owner_split()) {
        Some(o) => {
            facts.push(format!("owner: {}", o.label()));
            if !o.teams.is_empty() {
                facts.push(format!(
                    "team: {}",
                    app.team_name(&c.owner, c.team.as_deref())
                ));
            }
        }
        None => facts.push(format!("owner: {}", c.owner_label())),
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
    out.extend(wrapped(&facts.join(" · "), w, fg(DIM), None));
    let detail = c.detail.as_ref();
    if needs {
        out.push(Line::raw(""));
        if c.no_agent() {
            out.push(Line::from(vec![
                Span::styled("▌ ", fg(YELLOW)),
                Span::styled("Nobody is at it", bold(YELLOW)),
            ]));
            out.extend(wrapped(
                "⏎ reopens it, resuming its conversation. C marks it done; x sets it aside.",
                w,
                fg(FG),
                Some(("▌ ", YELLOW)),
            ));
        } else {
            out.push(Line::from(vec![
                Span::styled("▌ ", fg(ORANGE)),
                Span::styled("What it needs from you", bold(ORANGE)),
            ]));
            let text = detail
                .and_then(|d| d.needs.clone())
                .unwrap_or_else(|| c.why.clone());
            out.extend(wrapped(&text, w, fg(FG), Some(("▌ ", ORANGE))));
            let how = if c.is_open_workspace() {
                if c.tab_id.is_some() {
                    "⏎ goes to its tab.".to_string()
                } else {
                    "⏎ goes to its agent.".to_string()
                }
            } else {
                crate::ui::where_to_answer(app, c)
            };
            out.push(Line::from(vec![
                Span::styled("▌ ", fg(ORANGE)),
                Span::styled(how, fg(DIM)),
            ]));
        }
    }
    if let Some(d) = detail {
        if let Some(words) = &d.words {
            out.push(Line::raw(""));
            out.push(section(if itself {
                "HOW IT STARTED"
            } else {
                "WHY IT WAS MADE"
            }));
            for (n, l) in wrap(words, w.saturating_sub(2).max(10), 4)
                .into_iter()
                .enumerate()
            {
                out.push(Line::from(vec![
                    Span::styled(if n == 0 { "“ " } else { "  " }, fg(DIM)),
                    Span::styled(l, fg(FG2)),
                ]));
            }
        }
        if !d.criteria.is_empty() {
            out.push(Line::raw(""));
            let met = d.criteria.iter().filter(|c| c.met == Some(true)).count();
            // Only what an agent ticked counts as met: closing a card claims nothing.
            let count = if met == 0 {
                format!("   {}, not ticked off yet", d.criteria.len())
            } else {
                format!("   {met} of {} ticked off", d.criteria.len())
            };
            out.push(Line::from(vec![
                Span::styled("DONE WHEN", bold(FG2)),
                Span::styled(count, fg(DIM)),
            ]));
            for cr in &d.criteria {
                let (g, col) = if cr.met == Some(true) {
                    ("✓ ", GREEN)
                } else {
                    ("· ", DIM)
                };
                out.push(Line::from(vec![
                    Span::styled(g, fg(col)),
                    Span::styled(trunc(&cr.text, w.saturating_sub(2)), fg(FG2)),
                ]));
            }
        }
        if let Some(msg) = &d.message {
            out.push(Line::raw(""));
            let mut h = vec![Span::styled("ITS LAST UPDATE", bold(FG2))];
            if let Some(status) = &d.status {
                h.push(Span::styled(format!("   {status}"), fg(DIM)));
            }
            out.push(Line::from(h));
            out.extend(wrapped(msg, w, fg(FG), Some(("│ ", BORDER))));
        }
        if let Some(result) = &d.result {
            out.push(Line::raw(""));
            out.push(section(&format!("WHAT {} DELIVERED", c.reference)));
            out.extend(wrapped(result, w, fg(FG), Some(("✓ ", GREEN))));
        }
    }
    if let Some(agent) = app.account_agent(c).filter(|a| !a.account.is_empty()) {
        out.push(Line::raw(""));
        out.push(Line::from(vec![
            Span::styled("ACCOUNT   ", bold(FG2)),
            Span::styled(agent.account.clone(), fg(FG)),
        ]));
    }
    f.render_widget(Paragraph::new(out), d);
}

fn footer(f: &mut Frame, rect: Rect, app: &App, holder: &Card, focus: &str, here: bool) {
    let mut spans = Vec::new();
    let mut hints: Vec<(String, String)> = Vec::new();
    if !here {
        // The row it was on has left the card: nothing to act on until j or k chooses one.
        spans.push(Span::styled("nothing selected   ", fg(DIM)));
    } else if focus.starts_with("group:") {
        hints.push(("⏎".into(), "opens the list".into()));
    } else {
        let c = if focus.is_empty() {
            Some(holder)
        } else {
            app.sel.key.as_deref().and_then(|k| app.card(k))
        };
        if let Some(c) = c {
            let color = if family::needs(c) {
                YELLOW
            } else if family::finished(c) {
                GREEN
            } else {
                CYAN
            };
            spans.extend(card_badge(c, color));
            let enter = if c.is_open_workspace() {
                if focus.is_empty() {
                    Some("its own agent".to_string())
                } else if c.tab_id.is_some() {
                    Some("its tab".into())
                } else if c.machine_index != holder.machine_index {
                    Some(format!("its agent on {}", c.machine))
                } else {
                    Some(format!(
                        "its workspace in {}",
                        c.session.clone().unwrap_or_default()
                    ))
                }
            } else if c.outside_herdr().is_some() && !c.done() && app.actions_on() {
                Some("move it into herdr".into())
            } else if c.can_reopen_done() && app.actions_on() {
                Some("ask to reopen it".into())
            } else if c.reopenable() && app.actions_on() {
                Some("reopen it".into())
            } else {
                None
            };
            if let Some(e) = enter {
                hints.push(("⏎".into(), e));
            }
            hints.push(("space".into(), "peek".into()));
            if !focus.is_empty()
                && app
                    .card_index(&c.key)
                    .and_then(|k| app.board.down.get(k))
                    .is_some_and(|d| !d.is_empty())
            {
                hints.push(("o".into(), "its own screen".into()));
            }
            if crate::app::waiting_work(c) {
                hints.push(("d".into(), "start it".into()));
                hints.push(("e".into(), "edit its text".into()));
            }
            if c.closable() && app.actions_on() {
                hints.push(("C".into(), "done".into()));
                hints.push(("x".into(), "won’t do".into()));
            }
            if c.cancellable() {
                if let Some(m) = move_hint(app, c) {
                    hints.push(("s".into(), m));
                }
                if app.actions_on() {
                    hints.push(("X".into(), "cancel".into()));
                }
            }
        }
    }
    let refs: Vec<(&str, &str)> = hints
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    spans.extend(key_hints(&refs));
    spans.push(Span::styled("   │   ", fg(BORDER)));
    spans.extend(key_hints(&[
        ("j k", "move"),
        ("n N", "next that needs you"),
        ("esc", "back"),
    ]));
    put(
        f.buffer_mut(),
        rect.x,
        rect.y,
        rect.width,
        fit(spans, rect.width as usize),
    );
}
