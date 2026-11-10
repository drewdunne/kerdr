use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;

use clap::{Parser, Subcommand};

use kerdr::app::App;
use kerdr::board::{self, MachineState, Scope};
use kerdr::config::{self, Config};
use kerdr::errlog::{Entry, ErrorLog};
use kerdr::herdr_keys;
use kerdr::poll::{self, Conn};
use kerdr::report::{self, Report};
use kerdr::state::State;
use kerdr::transport;

/// An opinionated kanban cockpit for driving herdr sessions.
#[derive(Parser)]
#[command(name = "kerdr", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the board on a made-up team's work, without herdr.
    Demo {
        /// Show it as it looks when a machine drops off the network.
        #[arg(long)]
        away: bool,
    },
    /// Tell Kerdr where this workspace's work stands. For agents, from inside a herdr pane.
    Report {
        /// backlog, plan, design, build, test, deploy or maintain
        #[arg(long)]
        stage: Option<String>,
        /// needs-you, waiting, running, blocked, idle or done
        #[arg(long)]
        status: Option<String>,
        /// One line about what it's doing, or what it needs.
        #[arg(long)]
        note: Option<String>,
        /// Done-when progress, like 1/3.
        #[arg(long)]
        progress: Option<String>,
        /// Take everything this workspace reported back.
        #[arg(long)]
        clear: bool,
        /// A workspace id, when not run from inside the workspace.
        #[arg(long)]
        workspace: Option<String>,
        /// That workspace's herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Print Kerdr's two keys for herdr's config, or add them with --install.
    HerdrKeys {
        #[arg(long)]
        install: bool,
        /// The herdr config to add them to (default: this machine's).
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Check every machine: reachable, which herdr, its feed, and whether Kerdr's keys are in.
    Doctor {
        /// Add Kerdr's keys to this machine's herdr config where they're missing, and reload
        /// each running session's config to pick them up.
        #[arg(long)]
        fix: bool,
    },
    /// Print the board once, as JSON.
    Cards,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    kerdr::errlog::keep_panics();
    let result = match cli.command {
        None => open_board(),
        Some(Cmd::Demo { away }) => open_demo(away),
        Some(Cmd::Report {
            stage,
            status,
            note,
            progress,
            clear,
            workspace,
            session,
        }) => report::run(&Report {
            stage,
            status,
            note,
            progress,
            clear,
            workspace,
            session,
        })
        .map_err(anyhow::Error::msg),
        Some(Cmd::HerdrKeys { install, config }) => herdr_keys_cmd(install, config),
        Some(Cmd::Doctor { fix }) => doctor(fix),
        Some(Cmd::Cards) => cards(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ErrorLog::on_this_machine().write(
                &Entry {
                    source: "kerdr",
                    shown: "fail",
                    message: e.to_string(),
                    ..Default::default()
                },
                &[],
                &[],
            );
            eprintln!("kerdr: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load_config() -> anyhow::Result<Config> {
    Config::load().map_err(anyhow::Error::msg)
}

fn machine_states(cfg: &Config) -> Vec<MachineState> {
    let state = State::load();
    cfg.machines(&config::hostname(), &config::picker_machines())
        .into_iter()
        .map(|mut m| {
            if state.hidden.iter().any(|h| h.eq_ignore_ascii_case(&m.name)) && !m.is_local() {
                m.hidden = true;
            }
            MachineState::new(m)
        })
        .collect()
}

fn open_board() -> anyhow::Result<()> {
    let cfg = load_config()?;
    let machines = machine_states(&cfg);
    let mut app = App::new(cfg, machines);
    app.prefix = herdr_keys::prefix();
    kerdr::tui::run(app, true)
}

fn open_demo(away: bool) -> anyhow::Result<()> {
    let mut app = App::new(Config::default(), kerdr::demo::machines(away));
    app.demo = true;
    app.fixed_now = Some(kerdr::demo::now());
    app.rebuild();
    kerdr::tui::run(app, false)
}

fn herdr_keys_cmd(install: bool, path: Option<PathBuf>) -> anyhow::Result<()> {
    if !install {
        print!("{}", herdr_keys::SNIPPET.trim_start());
        return Ok(());
    }
    let path = path.unwrap_or_else(herdr_keys::herdr_config_path);
    if herdr_keys::install(&path).map_err(anyhow::Error::msg)? {
        println!("Added Kerdr's two keys to {}.", path.display());
        println!(
            "Reload each running session to pick them up: herdr --session NAME server reload-config"
        );
    } else {
        println!("Kerdr's keys are already in {}.", path.display());
    }
    Ok(())
}

/// Reads every machine at once.
fn read_all(cfg: &Config) -> Vec<MachineState> {
    let mut states = machine_states(cfg);
    let handles: Vec<_> = states
        .iter()
        .map(|s| {
            let m = s.machine.clone();
            thread::spawn(move || {
                // This machine is read even when its cards are hidden (intent 0591).
                if m.hidden && !m.is_local() {
                    (Conn::Hidden, None)
                } else {
                    poll::read_once(&m)
                }
            })
        })
        .collect();
    for (s, h) in states.iter_mut().zip(handles) {
        let (conn, data) = h.join().unwrap_or((Conn::Connecting, None));
        s.conn = conn;
        if let Some(data) = data {
            s.take(data, chrono::Local::now());
        }
    }
    states
}

fn doctor(fix: bool) -> anyhow::Result<()> {
    let cfg = load_config()?;
    println!(
        "Kerdr {} on {}",
        env!("CARGO_PKG_VERSION"),
        config::hostname()
    );
    println!("config: {}", config::config_path().display());
    let states = read_all(&cfg);
    let now = chrono::Local::now();
    let mut all_ok = true;
    for s in &states {
        let how = s
            .machine
            .ssh
            .as_deref()
            .map(|h| format!("ssh {h}"))
            .unwrap_or_else(|| "this machine".into());
        println!();
        println!("{} {}  ({how})", s.conn.glyph(), s.name());
        println!("  connection: {}", s.conn.short(now));
        if !s.conn.is_live() && !matches!(s.conn, Conn::Hidden) {
            all_ok = false;
        }
        // A diagnosis says what herdr or the feed printed where a read failed, under Kerdr's own
        // words for it, which are all the board says (intent 0591).
        let printed = |what: &str| {
            if let Some(text) = s.said.get(what) {
                println!("    it printed:");
                for line in text.lines() {
                    println!("      {line}");
                }
            }
        };
        if let Some((why, _)) = &s.sessions_failing {
            println!("  herdr's sessions: {why}");
            printed("sessions");
            all_ok = false;
        }
        if let Some(d) = &s.data {
            println!(
                "  herdr: {}",
                d.version.clone().unwrap_or_else(|| "?".into())
            );
            let running: Vec<&str> = d
                .sessions
                .iter()
                .filter(|x| x.running)
                .map(|x| x.name.as_str())
                .collect();
            println!(
                "  sessions running: {}",
                if running.is_empty() {
                    "none".into()
                } else {
                    running.join(", ")
                }
            );
            for (name, snap) in &d.snapshots {
                if let Err(e) = snap {
                    println!("  session {name}: couldn't read it ({e})");
                    printed(&format!("session {name}"));
                    all_ok = false;
                }
            }
        }
        match (&s.machine.feed, &s.feed, &s.feed_error) {
            (None, _, _) => println!("  feed: none (herdr workspaces only)"),
            (Some(cmd), Some(f), _) => {
                println!("  feed: {} cards from {cmd}", f.cards.len());
                for w in &f.warnings {
                    println!("    warning: {w}");
                }
            }
            (Some(cmd), None, Some(e)) => {
                println!("  feed: failed ({e}) from {cmd}");
                printed("feed");
                all_ok = false;
            }
            (Some(cmd), None, None) => println!("  feed: not read ({cmd})"),
        }
        if s.conn.is_live() {
            let has = s.data.as_ref().is_some_and(|d| d.keys);
            if !has && fix && s.machine.is_local() {
                // The same install as kerdr herdr-keys --install, then each running session
                // reloads its config to pick them up (intent 0591, K3-71).
                println!("  Kerdr's keys in herdr's config: no; adding them");
                if !fix_keys_here(s) {
                    all_ok = false;
                }
            } else {
                println!(
                    "  Kerdr's keys in herdr's config: {}",
                    if has {
                        "yes"
                    } else if s.machine.is_local() {
                        "no (kerdr doctor --fix adds them)"
                    } else {
                        "no (kerdr doctor --fix, on that machine)"
                    }
                );
            }
        }
    }
    let board = board::build(&states, &cfg);
    let s = board::summarize(&board, &Scope::All, &states);
    println!();
    println!(
        "board: {} cards, {} need you, {} running, {} waiting, backlog {}{}",
        board.cards.len(),
        s.needs_you,
        s.running,
        s.waiting,
        s.backlog,
        s.verdict
            .map(|v| format!(" · {}", v.text()))
            .unwrap_or_default()
    );
    if all_ok {
        Ok(())
    } else {
        Err(anyhow::anyhow!("some machines need attention (above)"))
    }
}

/// Adds Kerdr's keys to this machine's herdr config and reloads each running session's config,
/// saying what it did. Whether it all went.
fn fix_keys_here(s: &MachineState) -> bool {
    let path = herdr_keys::herdr_config_path();
    match herdr_keys::install(&path) {
        Ok(true) => println!("    added Kerdr's two keys to {}", path.display()),
        Ok(false) => println!("    Kerdr's keys are already in {}", path.display()),
        Err(e) => {
            println!("    couldn't add them: {e}");
            return false;
        }
    }
    let running: Vec<&str> = s
        .data
        .as_ref()
        .map(|d| {
            d.sessions
                .iter()
                .filter(|x| x.running)
                .map(|x| x.name.as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut ok = true;
    for name in running {
        let out = transport::run_herdr(
            &transport::Target::Local,
            &s.machine.herdr,
            &["--session", name, "server", "reload-config"],
            std::time::Duration::from_secs(10),
        );
        if out.ok() {
            println!("    session {name} reloaded its config");
        } else {
            ok = false;
            let why = if out.timed_out {
                "it didn't answer within 10 seconds".to_string()
            } else {
                format!("exit {:?}: {}", out.status, out.stderr.trim())
            };
            println!("    session {name} didn't reload its config ({why})");
        }
    }
    ok
}

fn cards() -> anyhow::Result<()> {
    let cfg = load_config()?;
    let states = read_all(&cfg);
    let board = board::build(&states, &cfg);
    let cards: Vec<serde_json::Value> = board
        .cards
        .iter()
        .map(|c| {
            serde_json::json!({
                "key": c.key,
                "ref": c.reference,
                "title": c.title,
                "stage": c.stage.id(),
                "attention": c.attention.label(),
                "why": c.why,
                "machine": c.machine,
                "session": c.session,
                "workspace": c.workspace_id,
                "workspace_label": c.workspace_label,
                "tab": c.tab_id,
                "agents": c.agents.iter().map(|a| &a.pane_id).collect::<Vec<_>>(),
                "cached": c.cached_at.is_some(),
                "owner": c.owner_label(),
                "project": c.project(),
                "dismissed": c.dismissed(),
                "created": c.detail.as_ref().and_then(|d| d.created.as_deref()),
                "refreshed_at": c.detail.as_ref().and_then(|d| d.refreshed_at.as_deref()),
                "backlog_rank": c.backlog_rank(),
                "progress": c.progress.map(|(m, t)| format!("{m}/{t}")),
            })
        })
        .collect();
    let machines: Vec<serde_json::Value> = states
        .iter()
        .map(|s| serde_json::json!({"name": s.name(), "connection": s.conn.short(chrono::Local::now())}))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"machines": machines, "cards": cards}))?
    );
    Ok(())
}
