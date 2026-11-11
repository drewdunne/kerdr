//! Account layouts, using the same palette and panels as the board.
use super::*;
use crate::accounts::{Account, AccountCopy, Figures, pause_glyph};
use crate::app::account_controls::{AccountScreen, AddAccount, Row};

fn account_color(a: &Account) -> ratatui::style::Color {
    match a.status.as_str() {
        "out" | "sign_in" | "missing" => RED,
        "ready" => GREEN,
        _ => DIM,
    }
}

/// The current account for the top bar and the accounts screen's header, where it can't be missed:
/// `★ current KBZ_C`, and on a machine where it can't start new sessions, why (`out until Mon
/// 14:00 here`), in a warning colour, since new sessions there use their lists (intent 0455).
pub(super) fn current_tag(app: &App, mi: usize) -> Vec<Span<'static>> {
    let Some(c) = app.account_feed(mi).and_then(|f| f.current_account()) else {
        return vec![];
    };
    match c.held_back("here") {
        None => vec![Span::styled(format!("★ current {}", c.account), bold(CYAN))],
        Some(why) => vec![
            Span::styled(format!("★ current {}", c.account), bold(YELLOW)),
            Span::styled(format!(" · {why}"), fg(YELLOW)),
        ],
    }
}

/// The current account's code alone, `★ KBZ_C`, in the same colour: what's left when the bar is
/// too narrow for why.
pub(super) fn current_short(app: &App, mi: usize) -> Vec<Span<'static>> {
    let Some(c) = app.account_feed(mi).and_then(|f| f.current_account()) else {
        return vec![];
    };
    let color = if c.starts_sessions() { CYAN } else { YELLOW };
    vec![Span::styled(format!("★ {}", c.account), bold(color))]
}

/// What fits first of: the current account and every usage figure, the current account with
/// why, then its code alone. Usage gives way before the current account does.
pub(super) fn current_and_usage(app: &App, mi: usize, room: usize) -> Vec<Span<'static>> {
    let spans_w = |s: &[Span<'static>]| s.iter().map(|s| width(&s.content)).sum::<usize>();
    let tag = current_tag(app, mi);
    let mut full = tag.clone();
    let usage = usage(app, mi);
    if !full.is_empty() && !usage.is_empty() {
        full.push(Span::raw(" "));
    }
    full.extend(usage);
    if spans_w(&full) <= room || tag.is_empty() {
        return full;
    }
    if spans_w(&tag) <= room {
        return tag;
    }
    current_short(app, mi)
}

/// The accounts screen's block about the current account: which it is and since when, what each
/// machine makes of it, and the keys that set and clear it.
fn current_block(app: &App, mi: usize, w: usize) -> Vec<Line<'static>> {
    let mut lines = vec![heading("CURRENT ACCOUNT")];
    let Some(c) = app.account_feed(mi).and_then(|f| f.current_account()) else {
        lines.extend(wrapped(
            "None: each owner's list chooses. c on an account sets one for every new session.",
            w,
            fg(DIM),
            None,
        ));
        return lines;
    };
    let since = c
        .since
        .as_deref()
        .map(|t| format!(", set {}", crate::accounts::moment(t, app.now())))
        .unwrap_or_default();
    let name = if c.name.trim().is_empty() {
        String::new()
    } else {
        format!(" ({})", c.name)
    };
    lines.push(Line::from(vec![
        Span::styled(format!("★ {}", c.account), bold(CYAN)),
        Span::styled(
            format!("{name}{since}: new sessions start on it, ahead of their lists."),
            fg(FG),
        ),
    ]));
    // Each machine's own feed says how the current account stands there.
    for m in app.machines.iter().filter(|m| m.is_shown()) {
        let Some(feed) = &m.feed else { continue };
        let cached = if m.conn.is_live() { "" } else { " (cached)" };
        let (text, color) = match feed.current_account() {
            Some(here) if here.account != c.account => (
                format!(
                    "{}: still on {} until it catches up{cached}",
                    m.name(),
                    here.account
                ),
                DIM,
            ),
            Some(here) => match here.held_back("there") {
                None => (
                    format!("{}: new sessions start on it{cached}", m.name()),
                    GREEN,
                ),
                Some(why) => (
                    format!(
                        "{}: {why}, so new sessions use their lists{cached}",
                        m.name()
                    ),
                    YELLOW,
                ),
            },
            None if feed.accounts.is_empty() || !feed.knows_current() => continue,
            None => (
                format!(
                    "{}: not set there yet; it follows within minutes{cached}",
                    m.name()
                ),
                DIM,
            ),
        };
        lines.extend(wrapped(&format!("  {text}"), w, fg(color), None));
    }
    lines.push(line("c on an account makes it current · C clears it", DIM));
    lines
}

/// The top bar's accounts, grouped by provider in the feed's order: `Claude KBZ_C 86│100 · ARK_C
/// 12│30   Codex KBZ_X 7`.
pub(super) fn usage(app: &App, mi: usize) -> Vec<Span<'static>> {
    let Some(feed) = app.account_feed(mi) else {
        return vec![];
    };
    let providers = app.providers(mi);
    let order = providers.order();
    let rank = |id: &str| order.iter().position(|p| p == id).unwrap_or(usize::MAX);
    let mut accounts: Vec<_> = feed.accounts.iter().enumerate().collect();
    accounts.sort_by_key(|(i, a)| (rank(&a.provider), *i));
    let mut spans = Vec::new();
    let mut provider = None;
    for (_, a) in accounts {
        if provider != Some(&a.provider) {
            spans.push(Span::styled(
                format!("  {} ", providers.label(&a.provider)),
                fg(FAINT),
            ));
            provider = Some(&a.provider);
        } else {
            spans.push(Span::styled(" · ", fg(FAINT)));
        }
        spans.push(Span::styled(format!("{} ", a.code), fg(DIM)));
        spans.push(Span::styled(
            if matches!(a.status.as_str(), "sign_in" | "missing") {
                "✕ sign in".into()
            } else {
                app.account_figures(a, Figures::Short)
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
    // On the owner split, the chosen owner's: its main session, on the machine its work runs on
    // (intent 0595).
    let owned = match app.scope() {
        Scope::Owner(_) | Scope::Team(..) => app.owner_machine().and_then(|mi| {
            let o = app.owner_info(app.scope().owner()?)?;
            Some((mi, o.code.clone()?))
        }),
        _ => None,
    };
    let viewed = match (app.scope(), &owned) {
        (Scope::Session(machine, session), _) => {
            app.machine_index(machine).map(|mi| (mi, session.clone()))
        }
        (_, Some((mi, code))) => Some((*mi, code.clone())),
        _ => None,
    };
    let Some((mi, session)) = viewed else {
        return vec![];
    };
    let session = session.as_str();
    // A current account that's ready there goes ahead of the session's list (intent 0455).
    if let Some(c) = app
        .account_feed(mi)
        .and_then(|f| f.current_account())
        .filter(|c| c.starts_sessions())
    {
        let provider = app
            .account(mi, &c.account)
            .map(|a| app.providers(mi).label(&a.provider))
            .unwrap_or_default();
        return vec![
            Span::styled(format!("new in {session}: "), fg(DIM)),
            Span::styled(format!("{provider} {} ", c.account), bold(FG)),
            Span::styled("★ current  ", bold(CYAN)),
        ];
    }
    let owner = match app.scope() {
        Scope::Session(..) => app.account_owner(mi, session),
        scope => scope
            .owner()
            .and_then(|id| app.account_feed(mi)?.owners.iter().find(|o| o.id == id)),
    };
    if let Some(owner) = owner
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
            .account(mi, first)
            .map(|a| app.providers(mi).label(&a.provider))
            .unwrap_or_default();
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
/// A row that can't be chosen, and says why in its text: dim until the cursor is on it.
fn held_back(text: impl Into<String>, yes: bool) -> Line<'static> {
    if yes {
        selected(text, yes)
    } else {
        Line::styled(format!("  {}", text.into()), fg(DIM))
    }
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
    app.account(machine, code)
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
                        "{} › {} · {}",
                        app.machines[target.machine].name(),
                        target.agent.session,
                        if target.agent.account.is_empty() {
                            "no account recorded yet".to_string()
                        } else {
                            format!("on {}", target.agent.account)
                        }
                    ),
                    DIM,
                ),
                line(
                    if target.agent.waiting_for_usage() {
                        // When it comes back, where that's known (intent 0591).
                        match app
                            .account(target.machine, &target.agent.account)
                            .filter(|a| a.status == "out")
                            .and_then(|a| a.until.as_deref())
                        {
                            Some(until) => format!(
                                "Usage Limit Reached: {} is back at {}",
                                target.agent.account,
                                crate::accounts::reset_time(until)
                            ),
                            None => "Usage Limit Reached".to_string(),
                        }
                    } else {
                        "Choose an account".to_string()
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
                } else if let Some(a) = app.account(target.machine, code) {
                    format!(
                        "{:<7} {:<8} {:<15} {}  {}{}",
                        a.code,
                        app.providers(target.machine).label(&a.provider),
                        a.name,
                        app.account_numbers(a),
                        a.status_label(),
                        if a.code == target.agent.account {
                            " (on it now)"
                        } else {
                            ""
                        }
                    )
                } else {
                    format!("{code} · no longer available")
                };
                // An account this agent may not move to: dim, with the reason (intent 0591).
                lines.push(match app.held_to(target.machine, &target.agent, code) {
                    Some(stays) => held_back(format!("{text} · has to stay on {stays}"), i == *sel),
                    None => selected(text, i == *sel),
                });
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
        AccountScreen::Start {
            reference,
            start,
            choices,
            sel,
            ..
        } => {
            let resumes = start.how == crate::app::run_state::StartHow::Reopen;
            let fresh = start.how == crate::app::run_state::StartHow::Fresh;
            let state = if fresh {
                "no agent running and no intent; a fresh agent starts in a new tab"
            } else if start.outside {
                "running in a terminal outside herdr; it moves into herdr"
            } else if resumes {
                "no agent running; it resumes its conversation"
            } else {
                "not started yet"
            };
            let mut lines = vec![
                line(
                    format!("{} · {state}", app.machines[start.machine].name()),
                    DIM,
                ),
                line("Choose an account", DIM),
                Line::raw(""),
            ];
            let selected_line = lines.len() + *sel;
            for (i, code) in choices.iter().enumerate() {
                let text = if code == "list" {
                    "follow its owner's list".into()
                } else if let Some(a) = app.account(start.machine, code) {
                    format!(
                        "{:<7} {:<8} {:<15} {}  {}",
                        a.code,
                        app.providers(start.machine).label(&a.provider),
                        a.name,
                        app.account_numbers(a),
                        a.status_label(),
                    )
                } else {
                    format!("{code} · no longer available")
                };
                lines.push(selected(text, i == *sel));
            }
            lines.push(Line::raw(""));
            // What happens first, before the start itself (intent 0591).
            let mut first = String::new();
            if start.restore {
                first.push_str("It comes back from won't do first. ");
            }
            if start.update {
                first.push_str(&format!(
                    "{}'s ark-sdlc is brought up to date first, since its backend can't take an account yet. ",
                    app.machines[start.machine].name()
                ));
            }
            let then = if fresh {
                "A fresh agent of the chosen account's program starts in a new tab of this workspace, and stays on that account until moved or it runs out. There's no conversation to resume."
            } else if start.outside {
                "Its conversation stops in the terminal once its turn ends and carries on in herdr on the chosen account, and stays there until moved or it runs out."
            } else if resumes {
                "It starts again on the chosen account, resuming its conversation, and stays there until moved or it runs out."
            } else {
                "Its agent starts on the chosen account, and stays there until moved or it runs out."
            };
            lines.extend(wrapped(&format!("{first}{then}"), 68, fg(FG2), None));
            let title = if fresh {
                format!("Start an agent in {} on an account", reference)
            } else if start.restore {
                format!("Bring {} back on an account", reference)
            } else if start.outside {
                format!("Move {} into herdr on an account", reference)
            } else {
                format!("Start {} on an account", reference)
            };
            modal(
                f,
                area,
                &title,
                lines,
                "⏎ start    j k choose    esc cancel",
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
            let mut lines = wrapped(
                &format!(
                    "{} · {}. New work starts on the first that's ready; when one runs out, the next.",
                    owner.name,
                    if owner.own {
                        "own list"
                    } else {
                        "follows parent"
                    }
                ),
                68,
                fg(DIM),
                None,
            );
            lines.push(Line::raw(""));
            let top = lines.len();
            let providers = app.providers(*machine);
            let find = |code: &str| app.account(*machine, code);
            // Each line names its provider, so the list reads as a provider order.
            for (i, code) in models.iter().enumerate() {
                let text = match find(code) {
                    Some(a) => format!(
                        "{}. {:<7} {:<8} {:<16} {}",
                        i + 1,
                        code,
                        providers.label(&a.provider),
                        a.name,
                        app.account_numbers(a)
                    ),
                    None => format!("{}. {code:<7} no longer available", i + 1),
                };
                lines.push(selected(text, i == *sel));
            }
            lines.push(Line::raw(""));
            // What new work runs with the list as it stands: the first ready account's program.
            if let Some(a) = models
                .iter()
                .filter_map(|c| find(c))
                .find(|a| a.ready())
                .or_else(|| models.first().and_then(|c| find(c)))
            {
                // A current account goes ahead of the list wherever it's ready (intent 0455).
                let current = app
                    .account_feed(*machine)
                    .and_then(|f| f.current_account())
                    .map(|c| {
                        if c.starts_sessions() {
                            format!(" While {} is the current account, new work starts on it instead; this list chooses once it's cleared.", c.account)
                        } else {
                            format!(" {} is the current account, but it can't start sessions here now, so this list chooses.", c.account)
                        }
                    })
                    .unwrap_or_default();
                lines.extend(wrapped(
                    &format!(
                        "New {} work starts on {} and runs {}.{current} Agents already running stay on what they run.",
                        owner.name,
                        a.code,
                        providers.program_label(&a.runs)
                    ),
                    68,
                    fg(FG2),
                    None,
                ));
                lines.push(Line::raw(""));
            }
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
                &format!("{}'s ordered models", owner.name),
                lines,
                "⏎ save    space tick    j k choose    esc cancel",
                sel + top,
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
        AccountScreen::Provider {
            machine,
            choices,
            sel,
        } => {
            let providers = app.providers(*machine);
            let mut lines: Vec<_> = choices
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let runs: Vec<String> = p
                        .programs
                        .iter()
                        .map(|id| providers.program_label(id))
                        .collect();
                    selected(
                        format!(
                            "{:<10} {}",
                            p.name(),
                            if runs.is_empty() {
                                String::new()
                            } else {
                                format!("runs {}", runs.join(" or "))
                            }
                        ),
                        i == *sel,
                    )
                })
                .collect();
            lines.push(Line::raw(""));
            if let Some(p) = choices.get(*sel)
                && let Some(program) = p.programs.first()
            {
                lines.extend(wrapped(
                    &format!(
                        "The provider decides the program: an agent on a {} account runs {}.",
                        p.name(),
                        providers.program_label(program)
                    ),
                    68,
                    fg(DIM),
                    None,
                ));
            }
            modal(
                f,
                area,
                "Add an account: which provider?",
                lines,
                "j k choose    ⏎ next    esc cancel",
                *sel,
                CYAN,
            );
        }
        AccountScreen::Add(add) => {
            let field = add.field;
            let text = |value: &str, suggested: &str, label: &str, at: usize| -> Line<'static> {
                let cursor = if field == at { "▏" } else { "" };
                if value.is_empty() && !suggested.is_empty() {
                    // The suggestion stands until something is typed over it.
                    let yes = field == at;
                    Line::from(vec![
                        Span::styled(
                            format!("{} {label:<9} ", if yes { "›" } else { " " }),
                            if yes { bold(FG).bg(SEL) } else { fg(FG2) },
                        ),
                        Span::styled(
                            format!("{suggested}{cursor}"),
                            if yes { fg(DIM).bg(SEL) } else { fg(DIM) },
                        ),
                    ])
                } else {
                    selected(format!("{label:<9} {value}{cursor}"), field == at)
                }
            };
            let mut lines = vec![
                text(&add.code, &add.suggested.0, "code", 0),
                text(&add.name, &add.suggested.1, "name", 1),
            ];
            // The provider's own questions, each in its words.
            for (i, (f, value)) in add.provider.fields.iter().zip(&add.extra).enumerate() {
                let label = if f.label.is_empty() { &f.id } else { &f.label };
                let hint = if f.required { "" } else { "optional" };
                lines.push(text(value, hint, label, AddAccount::FIELDS_AT + i));
            }
            let signs_in = add.provider.signin.is_some() || !app.offered(add.machine).listed();
            if !add.provider.kinds.is_empty() {
                lines.push(Line::raw(""));
                lines.push(line(if signs_in { "sign-in" } else { "kind" }, DIM));
            }
            let kinds_at = add.kinds_at();
            for (i, kind) in add.provider.kinds.iter().enumerate() {
                let at = kinds_at + i;
                let has = app.codes_of(add.machine, &add.provider.id, kind);
                lines.push(selected(
                    format!(
                        "({}) {}{}",
                        if add.kind == i { "•" } else { " " },
                        add.provider.kind_label(kind),
                        if has.is_empty() {
                            String::new()
                        } else {
                            format!("   {} uses it", has.join(", "))
                        }
                    ),
                    field == at,
                ));
            }
            lines.push(Line::raw(""));
            lines.push(line("machines", DIM));
            let machines_at = add.machines_at();
            for (i, &mi) in add.machines.iter().enumerate() {
                lines.push(selected(
                    format!(
                        "[{}] {}{}",
                        if add.chosen[i] { "x" } else { " " },
                        app.machines[mi].name(),
                        if app.machines[mi].conn.is_live() {
                            ""
                        } else {
                            " · offline"
                        }
                    ),
                    field == machines_at + i,
                ));
            }
            lines.push(Line::raw(""));
            lines.extend(wrapped(
                "It joins no list until you add it with S, so nothing runs on it yet.",
                68,
                fg(DIM),
                None,
            ));
            let at_line = match field {
                f if f < kinds_at => f,
                f if f < machines_at => f + 2,
                f => f + 4,
            };
            modal(
                f,
                area,
                &format!("Add a {} account", add.provider.name()),
                lines,
                if signs_in {
                    "⏎ next: sign in    tab field    space choose    esc cancel"
                } else {
                    "⏎ add    tab field    space choose    esc cancel"
                },
                at_line,
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
            let mut code_at: Option<usize> = None;
            for (i, &mi) in s.machines.iter().enumerate() {
                lines.push(line(
                    format!(
                        "{:<22} {}",
                        app.machines[mi].name(),
                        if i < s.current {
                            "✓ signed in"
                        } else if i == s.current {
                            if s.failed {
                                if s.set_aside {
                                    "set aside for a token"
                                } else if s.replaced {
                                    "replaced by a newer sign-in"
                                } else {
                                    "✕ sign-in stopped"
                                }
                            } else if s.user_code.is_some() {
                                "◐ type the code on the page"
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
                fg(if s.went_wrong() { RED } else { FG2 }),
                None,
            ));
            if s.current < s.machines.len()
                && let Some(code) = &s.user_code
            {
                // The provider's page takes a code: show it big, with nothing to paste back.
                lines.push(Line::raw(""));
                lines.extend(wrapped(
                    &typed_code_intro(s.url.as_deref(), s.expires.as_deref(), app.now()),
                    68,
                    fg(FG),
                    None,
                ));
                let edge = "─".repeat(width(code) + 4);
                lines.push(line(format!("╭{edge}╮"), YELLOW));
                lines.push(Line::from(vec![
                    Span::styled("│  ", fg(YELLOW)),
                    Span::styled(code.clone(), bold(YELLOW)),
                    Span::styled("  │", fg(YELLOW)),
                ]));
                lines.push(line(format!("╰{edge}╯"), YELLOW));
                code_at = Some(lines.len() - 1);
                lines.push(Line::from(key_hints(&[
                    ("c", "copy code"),
                    ("o", "open the page again"),
                    ("l", "copy link"),
                ])));
                if let Some(url) = &s.url {
                    lines.extend(wrapped(url, 68, fg(DIM), None));
                }
                lines.push(Line::raw(""));
                lines.extend(wrapped(
                    "Nothing to paste back here: it ticks by itself once you've typed the code and allowed it.",
                    68,
                    fg(FG2),
                    None,
                ));
            } else if s.current < s.machines.len() {
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
                    // Which code: a six-digit emailed one was once typed here and refused.
                    lines.extend(wrapped(
                        "Once you allow it, the page shows a code: paste that here. A short code emailed to you goes in the browser, not here.",
                        68,
                        fg(DIM),
                        None,
                    ));
                    lines.push(selected(
                        format!(
                            "code from the page   {}{}",
                            "•".repeat(s.page_code.chars().count().min(30)),
                            if s.editing { "▏" } else { "" }
                        ),
                        s.editing,
                    ));
                    if let Some(note) = &s.note {
                        lines.extend(wrapped(note, 68, fg(YELLOW), None));
                    }
                    // While a code is typed, or refused, scroll to it rather than the machine row.
                    if s.editing || s.note.is_some() {
                        code_at = Some(lines.len() - 1);
                    }
                }
            }
            // Named every time: "on each machine" over a one-machine sign-in hid where it ran.
            let names: Vec<String> = s
                .machines
                .iter()
                .map(|&mi| app.machines[mi].name())
                .collect();
            let title = match (&s.copy, names.as_slice()) {
                (Some((_, label)), [first, ..]) => {
                    format!("Sign {} in on {first}: {label}", s.code)
                }
                (None, [one]) => format!("Sign {} in on {one}", s.code),
                _ => format!("Sign {} in on {}", s.code, names.join(", then ")),
            };
            let hints = if done && s.copy.is_some() {
                "Signed in.    esc accounts".to_string()
            } else if done {
                "All selected machines signed in.    esc accounts".to_string()
            } else if s.replaced {
                "r take it back    esc close".to_string()
            } else if s.failed {
                format!(
                    "r try again{}    esc later",
                    if s.paste { "    p paste token" } else { "" }
                )
            } else if s.expired(app.now()) {
                "r start again    esc stop".to_string()
            } else if s.user_code.is_some() {
                "c copy code    o open page    l copy link    esc stop".to_string()
            } else {
                format!(
                    "tab code / buttons    ⏎ send code{}    esc stop",
                    if s.paste { "    p token" } else { "" }
                )
            };
            modal(
                f,
                area,
                &title,
                lines,
                &hints,
                code_at.unwrap_or(s.current + top),
                if s.went_wrong() { RED } else { CYAN },
            );
        }
        AccountScreen::Where {
            code,
            message,
            choices,
            sel,
            ..
        } => {
            let mut lines = wrapped(message, 68, fg(FG), None);
            lines.push(Line::raw(""));
            let top = lines.len();
            lines.extend(
                choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| selected(c.label.clone(), i == *sel)),
            );
            modal(
                f,
                area,
                &format!("Sign {code} in where?"),
                lines,
                "j k choose    ⏎ sign in there    esc back",
                top + sel,
                YELLOW,
            );
        }
        AccountScreen::Confirm {
            machine,
            request,
            message,
            yes,
            no,
            target,
            choices,
            ..
        } => {
            let mut body = wrapped(message, 68, fg(FG), None);
            // More than yes and no: one answer to a line, as the board's questions list them
            // (intent 0591). Otherwise the one line it always had.
            let mut hints = if choices.is_empty() {
                format!("y {yes}    n {no}    esc cancel")
            } else {
                body.push(Line::raw(""));
                let keys = crate::actions::choice_keys(choices);
                let mut answers = vec![("y".to_string(), yes.clone())];
                for (choice, key) in choices.iter().zip(keys) {
                    if let Some(key) = key {
                        answers.push((key.to_string(), choice.label.clone()));
                    }
                }
                answers.push(("n".to_string(), no.clone()));
                for (key, words) in answers {
                    body.push(Line::from(vec![
                        Span::styled(format!("{key}  "), bold(FG)),
                        Span::styled(words, fg(FG2)),
                    ]));
                }
                "esc cancel".to_string()
            };
            // Longer than the screen holds: it scrolls, and y shows the rest before it answers,
            // as the board's question box does (Codex's review). The rows modal gives its body.
            let rows = ((body.len() as u16 + 6).max(10))
                .min(area.height.saturating_sub(2))
                .min(area.height.saturating_sub(1))
                .saturating_sub(6);
            let scroll = app.question_view.fit(
                request["action"].as_str().unwrap_or_default(),
                message,
                body.len() as u16,
                rows,
            );
            if body.len() as u16 > rows {
                hints = format!("pgdn pgup the rest of it    {hints}");
            }
            // modal keeps its selected line in view: the last row shown, scrolled to here.
            let shown_to = if scroll == 0 {
                0
            } else {
                (scroll + rows.saturating_sub(1)) as usize
            };
            modal(
                f,
                area,
                &confirm_title(app, *machine, request, target.as_deref()),
                body,
                &hints,
                shown_to,
                YELLOW,
            );
        }
    }
}

/// What a sign-in typed on the provider's page says above the code: where, and for how long.
fn typed_code_intro(
    url: Option<&str>,
    expires: Option<&str>,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let page = url
        .and_then(|u| u.strip_prefix("https://"))
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .map_or("On the sign-in page".to_string(), |host| {
            format!("On the page at {host}")
        });
    let lasts = expires
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| (t.with_timezone(&chrono::Local) - now).num_minutes())
        .map_or(String::new(), |m| match m {
            m if m <= 0 => " It may have run out: r starts again.".into(),
            1 => " It's good for about a minute.".into(),
            m => format!(" It's good for {m} minutes."),
        });
    format!("{page}, type this code.{lasts}")
}

/// The question's title: a move to an account another program runs is a hand-over, named so.
fn confirm_title(
    app: &App,
    machine: usize,
    request: &serde_json::Value,
    target: Option<&crate::app::account_controls::MoveTarget>,
) -> String {
    if request["action"] == "account-move"
        && let Some(t) = target
        && let Some(to) = request["to"].as_str()
        && let Some(a) = app.account(machine, to)
        && !t.agent.program.is_empty()
        && !a.runs.is_empty()
        && t.agent.program != a.runs
    {
        return format!(
            "Hand {} over to {}?",
            t.reference,
            app.providers(machine).program_label(&a.runs)
        );
    }
    if request["action"] == crate::app::resume::RESUME {
        // `label` is the workspace's own name (the end-to-end check's third failure).
        return match request["label"].as_str().filter(|l| !l.is_empty()) {
            Some(label) => format!("Resume workspace {label}?"),
            None => "Resume its conversation?".into(),
        };
    }
    "Account change".into()
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

/// An account's line: its code, name, provider and kind, in its provider's words.
fn account_line(app: &App, mi: usize, a: &Account, yes: bool) -> Line<'static> {
    let providers = app.providers(mi);
    let mut row = selected(
        format!(
            "{:<7} {:<16} {} · {}",
            a.code,
            a.name,
            providers.label(&a.provider),
            providers.kind_label(&a.provider, &a.kind)
        ),
        yes,
    );
    if is_current(app, mi, &a.code) {
        row.spans.push(Span::styled("  ★ current", bold(CYAN)));
    }
    row
}

/// Whether this machine's feed names the account as the current one.
fn is_current(app: &App, mi: usize, code: &str) -> bool {
    app.account_feed(mi)
        .and_then(|f| f.current_account())
        .is_some_and(|c| c.account == code)
}

/// The window a plan lacks that its provider can have, as `weekly limit only` for a plan whose
/// one window is weekly: why there's one number where others have two.
fn lone_window(app: &App, a: &Account) -> Option<String> {
    let windows = app.account_windows(a);
    let [one] = windows.as_slice() else {
        return None;
    };
    (!app.account_lacking(a).is_empty()).then(|| format!("{} limit only", one.name()))
}

/// An account's figures and status, then a line for each window that has reset since its
/// freshest reading, whose figure shows as unknown.
fn status_lines(app: &App, a: &Account) -> Vec<Line<'static>> {
    let mut first = vec![Span::styled(
        format!("          {}  {}", app.account_numbers(a), a.status_label()),
        fg(account_color(a)),
    )];
    if let Some(only) = lone_window(app, a) {
        first.push(Span::styled(format!("   {only}"), fg(DIM)));
    }
    let mut lines = vec![Line::from(first)];
    if let Some(u) = app.account_usage(a) {
        for (window, at) in u.reset_since(app.now()) {
            lines.push(line(
                format!("          {window} window reset {at}; use since unknown"),
                DIM,
            ));
        }
    }
    lines
}

/// What Enter does on an account's line, naming every machine it signs in on (intent 0327).
fn enter_hint(app: &App, mi: usize, a: &Account, copy: Option<&AccountCopy>) -> String {
    let name = app.machines.get(mi).map(|m| m.name()).unwrap_or_default();
    if !app.offered(mi).signs_in(&a.provider) {
        return format!("⏎ test it on {name} (it needs no sign-in)");
    }
    match copy {
        Some(c) if a.copy_ready(c) => format!("⏎ signed in on {name}: sign in again? · t test it"),
        Some(_) => format!("⏎ sign in this copy on {name} · t test it"),
        None if a.ready() => format!("⏎ ready on {name}: choose where to sign in · t test"),
        None => {
            let names: Vec<String> = app
                .signin_order(&a.code, false, mi)
                .into_iter()
                .map(|i| app.machines[i].name())
                .collect();
            format!("⏎ sign in on {} · t test", names.join(", then "))
        }
    }
}

/// The detail panel's usage: each window the account has, its share and reset, and which
/// windows its provider can have that this plan doesn't.
fn usage_detail(app: &App, a: &Account, w: usize) -> Vec<Line<'static>> {
    let windows = app.account_windows(a);
    if app.account_usage(a).is_none() || windows.is_empty() {
        return vec![];
    }
    let mut lines = vec![Line::raw(""), heading("USAGE")];
    for win in &windows {
        // As the account's line has it: unknown once its window has reset since the reading.
        let used = win
            .current(app.now())
            .map_or("—".to_string(), |v| format!("{v:.0}%"));
        let resets = win
            .resets
            .as_deref()
            .map(|t| format!(" · resets {}", crate::accounts::reset_time(t)))
            .unwrap_or_default();
        lines.extend(wrapped(
            &format!(
                "{used} of {}{resets}",
                crate::accounts::window_period(win.minutes)
            ),
            w,
            fg(FG2),
            None,
        ));
    }
    let missing = app.account_lacking(a);
    if !missing.is_empty() {
        lines.extend(wrapped(
            &format!(
                "This plan has no {} window, so {}.",
                missing.join(" or "),
                if windows.len() == 1 {
                    "one number".to_string()
                } else {
                    format!("{} numbers", windows.len())
                }
            ),
            w,
            fg(DIM),
            None,
        ));
    }
    lines
}

/// Where an account stands in the owners' own lists: `Last for Arkship and Kibbelz`, or that
/// it's in none, so nothing runs on it yet.
fn in_the_lists(feed: &crate::feed::Feed, code: &str, w: usize) -> Vec<Line<'static>> {
    let mut places: Vec<(String, Vec<String>)> = Vec::new();
    for o in feed.owners.iter().filter(|o| o.own || o.parent.is_none()) {
        let Some(i) = o.models.iter().position(|c| c == code) else {
            continue;
        };
        let place = match (i, o.models.len()) {
            (0, _) => "First".to_string(),
            (i, n) if i + 1 == n => "Last".to_string(),
            (i, n) => format!("{} of {n}", i + 1),
        };
        match places.iter_mut().find(|(p, _)| *p == place) {
            Some((_, names)) => names.push(o.name.clone()),
            None => places.push((place, vec![o.name.clone()])),
        }
    }
    let current = feed.current_account().is_some_and(|c| c.account == code);
    let text = if places.is_empty() && current {
        "In no list, but it's the current account: new sessions start on it wherever it's ready. Add it with S for when it's cleared.".to_string()
    } else if places.is_empty() {
        "In no list yet: nothing runs on it until you add it with S, or make it current with c."
            .to_string()
    } else {
        let said: Vec<String> = places
            .into_iter()
            .map(|(place, names)| format!("{place} for {}", and_list(&names)))
            .collect();
        format!(
            "{}. Whatever is first is what new work runs{}.",
            said.join("; "),
            if feed.current_account().is_some() {
                ", whenever the current account can't"
            } else {
                ""
            }
        )
    };
    let mut lines = vec![Line::raw(""), heading("IN THE LISTS")];
    lines.extend(wrapped(&text, w, fg(FG2), None));
    lines
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn list(f: &mut Frame, area: Rect, app: &App, mi: usize, rows: &[Row], sel: usize, model: usize) {
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(fg(FG).bg(BG)), area);
    let inner = pad(area, 2, 1);
    let name = app.machines.get(mi).map(|m| m.name()).unwrap_or_default();
    let title = format!("kerdr › accounts   {name}");
    let room = (inner.width as usize).saturating_sub(width(&title) + 2);
    // put_lr cuts the title short before the current account, which always fits.
    let right = current_and_usage(app, mi, room);
    put_lr(
        f.buffer_mut(),
        inner,
        inner.y,
        vec![Span::styled(title, bold(CYAN))],
        right,
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
    let mut lines = vec![];
    if feed.knows_current() {
        lines.extend(current_block(app, mi, tw));
        lines.push(Line::raw(""));
    }
    lines.extend([heading("ACCOUNTS"), table_edge(tw, true)]);
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
                table_row(line("  owner                   ordered models", DIM), tw),
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
                        lines.push(table_row(account_line(app, mi, a, false), tw));
                        lines.extend(status_lines(app, a).into_iter().map(|l| table_row(l, tw)));
                    }
                    if i == sel {
                        selected_line = lines.len();
                    }
                    lines.push(table_row(copy_line(c, i == sel, cols), tw));
                }
            }
            Row::Account(code) => {
                if let Some(a) = feed.accounts.iter().find(|a| &a.code == code) {
                    lines.push(table_row(account_line(app, mi, a, i == sel), tw));
                    lines.extend(status_lines(app, a).into_iter().map(|l| table_row(l, tw)));
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
                    let providers = app.providers(mi);
                    detail.push(heading(format!("{} · {}", a.code, a.name)));
                    detail.extend(wrapped(
                        &format!(
                            "{} · runs {} · {}",
                            providers.label(&a.provider),
                            providers.program_label(&a.runs),
                            providers.kind_label(&a.provider, &a.kind)
                        ),
                        w,
                        fg(DIM),
                        None,
                    ));
                    detail.push(Line::raw(""));
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
                    detail.extend(usage_detail(app, a, w));
                    detail.extend(in_the_lists(feed, &a.code, w));
                    if !feed.knows_current() {
                    } else if is_current(app, mi, &a.code) {
                        detail.push(Line::raw(""));
                        detail.extend(wrapped(
                            "★ The current account: new sessions on every machine start on it, ahead of their lists, wherever it's ready. C clears it.",
                            w,
                            fg(CYAN),
                            None,
                        ));
                    } else {
                        detail.push(Line::raw(""));
                        detail.extend(wrapped(
                            "c makes it the current account: new sessions on every machine start on it, ahead of their lists.",
                            w,
                            fg(DIM),
                            None,
                        ));
                    }
                    detail.push(Line::raw(""));
                    // Names the machine: "this machine" read as the laptop Kerdr ran on (intent 0327).
                    let enter = enter_hint(app, mi, a, copy);
                    detail.extend(wrapped(
                        &format!("{enter} · e rename · x remove · tab another machine"),
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
        // On an account's line it says where Enter signs in, since narrow screens hide the panel.
        let account_hint = match rows.get(sel) {
            Some(row @ (Row::Account(code) | Row::Copy(code, _))) => {
                feed.accounts.iter().find(|a| &a.code == code).map(|a| {
                    let copy = match row {
                        Row::Copy(_, id) => a.copy(id),
                        _ => None,
                    };
                    enter_hint(app, mi, a, copy)
                })
            }
            _ => None,
        };
        let make = if feed.knows_current() {
            " · c make current"
        } else {
            ""
        };
        let clear = if feed.current_account().is_some() {
            " · C clear current"
        } else {
            ""
        };
        let hints = match account_hint {
            Some(hint) => format!(
                "j k move · {hint} · e rename · x remove{make}{clear} · + new · tab machine · esc board"
            ),
            None => format!(
                "j k move · h l model · ⏎ first · J K reorder · a d add/drop · + new · e t x{clear} · tab machine · esc board"
            ),
        };
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
