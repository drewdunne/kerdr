//! Account layouts, using the same Tokyo Night palette and panels as the board.
use super::*;
use crate::accounts::{Account, AccountCopy, pause_glyph, provider_name};
use crate::app::account_controls::{AccountScreen, Row};

fn account_color(a: &Account) -> ratatui::style::Color {
    match a.status.as_str() {
        "out" | "sign_in" | "missing" => RED,
        "ready" => GREEN,
        _ => DIM,
    }
}

pub(super) fn usage(app: &App, mi: usize) -> Vec<Span<'static>> {
    let Some(feed) = app.account_feed(mi) else {
        return vec![];
    };
    let mut accounts: Vec<_> = feed.accounts.iter().collect();
    accounts.sort_by(|a, b| a.provider.cmp(&b.provider));
    let mut spans = Vec::new();
    let mut provider = "";
    for a in accounts {
        if provider != a.provider {
            spans.push(Span::styled(
                format!("  {} ", provider_name(&a.provider)),
                fg(FAINT),
            ));
            provider = &a.provider;
        } else {
            spans.push(Span::styled(" · ", fg(FAINT)));
        }
        spans.push(Span::styled(format!("{} ", a.code), fg(DIM)));
        spans.push(Span::styled(
            if matches!(a.status.as_str(), "sign_in" | "missing") {
                "✕ sign in".into()
            } else {
                a.numbers()
            },
            fg(if a.status == "ready" {
                FG2
            } else {
                account_color(a)
            }),
        ));
        if a.status == "out" {
            spans.push(Span::styled(format!(" {}", pause_glyph()), fg(RED)));
        }
    }
    spans
}

pub(super) fn new_intents(app: &App) -> Vec<Span<'static>> {
    if let Scope::Session(machine, session) = app.scope()
        && let Some(mi) = app.machine_index(machine)
        && let Some(owner) = app.account_owner(mi, session)
        && let Some(feed) = app.account_feed(mi)
        && let Some(first) = owner
            .models
            .iter()
            .find(|code| {
                feed.accounts
                    .iter()
                    .any(|a| &a.code == *code && a.status == "ready")
            })
            .or_else(|| owner.models.first())
    {
        let provider = app
            .account_feed(mi)
            .and_then(|f| f.accounts.iter().find(|a| &a.code == first))
            .map(|a| provider_name(&a.provider))
            .unwrap_or("");
        return vec![
            Span::styled(format!("new in {session}: "), fg(DIM)),
            Span::styled(format!("{provider} {first}  "), bold(FG)),
        ];
    }
    vec![]
}

fn line(text: impl Into<String>, color: ratatui::style::Color) -> Line<'static> {
    Line::styled(text.into(), fg(color))
}
fn heading(text: impl Into<String>) -> Line<'static> {
    Line::styled(text.into(), bold(FG))
}
fn selected(text: impl Into<String>, yes: bool) -> Line<'static> {
    Line::styled(
        format!("{} {}", if yes { "›" } else { " " }, text.into()),
        if yes { bold(FG).bg(SEL) } else { fg(FG2) },
    )
}

fn stored_color(c: &AccountCopy) -> ratatui::style::Color {
    if c.stored { GREEN } else { RED }
}
fn works_color(c: &AccountCopy) -> ratatui::style::Color {
    match c.works {
        Some(true) => GREEN,
        Some(false) => RED,
        None => DIM,
    }
}

/// Column widths for copy lines, shared by every account on the screen so they line up.
#[derive(Clone, Copy)]
struct CopyColumns {
    label: usize,
    works: usize,
}

impl CopyColumns {
    fn of(accounts: &[Account]) -> Self {
        let copies = || accounts.iter().flat_map(|a| a.shown_copies());
        CopyColumns {
            label: copies().map(|c| width(c.name())).max().unwrap_or(0).min(20) + 2,
            works: copies().map(|c| width(&c.works_label())).max().unwrap_or(0) + 2,
        }
    }
}

fn padded(text: &str, w: usize) -> String {
    let text = trunc(text, w.saturating_sub(2).max(1));
    format!("{text}{}", " ".repeat(w.saturating_sub(width(&text))))
}

/// One copy of an account's sign-in: its label, stored or not, works or not (and when), its note.
fn copy_line(c: &AccountCopy, yes: bool, cols: CopyColumns) -> Line<'static> {
    let style = |color| {
        if yes { bold(color).bg(SEL) } else { fg(color) }
    };
    // The label lines up with the usage numbers above it.
    let line = Line::from(vec![
        Span::styled(format!("{:<9}", if yes { "›" } else { "" }), style(FG)),
        Span::styled(
            padded(c.name(), cols.label),
            style(if yes { FG } else { FG2 }),
        ),
        Span::styled(padded(c.stored_label(), 14), style(stored_color(c))),
        Span::styled(padded(&c.works_label(), cols.works), style(works_color(c))),
        Span::styled(c.note.clone().unwrap_or_default(), style(DIM)),
    ]);
    // Like every selected row, the highlight fills the row's width.
    if yes {
        line.style(bold(FG).bg(SEL))
    } else {
        line
    }
}

fn copy_label(app: &App, machine: usize, code: &str, copy: &str) -> String {
    app.account_feed(machine)
        .and_then(|f| f.accounts.iter().find(|a| a.code == code))
        .and_then(|a| a.copy(copy))
        .map_or_else(|| copy.to_string(), |c| c.name().to_string())
}

fn modal(
    f: &mut Frame,
    area: Rect,
    title: &str,
    body: Vec<Line<'static>>,
    hints: &str,
    selected_line: usize,
    color: ratatui::style::Color,
) {
    let rect = centered(
        area,
        84,
        (body.len() as u16 + 6)
            .max(10)
            .min(area.height.saturating_sub(2)),
        None,
    );
    dialog(f, rect, color);
    let inner = pad(rect, 2, 1);
    if inner.height == 0 {
        return;
    }
    put(
        f.buffer_mut(),
        inner.x,
        inner.y,
        inner.width,
        vec![Span::styled(trunc(title, inner.width as usize), bold(FG))],
    );
    let content = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let offset = selected_line.saturating_sub(content.height.saturating_sub(1) as usize);
    f.render_widget(Paragraph::new(body).scroll((offset as u16, 0)), content);
    if inner.height > 1 {
        f.render_widget(
            Paragraph::new(hints).style(fg(DIM)),
            Rect {
                y: inner.bottom() - 1,
                height: 1,
                ..inner
            },
        );
    }
}

pub(super) fn draw(f: &mut Frame, area: Rect, app: &App, screen: &AccountScreen) {
    match screen {
        AccountScreen::PasteToken {
            machine,
            code,
            copy,
            token,
            ..
        } => modal(
            f,
            area,
            &match copy {
                Some(copy) => format!(
                    "Paste a token for {code}: {}",
                    copy_label(app, *machine, code, copy)
                ),
                None => format!("Paste a token for {code}"),
            },
            vec![
                line(
                    if copy.is_some() {
                        "Only this machine and this copy receive it."
                    } else {
                        "Only this machine receives it."
                    },
                    DIM,
                ),
                Line::raw(""),
                selected(
                    format!("{}▏", "•".repeat(token.0.chars().count().min(48))),
                    true,
                ),
            ],
            "⏎ send token    esc cancel",
            2,
            CYAN,
        ),
        AccountScreen::List {
            machine,
            rows,
            sel,
            model,
        } => list(f, area, app, *machine, rows, *sel, *model),
        AccountScreen::Move {
            target,
            choices,
            sel,
        } => {
            let mut lines = vec![
                line(
                    format!(
                        "{} › {} · on {}",
                        app.machines[target.machine].name(),
                        target.agent.session,
                        target.agent.account
                    ),
                    DIM,
                ),
                line(
                    if target.agent.waiting_for_usage() {
                        "Usage Limit Reached"
                    } else {
                        "Choose an account"
                    },
                    if target.agent.waiting_for_usage() {
                        RED
                    } else {
                        DIM
                    },
                ),
                Line::raw(""),
            ];
            let selected_line = lines.len() + *sel;
            for (i, code) in choices.iter().enumerate() {
                let text = if code == "list" {
                    "follow the list".into()
                } else if let Some(a) = app
                    .account_feed(target.machine)
                    .and_then(|f| f.accounts.iter().find(|a| &a.code == code))
                {
                    format!(
                        "{:<8} {:<16} {}  {}{}",
                        a.code,
                        a.name,
                        a.numbers(),
                        a.status_label(),
                        if a.code == target.agent.account {
                            " (now)"
                        } else {
                            ""
                        }
                    )
                } else {
                    format!("{code} · no longer available")
                };
                lines.push(selected(text, i == *sel));
            }
            lines.push(Line::raw(""));
            if let Some(problem) = target
                .agent
                .problem
                .as_deref()
                .filter(|p| !p.trim().is_empty())
            {
                lines.extend(wrapped(problem, 68, fg(RED), None));
                lines.push(Line::raw(""));
            }
            lines.extend(wrapped("The session restarts on the chosen account and stays there until moved again or it runs out. Another program starts a fresh agent from the intent's log.",68,fg(FG2),None));
            modal(
                f,
                area,
                &format!("Move {} to another account", target.reference),
                lines,
                "⏎ move    j k choose    esc cancel",
                selected_line,
                CYAN,
            );
        }
        AccountScreen::Models {
            machine,
            owner,
            models,
            sel,
            move_waiting,
        } => {
            let mut lines = vec![
                line(
                    format!(
                        "{} · {}",
                        owner.name,
                        if owner.own {
                            "own list"
                        } else {
                            "follows parent"
                        }
                    ),
                    DIM,
                ),
                Line::raw(""),
            ];
            for (i, code) in models.iter().enumerate() {
                let numbers = app
                    .account_feed(*machine)
                    .and_then(|f| f.accounts.iter().find(|a| &a.code == code))
                    .map(|a| a.numbers())
                    .unwrap_or_default();
                lines.push(selected(format!("{:<8} {numbers}", code), i == *sel));
            }
            lines.push(Line::raw(""));
            lines.push(line(
                format!(
                    "[{}] move waiting sessions on every machine",
                    if *move_waiting { "x" } else { " " }
                ),
                if *move_waiting { GREEN } else { FG2 },
            ));
            lines.push(line("J K reorder · a add · d drop · ⌫ follow parent", DIM));
            modal(
                f,
                area,
                &format!("Default accounts: {}", owner.name),
                lines,
                "⏎ save    space tick    j k choose    esc cancel",
                sel + 2,
                CYAN,
            );
        }
        AccountScreen::AddModel { choices, sel, .. } => {
            let mut lines: Vec<_> = choices
                .iter()
                .enumerate()
                .map(|(i, c)| selected(c, i == *sel))
                .collect();
            if lines.is_empty() {
                lines.push(line("Every account is already in this list.", DIM));
            }
            modal(
                f,
                area,
                "Add to ordered models",
                lines,
                "⏎ add    j k choose    esc cancel",
                *sel,
                CYAN,
            );
        }
        AccountScreen::Add {
            code,
            name,
            field,
            machines,
            chosen,
            ..
        } => {
            let mut lines = vec![
                selected(
                    format!("code      {code}{}", if *field == 0 { "▏" } else { "" }),
                    *field == 0,
                ),
                Line::raw(""),
                selected(
                    format!("name      {name}{}", if *field == 1 { "▏" } else { "" }),
                    *field == 1,
                ),
                Line::raw(""),
                line("machines", DIM),
            ];
            for (i, &mi) in machines.iter().enumerate() {
                lines.push(selected(
                    format!(
                        "[{}] {}{}",
                        if chosen[i] { "x" } else { " " },
                        app.machines[mi].name(),
                        if app.machines[mi].conn.is_live() {
                            ""
                        } else {
                            " · offline"
                        }
                    ),
                    *field == i + 2,
                ));
            }
            modal(
                f,
                area,
                "Add a Claude account",
                lines,
                "⏎ next: sign in    tab field    space tick    esc cancel",
                if *field < 2 { field * 2 } else { field + 3 },
                CYAN,
            );
        }
        AccountScreen::Rename { code, name, .. } => modal(
            f,
            area,
            &format!("Rename {code}"),
            vec![selected(format!("name   {name}▏"), true)],
            "⏎ save    esc cancel",
            0,
            CYAN,
        ),
        AccountScreen::Signin(s) => {
            let done = s.current >= s.machines.len();
            let mut lines = match &s.copy {
                // Esc only undoes a sign-in that hasn't finished; once done, the copy has changed.
                Some((_, label)) => wrapped(
                    &if done {
                        format!(
                            "The {label} copy is signed in. The other copies stayed as they were."
                        )
                    } else {
                        format!(
                            "Only the {label} copy changes. The other copies stay as they are, and esc before it finishes leaves this one as it was."
                        )
                    },
                    68,
                    fg(DIM),
                    None,
                ),
                None => vec![line("Each machine makes its own sign-in.", DIM)],
            };
            lines.push(Line::raw(""));
            let top = lines.len();
            for (i, &mi) in s.machines.iter().enumerate() {
                lines.push(line(
                    format!(
                        "{:<22} {}",
                        app.machines[mi].name(),
                        if i < s.current {
                            "✓ signed in"
                        } else if i == s.current {
                            if s.failed {
                                "✕ sign-in stopped"
                            } else {
                                "◐ authorize in your browser"
                            }
                        } else {
                            "waiting"
                        }
                    ),
                    if i < s.current {
                        GREEN
                    } else if i == s.current {
                        YELLOW
                    } else {
                        DIM
                    },
                ));
            }
            lines.push(Line::raw(""));
            lines.extend(wrapped(
                &s.message,
                68,
                fg(if s.failed { RED } else { FG2 }),
                None,
            ));
            if s.current < s.machines.len() {
                lines.push(Line::raw(""));
                lines.push(Line::from(key_hints(&[
                    ("c", "copy link"),
                    ("o", "open it again"),
                ])));
                if let Some(url) = &s.url {
                    lines.extend(wrapped(url, 68, fg(DIM), None));
                }
                if s.needs_code {
                    lines.push(Line::raw(""));
                    lines.push(selected(
                        format!(
                            "code from the page   {}{}",
                            "•".repeat(s.page_code.chars().count().min(30)),
                            if s.editing { "▏" } else { "" }
                        ),
                        s.editing,
                    ));
                }
            }
            let title = match (&s.copy, s.machines.first()) {
                (Some((_, label)), Some(&mi)) => {
                    format!("Sign {} in on {}: {label}", s.code, app.machines[mi].name())
                }
                _ => format!("Sign {} in on each machine", s.code),
            };
            modal(
                f,
                area,
                &title,
                lines,
                if done && s.copy.is_some() {
                    "Signed in.    esc accounts"
                } else if done {
                    "All selected machines signed in.    esc accounts"
                } else if s.failed {
                    "r try again    p paste token    esc later"
                } else {
                    "tab code / buttons    ⏎ send code    p token    esc stop"
                },
                s.current + top,
                if s.failed { RED } else { CYAN },
            );
        }
        AccountScreen::Confirm {
            message, yes, no, ..
        } => {
            modal(
                f,
                area,
                "Account change",
                wrapped(message, 68, fg(FG), None),
                &format!("y {yes}    n {no}    esc cancel"),
                0,
                YELLOW,
            );
        }
    }
}

fn table_edge(w: usize, top: bool) -> Line<'static> {
    line(
        format!(
            "{}{}{}",
            if top { "╭" } else { "╰" },
            "─".repeat(w.saturating_sub(2)),
            if top { "╮" } else { "╯" }
        ),
        BORDER,
    )
}
fn table_row(mut row: Line<'static>, w: usize) -> Line<'static> {
    let inner = w.saturating_sub(4);
    let spans = fit(row.spans, inner);
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    let mut out = vec![Span::styled("│ ", fg(BORDER))];
    out.extend(spans);
    out.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
    out.push(Span::styled(" │", fg(BORDER)));
    row.spans = out;
    row
}

fn account_line(a: &Account, yes: bool) -> Line<'static> {
    selected(
        format!(
            "{:<6} {:<14} {} · {}",
            a.code,
            a.name,
            provider_name(&a.provider),
            if a.kind == "login" {
                "saved sign-in"
            } else if a.kind == "token" {
                "token per machine"
            } else {
                &a.kind
            }
        ),
        yes,
    )
}
fn status_line(a: &Account) -> Line<'static> {
    line(
        format!("         {}  {}", a.numbers(), a.status_label()),
        account_color(a),
    )
}

fn list(f: &mut Frame, area: Rect, app: &App, mi: usize, rows: &[Row], sel: usize, model: usize) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(fg(FG).bg(BG)), area);
    let inner = pad(area, 2, 1);
    let name = app.machines.get(mi).map(|m| m.name()).unwrap_or_default();
    put_lr(
        f.buffer_mut(),
        inner,
        inner.y,
        vec![Span::styled(
            format!("kerdr › accounts   {name}"),
            bold(CYAN),
        )],
        usage(app, mi),
    );
    let Some(feed) = app.account_feed(mi) else {
        f.render_widget(Paragraph::new("This machine's feed has no account information yet.\n\nTab another machine · Esc board").style(fg(DIM)),Rect{y:inner.y+2,height:inner.height.saturating_sub(2),..inner});
        return;
    };
    let content = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let panels = if inner.width >= 110 {
        Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).split(content)
    } else {
        Layout::horizontal([Constraint::Percentage(100), Constraint::Length(0)]).split(content)
    };
    let tw = panels[0].width.saturating_sub(2) as usize;
    let mut lines = vec![heading("ACCOUNTS"), table_edge(tw, true)];
    let mut owner_section = false;
    let mut selected_line = 0;
    let cols = CopyColumns::of(&feed.accounts);
    for (i, row) in rows.iter().enumerate() {
        if matches!(row, Row::Owner(_)) && !owner_section {
            lines.extend([
                table_row(line("+ add an account", DIM), tw),
                table_edge(tw, false),
                Line::raw(""),
                heading("NEW INTENTS"),
                table_edge(tw, true),
                table_row(line("  tenant                  ordered models", DIM), tw),
            ]);
            owner_section = true;
        }
        if i == sel {
            selected_line = lines.len();
        }
        match row {
            Row::Copy(code, id) => {
                if let Some(a) = feed.accounts.iter().find(|a| &a.code == code)
                    && let Some(c) = a.copy(id)
                {
                    if a.shown_copies().first().map(|c| &c.id) == Some(id) {
                        lines.push(table_row(account_line(a, false), tw));
                        lines.push(table_row(status_line(a), tw));
                    }
                    if i == sel {
                        selected_line = lines.len();
                    }
                    lines.push(table_row(copy_line(c, i == sel, cols), tw));
                }
            }
            Row::Account(code) => {
                if let Some(a) = feed.accounts.iter().find(|a| &a.code == code) {
                    lines.push(table_row(account_line(a, i == sel), tw));
                    lines.push(table_row(status_line(a), tw));
                }
            }
            Row::Owner(id) => {
                if let Some(o) = feed.owners.iter().find(|o| &o.id == id) {
                    let prefix = if o.parent.is_some() { "  " } else { "" };
                    let models = if o.own || o.parent.is_none() {
                        o.models
                            .iter()
                            .enumerate()
                            .map(|(index, code)| {
                                if i == sel && index == model {
                                    format!("[{code}]")
                                } else {
                                    code.clone()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" → ")
                    } else {
                        format!(
                            "as {}",
                            o.parent
                                .as_ref()
                                .and_then(|p| feed.owners.iter().find(|x| &x.id == p))
                                .map(|p| p.name.as_str())
                                .unwrap_or("parent")
                        )
                    };
                    lines.push(table_row(
                        selected(
                            format!("{:<24} {}", format!("{prefix}{}", o.name), models),
                            i == sel,
                        ),
                        tw,
                    ));
                }
            }
        }
    }
    lines.extend([
        table_edge(tw, false),
        Line::raw(""),
        heading("MACHINES BELONG TO"),
    ]);
    for m in app.machines.iter().filter(|m| m.is_shown()) {
        if let Some(f) = &m.feed {
            let owner = f
                .machine_owner
                .as_ref()
                .and_then(|id| f.owners.iter().find(|o| &o.id == id))
                .map(|o| o.name.as_str())
                .or(f.machine_owner.as_deref())
                .unwrap_or("unknown");
            lines.push(line(format!("{}  {owner}", m.name()), DIM));
        }
    }
    lines.push(line(
        format!(
            "owners: {}",
            feed.owners_source.as_deref().unwrap_or("unknown")
        ),
        FAINT,
    ));
    for session in &feed.unowned_sessions {
        lines.push(line(format!("✕ unowned session: {session}"), RED));
    }
    let scroll = selected_line.saturating_sub(panels[0].height.saturating_sub(6) as usize);
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), panels[0]);
    if panels[1].width > 0 {
        panel(f, panels[1], CYAN, PANEL, BorderType::Rounded);
        let right = pad(panels[1], 2, 1);
        let w = right.width as usize;
        let mut detail = vec![];
        match rows.get(sel) {
            Some(Row::Owner(id)) => {
                if let Some(o) = feed.owners.iter().find(|o| &o.id == id) {
                    detail.push(heading(&o.name));
                    detail.extend(wrapped(
                        &format!("{} · sessions {}", o.kind, o.sessions.join(", ")),
                        w,
                        fg(DIM),
                        None,
                    ));
                    detail.push(Line::raw(""));
                    for (i, code) in o.models.iter().enumerate() {
                        detail.push(selected(format!("{}. {code}", i + 1), i == model));
                    }
                    detail.push(Line::raw(""));
                    detail.extend(wrapped("⏎ first · h l choose model · J K reorder · a add · d drop · ⌫ follow parent · S edit list",w,fg(DIM),None));
                }
            }
            Some(row @ (Row::Account(code) | Row::Copy(code, _))) => {
                if let Some(a) = feed.accounts.iter().find(|a| &a.code == code) {
                    detail.extend([
                        heading(format!("{} · {}", a.code, a.name)),
                        line(provider_name(&a.provider), DIM),
                        Line::raw(""),
                    ]);
                    let copy = match row {
                        Row::Copy(_, id) => a.copy(id),
                        _ => None,
                    };
                    if let Some(c) = copy {
                        detail.extend(wrapped(
                            &format!("{} on {name}", c.name()),
                            w,
                            bold(FG),
                            None,
                        ));
                        detail.push(line(c.stored_label(), stored_color(c)));
                        detail.push(line(c.works_label(), works_color(c)));
                        if let Some(note) = c.note.as_deref().filter(|n| !n.trim().is_empty()) {
                            detail.extend(wrapped(note, w, fg(DIM), None));
                        }
                        detail.push(Line::raw(""));
                        detail.extend(wrapped(
                            "Signing in this copy leaves the other copies as they are.",
                            w,
                            fg(FG2),
                            None,
                        ));
                        detail.push(Line::raw(""));
                    }
                    for m in app.machines.iter().filter(|m| m.is_shown()) {
                        if let Some(a) = m
                            .feed
                            .as_ref()
                            .and_then(|f| f.accounts.iter().find(|a| &a.code == code))
                        {
                            detail.extend(wrapped(
                                &format!(
                                    "{}: {}{}",
                                    m.name(),
                                    a.status_label(),
                                    if m.conn.is_live() { "" } else { " (cached)" }
                                ),
                                w,
                                fg(account_color(a)),
                                None,
                            ));
                        }
                    }
                    detail.push(Line::raw(""));
                    detail.extend(wrapped(
                        if copy.is_some() {
                            "⏎ sign in this copy · t test it · e rename · x remove · tab another machine"
                        } else {
                            "⏎ sign in on this machine · e rename · t test · x remove · tab another machine"
                        },
                        w,
                        fg(DIM),
                        None,
                    ));
                }
            }
            _ => {}
        }
        detail.extend([Line::raw(""), heading("WHEN ONE RUNS OUT")]);
        detail.extend(wrapped("It moves to the next in the list by itself. Same program: the same conversation. Another program: a fresh agent takes over from the intent's log. Nothing left: it waits until the first is back.",w,fg(FG2),None));
        f.render_widget(Paragraph::new(detail), right);
    }
    if inner.height > 1 {
        f.render_widget(Paragraph::new("j k move · h l model · ⏎ first / sign in · J K reorder · a d add/drop · + new · e t x · tab machine · esc board").style(fg(DIM)),Rect{y:inner.bottom()-1,height:1,..inner});
    }
}
