//! A made-up team's board, for `kerdr demo` and the README's screenshots. It goes through the
//! same code as real machines: synthetic herdr snapshots and a synthetic feed.

use chrono::{DateTime, Local, TimeZone};

use crate::board::MachineState;
use crate::config::Machine;
use crate::feed::{
    Criterion, Feed, FeedCard, FeedSlots, FeedWorkspace, Home, LogEntry, TimelineEntry, WaitsOn,
};
use crate::herdr::{Agent, SessionInfo, Snapshot, Workspace};
use crate::poll::{Conn, MachineData};

pub fn now() -> DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 10, 21, 17, 46, 0)
        .single()
        .expect("a valid time")
}

fn at(h: u32, m: u32) -> String {
    Local
        .with_ymd_and_hms(2026, 10, 21, h, m, 0)
        .single()
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

struct C<'a> {
    id: &'a str,
    title: &'a str,
    stage: &'a str,
    state: &'a str,
    attention: Option<&'a str>,
    status: Option<&'a str>,
    session: &'a str,
    machine: &'a str,
    ws: Option<(&'a str, &'a str)>,
}

fn card(c: C) -> FeedCard {
    FeedCard {
        id: c.id.into(),
        title: c.title.into(),
        stage: Some(c.stage.into()),
        state: Some(c.state.into()),
        attention: c.attention.map(Into::into),
        status: c.status.map(Into::into),
        home: Some(Home {
            machine: Some(c.machine.into()),
            session: Some(c.session.into()),
        }),
        workspace: c.ws.map(|(id, label)| FeedWorkspace {
            session: Some(c.session.into()),
            id: Some(id.into()),
            label: Some(label.into()),
            tab: None,
        }),
        ..Default::default()
    }
}

fn ws(id: &str, label: &str) -> Workspace {
    Workspace {
        workspace_id: id.into(),
        label: label.into(),
        ..Default::default()
    }
}

fn agent(ws: &str, status: &str, title: &str) -> Agent {
    Agent {
        agent: Some("claude".into()),
        agent_status: Some(status.into()),
        pane_id: format!("{ws}:p1"),
        workspace_id: ws.into(),
        terminal_title_stripped: Some(title.into()),
        ..Default::default()
    }
}

fn session(name: &str) -> SessionInfo {
    SessionInfo {
        name: name.into(),
        running: true,
        default: name == "default",
    }
}

/// The machines and what each says. `away` drops the build box, as on a train.
pub fn machines(away: bool) -> Vec<MachineState> {
    let mut laptop = MachineState::new(Machine {
        name: "laptop".into(),
        ssh: None,
        herdr: "herdr".into(),
        feed: Some("cards --json".into()),
        hidden: false,
    });
    let mut build_box = MachineState::new(Machine {
        name: "build-box".into(),
        ssh: Some("build-box".into()),
        herdr: "herdr".into(),
        feed: Some("cards --json".into()),
        hidden: false,
    });
    let mut gpu = MachineState::new(Machine {
        name: "gpu-rig".into(),
        ssh: Some("gpu-rig".into()),
        herdr: "herdr".into(),
        feed: None,
        hidden: false,
    });
    let mut mac = MachineState::new(Machine {
        name: "old-mac".into(),
        ssh: Some("old-mac".into()),
        herdr: "herdr".into(),
        feed: None,
        hidden: false,
    });

    // The laptop: the web app's work.
    let laptop_snap = Snapshot {
        version: Some("0.9.5".into()),
        protocol: Some(22),
        workspaces: vec![
            ws("w1", "41-checkout-expired-cards"),
            ws("w2", "45-dark-mode"),
            ws("w3", "47-search-typos"),
            ws("w4", "34-team-invites"),
        ],
        agents: vec![
            agent("w1", "idle", "Retry copy for declined cards"),
            agent("w2", "blocked", "Two palettes for dark mode"),
            agent("w3", "working", "Spelling suggestions in the index"),
            agent("w4", "idle", "Team invites"),
        ],
        ..Default::default()
    };
    laptop.conn = Conn::Live {
        latency: std::time::Duration::from_millis(41),
    };
    laptop.last_ok = Some(now());
    laptop.data = Some(MachineData {
        host: Some("laptop".into()),
        version: Some("0.9.5".into()),
        sessions: vec![session("web")],
        snapshots: [("web".to_string(), Ok(laptop_snap))].into_iter().collect(),
        keys: true,
        feed: None,
        feed_failed_after: None,
    });

    // The build box: the API and the background lane.
    let box_snap = Snapshot {
        version: Some("0.9.5".into()),
        protocol: Some(22),
        workspaces: vec![
            ws("w1", "44-api-rate-limits"),
            ws("w2", "39-flaky-login-test"),
            ws("w3", "38-backup-restore"),
        ],
        agents: vec![
            agent("w1", "working", "Token buckets per API key"),
            agent("w2", "idle", "Login test"),
            agent("w3", "idle", "Restore drill"),
        ],
        ..Default::default()
    };
    let bg_snap = Snapshot {
        version: Some("0.9.5".into()),
        protocol: Some(22),
        workspaces: vec![
            ws("w1", "46-docs-domain"),
            ws("w2", "42-invoice-pdfs"),
            ws("w3", "nightly-deps"),
        ],
        agents: vec![
            agent("w1", "working", "Redirects for every old docs page"),
            agent("w2", "idle", "Invoice PDFs"),
            agent("w3", "working", "Bump dependencies"),
        ],
        ..Default::default()
    };
    build_box.conn = if away {
        Conn::Reconnecting {
            since: now() - chrono::Duration::seconds(40),
            attempt: 2,
            retry_at: now() + chrono::Duration::seconds(8),
            detail: "Connection timed out".into(),
        }
    } else {
        Conn::Live {
            latency: std::time::Duration::from_millis(63),
        }
    };
    build_box.last_ok = Some(if away {
        now() - chrono::Duration::minutes(4)
    } else {
        now()
    });
    build_box.data = Some(MachineData {
        host: Some("build-box".into()),
        version: Some("0.9.5".into()),
        sessions: vec![session("api"), session("background")],
        snapshots: [
            ("api".to_string(), Ok(box_snap)),
            ("background".to_string(), Ok(bg_snap)),
        ]
        .into_iter()
        .collect(),
        keys: true,
        feed: None,
        feed_failed_after: None,
    });

    gpu.conn = Conn::HostKey {
        detail: "Host key verification failed.".into(),
    };
    mac.conn = Conn::HerdrTooOld {
        version: "0.8.2".into(),
    };

    let mut cards = vec![
        card(C {
            id: "41",
            title: "Checkout handles expired cards",
            stage: "test",
            state: "active",
            attention: Some("needs_you"),
            status: Some("Waiting on you"),
            session: "web",
            machine: "laptop",
            ws: Some(("w1", "41-checkout-expired-cards")),
        }),
        card(C {
            id: "45",
            title: "Dark mode for the dashboard",
            stage: "design",
            state: "active",
            attention: Some("running"),
            status: Some("Under way"),
            session: "web",
            machine: "laptop",
            ws: Some(("w2", "45-dark-mode")),
        }),
        card(C {
            id: "47",
            title: "Search results fix typos",
            stage: "plan",
            state: "active",
            attention: Some("running"),
            status: Some("Under way"),
            session: "web",
            machine: "laptop",
            ws: Some(("w3", "47-search-typos")),
        }),
        card(C {
            id: "44",
            title: "Rate limits on the public API",
            stage: "build",
            state: "active",
            attention: Some("running"),
            status: Some("Under way"),
            session: "api",
            machine: "build-box",
            ws: Some(("w1", "44-api-rate-limits")),
        }),
        card(C {
            id: "39",
            title: "Flaky login test on CI",
            stage: "build",
            state: "active",
            attention: Some("blocked"),
            status: Some("Blocked"),
            session: "api",
            machine: "build-box",
            ws: Some(("w2", "39-flaky-login-test")),
        }),
        card(C {
            id: "38",
            title: "Nightly backups restore cleanly",
            stage: "deploy",
            state: "active",
            attention: Some("waiting"),
            status: Some("Waiting on the staging pipeline"),
            session: "api",
            machine: "build-box",
            ws: Some(("w3", "38-backup-restore")),
        }),
        card(C {
            id: "46",
            title: "Docs move to the new domain",
            stage: "build",
            state: "background",
            attention: Some("running"),
            status: Some("Under way"),
            session: "background",
            machine: "build-box",
            ws: Some(("w1", "46-docs-domain")),
        }),
        card(C {
            id: "42",
            title: "Invoice PDFs render on phones",
            stage: "test",
            state: "background",
            attention: Some("waiting"),
            status: Some("Waiting on the second reviewer"),
            session: "background",
            machine: "build-box",
            ws: Some(("w2", "42-invoice-pdfs")),
        }),
        card(C {
            id: "52",
            title: "Single sign-on for teams",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "33",
            title: "Audit log for billing changes",
            stage: "backlog",
            state: "shelved",
            attention: None,
            status: None,
            session: "api",
            machine: "build-box",
            ws: None,
        }),
        card(C {
            id: "31",
            title: "Webhooks retry with backoff",
            stage: "backlog",
            state: "shelved",
            attention: None,
            status: None,
            session: "api",
            machine: "build-box",
            ws: None,
        }),
        card(C {
            id: "53",
            title: "Weekly digest email",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "50",
            title: "Onboarding email sequence",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "27",
            title: "Export reports to CSV",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "24",
            title: "Keyboard shortcuts in the editor",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "21",
            title: "Localize the pricing page",
            stage: "backlog",
            state: "queued",
            attention: None,
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "36",
            title: "Password reset links expire",
            stage: "maintain",
            state: "closed",
            attention: Some("done"),
            status: None,
            session: "web",
            machine: "laptop",
            ws: None,
        }),
        card(C {
            id: "35",
            title: "Faster cold start",
            stage: "maintain",
            state: "closed",
            attention: Some("done"),
            status: None,
            session: "api",
            machine: "build-box",
            ws: None,
        }),
        card(C {
            id: "34",
            title: "Team invites",
            stage: "maintain",
            state: "closed",
            attention: Some("done"),
            status: None,
            session: "web",
            machine: "laptop",
            ws: Some(("w4", "34-team-invites")),
        }),
        card(C {
            id: "30",
            title: "Status page",
            stage: "maintain",
            state: "closed",
            attention: Some("done"),
            status: None,
            session: "api",
            machine: "build-box",
            ws: None,
        }),
    ];
    for c in cards.iter_mut() {
        c.reference = Some(format!("#{}", c.id));
        c.updated = Some(at(17, 11));
        match c.id.as_str() {
            "41" => {
                c.updated = Some(at(17, 11));
                c.story = Some("As a shopper whose card expired, I want checkout to tell me what happened and let me retry, so that I don't give up on the order.".into());
                c.needs = Some("Your OK on the retry message before it ships: it reads \"Your card expired. Try another, or update this one.\"".into());
                c.message = Some("Expired and declined cards now get their own messages, and the retry keeps the basket. Two of three checks pass; the copy is the last one, and it's yours to approve.".into());
                c.criteria = vec![
                    Criterion {
                        text: "An expired card shows its own message, not a generic error.".into(),
                        met: Some(true),
                    },
                    Criterion {
                        text: "Retrying keeps the basket and the address.".into(),
                        met: Some(true),
                    },
                    Criterion {
                        text: "The message is approved by the product owner.".into(),
                        met: Some(false),
                    },
                ];
                c.timeline = vec![
                    TimelineEntry {
                        stage: "plan".into(),
                        date: Some("2026-10-19".into()),
                        note: Some("stated, triaged".into()),
                    },
                    TimelineEntry {
                        stage: "design".into(),
                        date: Some("2026-10-19".into()),
                        note: Some("two messages".into()),
                    },
                    TimelineEntry {
                        stage: "build".into(),
                        date: Some("2026-10-20".into()),
                        note: Some("retry flow".into()),
                    },
                    TimelineEntry {
                        stage: "test".into(),
                        date: Some("2026-10-21".into()),
                        note: Some("your OK on copy".into()),
                    },
                ];
                c.log = vec![
                    LogEntry {
                        date: Some("2026-10-21".into()),
                        text: "End-to-end tests pass for expired, declined and stolen cards."
                            .into(),
                    },
                    LogEntry {
                        date: Some("2026-10-20".into()),
                        text: "Retry keeps the basket; the address form no longer resets.".into(),
                    },
                    LogEntry {
                        date: Some("2026-10-19".into()),
                        text: "Triaged: needs one answer from you, the wording.".into(),
                    },
                ];
                c.words = Some(
                    "\"People with an expired card just see 'payment failed' and leave.\"".into(),
                );
            }
            "45" => {
                c.needs = Some("Pick one of two palettes; both are in its pane.".into());
                c.updated = Some(at(17, 38));
            }
            "39" => c.blockers = vec!["The CI runner is out of disk space.".into()],
            "53" => {
                c.waits_on = vec![WaitsOn {
                    reference: Some("#44".into()),
                    title: Some("Rate limits on the public API".into()),
                    open: Some(true),
                }]
            }
            "52" => c.priority = Some("high".into()),
            "27" => c.stale_days = Some(18),
            "24" => c.stale_days = Some(16),
            "21" => c.stale_days = Some(15),
            "50" => c.stale_days = Some(4),
            "36" => c.closed = Some("2026-10-21".into()),
            "35" => c.closed = Some("2026-10-19".into()),
            "34" => c.closed = Some("2026-10-20".into()),
            "30" => c.closed = Some("2026-10-18".into()),
            _ => {}
        }
        if c.state.as_deref() == Some("background") {
            c.lane = Some("background".into());
        }
    }
    let (laptop_cards, box_cards): (Vec<FeedCard>, Vec<FeedCard>) = cards
        .into_iter()
        .partition(|c| c.home.as_ref().and_then(|h| h.machine.as_deref()) == Some("laptop"));
    laptop.feed = Some(Feed {
        kerdr_feed: 1,
        source: Some("demo".into()),
        machine: Some("laptop".into()),
        sessions: [(
            "web".to_string(),
            FeedSlots {
                used: 3,
                limit: Some(3),
                max: Some(5),
            },
        )]
        .into_iter()
        .collect(),
        cards: laptop_cards,
        ..Default::default()
    });
    build_box.feed = Some(Feed {
        kerdr_feed: 1,
        source: Some("demo".into()),
        machine: Some("build-box".into()),
        sessions: [
            (
                "api".to_string(),
                FeedSlots {
                    used: 3,
                    limit: Some(3),
                    max: Some(5),
                },
            ),
            (
                "background".to_string(),
                FeedSlots {
                    used: 3,
                    limit: None,
                    max: None,
                },
            ),
        ]
        .into_iter()
        .collect(),
        cards: box_cards,
        ..Default::default()
    });
    for m in [&mut laptop, &mut build_box] {
        let f = m.feed.as_mut().unwrap();
        f.accounts=serde_json::from_value(serde_json::json!([
            {"code":"KBZ","name":"Personal","provider":"claude","kind":"login","runs":"claude","status":"out","until":"2026-10-22T09:00:00-05:00","pool":"week","usage":{"five":86,"week":100}},
            {"code":"ARK","name":"Studio","provider":"claude","kind":"token","runs":"claude","status":"ready","usage":{"five":12,"week":30}}
        ])).unwrap();
        f.owners=serde_json::from_value(serde_json::json!([
            {"id":"studio","kind":"organization","name":"Example Studio","models":["ARK","KBZ"],"own":true,"sessions":["default"]},
            {"id":"shop","kind":"tenant","name":"Shop","parent":"studio","models":["ARK","KBZ"],"own":false,"sessions":["web","shop-bg"]},
            {"id":"platform","kind":"tenant","name":"Platform","parent":"studio","models":["KBZ","ARK"],"own":true,"sessions":["api","background"]}
        ])).unwrap();
        f.owners_source = Some("mock".into());
        f.machine_owner = Some("studio".into());
        for (session, snap) in &m.data.as_ref().unwrap().snapshots {
            if let Ok(snap) = snap {
                for agent in &snap.agents {
                    let waiting = session == "api" && agent.pane_id == "w1:p1";
                    f.agents.push(crate::accounts::AccountAgent {
                        pane: agent.pane_id.clone(),
                        session: session.clone(),
                        conversation: format!("demo-{session}-{}", agent.pane_id),
                        account: if waiting { "KBZ" } else { "ARK" }.into(),
                        how: "list".into(),
                        waiting: waiting.then(|| crate::accounts::Waiting {
                            why: "usage".into(),
                            until: Some("2026-10-22T09:00:00-05:00".into()),
                        }),
                    });
                }
            }
        }
    }
    vec![laptop, build_box, gpu, mac]
}

/// Every account dialog, also used by the all-sizes rendering checks and screenshot generator.
pub fn account_screens(app: &crate::app::App) -> Vec<crate::app::Mode> {
    use crate::app::{
        Mode,
        account_controls::{AccountScreen as S, MoveTarget, Signin},
    };
    let owner = app.machines[0].feed.as_ref().unwrap().owners[0].clone();
    let card = app
        .board
        .cards
        .iter()
        .find(|c| !c.agents.is_empty() && app.account_agent(c).is_some())
        .unwrap();
    let target = Box::new(MoveTarget {
        machine: card.machine_index,
        key: card.key.clone(),
        reference: card.reference.clone(),
        agent: app.account_agent(card).unwrap().clone(),
    });
    let list = Mode::Accounts {
        screen: S::List {
            machine: 0,
            rows: app.account_rows(0),
            sel: 3,
            model: 0,
        },
        back: Box::new(Mode::Board),
    };
    let move_screen = Mode::Accounts {
        screen: S::Move {
            target: target.clone(),
            choices: vec!["ARK".into(), "KBZ".into(), "list".into()],
            sel: 0,
        },
        back: Box::new(Mode::Board),
    };
    let mut result=vec![list.clone(),move_screen.clone(),Mode::Accounts{screen:S::Confirm{machine:0,request:serde_json::json!({"action":"account-move","pane":target.agent.pane,"to":"ARK","confirm":"demo-confirm"}),message:"This agent is mid-step. Move it when it next stops? Restarting also ends its background job.".into(),yes:"move when it stops".into(),no:"leave it".into(),target:Some(target),ready_at:std::time::Instant::now()},back:Box::new(move_screen)}];
    for screen in [
        S::PasteToken {
            machine: 0,
            code: "ARK".into(),
            token: Default::default(),
        },
        S::Models {
            machine: 0,
            models: owner.models.clone(),
            owner,
            sel: 0,
            move_waiting: true,
        },
        S::AddModel {
            machine: 0,
            choices: vec!["ARK".into()],
            sel: 0,
        },
        S::Add {
            machine: 0,
            code: "NEW".into(),
            name: "Studio".into(),
            field: 0,
            machines: vec![0, 1],
            chosen: vec![true, true],
        },
        S::Rename {
            machine: 0,
            code: "ARK".into(),
            name: "Studio".into(),
        },
        S::Signin(Signin {
            code: "ARK".into(),
            machines: vec![0, 1],
            current: 1,
            ticket: Some("demo-ticket".into()),
            url: Some("https://example.com/signin".into()),
            needs_code: true,
            page_code: String::new(),
            editing: false,
            poll_at: std::time::Instant::now(),
            failed: false,
            message: "Authorize in the browser window that just opened.".into(),
        }),
        S::Signin(Signin {
            code: "KBZ".into(),
            machines: vec![1],
            current: 0,
            ticket: Some("demo-ticket".into()),
            url: Some("https://example.com/signin".into()),
            needs_code: true,
            page_code: String::new(),
            editing: true,
            poll_at: std::time::Instant::now(),
            failed: true,
            message: "The saved sign-in needs you to sign in again.".into(),
        }),
    ] {
        result.push(Mode::Accounts {
            screen,
            back: Box::new(list.clone()),
        });
    }
    result
}
