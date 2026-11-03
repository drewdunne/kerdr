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

    /// Reads the machine again now, its feed too: back from herdr, where you may have just
    /// answered an agent whose pane never stopped working (a workflow running in it), only the
    /// feed can say the question's answered (intent 0420).
    fn wake_feed(&self, i: usize) {
        if let Some(Some(p)) = self.pollers.get(i) {
            let _ = p.wake.send(Wake::Feed);
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
    let (action_tx, action_rx) = mpsc::channel::<Answer>();
    let runner = Runner::spawn(action_tx.clone());
    let (image_tx, image_rx) = mpsc::channel::<(u64, crate::images::Loading)>();
    // When q last said what still runs: a second q soon after quits (intent 0445).
    let mut quit_asked: Option<Instant> = None;
    // Colours from the desktop theme, read again when it changes (palette.rs).
    let mut theme = crate::palette::ThemeWatch::new(app.cfg.theme.colors.as_deref());
    theme.check(true);
    loop {
        theme.check(false);
        while let Ok((id, loading)) = image_rx.try_recv() {
            // A new intent sent while its image was still being read goes now (intent 0445).
            if let Action::Mutate { machine, request } = app.images_loaded(id, loading) {
                start_action(app, &action_tx, &runner, machine, request);
            }
        }
        while let Ok(answer) = action_rx.try_recv() {
            let (job, machine, request, result) = match answer {
                Answer::Started(n) => {
                    app.resolver.started(n);
                    continue;
                }
                Answer::Done(done) => *done,
            };
            let succeeded = result.as_ref().is_ok_and(|r: &crate::actions::Response| {
                r.ok && !(request["action"] == "account-signin"
                    && (r.status.as_deref() == Some("waiting") || request["step"] == "cancel"))
            });
            match job {
                Some(n) => {
                    app.finish_job(n, request, result);
                    // Finished with: its record's next change may start. One sent on to another
                    // machine keeps its place in the runner's queue, and runs again from there.
                    if !app.resolver.sent_on(n) {
                        let _ = runner.tx.send(ToRunner::Done(n));
                    }
                }
                None => app.finish_action(machine, request, result),
            }
            if succeeded && let Some(l) = live.as_deref() {
                for poller in l.pollers.iter().flatten() {
                    let _ = poller.wake.send(Wake::Feed);
                }
            }
        }
        // Changes to cards go to the runner, which starts each in its turn (intents 0416, 0445).
        run_ready(app, &runner);
        if app.resolver.expire() {
            app.rebuild();
        }
        if let Some(url) = app.signin_open.take() {
            open_signin(app, &url);
        }
        if !app.demo
            && let Action::Mutate { machine, request } = app.account_tick()
        {
            start_action(app, &action_tx, &runner, machine, request);
        }
        // A reopen answered with its workspace while you're still on its card: go in, and on
        // ctrl+space space carry on to the next card that needs you, as from any other.
        // Not while a failure is up: going into herdr would hide it before it's read.
        if !app.failing()
            && let Some(open) = app.open_after.take()
        {
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
        // A resend to a record's real home, or a card whose workspace closed as you went in
        // reopening (intent 0411), whatever account action runs (intent 0445).
        if !app.demo
            && let Action::Mutate { machine, request } = app.next_owed()
        {
            start_action(app, &action_tx, &runner, machine, request);
        }
        for ev in inbox.take() {
            app.apply(ev);
        }
        // A start that waited for its intent to be written up goes once it is (intent 0445).
        while let Action::Mutate { machine, request } = app.start_written_up() {
            start_action(app, &action_tx, &runner, machine, request);
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
        if !matches!(action, Action::Quit) {
            quit_asked = None;
        }
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
                // Only what would be lost holds Kerdr open: a change not yet sent, or a new
                // intent's words still on their way. A start or move already running finishes
                // without Kerdr, so a second q quits (intent 0445).
                if let Some(text) = app.quit_held() {
                    app.say(text, Tone::Warn);
                } else if app.busy || !app.account_queue.is_empty() {
                    app.say(
                        "An account change is still running. Wait for its result before quitting.",
                        Tone::Warn,
                    );
                } else if let Some(text) = app.still_running()
                    && quit_asked.is_none_or(|at| at.elapsed() > Duration::from_secs(5))
                {
                    quit_asked = Some(Instant::now());
                    app.say(format!("{text} q again quits."), Tone::Info);
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
                start_action(app, &action_tx, &runner, machine, request);
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
                        // Machines not answering are tried again; the rest are read now, feeds
                        // and all, so R brings the board up to date (intent 0445).
                        let mut n = 0;
                        for i in 0..app.machines.len() {
                            if !app.machines[i].is_shown() {
                                continue;
                            }
                            if app.machines[i].conn.is_live() {
                                l.wake_feed(i);
                            } else {
                                l.wake(i);
                                n += 1;
                            }
                        }
                        let text = if n == 0 {
                            "Reading every machine now.".to_string()
                        } else {
                            format!("Reading every machine now, and trying {n} again.")
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

/// An answer: its change in the action resolution tool (None for an account action), its machine,
/// the request without credentials, and what came back.
type ActionResult = (
    Option<u64>,
    usize,
    serde_json::Value,
    Result<crate::actions::Response, String>,
);

/// What comes back to the board's thread: a change's command has started, or an action answered.
enum Answer {
    Started(u64),
    Done(Box<ActionResult>),
}

/// What the runner is told: a change to run in its turn, or that one has answered.
enum ToRunner {
    Run {
        n: u64,
        machine: usize,
        key: Option<String>,
        target: Machine,
        command: String,
        request: serde_json::Value,
    },
    Done(u64),
}

/// Runs changes to cards in their turn on a thread of its own, which never touches App: a change
/// whose turn comes while Andrew is inside a workspace starts then, not once he's back (intent
/// 0445). Its answers wait on the channel until the board reads them.
struct Runner {
    tx: mpsc::Sender<ToRunner>,
}

impl Runner {
    fn spawn(answers: mpsc::Sender<Answer>) -> Runner {
        let (tx, rx) = mpsc::channel::<ToRunner>();
        let back = tx.clone();
        std::thread::spawn(move || {
            let mut queue = crate::resolve::Queue::default();
            let mut waiting: std::collections::HashMap<
                u64,
                (usize, Machine, String, serde_json::Value),
            > = Default::default();
            for msg in rx {
                match msg {
                    ToRunner::Run {
                        n,
                        machine,
                        key,
                        target,
                        command,
                        request,
                    } => {
                        queue.push(n, machine, key, &request);
                        waiting.insert(n, (machine, target, command, request));
                    }
                    ToRunner::Done(n) => queue.done(n),
                }
                for n in queue.startable() {
                    let Some((machine, target, command, request)) = waiting.remove(&n) else {
                        continue;
                    };
                    let _ = answers.send(Answer::Started(n));
                    let (answers, back) = (answers.clone(), back.clone());
                    std::thread::spawn(move || {
                        let result = crate::actions::run(&target, &command, &request);
                        let metadata = crate::actions::request_metadata(&request);
                        drop(request);
                        // A refusal naming another home holds its record's place until the board
                        // has sent it on there, or not (intent 0416).
                        let held = result
                            .as_ref()
                            .is_ok_and(crate::actions::Response::sends_on);
                        let _ = answers.send(Answer::Done(Box::new((
                            Some(n),
                            machine,
                            metadata,
                            result,
                        ))));
                        if !held {
                            let _ = back.send(ToRunner::Done(n));
                        }
                    });
                }
            }
        });
        Runner { tx }
    }
}

/// Runs an action in the background; its answer comes back on the channel. A change to a card goes
/// to the action resolution tool, which shows it at once and runs it in its turn (intent 0416); an
/// account action waits for its answer, one at a time.
fn start_action(
    app: &mut App,
    tx: &mpsc::Sender<Answer>,
    runner: &Runner,
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
    if !request["action"]
        .as_str()
        .is_some_and(|a| a.starts_with("account-"))
    {
        app.queue_action(machine, request);
        run_ready(app, runner);
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
        let _ = tx.send(Answer::Done(Box::new((None, machine, metadata, result))));
    });
}

/// Hands every new change to a card to the runner, which starts each in its turn.
fn run_ready(app: &mut App, runner: &Runner) {
    for (n, machine, request) in app.resolver.ready() {
        let command = app.cfg.actions.command.clone();
        let target = app.machines.get(machine).map(|m| m.machine.clone());
        let (Some(command), Some(target)) = (command, target) else {
            let metadata = crate::actions::request_metadata(&request);
            app.finish_job(
                n,
                metadata,
                Err("actions are disabled; configure [actions] command".into()),
            );
            continue;
        };
        let key = app.resolver.key_of(n);
        let _ = runner.tx.send(ToRunner::Run {
            n,
            machine,
            key,
            target,
            command,
            request,
        });
    }
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
        l.wake_feed(mi);
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
            app.say(
                format!(
                    "{} is on {}, which isn't answering: Kerdr goes in when it answers. Esc on the board stops waiting.",
                    card.reference,
                    app.machines[mi].name()
                ),
                Tone::Info,
            );
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
        // The next card is chosen only on ctrl+space space, or when one Kerdr picked has gone.
        let choosing_next = matches!(outcome, Outcome::Next)
            || (!yours && matches!(outcome, Outcome::Closed | Outcome::Stopped));
        if choosing_next {
            // The board shows while the machines are read, not a blank screen (intent 0445).
            app.say("Finding the next card that needs you…", Tone::Info);
            let _ = terminal.draw(|f| crate::ui::draw(f, app));
        }
        read_again(app, live.as_deref(), inbox, mi, choosing_next);
        app.unsay("Finding the next card that needs you…");
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
            if i == mi {
                // What herdr shows comes first and fast; the feed follows on its own, so the
                // wait below never waits out a feed read (intent 0445).
                if choosing_next {
                    l.wake(i);
                }
                l.wake_feed(i);
            } else {
                l.wake(i);
            }
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

    #[cfg(unix)]
    #[test]
    fn the_runner_starts_a_record_s_next_change_without_the_board() {
        // Nothing reads the answers while it runs, as while Andrew is inside a workspace: the
        // second change to the record still starts once the first has answered (intent 0445).
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let command =
            "cat >/dev/null\nsleep 0.2\nprintf '%s' '{\"ok\":true,\"message\":\"done\"}'\n";
        for n in [1, 2] {
            runner
                .tx
                .send(ToRunner::Run {
                    n,
                    machine: 0,
                    key: Some("here:7".into()),
                    target: machine.clone(),
                    command: command.into(),
                    request: serde_json::json!({"action":"move","id":"7","delta":1}),
                })
                .unwrap();
        }
        let mut seen = vec![];
        while seen.len() < 4 {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                Answer::Started(n) => seen.push(format!("started {n}")),
                Answer::Done(done) => {
                    assert!(done.3.as_ref().is_ok_and(|r| r.ok), "{:?}", done.3);
                    seen.push(format!("done {}", done.0.unwrap()));
                }
            }
        }
        assert_eq!(seen, ["started 1", "done 1", "started 2", "done 2"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_change_refused_for_another_home_holds_its_card_s_place_until_sent_on() {
        // J is refused, naming another home: K waits while the board sends J on, and J, sent
        // again to the other machine, goes before K (intent 0416).
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let answer = |json: &str| format!("cat >/dev/null\nprintf '%s' '{json}'\n");
        let refused = answer(r#"{"ok":false,"message":"lives elsewhere","home":"there/ARK"}"#);
        let ok = answer(r#"{"ok":true,"message":"done"}"#);
        let run = |n: u64, machine_index: usize, command: &str| ToRunner::Run {
            n,
            machine: machine_index,
            key: Some("here:7".into()),
            target: machine.clone(),
            command: command.into(),
            request: serde_json::json!({"action":"move","id":"7","delta":1}),
        };
        runner.tx.send(run(1, 0, &refused)).unwrap();
        runner.tx.send(run(2, 0, &ok)).unwrap();
        let next = |wait: u64| match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(Answer::Started(n)) => Some(format!("started {n}")),
            Ok(Answer::Done(done)) => Some(format!("done {}", done.0.unwrap())),
            Err(_) => None,
        };
        assert_eq!(next(10_000).as_deref(), Some("started 1"));
        assert_eq!(next(10_000).as_deref(), Some("done 1"));
        assert_eq!(next(500), None, "K waits while J is sent on");
        runner.tx.send(run(1, 1, &ok)).unwrap();
        let seen: Vec<String> = (0..4).filter_map(|_| next(10_000)).collect();
        assert_eq!(seen, ["started 1", "done 1", "started 2", "done 2"]);
    }
}
