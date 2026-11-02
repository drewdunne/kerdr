//! The running board: draws, reads keys, and listens to every machine's reader.

use std::io::stdout;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;

use crate::app::{Action, App, Selection, Tone};
use crate::config::Machine;
use crate::herdr_keys;
use crate::jump::{self, Jump, Outcome, TerminalFlags};
use crate::poll::{self, Conn, Inbox, Poller, Timing, Wake};
use crate::state::State;

/// How long the board waits, on the way to the next card that needs you, for the live machines
/// to say what's there now.
const FRESH_WAIT: Duration = Duration::from_millis(1500);

struct Live {
    pollers: Vec<Option<Poller>>,
    inbox: Arc<Inbox>,
}

impl Live {
    fn timing(app: &App, m: &Machine) -> Timing {
        let every = if m.is_local() {
            app.cfg.machines.local_every
        } else {
            app.cfg.machines.remote_every
        };
        Timing {
            every: Duration::from_secs_f64(every.max(0.5)),
            feed_every: Duration::from_secs(app.cfg.feed.every.max(2)),
        }
    }

    fn start(&mut self, app: &App, i: usize) {
        let m = app.machines[i].machine.clone();
        let timing = Self::timing(app, &m);
        let reader = self.inbox.next_reader(i);
        self.pollers[i] = Some(poll::spawn(i, m, timing, self.inbox.clone(), reader));
    }

    fn stop(&mut self, i: usize) {
        if let Some(p) = self.pollers[i].take() {
            self.inbox.next_reader(i);
            let _ = p.wake.send(Wake::Stop);
        }
    }

    fn wake(&self, i: usize) {
        if let Some(Some(p)) = self.pollers.get(i) {
            let _ = p.wake.send(Wake::Now);
        }
    }
}

pub fn run(mut app: App, live: bool) -> anyhow::Result<()> {
    // One read per machine at most, the newest: while you're inside herdr nobody applies them.
    let inbox = Arc::new(Inbox::default());
    let mut live = live.then(|| Live {
        pollers: (0..app.machines.len()).map(|_| None).collect(),
        inbox: inbox.clone(),
    });
    if let Some(l) = live.as_mut() {
        for i in 0..app.machines.len() {
            if app.machines[i].is_shown() {
                l.start(&app, i);
            }
        }
    }

    #[cfg(windows)]
    let _console_modes = crate::windows::ConsoleModes::capture();
    let mut terminal = ratatui::init();
    let keyboard = supports_keyboard_enhancement().unwrap_or(false);
    if keyboard {
        let _ = execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let _ = execute!(stdout(), ratatui::crossterm::event::EnableBracketedPaste);
    let flags = TerminalFlags { keyboard };
    let client_config = herdr_keys::write_client_config().ok();
    if std::env::var_os("HERDR_PANE_ID").is_some() && !app.demo {
        app.say(
            "Kerdr is running inside herdr: jumping in opens herdr inside herdr. A plain terminal works best.",
            Tone::Warn,
        );
    }

    let result = event_loop(
        &mut terminal,
        &flags,
        &mut app,
        live.as_mut(),
        &inbox,
        client_config,
    );
    if keyboard {
        let _ = execute!(
            stdout(),
            ratatui::crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    let _ = execute!(stdout(), ratatui::crossterm::event::DisableBracketedPaste);
    ratatui::restore();
    if let Some(l) = live.as_mut() {
        for i in 0..l.pollers.len() {
            l.stop(i);
        }
    }
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    mut live: Option<&mut Live>,
    inbox: &Inbox,
    client_config: Option<PathBuf>,
) -> anyhow::Result<()> {
    let (action_tx, action_rx) = mpsc::channel::<ActionResult>();
    let (image_tx, image_rx) = mpsc::channel::<(u64, crate::images::Loading)>();
    loop {
        while let Ok((id, loading)) = image_rx.try_recv() {
            app.images_loaded(id, loading);
        }
        while let Ok((machine, request, result)) = action_rx.try_recv() {
            let succeeded = result.as_ref().is_ok_and(|r: &crate::actions::Response| {
                r.ok && !(request["action"] == "account-signin"
                    && (r.status.as_deref() == Some("waiting") || request["step"] == "cancel"))
            });
            app.finish_action(machine, request, result);
            if succeeded && let Some(l) = live.as_deref() {
                for poller in l.pollers.iter().flatten() {
                    let _ = poller.wake.send(Wake::Feed);
                }
            }
        }
        if let Some(url) = app.signin_open.take() {
            open_signin(app, &url);
        }
        if !app.demo
            && let Action::Mutate { machine, request } = app.account_tick()
        {
            start_action(app, &action_tx, machine, request);
        }
        // A reopen answered with its workspace while you're still on its card: go in, and on
        // ctrl+space space carry on to the next card that needs you, as from any other.
        if let Some(open) = app.open_after.take() {
            let outcome = jump_workspace(
                terminal,
                flags,
                app,
                live.as_deref_mut(),
                client_config.as_ref(),
                open.machine,
                open.session,
                open.workspace,
                Some(open.label),
                open.tab,
            );
            if outcome == Some(Outcome::Next) {
                // Choose from the board as it is now, as jump_cards does, not as it was on the way in.
                read_again(app, live.as_deref(), inbox, open.machine, true);
                match app.next_needs_you_after(&open.from, &[]) {
                    Some(next) => jump_cards(
                        terminal,
                        flags,
                        app,
                        live.as_deref_mut(),
                        inbox,
                        client_config.as_ref(),
                        next,
                        false,
                    ),
                    None => app.say("Nothing else needs you right now.", Tone::Good),
                }
            }
        }
        // Once nothing runs: a card whose workspace closed as you went in reopens, or a ⏎ pressed
        // while another action ran opens its card (intent 0411).
        if !app.demo {
            match app.next_owed() {
                Action::Mutate { machine, request } => {
                    start_action(app, &action_tx, machine, request)
                }
                Action::Jump(key) => jump_cards(
                    terminal,
                    flags,
                    app,
                    live.as_deref_mut(),
                    inbox,
                    client_config.as_ref(),
                    key,
                    true,
                ),
                _ => {}
            }
        }
        for ev in inbox.take() {
            app.apply(ev);
        }
        if let Some(key) = app.pending_jump_ready() {
            jump_cards(
                terminal,
                flags,
                app,
                live.as_deref_mut(),
                inbox,
                client_config.as_ref(),
                key,
                true,
            );
        }
        app.width = terminal.size()?.width;
        terminal.draw(|f| crate::ui::draw(f, app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let action = match event::read()? {
            Event::Key(key) => app.on_key(key),
            Event::Paste(text) => app.on_paste(&text),
            _ => continue,
        };
        match action {
            Action::None => {}
            Action::LoadImages(source) => {
                if let Some((id, room)) = app.begin_image_load(&source) {
                    let tx = image_tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send((id, source.load(room)));
                    });
                }
            }
            Action::CopyLink(url) => {
                if crate::accounts::valid_signin_url(&url) && to_clipboard(&url) {
                    app.say("Sign-in link sent to the terminal clipboard.", Tone::Info);
                }
            }
            Action::CopyCode(code) => {
                if crate::app::account_controls::user_code_ok(&code) && to_clipboard(&code) {
                    app.say(
                        format!(
                            "{code} sent to the terminal clipboard: type it on the sign-in page."
                        ),
                        Tone::Info,
                    );
                }
            }
            Action::OpenLink(url) => open_signin(app, &url),
            Action::Quit => {
                if app.busy || !app.account_queue.is_empty() {
                    app.say(
                        "An intent change is still running. Wait for its result before quitting.",
                        Tone::Warn,
                    );
                } else {
                    return Ok(());
                }
            }
            Action::Mutate { machine, request } => {
                if app.demo {
                    app.say(
                        "This is the demo: intent controls change your real board only.",
                        Tone::Info,
                    );
                    continue;
                }
                if app.busy {
                    continue;
                }
                start_action(app, &action_tx, machine, request);
            }
            Action::Jump(card) => {
                if app.demo {
                    app.say("This is the demo: on your own board, ⏎ opens herdr on this card's workspace.", Tone::Info);
                } else {
                    jump_cards(
                        terminal,
                        flags,
                        app,
                        live.as_deref_mut(),
                        inbox,
                        client_config.as_ref(),
                        card,
                        true,
                    );
                }
            }
            Action::JumpWorkspace {
                machine,
                session,
                id,
            } => {
                if app.demo {
                    app.say("This is the demo: there's no herdr behind it.", Tone::Info);
                    continue;
                }
                let Some(mi) = app.machine_index(&machine) else {
                    continue;
                };
                jump_workspace(
                    terminal,
                    flags,
                    app,
                    live.as_deref_mut(),
                    client_config.as_ref(),
                    mi,
                    session,
                    id,
                    None,
                    None,
                );
            }
            Action::Retry(target) => {
                let Some(l) = live.as_deref() else {
                    app.say("This is the demo: nothing to retry.", Tone::Info);
                    continue;
                };
                match target {
                    Some(i) => {
                        l.wake(i);
                        app.say(
                            format!("Trying {} again now.", app.machines[i].name()),
                            Tone::Info,
                        );
                    }
                    None => {
                        let mut n = 0;
                        for i in 0..app.machines.len() {
                            if !app.machines[i].conn.is_live() && app.machines[i].is_shown() {
                                l.wake(i);
                                n += 1;
                            }
                        }
                        let text = if n == 0 {
                            "Every machine is answering.".to_string()
                        } else {
                            format!("Trying {n} machines again now.")
                        };
                        app.say(text, Tone::Info);
                    }
                }
            }
            Action::Answer(i) => {
                let Some(host) = app.machines[i].machine.ssh.clone() else {
                    continue;
                };
                if app.demo {
                    app.say("This is the demo: on your own board, ⏎ runs ssh here so you can answer its prompt.", Tone::Info);
                    continue;
                }
                let intro = format!(
                    "Kerdr: SSH to {host} needs you. Answer its question below; Kerdr comes back when it's done.\n(It runs: ssh {host} true)"
                );
                let ok = jump::foreground(
                    terminal,
                    flags,
                    &intro,
                    Command::new("ssh").args([host.as_str(), "true"]),
                );
                if let Some(l) = live.as_deref() {
                    l.wake(i);
                }
                app.say(
                    if ok {
                        "Answered. Reading it again now."
                    } else {
                        "SSH didn't finish; the machine still needs you."
                    },
                    if ok { Tone::Good } else { Tone::Warn },
                );
            }
            Action::Setup(i) => {
                let Some(host) = app.machines[i].machine.ssh.clone() else {
                    continue;
                };
                if app.demo {
                    app.say(
                        "This is the demo: on your own board, u runs herdr's own setup there.",
                        Tone::Info,
                    );
                    continue;
                }
                let intro = format!(
                    "Kerdr: herdr's own setup for {host} runs now and asks before it installs, updates or restarts anything.\nWhen herdr opens, ctrl+space q brings you back to the board.\n(It runs: herdr --remote {host})"
                );
                let ok = jump::foreground(
                    terminal,
                    flags,
                    &intro,
                    Command::new("herdr").args(["--remote", host.as_str()]),
                );
                if let Some(l) = live.as_deref() {
                    l.wake(i);
                }
                if !ok {
                    app.say("herdr's setup didn't finish.", Tone::Warn);
                }
            }
            Action::ToggleHidden(i) => {
                let name = app.machines[i].name();
                let mut state = State::load();
                if matches!(app.machines[i].conn, Conn::Hidden) {
                    state.hidden.retain(|h| {
                        !h.eq_ignore_ascii_case(&name)
                            && !h.eq_ignore_ascii_case(&app.machines[i].machine.name)
                    });
                    app.machines[i].machine.hidden = false;
                    app.machines[i].conn = Conn::Connecting;
                    if let Some(l) = live.as_deref_mut() {
                        l.start(app, i);
                    }
                    app.say(format!("Showing {name} again."), Tone::Info);
                } else {
                    state.hidden.push(app.machines[i].machine.name.clone());
                    app.machines[i].machine.hidden = true;
                    app.machines[i].conn = Conn::Hidden;
                    if let Some(l) = live.as_deref_mut() {
                        l.stop(i);
                    }
                    app.say(
                        format!("Hid {name}. x in machines shows it again."),
                        Tone::Info,
                    );
                }
                if !app.demo {
                    state.save();
                }
                app.rebuild();
            }
        }
    }
}

type ActionResult = (
    usize,
    serde_json::Value,
    Result<crate::actions::Response, String>,
);

/// Runs an action in the background; its answer comes back on the channel.
fn start_action(
    app: &mut App,
    tx: &mpsc::Sender<ActionResult>,
    machine: usize,
    request: serde_json::Value,
) {
    if app.demo {
        app.say(
            "This is the demo: intent controls change your real board only.",
            Tone::Info,
        );
        return;
    }
    if app.busy {
        return;
    }
    let Some(command) = app.cfg.actions.command.clone() else {
        return;
    };
    let Some(target) = app.machines.get(machine).map(|m| m.machine.clone()) else {
        return;
    };
    let tx = tx.clone();
    app.busy = true;
    app.busy_id = request["id"].as_str().map(str::to_string);
    app.busy_since = Some(Instant::now());
    app.busy_label = app.working_on(&request);
    if request["action"] != "account-signin" {
        app.say(app.busy_label.clone(), Tone::Info);
    }
    std::thread::spawn(move || {
        let result = crate::actions::run(&target, &command, &request);
        let metadata = crate::actions::request_metadata(&request);
        drop(request);
        let _ = tx.send((machine, metadata, result));
    });
}

/// Opens herdr full screen on one workspace by its id, whether or not a card claims it yet: a
/// workspace no card claims, or one a reopen just made, on the tab it opened when it names one.
/// The outcome, when herdr opened.
#[allow(clippy::too_many_arguments)]
fn jump_workspace(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    live: Option<&mut Live>,
    client_config: Option<&PathBuf>,
    mi: usize,
    session: String,
    id: String,
    label: Option<String>,
    tab: Option<String>,
) -> Option<Outcome> {
    let machine = app.machines[mi].name();
    if !app.machines[mi].conn.is_live() {
        app.say(
            format!("{machine} isn't answering; r tries it now."),
            Tone::Warn,
        );
        return None;
    }
    let label = label
        .or_else(|| {
            app.board
                .workspaces
                .iter()
                .find(|w| w.machine == machine && w.session == session && w.id == id)
                .map(|w| w.label.clone())
        })
        .unwrap_or_else(|| id.clone());
    let via = via_for(app, mi, &session);
    let j = workspace_jump(app, mi, session, id, label, tab, via);
    let outcome = run_jump(terminal, flags, app, &j, client_config);
    if let Some(l) = live.as_deref() {
        l.wake(mi);
    }
    match &outcome {
        Outcome::Failed(e) => app.say(e.clone(), Tone::Warn),
        Outcome::Closed => app.say(closed(&format!("Workspace {}", j.label)), Tone::Warn),
        Outcome::Stopped => app.say(stopped(&j.session, &j.machine.name), Tone::Warn),
        Outcome::TabGone => app.say(
            format!("{}'s new tab closed before Kerdr got there.", j.label),
            Tone::Warn,
        ),
        Outcome::Board | Outcome::Next => {}
    }
    Some(outcome)
}

/// The jump into one workspace by its id: onto the tab a reopen opened there, and only that tab,
/// when it names one, since the workspace's active tab is still whatever was there before.
fn workspace_jump(
    app: &App,
    mi: usize,
    session: String,
    id: String,
    label: String,
    tab: Option<String>,
    via: jump::Via,
) -> Jump {
    Jump {
        machine: app.machines[mi].machine.clone(),
        session,
        workspace: id,
        strict: tab.is_some(),
        tab,
        label,
        others: app.needs_you_order().len(),
        via,
        prefix: app.prefix.clone(),
        keys: keys_on(app, mi),
    }
}

/// Goes into herdr, saying first that it's opening the card, since herdr's own calls can take
/// seconds on a slow connection and the board would look frozen meanwhile (intent 0411).
fn run_jump(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    j: &Jump,
    client_config: Option<&PathBuf>,
) -> Outcome {
    let opening = jump::opening(&j.label, &j.machine.name, j.machine.ssh.is_none());
    app.say(opening.clone(), Tone::Info);
    let _ = terminal.draw(|f| crate::ui::draw(f, app));
    let outcome = jump::run(
        terminal,
        flags,
        j,
        client_config
            .map(|p| p.as_path())
            .unwrap_or(std::path::Path::new("")),
    );
    app.unsay(&opening);
    outcome
}

/// Whether Kerdr's two keys are installed in that machine's herdr config.
fn keys_on(app: &App, mi: usize) -> bool {
    app.machines[mi].data.as_ref().is_some_and(|d| d.keys)
}

/// How to attach to a session: the session's own herdr version against this machine's.
fn via_for(app: &App, mi: usize, session: &str) -> jump::Via {
    let m = &app.machines[mi];
    let server = m.snapshot(session).and_then(|s| s.version.clone());
    let here = app
        .machines
        .first()
        .and_then(|h| h.data.as_ref())
        .and_then(|d| d.version.clone());
    jump::choose_via(&m.machine, server.as_deref(), here.as_deref())
}

/// Jumps into a card, and on ctrl+space space keeps going to the next one that needs you. `yours`
/// is whether you picked the first card yourself.
#[allow(clippy::too_many_arguments)]
fn jump_cards(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    live: Option<&mut Live>,
    inbox: &Inbox,
    client_config: Option<&PathBuf>,
    first: String,
    mut yours: bool,
) {
    let mut key = first;
    // Cards Kerdr picked whose workspace turned out to have closed: skipped from then on.
    let mut closed_on_the_way: Vec<String> = Vec::new();
    loop {
        let Some(card) = app.card(&key).cloned() else {
            return;
        };
        let (Some(session), Some(workspace)) = (card.session.clone(), card.workspace_id.clone())
        else {
            app.say(
                format!("{} has no herdr workspace to jump into.", card.reference),
                Tone::Info,
            );
            return;
        };
        let mi = card.machine_index;
        if mi >= app.machines.len() {
            return;
        }
        if !app.machines[mi].conn.is_live() {
            app.wait_to_jump(&card);
            if let Some(l) = live.as_deref() {
                l.wake(mi);
            }
            return;
        }
        app.sel = Selection {
            col: card.stage.index(),
            key: Some(key.clone()),
        };
        let others = app.needs_you_order().iter().filter(|k| **k != key).count();
        let via = via_for(app, mi, &session);
        // Its own tab, never another's: a child's, or the card's own agent's (intent 0349).
        let (tab, strict) = app.jump_tab(&card);
        let j = Jump {
            machine: app.machines[mi].machine.clone(),
            session,
            workspace,
            tab,
            strict,
            label: card
                .workspace_label
                .clone()
                .unwrap_or_else(|| card.reference.clone()),
            others,
            via,
            prefix: app.prefix.clone(),
            keys: keys_on(app, mi),
        };
        let outcome = run_jump(terminal, flags, app, &j, client_config);
        let choosing_next = matches!(outcome, Outcome::Next | Outcome::Closed | Outcome::Stopped);
        read_again(app, live.as_deref(), inbox, mi, choosing_next);
        match outcome {
            Outcome::Board => return,
            Outcome::Failed(e) => {
                app.say(e, Tone::Warn);
                return;
            }
            // The card you picked yourself: work still under way reopens; otherwise say so. One
            // Kerdr picked for you: go on to the next.
            Outcome::Closed if yours => {
                // herdr has just said it closed, so a board slow to notice still counts it gone;
                // a card that has meanwhile moved to another workspace doesn't reopen.
                if !app.closed_under_you(&key, card.workspace_id.as_deref()) {
                    app.say(closed(&card.workspace_name()), Tone::Warn);
                }
                return;
            }
            Outcome::Closed => closed_on_the_way.push(key.clone()),
            // Its herdr session has stopped: work under way or done reopens, which starts the
            // session again; otherwise say so.
            Outcome::Stopped if yours => {
                if !app.closed_under_you(&key, card.workspace_id.as_deref()) {
                    app.say(stopped(&j.session, &app.machines[mi].name()), Tone::Warn);
                }
                return;
            }
            Outcome::Stopped => closed_on_the_way.push(key.clone()),
            // The tab you picked has closed: a piece's reopens, a card's own asks to reopen it.
            Outcome::TabGone if yours => {
                app.tab_gone(&key);
                return;
            }
            Outcome::TabGone => closed_on_the_way.push(key.clone()),
            Outcome::Next => {}
        }
        match app.next_needs_you_after(&key, &closed_on_the_way) {
            Some(next) => {
                key = next;
                yours = false;
            }
            None => {
                app.say("Nothing else needs you right now.", Tone::Good);
                return;
            }
        }
    }
}

/// Reads the card's machine again and applies what's waiting. Choosing the next card, it reads
/// every live machine and waits a moment (FRESH_WAIT at most) for what they say, so the choice is
/// made from the board as it is now, not as it was when you went into herdr.
fn read_again(app: &mut App, live: Option<&Live>, inbox: &Inbox, mi: usize, choosing_next: bool) {
    if let Some(l) = live {
        let mut machines = vec![mi];
        if choosing_next {
            machines.extend(
                (0..app.machines.len()).filter(|&i| {
                    i != mi && app.machines[i].conn.is_live() && l.pollers[i].is_some()
                }),
            );
        }
        let since = Instant::now();
        for &i in &machines {
            l.wake(i);
        }
        if choosing_next {
            inbox.wait_since(&machines, since, FRESH_WAIT);
        }
    }
    for ev in inbox.take() {
        app.apply(ev);
    }
}

/// What the board says when herdr's session there isn't running.
fn stopped(session: &str, machine: &str) -> String {
    format!("herdr session {session} on {machine} has stopped since the board last read it.")
}

/// What the board says when herdr finds that a workspace has closed.
fn closed(workspace: &str) -> String {
    format!("{workspace} has closed since the board last read it.")
}

/// Sends text to the terminal's clipboard (OSC 52), on every platform's terminal alike.
fn to_clipboard(text: &str) -> bool {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = stdout();
    write!(out, "\x1b]52;c;{encoded}\x07")
        .and_then(|_| out.flush())
        .is_ok()
}

fn open_signin(app: &mut App, url: &str) {
    if app.demo {
        app.say("Demo sign-in: no browser opened.", Tone::Info);
        return;
    }
    match crate::accounts::open_signin_url(url) {
        Ok(()) => {}
        Err(error) => app.say(
            format!("Could not open the browser: {error}. Copy the link instead (c, or l where the page takes a code)."),
            Tone::Warn,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reopen_that_opened_a_tab_lands_on_that_tab_only() {
        let app = App::new(
            crate::config::Config::default(),
            crate::demo::machines(false),
        );
        let j = workspace_jump(
            &app,
            0,
            "web".into(),
            "w4".into(),
            "41-checkout".into(),
            Some("w4:t2".into()),
            jump::Via::Local,
        );
        assert_eq!((j.tab.as_deref(), j.strict), (Some("w4:t2"), true));
        let j = workspace_jump(
            &app,
            0,
            "web".into(),
            "w4".into(),
            "41-checkout".into(),
            None,
            jump::Via::Local,
        );
        assert_eq!((j.tab, j.strict), (None, false));
    }
}
