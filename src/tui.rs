//! The running board: draws, reads keys, and listens to every machine's reader.

use std::cell::RefCell;
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
use crate::hold::{Holds, Place, Spot};
use crate::jump::{self, Jump, Outcome, TerminalFlags};
use crate::poll::{self, Conn, Inbox, Poller, Timing, Wake};
use crate::state::State;

/// How long the board waits, on the way to the next card that needs you, for the live machines
/// to say what's there now. It takes no keys meanwhile, so it's short: a read without the feed
/// lands well inside it, and giving up only chooses from a read a few seconds old (intent 0491).
const FRESH_WAIT: Duration = Duration::from_millis(250);

/// How long a save from a closed editor waits for its machine to be read afresh. The board takes
/// no reads while your editor is open, so how a machine stood when it opened says nothing about
/// now; a machine that can't be reached takes seconds to say so, and past this the save goes and
/// its answer says it (the end-to-end check's P1).
const EDIT_CHECK_WAIT: Duration = Duration::from_secs(3);

/// How long after coming back to the board its turn stays quick for the read asked for then.
const READ_LANDS: Duration = Duration::from_secs(1);

/// How often the machine of a start a ⏎ waits on has its feed read again, which is the only way the
/// board learns its workspace has opened. No faster: a feed read costs that machine seconds of
/// work (intent 0491).
const START_READ_EVERY: Duration = Duration::from_secs(5);

/// When each machine of a start a ⏎ waits on was last asked for its feed for that.
#[derive(Default)]
struct StartReads(std::collections::HashMap<usize, Instant>);

impl StartReads {
    /// Of the machines a ⏎ waits on now, those whose feed is due. At once for a machine just
    /// added: the ⏎ asks for the first read, so the wait doesn't grow by a whole gap.
    fn due(&mut self, running: &[usize], now: Instant) -> Vec<usize> {
        self.0.retain(|machine, _| running.contains(machine));
        let mut due = vec![];
        for &machine in running {
            let waited = self.0.get(&machine).map(|last| now.duration_since(*last));
            if waited.is_none_or(|w| w >= START_READ_EVERY) {
                self.0.insert(machine, now);
                due.push(machine);
            }
        }
        due
    }
}

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
    // A backlog item's text a killed Kerdr left in a folder it edits in (intents 0414, 0591).
    for folder in crate::edit::folders() {
        crate::edit::sweep(&folder);
    }
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
    // Every feed reaches the runner as it's read, so a start waiting for its write-up goes as
    // soon as it's done, wherever Andrew is (intent 0445).
    let feeds = runner.tx.clone();
    inbox.watch_feeds(move |machine, feed| {
        let intakes = feed
            .cards
            .iter()
            .map(|c| (c.id.clone(), c.intake.clone()))
            .collect();
        let holds = feed.supports(crate::app::holding::HOLD);
        let footings = feed
            .cards
            .iter()
            .map(|c| {
                let rev = c.rev.clone().filter(|_| holds);
                (c.id.clone(), (crate::resolve::Footing::of(c), rev))
            })
            .collect();
        let _ = feeds.send(ToRunner::Feed {
            machine,
            intakes,
            footings,
        });
    });
    let (image_tx, image_rx) = mpsc::channel::<(u64, crate::images::Loading)>();
    let readers = crate::images::Workers::default();
    let holds = RefCell::new(if live.is_some() && !app.demo {
        Holds::on()
    } else {
        Holds::off()
    });
    let bg = Bg {
        images: &image_rx,
        answers: &action_tx,
        runner: &runner,
        inbox,
        holds: &holds,
    };
    // Colours from the desktop theme, read again when it changes (palette.rs).
    let mut theme = crate::palette::ThemeWatch::new(app.cfg.theme.colors.as_deref());
    theme.check(true);
    // Back from herdr or your editor: the read that was asked for lands in moments, and the board
    // should show it as it lands (intent 0491).
    let (mut away_seen, mut back_at) = (app.away, None::<Instant>);
    let mut start_reads = StartReads::default();
    loop {
        if app.away != away_seen {
            away_seen = app.away;
            back_at = Some(Instant::now());
        }
        theme.check(false);
        send_loaded_images(app, &bg, None);
        while let Ok(answer) = action_rx.try_recv() {
            let (job, machine, request, result, stderr, unreached) = match answer {
                Answer::Started(n) => {
                    // A start that waited for its write-up shows from now.
                    app.job_started(n);
                    continue;
                }
                Answer::Handed(n) => {
                    app.resolver.handed(n);
                    continue;
                }
                Answer::Dropped(n, dropped) => {
                    app.job_drop_answered(n, dropped);
                    continue;
                }
                // How followed work stands (intent 0591): once it has ended, its machine's feed is
                // read now, so its card shows it.
                Answer::Progress(token, machine, result) => {
                    if app.progress_answered(&token, result)
                        && let Some(l) = live.as_deref()
                    {
                        l.wake_feed(machine);
                    }
                    continue;
                }
                Answer::Update(machine, n, result) => {
                    app.update_answered(machine, n, result);
                    if let Some(l) = live.as_deref() {
                        l.wake_feed(machine);
                    }
                    continue;
                }
                // Sent again by itself (intent 0591): said quietly, on its card or the busy line.
                Answer::Resending(job, machine, why) => {
                    app.resending(job, machine, &why);
                    continue;
                }
                Answer::NotWrittenUp(n) => {
                    app.starts_from_words(n);
                    continue;
                }
                Answer::Changed(n) => {
                    app.gate_changed(n);
                    continue;
                }
                Answer::Done(done) => *done,
            };
            // Done, or done in part (intent 0591): its machine's feed is read now, to show it.
            let succeeded = result.as_ref().is_ok_and(|r: &crate::actions::Response| {
                (r.ok || r.partial)
                    && !(request["action"] == "account-signin"
                        && (r.signin_under_way() || request["step"] == "cancel"))
            });
            // For the error log, should it be a failure.
            app.answer_stderr = stderr;
            // Its machine couldn't be reached, so nothing was sent: read it now, so the board
            // shows it away and what comes next for it is held (the end-to-end check's P1).
            if unreached.is_some()
                && let Some(l) = live.as_deref()
            {
                l.wake(machine);
            }
            match job {
                Some(n) => {
                    app.answer_unreached = unreached;
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
        // q waited for a hand-over, which is done: Kerdr quits by itself (intent 0591).
        if app.quit_due() && app.quit_now() {
            return Ok(());
        }
        // Changes to cards go to the runner, which starts each in its turn (intents 0416, 0445).
        run_ready(app, &runner);
        // Followed work: how it stands, asked on threads of their own (intent 0591).
        ask_progress(app, &action_tx);
        if let Some(l) = live.as_deref() {
            for machine in start_reads.due(&app.start_awaited_on(), Instant::now()) {
                l.wake_feed(machine);
            }
        }
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
                &bg,
                open.machine,
                open.session,
                open.workspace,
                Some(open.label),
                open.tab,
            );
            if outcome == Some(Outcome::Next) {
                go_on_to_next(
                    terminal,
                    flags,
                    app,
                    live.as_deref_mut(),
                    inbox,
                    client_config.as_ref(),
                    &bg,
                    open.machine,
                    &open.from,
                );
            }
        }
        // A backlog item's text you just asked for with e: your editor, full screen (intent 0414).
        if let Some(key) = app.editor_due() {
            edit_text(terminal, flags, app, live.as_deref(), &bg, &key)?;
        }
        // A resend to a record's real home, or a card whose workspace closed as you went in
        // reopening (intent 0411), whatever account action runs (intent 0445).
        if !app.demo
            && let Action::Mutate { machine, request } = app.next_owed()
        {
            start_action(app, &action_tx, &runner, machine, request);
        }
        // s: the picker once a machine's accounts have come, and an account picked for a card
        // that's owed it (brought back, its backend brought up to date first: intent 0591).
        app.accounts_wait_due();
        ask_update(app, &action_tx);
        if !app.demo
            && let Action::Mutate { machine, request } = app.account_owed()
        {
            start_action(app, &action_tx, &runner, machine, request);
        }
        // `R`'s feeds, looked for first: what's found here is applied by the time it says so.
        let fed = app.refreshed(inbox);
        for ev in inbox.take() {
            app.apply(ev);
        }
        app.settle_refresh(&fed);
        holds.borrow_mut().sync(app);
        // e that waited for a write-up or a save: its text is read, or opens (intent 0591,
        // K1-78); a start kept waiting for an editor that won't open goes (K1-82).
        if let Action::Mutate { machine, request } = app.edit_after_due().unwrap_or(Action::None) {
            start_action(app, &action_tx, &runner, machine, request);
        }
        app.edit_pauses_due();
        run_ready(app, &runner);
        match app.pending_jump_ready() {
            Some(Action::Jump(key)) => jump_cards(
                terminal,
                flags,
                app,
                live.as_deref_mut(),
                inbox,
                client_config.as_ref(),
                &bg,
                key,
                true,
            ),
            Some(Action::JumpWorkspace {
                machine,
                session,
                id,
            }) => {
                // The tab it was asked for, as a reopen's answer named it (Codex's review).
                let (label, tab) = app.pending_go.take().unzip();
                if let Some(mi) = app.machine_index(&machine) {
                    jump_workspace(
                        terminal,
                        flags,
                        app,
                        live.as_deref_mut(),
                        client_config.as_ref(),
                        &bg,
                        mi,
                        session,
                        id,
                        label,
                        tab.flatten(),
                    );
                }
            }
            _ => {}
        }
        // ⏎ on a card whose start was running: its workspace has opened (intent 0491).
        let remembered = app.enter_when_open.clone();
        match app.enter_when_open_due() {
            Some(Action::Jump(key)) => jump_cards(
                terminal,
                flags,
                app,
                live.as_deref_mut(),
                inbox,
                client_config.as_ref(),
                &bg,
                key,
                true,
            ),
            Some(Action::JumpWorkspace {
                machine,
                session,
                id,
            }) => {
                if let Some(mi) = app.machine_index(&machine) {
                    let outcome = jump_workspace(
                        terminal,
                        flags,
                        app,
                        live.as_deref_mut(),
                        client_config.as_ref(),
                        &bg,
                        mi,
                        session,
                        id,
                        None,
                        None,
                    );
                    if outcome == Some(Outcome::Next)
                        && let Some(from) = remembered
                    {
                        go_on_to_next(
                            terminal,
                            flags,
                            app,
                            live.as_deref_mut(),
                            inbox,
                            client_config.as_ref(),
                            &bg,
                            mi,
                            &from,
                        );
                    }
                }
            }
            _ => {}
        }
        let size = terminal.size()?;
        app.width = size.width;
        app.height = size.height;
        app.keep_box_errors();
        terminal.draw(|f| crate::ui::draw(f, app))?;
        let wait = if back_at.is_some_and(|at| at.elapsed() < READ_LANDS) {
            crate::app::KEY_WAIT_COMING
        } else {
            app.key_wait()
        };
        if !event::poll(wait)? {
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
                if let Some(read) = app.begin_image_load(&source) {
                    let reading = Reading {
                        workers: &readers,
                        load: |source: crate::images::Source, room| source.load(room),
                    };
                    start_read(app, &image_tx, read, source, reading);
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
                // Only what's being handed over holds Kerdr open, and only for the moment that
                // takes: it quits by itself once it's done, and Esc stays. A start or move whose
                // command has the whole request carries on without Kerdr, starts waiting for
                // their write-up go into the hold file, and your words into the words file, so
                // nothing else stops q (intent 0591, release 4).
                if app.quit_key() {
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
                        &bg,
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
                    &bg,
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
                        // and all, so R brings the board up to date (intent 0445), and the board
                        // says when they have (intent 0491).
                        let (reading, trying) = app.ask_refresh(Instant::now());
                        for i in reading {
                            l.wake_feed(i);
                        }
                        for i in trying {
                            l.wake(i);
                        }
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
                app.away += 1;
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
                app.away += 1;
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
                if app.machines[i].machine.is_local() {
                    // This machine: its cards leave the board or come back, and it's read all
                    // along, for its accounts and the actions it runs (intent 0591, K1-10).
                    let machine_name = app.machines[i].machine.name.clone();
                    if app.machines[i].machine.hidden {
                        state.hidden.retain(|h| {
                            !h.eq_ignore_ascii_case(&name) && !h.eq_ignore_ascii_case(&machine_name)
                        });
                        app.machines[i].machine.hidden = false;
                        app.say(format!("Showing {name}'s cards again."), Tone::Info);
                    } else {
                        state.hidden.push(machine_name);
                        app.machines[i].machine.hidden = true;
                        app.say(
                            format!(
                                "Hid {name}'s cards. Kerdr still reads it, for its accounts and the actions it runs; x in machines shows them again."
                            ),
                            Tone::Info,
                        );
                    }
                    if !app.demo {
                        state.save();
                    }
                    app.rebuild();
                    continue;
                }
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
/// the request without credentials, what came back, and what its command printed on standard
/// error, for the error log.
type ActionResult = (
    Option<u64>,
    usize,
    serde_json::Value,
    Result<crate::actions::Response, String>,
    String,
    Option<crate::actions::Unreached>,
);

/// What comes back to the board's thread: a change's command has started, its command has the
/// whole request, a drop was answered (whether it came before the change started), or an action
/// answered.
enum Answer {
    Started(u64),
    Handed(u64),
    Dropped(u64, bool),
    Done(Box<ActionResult>),
    /// A progress request's answer: the followed work's token, its machine, and what came back.
    Progress(String, usize, Result<crate::actions::Response, String>),
    /// The answer to a request that asked an old backend to bring its ark-sdlc up to date, for
    /// an account picked with s (intent 0591).
    Update(usize, u64, Result<crate::actions::Response, String>),
    /// A change (by its number, or an account request) is being sent again by itself, with the
    /// same op, and why (intent 0591).
    Resending(Option<u64>, usize, crate::actions::Resend),
    /// A start that waited for its write-up goes ahead from the words as they are: it failed.
    NotWrittenUp(u64),
    /// A change that waited for its write-up was taken out unstarted: its card was set aside,
    /// finished or started meanwhile (Codex's review 60).
    Changed(u64),
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
        /// A start that waits for its intent's write-up, as that machine's feed shows it.
        gated: bool,
        /// Its machine's backend keeps ops: an answer lost on the way is asked for again.
        resends: bool,
        /// Its card's `rev` when the key was pressed, which a resend carries (intent 0591).
        rev: Option<String>,
        /// For one waiting for its write-up, its card's footing when the key was pressed: it
        /// doesn't go once that has moved (Codex's review 60).
        guard: Option<crate::resolve::Footing>,
        /// Sent again after its followed work ended refused with `retry`: how long it waits
        /// first, holding its record's turn, and the answer that stands if it can't go in time
        /// (intent 0591).
        after: Option<Box<crate::actions::After>>,
    },
    Done(u64),
    /// A change dropped before it started (Esc on a start waiting for its write-up).
    Drop(u64),
    /// A start waiting for its write-up kept waiting whatever that says (true), or let go to it
    /// again: e is open on its card (intent 0591, K1-82).
    Pause(u64, bool),
    /// A machine's feed as its reader brought it: each record by id, with its intake, and with
    /// its footing and `rev` (Codex's review 60).
    Feed {
        machine: usize,
        intakes: std::collections::HashMap<String, Option<String>>,
        footings: crate::resolve::Footings,
    },
}

/// A change waiting its turn in the runner: its machine, where and how it's sent, its request,
/// whether its backend keeps ops, its card's `rev`, and the wait before it's sent again after a
/// followed end.
type Waiting = (
    usize,
    Machine,
    String,
    serde_json::Value,
    bool,
    Option<String>,
    Option<Box<crate::actions::After>>,
);

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
            let mut waiting: std::collections::HashMap<u64, Waiting> = Default::default();
            // Each machine's newest feed, by record: a start that waits for its write-up checks
            // it as soon as it's asked for, not only at the next read.
            let mut seen: std::collections::HashMap<
                usize,
                std::collections::HashMap<String, Option<String>>,
            > = Default::default();
            // And each record's footing and `rev` there, which a change that waited for its
            // write-up is held to, and goes with (Codex's review 60).
            let mut footed: std::collections::HashMap<usize, crate::resolve::Footings> =
                Default::default();
            let mut gated_ones: std::collections::HashSet<u64> = Default::default();
            for msg in rx {
                let mut failed = vec![];
                let mut moved = vec![];
                match msg {
                    ToRunner::Run {
                        n,
                        machine,
                        key,
                        target,
                        command,
                        request,
                        gated,
                        resends,
                        rev,
                        guard,
                        after,
                    } => {
                        if gated {
                            queue.push_gated(n, machine, key, &request);
                            queue.guard(n, guard);
                            gated_ones.insert(n);
                            if let Some(footings) = footed.get(&machine) {
                                moved = queue.moved(machine, footings);
                            }
                            if let Some(intakes) = seen.get(&machine) {
                                failed = queue.written(machine, intakes);
                            }
                            if let Some(footings) = footed.get(&machine) {
                                queue.released(machine, footings);
                            }
                        } else {
                            queue.push(n, machine, key, &request);
                        }
                        waiting.insert(n, (machine, target, command, request, resends, rev, after));
                    }
                    ToRunner::Done(n) => queue.done(n),
                    // Let go again, it's judged by its machine's newest read at once.
                    ToRunner::Pause(n, paused) => {
                        queue.pause(n, paused);
                        let machine = waiting.get(&n).map(|w| w.0);
                        if let (false, Some(machine)) = (paused, machine)
                            && let Some(intakes) = seen.get(&machine)
                        {
                            failed = queue.written(machine, intakes);
                            if let Some(footings) = footed.get(&machine) {
                                queue.released(machine, footings);
                            }
                        }
                    }
                    // Said either way, so the board keeps a start Esc came too late for.
                    ToRunner::Drop(n) => {
                        let dropped = queue.drop_unstarted(n);
                        if dropped {
                            waiting.remove(&n);
                            gated_ones.remove(&n);
                        }
                        let _ = answers.send(Answer::Dropped(n, dropped));
                    }
                    ToRunner::Feed {
                        machine,
                        mut intakes,
                        footings,
                    } => {
                        moved = queue.moved(machine, &footings);
                        failed = queue.written(machine, &intakes);
                        queue.released(machine, &footings);
                        queue.forget_rewriting(machine, &mut intakes);
                        queue.read_passed(machine);
                        seen.insert(machine, intakes);
                        footed.insert(machine, footings);
                    }
                }
                // Its card was set aside, finished or started meanwhile: it never goes, and the
                // board says so.
                for n in moved {
                    waiting.remove(&n);
                    gated_ones.remove(&n);
                    let _ = answers.send(Answer::Changed(n));
                }
                // Its write-up failed: it starts from his words as they are, and the board says
                // so (intent 0591).
                for n in failed {
                    let _ = answers.send(Answer::NotWrittenUp(n));
                }
                for n in queue.startable() {
                    let Some((machine, target, command, mut request, resends, mut rev, after)) =
                        waiting.remove(&n)
                    else {
                        continue;
                    };
                    // One that waited for its write-up goes with its card's `rev` as the read
                    // that let it go shows it, from a backend that holds, never a later read's:
                    // changed after that, its backend asks rather than does it (Codex's reviews
                    // 60 and 62).
                    if gated_ones.remove(&n)
                        && request.get("rev").is_none()
                        && let Some(then) = queue.released_rev(n)
                    {
                        request["rev"] = serde_json::Value::String(then.clone());
                        rev = Some(then);
                    }
                    let _ = answers.send(Answer::Started(n));
                    let (answers, back) = (answers.clone(), back.clone());
                    std::thread::spawn(move || {
                        // Its command having the whole request lets Kerdr quit (intent 0445).
                        let handed = answers.clone();
                        let told = answers.clone();
                        // Sent again by itself, same op, while it holds its record's turn: what
                        // comes after it for the same card waits (intent 0591). One sent again
                        // after its followed work ended refused waits that refusal's `retry` first.
                        let answered = crate::actions::run_resending_after(
                            &target,
                            &command,
                            &request,
                            rev.as_deref(),
                            after.map(|a| *a),
                            Some(Box::new(move || {
                                let _ = handed.send(Answer::Handed(n));
                            })),
                            resends,
                            &mut |r| {
                                let _ = told.send(Answer::Resending(Some(n), machine, r.clone()));
                            },
                            &std::thread::sleep,
                        );
                        let metadata = crate::actions::request_metadata(&request);
                        drop(request);
                        // A refusal naming another home holds its record's place until the board
                        // has sent it on there, or not (intent 0416).
                        let held = answered
                            .result
                            .as_ref()
                            .is_ok_and(crate::actions::Response::sends_on);
                        let _ = answers.send(Answer::Done(Box::new((
                            Some(n),
                            machine,
                            metadata,
                            answered.result,
                            answered.stderr,
                            answered.unreached,
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

/// What the board's thread hands work on through, which a jump into herdr needs too.
struct Bg<'a> {
    images: &'a mpsc::Receiver<(u64, crate::images::Loading)>,
    answers: &'a mpsc::Sender<Answer>,
    runner: &'a Runner,
    inbox: &'a Inbox,
    holds: &'a RefCell<Holds>,
}

/// The longest a jump into herdr waits for the image a sent new intent waits for: a clipboard read
/// may take two commands of five seconds each.
const IMAGE_WAIT: Duration = Duration::from_secs(12);

/// Puts images read for the box in, and sends a new intent that waited for its image (intent
/// 0445). With `wait`, waits up to that long for that image first.
fn send_loaded_images(app: &mut App, bg: &Bg, wait: Option<Duration>) {
    let until = wait.map(|w| Instant::now() + w);
    loop {
        let got = match until {
            Some(until) if app.send_after_load.is_some() => {
                let left = until.saturating_duration_since(Instant::now());
                bg.images.recv_timeout(left).ok()
            }
            _ => bg.images.try_recv().ok(),
        };
        let Some((id, loading)) = got else {
            // What waited for a question's answer, or for a read past its deadline, goes now
            // (K1-35; Codex's review 85, 8).
            if let Action::Mutate { machine, request } = app.apply_reads() {
                start_action(app, bg.answers, bg.runner, machine, request);
            }
            return;
        };
        if let Action::Mutate { machine, request } = app.images_loaded(id, loading) {
            start_action(app, bg.answers, bg.runner, machine, request);
        }
    }
}

/// How a read for the box is done: on `workers`, at most images::MOST_READERS at once, read
/// with `load`.
pub(crate) struct Reading<'a, L> {
    pub workers: &'a crate::images::Workers,
    pub load: L,
}

/// Starts a read begun by begin_image_load; what it found comes back on `tx`. Every read runs on
/// a worker counted until it really ends, the board having let it go at its deadline or not; with
/// images::MOST_READERS running, the paste is refused and said (Codex's review 86, 6).
pub(crate) fn start_read<L>(
    app: &mut App,
    tx: &mpsc::Sender<(u64, crate::images::Loading)>,
    (id, room): (u64, crate::images::Room),
    source: crate::images::Source,
    reading: Reading<'_, L>,
) where
    L: FnOnce(crate::images::Source, crate::images::Room) -> crate::images::Loading
        + Send
        + 'static,
{
    use crate::images::{Loading, MOST_READERS};
    let Reading { workers, load } = reading;
    let tx = tx.clone();
    if !workers.start(MOST_READERS, move || {
        let _ = tx.send((id, load(source, room)));
    }) {
        let _ = app.images_loaded(
            id,
            Loading::refused(crate::images::too_many_reads(workers.running())),
        );
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
    // One at a time: one made some other way while another is on its way waits its turn, never
    // dropped (Codex's fourth look). Kerdr's own are made by send_account, which keeps them so.
    if app.busy {
        app.account_queue.push_back((machine, request));
        return;
    }
    let Some(command) = app.cfg.actions.command.clone() else {
        return;
    };
    let Some(target) = app.machines.get(machine).map(|m| m.machine.clone()) else {
        return;
    };
    let tx = tx.clone();
    // Its own op, kept for Kerdr's own resends of it (intent 0591), or the one it had when it's
    // sent again after its answer was lost (Codex's review 60).
    let request = app.account_op(request);
    // Every request to a backend that fixes input says so, an account's too (Codex's review 85, 6).
    let request = app.with_fix_input(machine, request);
    let resends = app.resends_to(machine);
    app.busy = true;
    app.busy_id = request["id"].as_str().map(str::to_string);
    app.busy_since = Some(Instant::now());
    app.busy_label = app.working_on(&request);
    if request["action"] != "account-signin" {
        app.say(app.busy_label.clone(), Tone::Info);
    }
    std::thread::spawn(move || {
        let told = tx.clone();
        let answered = crate::actions::run_resending(
            &target,
            &command,
            &request,
            None,
            resends,
            &mut |r| {
                let _ = told.send(Answer::Resending(None, machine, r.clone()));
            },
            &std::thread::sleep,
        );
        let metadata = crate::actions::request_metadata(&request);
        drop(request);
        let _ = tx.send(Answer::Done(Box::new((
            None,
            machine,
            metadata,
            answered.result,
            answered.stderr,
            answered.unreached,
        ))));
    });
}

/// Sends the progress requests due for followed work, each on a thread of its own, so a slow
/// machine never holds the board; their answers come back on the channel (intent 0591). They only
/// read, so nothing waits for them, and q doesn't either.
fn ask_progress(app: &mut App, tx: &mpsc::Sender<Answer>) {
    if app.demo {
        return;
    }
    for (token, machine, request) in app.progress_due() {
        let command = app.cfg.actions.command.clone();
        let target = app.machines.get(machine).map(|m| m.machine.clone());
        let (Some(command), Some(target)) = (command, target) else {
            app.progress_answered(&token, Err(crate::actions::NO_BACKEND.into()));
            continue;
        };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = crate::actions::run(&target, &command, &request);
            let _ = tx.send(Answer::Progress(token, machine, result));
        });
    }
}

/// Asks an old backend to bring its ark-sdlc up to date, on a thread of its own, when an account
/// picked with s waits for it: a read its backend pulls before (intent 0591).
fn ask_update(app: &mut App, tx: &mpsc::Sender<Answer>) {
    if app.demo {
        return;
    }
    let Some((machine, request, n)) = app.update_due() else {
        return;
    };
    let command = app.cfg.actions.command.clone();
    let target = app.machines.get(machine).map(|m| m.machine.clone());
    let (Some(command), Some(target)) = (command, target) else {
        app.update_answered(machine, n, Err(crate::actions::NO_BACKEND.into()));
        return;
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = crate::actions::run(&target, &command, &request);
        let _ = tx.send(Answer::Update(machine, n, result));
    });
}

/// Hands every new change to a card to the runner, which starts each in its turn.
fn run_ready(app: &mut App, runner: &Runner) {
    // A new intent waiting for the words file goes once it's written there (Codex's review 63, 2).
    app.keep_waiting_words();
    // An edit open in Kerdr's own box is kept as it's typed (Codex's review 85, 3).
    app.keep_open_edit();
    for n in app.resolver.take_dropped() {
        let _ = runner.tx.send(ToRunner::Drop(n));
    }
    // One from the hold file is written as possibly gone ahead first (Codex's review 65, 2).
    for (n, machine, request, gated) in app.hand_over() {
        let command = app.cfg.actions.command.clone();
        let target = app.machines.get(machine).map(|m| m.machine.clone());
        let (Some(command), Some(target)) = (command, target) else {
            let metadata = crate::actions::request_metadata(&request);
            app.finish_job(n, metadata, Err(crate::actions::NO_BACKEND.into()));
            continue;
        };
        let key = app.resolver.key_of(n);
        let resends = app.resends_to(machine);
        let rev = app.resolver.rev_of(n);
        // Its card's footing now, in the pass its key was taken in.
        let guard = gated.then(|| app.gate_footing(n)).flatten();
        // Sent again after its followed work ended refused with `retry` (intent 0591).
        let after = app.resolver.take_again(n).map(Box::new);
        let _ = runner.tx.send(ToRunner::Run {
            n,
            machine,
            key,
            target,
            command,
            request,
            gated,
            resends,
            rev,
            guard,
            after,
        });
    }
    // After the changes just handed over: a save of a text reaches the runner before the start it
    // lets go again, so that start waits behind the save (intent 0591, K1-82).
    for (n, paused) in app.resolver.take_pauses() {
        let _ = runner.tx.send(ToRunner::Pause(n, paused));
    }
}

/// Opens a backlog item's text in your editor, handing it the terminal as a jump hands it to
/// herdr, and sends the save when the text changed: through the action resolution tool, so the
/// card shows it and the board is yours again the moment the editor closes (intents 0414, 0416).
/// Changes already with the runner go on while the editor is open (intent 0445).
fn edit_text(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    live: Option<&Live>,
    bg: &Bg,
    key: &str,
) -> anyhow::Result<()> {
    let Some((id, file, intro, marker)) = app.edit_file(key) else {
        return Ok(());
    };
    // One that opens a window of its own is told to wait for it, and that's said (K1-84).
    let candidates = crate::edit::candidates_here(app.cfg.editor.as_deref());
    let set = crate::edit::set_editor_here(app.cfg.editor.as_deref());
    app.editor_note(key, set.as_deref());
    send_before_leaving(
        terminal,
        app,
        bg,
        "Sending your new intent before your editor opens: its image is still being read…",
    );
    app.away += 1;
    // A hang-up while the editor is open waits until its file is gone, then ends Kerdr.
    let stops = crate::edit::HoldStops::new();
    let ended = crate::edit::edit_by(&id, &file, marker.as_deref(), |path| {
        jump::hand_over(terminal, flags, || {
            crate::edit::launch(&candidates, path, &intro)
        })
    });
    if let Some(signal) = stops.held() {
        // Its save goes first if its machine answers, else it's held for it or kept in your words
        // file, and the line Kerdr ends with says which: never lost (intent 0591, K3-31).
        let action = app.edit_ended(key, ended);
        let command = app.cfg.actions.command.clone();
        let targets: Vec<_> = app.machines.iter().map(|m| m.machine.clone()).collect();
        let went = app.save_before_stopping(key, action, &mut |machine, request| match (
            &command,
            targets.get(machine),
        ) {
            (Some(command), Some(target)) => crate::actions::run(target, command, request),
            _ => Err(crate::actions::NO_BACKEND.into()),
        });
        let unkept = app
            .keep_on_quit()
            .err()
            .map(|why| format!(" {why}, so what it keeps for the next Kerdr was lost."))
            .unwrap_or_default();
        let went = went.map(|w| format!(" {w}")).unwrap_or_default() + &unkept;
        anyhow::bail!(
            "Kerdr was asked to stop (signal {signal}) while your editor was open.{went}"
        );
    }
    drop(stops);
    // A changed text: its machine is read afresh first, so one that stopped answering while the
    // editor was open has the save held, as any change for it is (the end-to-end check's P1).
    if matches!(ended, crate::edit::Ended::Wrote { touched: true, .. })
        && let Some((machine, reference)) = app.saved_to(key)
    {
        let checking = format!(
            "Checking {} is answering before {reference}'s new text goes…",
            app.machines[machine].name()
        );
        read_before_saving(app, live, bg.inbox, machine, EDIT_CHECK_WAIT, &mut |app| {
            app.say(checking.clone(), Tone::Info);
            let _ = terminal.draw(|f| crate::ui::draw(f, app));
        });
        app.unsay(&checking);
    }
    if let Action::Mutate { machine, request } = app.edit_ended(key, ended) {
        start_action(app, bg.answers, bg.runner, machine, request);
    }
    Ok(())
}

/// Before a save from a closed editor goes: what the machines said while the editor was open,
/// which nobody applied, is applied now, and the save's machine, if it still reads as live, is read
/// again, `wait` at most, with `waiting` told first. The save then goes to a machine that answered
/// a moment ago, or is held for one that didn't (the end-to-end check's P1).
fn read_before_saving(
    app: &mut App,
    live: Option<&Live>,
    inbox: &Inbox,
    machine: usize,
    wait: Duration,
    waiting: &mut dyn FnMut(&mut App),
) {
    for ev in inbox.take() {
        app.apply(ev);
    }
    let Some(l) = live else { return };
    let polled = l.pollers.get(machine).is_some_and(Option::is_some);
    if !polled || !app.machines.get(machine).is_some_and(|m| m.conn.is_live()) {
        return;
    }
    waiting(app);
    let since = Instant::now();
    l.wake(machine);
    inbox.wait_since(&[machine], since, wait);
    for ev in inbox.take() {
        app.apply(ev);
    }
}

/// Before the board hands the terminal over (into herdr, or to your editor): a new intent Enter
/// sent while its image was read goes first, so it isn't left waiting until he's back on the
/// board (review of intent 0445).
fn send_before_leaving(terminal: &mut DefaultTerminal, app: &mut App, bg: &Bg, sending: &str) {
    if app.send_after_load.is_some() {
        app.say(sending, Tone::Info);
        let _ = terminal.draw(|f| crate::ui::draw(f, app));
        send_loaded_images(app, bg, Some(IMAGE_WAIT));
        app.unsay(sending);
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
    bg: &Bg,
    mi: usize,
    session: String,
    id: String,
    label: Option<String>,
    tab: Option<String>,
) -> Option<Outcome> {
    let machine = app.machines[mi].name();
    let label = label
        .or_else(|| {
            app.board
                .workspaces
                .iter()
                .find(|w| w.machine == machine && w.session == session && w.id == id)
                .map(|w| w.label.clone())
        })
        .unwrap_or_else(|| id.clone());
    // Its machine isn't answering: Kerdr waits for it and goes in then, as ⏎ on a card does
    // (intent 0591, K3-09).
    if app.wait_for_workspace(mi, &session, &id, &label, tab.clone()) {
        if let Some(l) = live.as_deref() {
            l.wake(mi);
        }
        return None;
    }
    // Its session didn't answer its last read: Kerdr waits for it rather than refuse (intent 0591).
    if app.stuck_refusal(mi, &session, &label).is_some() {
        app.wait_for_stuck(mi, &session.clone(), &label, None, Some((session, id, tab)));
        if let Some(l) = live.as_deref() {
            l.wake(mi);
        }
        return None;
    }
    // Going in yourself: a start or reopen still on its way for it takes you nowhere later.
    app.went_into_workspace(mi, &session, &id);
    let via = via_for(app, mi, &session);
    let j = workspace_jump(app, mi, session, id, label, tab, via);
    let outcome = run_jump(terminal, flags, app, mi, &j, client_config, bg);
    if let Some(l) = live.as_deref() {
        // What herdr shows first, then the feed, as in read_again (intent 0491).
        l.wake(mi);
        l.wake_feed(mi);
    }
    // Only a jump that got no answer again remembers that it waited once (K3-12).
    if !matches!(outcome, Outcome::Unanswered(_)) {
        app.answer_waited = None;
    }
    match &outcome {
        // herdr there, or its machine, didn't answer: Kerdr waits for it and goes in then
        // (intent 0591, K3-12, K3-13).
        Outcome::Unanswered(why) => {
            app.wait_for_answer(
                mi,
                &j.session,
                &j.label,
                None,
                Some((j.session.clone(), j.workspace.clone(), j.tab.clone())),
                why,
            );
            if let Some(l) = live.as_deref() {
                l.wake(mi);
            }
        }
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
        others: app.others_needing_you(None),
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
    mi: usize,
    j: &Jump,
    client_config: Option<&PathBuf>,
    bg: &Bg,
) -> Outcome {
    send_before_leaving(
        terminal,
        app,
        bg,
        "Sending your new intent before going in: its image is still being read…",
    );
    let opening = jump::opening(&j.label, &j.machine.name, j.machine.ssh.is_none());
    app.away += 1;
    app.say(opening.clone(), Tone::Info);
    let _ = terminal.draw(|f| crate::ui::draw(f, app));
    let held = bg.holds.borrow_mut().entering(
        app,
        Spot {
            place: Place {
                machine: mi,
                session: j.session.clone(),
            },
            workspace: j.workspace.clone(),
        },
    );
    let mut opened = false;
    let mut keep_up = KeepUp {
        app,
        bg,
        said: None,
    };
    let outcome = jump::run(
        terminal,
        flags,
        j,
        client_config
            .map(|p| p.as_path())
            .unwrap_or(std::path::Path::new("")),
        held.as_ref(),
        &mut keep_up,
        &mut opened,
    );
    let waited = keep_up.said.take();
    bg.holds.borrow_mut().left(app, opened);
    app.unsay(&opening);
    if let Some(line) = waited {
        app.unsay(&line);
    }
    if let Some(note) = jump::take_note() {
        app.say(note, Tone::Info);
    }
    outcome
}

/// The board while a jump runs. While you're in there it keeps up, unseen, so what's held follows
/// what waits on you and the minute after you left another session still ends on time; while
/// herdr is asked again, it says why, and Esc stops it (intent 0591, K3-12).
struct KeepUp<'a, 'b> {
    app: &'a mut App,
    bg: &'a Bg<'b>,
    /// The last line said while it waited, taken down once the jump is over.
    said: Option<String>,
}

impl jump::Meanwhile for KeepUp<'_, '_> {
    fn tick(&mut self) {
        for ev in self.bg.inbox.take() {
            self.app.apply(ev);
        }
        self.bg.holds.borrow_mut().sync(self.app);
    }

    fn waiting(&mut self, terminal: &mut DefaultTerminal, line: &str) -> bool {
        if let Some(before) = self.said.replace(line.to_string()) {
            self.app.unsay(&before);
        }
        self.app.say(line, Tone::Info);
        let app = &*self.app;
        let _ = terminal.draw(|f| crate::ui::draw(f, app));
        jump::esc_pressed()
    }
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

/// Ctrl+Space Space from a workspace Kerdr went into by itself: on to the next card that needs you.
/// Chosen from the board as it is now, as jump_cards does, not as it was on the way in.
#[allow(clippy::too_many_arguments)]
fn go_on_to_next(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    live: Option<&mut Live>,
    inbox: &Inbox,
    client_config: Option<&PathBuf>,
    bg: &Bg,
    machine: usize,
    from: &str,
) {
    read_again(app, live.as_deref(), inbox, machine, true);
    match app.next_needs_you_after(from, &[]) {
        Some(next) => jump_cards(
            terminal,
            flags,
            app,
            live,
            inbox,
            client_config,
            bg,
            next,
            false,
        ),
        None => app.say("Nothing else needs you right now.", Tone::Good),
    }
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
    bg: &Bg,
    first: String,
    mut yours: bool,
) {
    let mut key = first;
    // Cards Kerdr picked whose workspace turned out to have closed: skipped from then on.
    let mut closed_on_the_way: Vec<String> = Vec::new();
    // Gone once more into the workspace the board named for a card after its first had closed:
    // never a third time round (intent 0591).
    let mut retried = false;
    loop {
        let Some(card) = app.card(&key).cloned() else {
            return;
        };
        let (Some(session), Some(workspace)) = (card.session.clone(), card.workspace_id.clone())
        else {
            // A ⏎ that waited (for its machine, for a start, on to the next card) makes plain ⏎'s
            // choice: start it, reopen it, or move it into herdr. Never a dead end (intent 0591).
            app.sel = Selection {
                col: app.col_of(&card),
                key: Some(key.clone()),
            };
            plain_choice(terminal, flags, app, live, client_config, bg, &key);
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
        // Its session didn't answer its last read: wait for it, up to a minute, rather than refuse
        // (intent 0591).
        if app.stuck_refusal(mi, &session, &card.reference).is_some() {
            app.wait_for_stuck(mi, &session, &card.reference, Some(&key), None);
            if let Some(l) = live.as_deref() {
                l.wake(mi);
            }
            return;
        }
        app.sel = Selection {
            col: app.col_of(&card),
            key: Some(key.clone()),
        };
        // Going in yourself: a start or reopen still on its way for it takes you nowhere later.
        app.went_into(&key);
        let others = app.others_needing_you(Some(&key));
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
        let outcome = run_jump(terminal, flags, app, mi, &j, client_config, bg);
        // herdr there, or its machine, didn't answer however many times it was asked: Kerdr waits
        // for it and goes in then, as for a machine that isn't answering (intent 0591, K3-12,
        // K3-13). Only a jump that got no answer again remembers that it waited once.
        if let Outcome::Unanswered(why) = &outcome {
            app.wait_for_answer(mi, &j.session, &card.reference, Some(&key), None, why);
            if let Some(l) = live.as_deref() {
                l.wake(mi);
            }
            return;
        }
        app.answer_waited = None;
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
                    match app.after_closed(&key, card.workspace_id.as_deref(), retried) {
                        Action::Jump(_) => {
                            retried = true;
                            continue;
                        }
                        Action::Mutate { machine, request } => {
                            start_action(app, bg.answers, bg.runner, machine, request)
                        }
                        // A bare workspace whose conversation the board knew: asked whether to
                        // resume it, where its backend can (intent 0591, K3-03).
                        _ => {
                            if !app.offer_resume_closed(&card) {
                                app.say(closed(&card.workspace_name()), Tone::Warn)
                            }
                        }
                    }
                }
                return;
            }
            Outcome::Closed => closed_on_the_way.push(key.clone()),
            // Its herdr session has stopped: work under way or done reopens, which starts the
            // session again; otherwise say so.
            Outcome::Stopped if yours => {
                if !app.closed_under_you(&key, card.workspace_id.as_deref()) {
                    match app.after_closed(&key, card.workspace_id.as_deref(), retried) {
                        Action::Jump(_) => {
                            retried = true;
                            continue;
                        }
                        Action::Mutate { machine, request } => {
                            start_action(app, bg.answers, bg.runner, machine, request)
                        }
                        _ => app.say(stopped(&j.session, &app.machines[mi].name()), Tone::Warn),
                    }
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
            // Taken above: Kerdr waits for an answer.
            Outcome::Unanswered(_) => return,
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

/// Plain ⏎'s choice for a card with no workspace open, made for a ⏎ that waited: what jump_action
/// says to do, done (intent 0591).
#[allow(clippy::too_many_arguments)]
fn plain_choice(
    terminal: &mut DefaultTerminal,
    flags: &TerminalFlags,
    app: &mut App,
    live: Option<&mut Live>,
    client_config: Option<&PathBuf>,
    bg: &Bg,
    key: &str,
) {
    match app.jump_action(key.to_string()) {
        Action::Mutate { machine, request } => {
            start_action(app, bg.answers, bg.runner, machine, request)
        }
        Action::JumpWorkspace {
            machine,
            session,
            id,
        } => {
            if let Some(mi) = app.machine_index(&machine) {
                jump_workspace(
                    terminal,
                    flags,
                    app,
                    live,
                    client_config,
                    bg,
                    mi,
                    session,
                    id,
                    None,
                    None,
                );
            }
        }
        _ => {}
    }
}

/// Reads the card's machine again and applies what's waiting. Choosing the next card, it reads
/// every live machine and waits a moment (FRESH_WAIT at most) for what they say, so the choice is
/// made from the board as it is now, not as it was when you went into herdr. A machine in the
/// middle of a feed read can't say anything new that soon, so it isn't waited for (intent 0491).
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
                // What herdr shows comes first and fast, so the card you just answered stops
                // saying it needs you; the feed follows on its own (intents 0445 and 0491).
                l.wake(i);
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
    if let Err(why) = crate::accounts::open_signin_url(url) {
        app.say(no_browser(&why, to_clipboard(url)), Tone::Warn);
    }
}

/// What the board says when no browser took a sign-in link: it went to the terminal's clipboard by
/// itself, as c sends it (intent 0591, K3-30), and the link isn't in the words, which the error log
/// keeps.
fn no_browser(why: &str, copied: bool) -> String {
    if copied {
        format!("No browser here ({why}); the sign-in link is on your clipboard.")
    } else {
        format!(
            "No browser here ({why}). Copy the link instead (c, or l where the page takes a code)."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo's new-intent box, open, as a paste finds it.
    fn box_open() -> App {
        let mut app = App::new(
            crate::config::Config::machine_split(),
            crate::demo::machines(false),
        );
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(matches!(app.mode, crate::app::Mode::Compose { .. }));
        app
    }

    fn words_in(app: &App) -> (String, usize) {
        match &app.mode {
            crate::app::Mode::Compose { draft, .. } => (draft.words.clone(), draft.images.len()),
            m => panic!("the box isn't open: {m:?}"),
        }
    }

    /// What the reads send back, put in as the board's loop puts it, until none comes for `wait`.
    fn take_back(
        app: &mut App,
        rx: &mpsc::Receiver<(u64, crate::images::Loading)>,
        wait: Duration,
    ) {
        while let Ok((id, loading)) = rx.recv_timeout(wait) {
            let _ = app.images_loaded(id, loading);
        }
    }

    /// Waits up to five seconds for `done`.
    fn soon(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while !done() {
            if Instant::now() > until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn reads_that_stall_are_counted_until_they_end_and_one_past_the_most_is_refused_and_said() {
        // Codex's review 86, 6: a mount that doesn't answer, and he pastes its image's path again
        // and again. The board stops waiting for each at its deadline, but its read is counted
        // until it really ends: past the most at once, a paste is refused and said, a clipboard
        // paste too, with nothing more left running; once the mount answers, pastes go in again.
        use crate::images::{Loaded, Loading, MOST_READERS, Source, Workers};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        use std::sync::{Arc, Condvar, Mutex};
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (stalled, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let workers = Workers::default();
        let (tx, rx) = mpsc::channel();
        let mut app = box_open();
        let path = || Source::Files(vec!["/mnt/slow/shot.png".into()]);
        let stall = || {
            let (gate, stalled, most) = (gate.clone(), stalled.clone(), most.clone());
            move |_source: Source, _room| {
                let now = stalled.fetch_add(1, SeqCst) + 1;
                most.fetch_max(now, SeqCst);
                let (open, opened) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = opened.wait(open).unwrap();
                }
                stalled.fetch_sub(1, SeqCst);
                Loading::refused("The mount answered at last.")
            }
        };
        fn reading<L>(workers: &Workers, load: L) -> Reading<'_, L> {
            Reading { workers, load }
        }
        for _ in 0..MOST_READERS + 3 {
            let read = app.begin_image_load(&path()).unwrap();
            start_read(&mut app, &tx, read, path(), reading(&workers, stall()));
            // Its deadline passes: the board stops waiting for it.
            for r in app.image_reads.iter_mut() {
                r.started = Instant::now() - Duration::from_secs(60);
            }
            let _ = app.apply_reads();
            assert!(app.image_reads.is_empty());
        }
        assert!(soon(|| stalled.load(SeqCst) >= MOST_READERS));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            most.load(SeqCst),
            MOST_READERS,
            "more stalled reads ran at once than the most"
        );
        let (said, _) = app.message().unwrap();
        assert!(
            said.contains(&format!("still waiting on {MOST_READERS} earlier reads")),
            "{said}"
        );
        // A clipboard paste: refused too, its reader never run.
        let ran = Arc::new(AtomicBool::new(false));
        let read = app.begin_image_load(&Source::Clipboard).unwrap();
        let took = ran.clone();
        let clipboard = reading(&workers, move |_source: Source, _room| {
            took.store(true, SeqCst);
            Loading {
                text: Some("words".into()),
                ..Default::default()
            }
        });
        start_read(&mut app, &tx, read, Source::Clipboard, clipboard);
        assert!(!ran.load(SeqCst));
        assert!(app.message().unwrap().0.contains("still waiting on"));
        // The mount answers: the reads end, and the count with them.
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(
            soon(|| workers.running() == 0),
            "{} still counted",
            workers.running()
        );
        take_back(&mut app, &rx, Duration::from_millis(100));
        assert_eq!(
            words_in(&app),
            (String::new(), 0),
            "a read let go put nothing in"
        );
        let read = app.begin_image_load(&path()).unwrap();
        let png = |_source: Source, _room| Loading {
            images: vec![Loaded {
                media_type: "image/png",
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
                from: "shot.png".into(),
            }],
            ..Default::default()
        };
        start_read(&mut app, &tx, read, path(), reading(&workers, png));
        take_back(&mut app, &rx, Duration::from_millis(500));
        assert_eq!(words_in(&app).1, 1, "{:?}", app.message());
    }

    #[test]
    fn no_browser_puts_the_link_on_the_clipboard_and_says_so_without_it() {
        // Intent 0591, K3-30.
        let said = no_browser("no desktop here to open a browser on", true);
        assert_eq!(
            said,
            "No browser here (no desktop here to open a browser on); the sign-in link is on your clipboard."
        );
        assert!(no_browser("xdg-open isn't installed", false).contains("Copy the link instead"));
    }

    #[test]
    fn a_save_from_a_closed_editor_goes_by_what_its_machine_said_while_the_editor_was_open() {
        // The end-to-end check's P1: the board takes no reads while the editor is open, so the
        // reads that came meanwhile wait in the inbox. Before the save goes they're applied: a
        // machine that stopped answering meanwhile has the save held, never sent into the dark.
        let mut app = App::new(
            crate::config::Config::default(),
            crate::demo::machines(false),
        );
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| crate::app::waiting_work(c) && c.intake().is_none() && !c.dismissed())
            .expect("a waiting backlog card in the demo")
            .clone();
        let m = card.machine_index;
        app.machines[m]
            .feed
            .as_mut()
            .unwrap()
            .supports
            .push("hold".into());
        assert!(app.machines[m].conn.is_live());
        let inbox = Inbox::default();
        let reader = inbox.next_reader(m);
        let since = app.now() - chrono::Duration::seconds(20);
        inbox.put(
            poll::Event {
                machine: m,
                conn: Conn::Reconnecting {
                    since,
                    attempt: 1,
                    retry_at: since,
                    detail: "ssh: connect to host: No route to host".into(),
                },
                data: None,
                at: since,
            },
            Instant::now(),
            reader,
        );
        // Known away from what came meanwhile: not read again, nothing waited for.
        read_before_saving(&mut app, None, &inbox, m, Duration::ZERO, &mut |_| {
            panic!("waited for a machine already known away")
        });
        assert!(!app.machines[m].conn.is_live());
        let id = card.detail.as_ref().unwrap().id.clone();
        app.action_key = Some(card.key.clone());
        app.queue_action(
            m,
            serde_json::json!({"action":"edit","id":id,"text":"Mine.","was":"sha256:aa","from":"here"}),
        );
        assert!(app.resolver.ready().is_empty());
        assert_eq!(app.kept.held.len(), 1);
        assert_eq!(app.kept.held[0].request["text"], "Mine.");
    }

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

    #[test]
    fn a_start_a_key_waits_on_has_its_machines_feed_read_at_once_then_every_five_seconds() {
        let mut reads = StartReads::default();
        let t = Instant::now();
        let at = |secs: u64| t + Duration::from_secs(secs);
        assert_eq!(reads.due(&[1], at(0)), [1], "the key asks for the first");
        assert!(reads.due(&[1], at(4)).is_empty());
        assert_eq!(reads.due(&[1], at(5)), [1]);
        assert!(reads.due(&[1], at(9)).is_empty());
        // Another machine counts from its own key; one nothing waits on is forgotten.
        assert_eq!(reads.due(&[1, 2], at(9)), [2]);
        assert_eq!(reads.due(&[1, 2], at(10)), [1]);
        assert_eq!(reads.due(&[2], at(14)), [2]);
        assert!(reads.due(&[], at(15)).is_empty());
        assert_eq!(reads.due(&[1], at(100)), [1], "a new key, a new first read");
    }

    /// What the runner says, in words: "started 1", "handed 1", "dropped 1 yes", "done 1 ok".
    #[cfg(unix)] // its tests run real commands, on Unix
    fn heard(answer: Answer) -> String {
        match answer {
            Answer::Started(n) => format!("started {n}"),
            Answer::Handed(n) => format!("handed {n}"),
            Answer::Dropped(n, d) => format!("dropped {n} {}", if d { "yes" } else { "no" }),
            Answer::Done(done) => format!(
                "done {} {}",
                done.0.unwrap(),
                match &done.3 {
                    Ok(r) if r.ok => "ok",
                    Ok(_) => "refused",
                    Err(_) => "failed",
                }
            ),
            Answer::Progress(token, _, _) => format!("progress {token}"),
            Answer::Update(machine, n, _) => format!("update {n} on machine {machine}"),
            Answer::NotWrittenUp(n) => format!("not written up {n}"),
            Answer::Changed(n) => format!("changed {n}"),
            Answer::Resending(n, _, why) => format!(
                "resending {} {}",
                n.unwrap_or(0),
                match why {
                    crate::actions::Resend::Retry { .. } => "retry",
                    crate::actions::Resend::Lost { .. } => "lost",
                }
            ),
        }
    }

    /// The next thing the runner says within `wait` ms, leaving out that a command has its
    /// request, which comes between its start and its answer.
    #[cfg(unix)]
    fn next_heard(rx: &mpsc::Receiver<Answer>, wait: u64) -> Option<String> {
        let until = Instant::now() + Duration::from_millis(wait);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let said = heard(rx.recv_timeout(left).ok()?);
            if !said.starts_with("handed") {
                return Some(said);
            }
        }
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
                    gated: false,
                    resends: false,
                    rev: None,
                    guard: None,
                    after: None,
                })
                .unwrap();
        }
        let mut seen = vec![];
        while seen.len() < 4 {
            if let Some(said) = next_heard(&rx, 10_000) {
                seen.push(said);
            }
        }
        assert_eq!(seen, ["started 1", "done 1 ok", "started 2", "done 2 ok"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_change_refused_with_retry_holds_its_card_s_place_while_the_runner_sends_it_again() {
        // The first is refused for something passing (a lock held): the runner says so, waits,
        // and sends it again with the same op, and the record's next change waits meanwhile
        // (intent 0591).
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.log");
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let log_q = crate::transport::quote(log.to_str().unwrap());
        let command = format!(
            "cat >> {log_q}\necho >> {log_q}\nif [ \"$(wc -l < {log_q})\" -lt 2 ]; then printf '%s' '{{\"ok\":false,\"retry\":1,\"message\":\"locked\"}}'; else printf '%s' '{{\"ok\":true,\"message\":\"done\"}}'; fi\n"
        );
        for n in [1, 2] {
            runner
                .tx
                .send(ToRunner::Run {
                    n,
                    machine: 0,
                    key: Some("here:7".into()),
                    target: machine.clone(),
                    command: command.clone(),
                    request: crate::actions::with_op(
                        serde_json::json!({"action":"move","id":"7","delta":1}),
                    ),
                    gated: false,
                    resends: true,
                    rev: None,
                    guard: None,
                    after: None,
                })
                .unwrap();
        }
        let mut seen = vec![];
        while seen.len() < 5 {
            if let Some(said) = next_heard(&rx, 10_000) {
                seen.push(said);
            }
        }
        assert_eq!(
            seen,
            [
                "started 1",
                "resending 1 retry",
                "done 1 ok",
                "started 2",
                "done 2 ok"
            ]
        );
        let sent: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0], sent[1], "sent again as it was, op and all");
        assert_ne!(sent[1], sent[2], "the next change has an op of its own");
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
            gated: false,
            resends: false,
            rev: None,
            guard: None,
            after: None,
        };
        runner.tx.send(run(1, 0, &refused)).unwrap();
        runner.tx.send(run(2, 0, &ok)).unwrap();
        let next = |wait: u64| next_heard(&rx, wait);
        assert_eq!(next(10_000).as_deref(), Some("started 1"));
        assert_eq!(next(10_000).as_deref(), Some("done 1 refused"));
        assert_eq!(next(500), None, "K waits while J is sent on");
        runner.tx.send(run(1, 1, &ok)).unwrap();
        let seen: Vec<String> = (0..4).filter_map(|_| next(10_000)).collect();
        assert_eq!(seen, ["started 1", "done 1 ok", "started 2", "done 2 ok"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_change_sent_again_after_its_follow_waits_first_holding_its_record_s_turn() {
        // Intent 0591: a start whose followed work ended refused with `retry` goes again after
        // that wait, with its op, and a change to the same card made meanwhile waits behind it.
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.log");
        let command = format!(
            "req=$(cat)\nprintf '%s\\n' \"$req\" >> '{}'\nprintf '%s' '{{\"ok\":true,\"message\":\"done\"}}'\n",
            log.display()
        );
        let last = crate::actions::parse_response(
            r##"{"ok":false,"retry":1,"message":"#7 didn't start yet; ⏎ again in a moment."}"##,
        )
        .unwrap();
        let run = |n: u64, request: serde_json::Value, after| ToRunner::Run {
            n,
            machine: 0,
            key: Some("here:7".into()),
            target: machine.clone(),
            command: command.clone(),
            request,
            gated: false,
            resends: true,
            rev: Some("r-7".into()),
            guard: None,
            after,
        };
        let began = Instant::now();
        let start = serde_json::json!({"action":"dispatch","id":"7","op":"k-0123abcd"});
        runner
            .tx
            .send(run(
                1,
                start,
                Some(Box::new(crate::actions::After {
                    wait: Duration::from_secs(1),
                    last,
                    since: crate::actions::Clocks::now(),
                })),
            ))
            .unwrap();
        runner
            .tx
            .send(run(
                2,
                serde_json::json!({"action":"close","id":"7","op":"k-4567ef01"}),
                None,
            ))
            .unwrap();
        let seen: Vec<String> = (0..5).filter_map(|_| next_heard(&rx, 10_000)).collect();
        assert_eq!(
            seen,
            [
                "started 1",
                "resending 1 retry",
                "done 1 ok",
                "started 2",
                "done 2 ok"
            ]
        );
        assert!(began.elapsed() >= Duration::from_secs(1), "it waited first");
        let sent: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(sent.len(), 2, "each sent once: {sent:?}");
        assert_eq!(sent[0]["op"], "k-0123abcd");
        assert_eq!(sent[0]["rev"], "r-7", "as a resend goes");
        assert_eq!(sent[1]["action"], "close");
    }

    /// The runner's resend after a followed end refused with `retry` 1: `since`, when Kerdr had it.
    #[cfg(unix)]
    fn after_follow(since: crate::actions::Clocks) -> Option<Box<crate::actions::After>> {
        let last = crate::actions::parse_response(
            r##"{"ok":false,"retry":1,"message":"#7 didn't start yet; ⏎ again in a moment."}"##,
        )
        .unwrap();
        Some(Box::new(crate::actions::After {
            wait: Duration::from_secs(1),
            last,
            since,
        }))
    }

    /// A dispatch of #7 run with `after`, by a backend that answers `answer` and logs each request:
    /// what the runner said of it, and the requests it was sent.
    #[cfg(unix)]
    fn followed_resend(
        after: Option<Box<crate::actions::After>>,
        answer: &str,
    ) -> (Vec<String>, Vec<serde_json::Value>) {
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.log");
        let command = format!(
            "req=$(cat)\nprintf '%s\\n' \"$req\" >> '{}'\nprintf '%s' '{answer}'\n",
            log.display()
        );
        runner
            .tx
            .send(ToRunner::Run {
                n: 1,
                machine: 0,
                key: Some("here:7".into()),
                target: machine,
                command,
                request: serde_json::json!({"action":"dispatch","id":"7","op":"k-0123abcd"}),
                gated: false,
                resends: true,
                rev: Some("r-7".into()),
                guard: None,
                after,
            })
            .unwrap();
        let mut seen = vec![];
        while let Some(said) = next_heard(&rx, 10_000) {
            let done = said.starts_with("done");
            seen.push(said);
            if done {
                break;
            }
        }
        // Nothing more comes after its answer.
        assert_eq!(next_heard(&rx, 1_500), None, "{seen:?}");
        let sent = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (seen, sent)
    }

    #[cfg(unix)]
    #[test]
    fn a_followed_end_is_sent_again_once_and_a_retry_answer_to_that_is_said_as_it_is() {
        // Codex's review 75, 2: after the one resend, a `retry` answer to it is the answer; the
        // runner's ordinary retrying doesn't send it a third time.
        let (seen, sent) = followed_resend(
            after_follow(crate::actions::Clocks::now()),
            r##"{"ok":false,"retry":1,"message":"#7 is not started yet; Enter again in a moment."}"##,
        );
        assert_eq!(seen, ["started 1", "resending 1 retry", "done 1 refused"]);
        assert_eq!(sent.len(), 1, "sent once: {sent:?}");
        assert_eq!(sent[0]["op"], "k-0123abcd");
    }

    #[cfg(unix)]
    #[test]
    fn a_followed_end_queued_before_a_sleep_isnt_sent_once_its_two_minutes_are_gone() {
        // Codex's review 75, 3: Kerdr had the end three minutes ago by the wall clock, and the
        // computer slept before the runner began (Linux's steady clock stops meanwhile). Its two
        // minutes count from that end, so nothing is sent and the refusal stands.
        let since = crate::actions::Clocks {
            steady: Instant::now(),
            wall: std::time::SystemTime::now() - Duration::from_secs(180),
        };
        let (seen, sent) = followed_resend(
            after_follow(since),
            r#"{"ok":true,"message":"Started #7."}"#,
        );
        assert_eq!(seen, ["started 1", "done 1 refused"]);
        assert!(sent.is_empty(), "nothing sent: {sent:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_runner_says_when_a_command_has_its_request_and_answers_every_drop() {
        // Review of intent 0445: q waits until a command has the whole request, and Esc on a
        // start whose write-up was read a moment before learns that it began.
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let command = "cat >/dev/null\nprintf '%s' '{\"ok\":true,\"message\":\"started\"}'\n";
        let run = |n: u64, gated: bool| ToRunner::Run {
            n,
            machine: 0,
            key: Some(format!("here:{n}")),
            target: machine.clone(),
            command: command.into(),
            request: serde_json::json!({"action":"dispatch","id":n.to_string()}),
            gated,
            resends: false,
            rev: None,
            guard: None,
            after: None,
        };
        let next = || heard(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        // Not yet written up: taken out before it started.
        runner.tx.send(run(1, true)).unwrap();
        runner.tx.send(ToRunner::Drop(1)).unwrap();
        assert_eq!(next(), "dropped 1 yes");
        // Its command has the request before it answers.
        runner.tx.send(run(2, false)).unwrap();
        let said: Vec<String> = (0..3).map(|_| next()).collect();
        assert_eq!(said, ["started 2", "handed 2", "done 2 ok"]);
        // Written up and started before the drop reached it: it goes on, and the board is told.
        runner.tx.send(run(3, true)).unwrap();
        runner
            .tx
            .send(ToRunner::Feed {
                machine: 0,
                intakes: std::collections::HashMap::from([("3".to_string(), None)]),
                footings: Default::default(),
            })
            .unwrap();
        assert_eq!(next(), "started 3");
        runner.tx.send(ToRunner::Drop(3)).unwrap();
        let mut said: Vec<String> = (0..3).map(|_| next()).collect();
        said.sort();
        assert_eq!(said, ["done 3 ok", "dropped 3 no", "handed 3"]);
    }

    #[cfg(unix)]
    #[test]
    fn going_into_herdr_sends_a_new_intent_whose_image_is_still_being_read() {
        // Review of intent 0445: Enter sent the words while their image was read, then Andrew
        // went straight into a workspace. They go before herdr takes the screen, not once he's
        // back on the board.
        let mut app = App::new(
            crate::config::Config::default(),
            crate::demo::machines(false),
        );
        app.cfg.actions.command = Some(
            "cat >/dev/null\nprintf '%s' '{\"ok\":true,\"message\":\"Saved.\",\"id\":\"9001\"}'\n"
                .into(),
        );
        let (answers, rx) = mpsc::channel();
        let runner = Runner::spawn(answers.clone());
        let (image_tx, image_rx) = mpsc::channel();
        let inbox = Inbox::default();
        let holds = RefCell::new(Holds::off());
        let bg = Bg {
            images: &image_rx,
            answers: &answers,
            runner: &runner,
            inbox: &inbox,
            holds: &holds,
        };
        app.image_reads.push_back(crate::app::ImageRead {
            id: 7,
            at: "The dock:".len(),
            for_send: true,
            started: Instant::now(),
            what: "Reading the clipboard",
            got: None,
        });
        app.send_after_load = Some((
            7,
            crate::app::Draft {
                words: "The dock:".into(),
                session: "web".into(),
                ..Default::default()
            },
        ));
        let sender = image_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let loading = crate::images::Loading {
                images: vec![crate::images::Loaded {
                    media_type: "image/png",
                    bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
                    ..Default::default()
                }],
                why: None,
                ..Default::default()
            };
            let _ = sender.send((7, loading));
        });
        send_loaded_images(&mut app, &bg, Some(Duration::from_secs(5)));
        assert!(app.send_after_load.is_none());
        assert!(
            app.resolver
                .unanswered_where(|r| r["action"] == "create")
                .is_some()
        );
        // The runner has it already: it starts while herdr has the screen.
        assert_eq!(next_heard(&rx, 10_000).as_deref(), Some("started 1"));
    }

    #[cfg(unix)]
    #[test]
    fn the_runner_starts_a_start_once_its_write_up_is_read_and_from_his_words_after_a_failed_one() {
        // Feeds reach it from the readers' threads, so this happens while Andrew is inside a
        // workspace too (intent 0445).
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let command = "cat >/dev/null\nprintf '%s' '{\"ok\":true,\"message\":\"started\"}'\n";
        let intakes = |pairs: &[(&str, Option<&str>)]| {
            pairs
                .iter()
                .map(|(id, v)| (id.to_string(), v.map(str::to_string)))
                .collect()
        };
        // Already read as written up before it's asked for: it goes at once.
        runner
            .tx
            .send(ToRunner::Feed {
                machine: 0,
                intakes: intakes(&[("8", None)]),
                footings: Default::default(),
            })
            .unwrap();
        for (n, id) in [(1, "7"), (2, "8"), (3, "9")] {
            runner
                .tx
                .send(ToRunner::Run {
                    n,
                    machine: 0,
                    key: Some(format!("here:{id}")),
                    target: machine.clone(),
                    command: command.into(),
                    request: serde_json::json!({"action":"dispatch","id":id}),
                    gated: true,
                    resends: false,
                    rev: None,
                    guard: None,
                    after: None,
                })
                .unwrap();
        }
        let next = || next_heard(&rx, 10_000).unwrap();
        assert_eq!(next(), "started 2");
        assert_eq!(next(), "done 2 ok");
        runner
            .tx
            .send(ToRunner::Feed {
                machine: 0,
                intakes: intakes(&[("7", Some("pending")), ("9", Some("failed"))]),
                footings: Default::default(),
            })
            .unwrap();
        // Its write-up failed: it starts from his words as they are, and that's said (intent 0591).
        assert_eq!(next(), "not written up 3");
        assert_eq!(next(), "started 3");
        assert_eq!(next(), "done 3 ok");
        runner
            .tx
            .send(ToRunner::Feed {
                machine: 0,
                intakes: intakes(&[("7", None)]),
                footings: Default::default(),
            })
            .unwrap();
        assert_eq!(next(), "started 1");
        assert_eq!(next(), "done 1 ok");
    }

    #[cfg(unix)]
    #[test]
    fn a_start_waiting_for_its_write_up_never_goes_once_its_card_is_set_aside_and_goes_with_its_rev()
     {
        // Codex's review 60, 5: d on #7 and #8 during their write-ups, then #7 is set aside
        // elsewhere. Once both are written up, #7 is taken out unstarted and the board told; #8
        // goes, with its card's rev as the read that let it go shows it, so its backend checks it.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.log");
        let (tx, rx) = mpsc::channel();
        let runner = Runner::spawn(tx);
        let machine = Machine {
            name: "here".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        };
        let log_q = crate::transport::quote(log.to_str().unwrap());
        let command = format!(
            "cat >> {log_q}\necho >> {log_q}\nprintf '%s' '{{\"ok\":true,\"message\":\"started\"}}'\n"
        );
        for (n, id) in [(1, "7"), (2, "8")] {
            runner
                .tx
                .send(ToRunner::Run {
                    n,
                    machine: 0,
                    key: Some(format!("here:{id}")),
                    target: machine.clone(),
                    command: command.clone(),
                    request: serde_json::json!({"action":"dispatch","id":id}),
                    gated: true,
                    resends: false,
                    rev: None,
                    guard: Some(crate::resolve::Footing::default()),
                    after: None,
                })
                .unwrap();
        }
        let footing = |set_aside: bool| {
            let footing = crate::resolve::Footing {
                set_aside,
                ..Default::default()
            };
            (footing, Some("r9".to_string()))
        };
        runner
            .tx
            .send(ToRunner::Feed {
                machine: 0,
                intakes: std::collections::HashMap::from([
                    ("7".to_string(), None),
                    ("8".to_string(), None),
                ]),
                footings: std::collections::HashMap::from([
                    ("7".to_string(), footing(true)),
                    ("8".to_string(), footing(false)),
                ]),
            })
            .unwrap();
        let seen: Vec<String> = (0..3).filter_map(|_| next_heard(&rx, 10_000)).collect();
        assert_eq!(seen, ["changed 1", "started 2", "done 2 ok"]);
        assert_eq!(next_heard(&rx, 300), None);
        let sent: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["id"], "8");
        assert_eq!(sent[0]["rev"], "r9");
    }

    #[cfg(unix)]
    #[test]
    fn a_start_let_go_by_its_write_up_but_waiting_behind_another_change_is_still_held_to_its_card()
    {
        // Codex's review 62, 4: d on #7 during its write-up, behind a y on #7 that's still
        // running. A read shows #7 written up, so the start no longer waits for that, only for y.
        // A later read shows #7 set aside: the start is taken out unstarted. Shown changed in any
        // other way, it goes once y is done, with the rev of the read that let it go, so its
        // backend asks about what changed since.
        let run = |later_set_aside: bool| {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("requests.log");
            let gate = dir.path().join("y-may-finish");
            let (tx, rx) = mpsc::channel();
            let runner = Runner::spawn(tx);
            let machine = Machine {
                name: "here".into(),
                ssh: None,
                herdr: "herdr".into(),
                feed: None,
                hidden: false,
            };
            let log_q = crate::transport::quote(log.to_str().unwrap());
            let gate_q = crate::transport::quote(gate.to_str().unwrap());
            let send = |n: u64, request: serde_json::Value, gated: bool, command: String| {
                runner
                    .tx
                    .send(ToRunner::Run {
                        n,
                        machine: 0,
                        key: Some("here:7".into()),
                        target: machine.clone(),
                        command,
                        request,
                        gated,
                        resends: false,
                        rev: None,
                        guard: gated.then(crate::resolve::Footing::default),
                        after: None,
                    })
                    .unwrap();
            };
            let answer = "printf '%s' '{\"ok\":true,\"message\":\"done\"}'\n";
            send(
                1,
                serde_json::json!({"action":"refresh","id":"7"}),
                false,
                format!("cat >/dev/null\nwhile [ ! -e {gate_q} ]; do sleep 0.05; done\n{answer}"),
            );
            assert_eq!(next_heard(&rx, 10_000).as_deref(), Some("started 1"));
            send(
                2,
                serde_json::json!({"action":"dispatch","id":"7"}),
                true,
                format!("cat >> {log_q}\necho >> {log_q}\n{answer}"),
            );
            let read = |set_aside: bool, rev: &str| ToRunner::Feed {
                machine: 0,
                intakes: std::collections::HashMap::from([("7".to_string(), None)]),
                footings: std::collections::HashMap::from([(
                    "7".to_string(),
                    (
                        crate::resolve::Footing {
                            set_aside,
                            ..Default::default()
                        },
                        Some(rev.to_string()),
                    ),
                )]),
            };
            runner.tx.send(read(false, "r9")).unwrap();
            runner.tx.send(read(later_set_aside, "r10")).unwrap();
            let mut heard: Vec<String> = next_heard(&rx, 500).into_iter().collect();
            std::fs::write(&gate, "").unwrap();
            while let Some(h) = next_heard(&rx, 1_000) {
                heard.push(h);
            }
            let sent: Vec<serde_json::Value> = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            (heard, sent)
        };
        let (heard, sent) = run(true);
        assert_eq!(heard, ["changed 2", "done 1 ok"]);
        assert!(sent.is_empty(), "{sent:?}");
        let (heard, sent) = run(false);
        assert_eq!(heard, ["done 1 ok", "started 2", "done 2 ok"]);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["rev"], "r9");
    }
}
