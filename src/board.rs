//! Builds the board from what each machine said: feed cards matched to the herdr workspaces
//! they run in, plus a card for every other workspace with an agent, and the slot count of
//! every session.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Local, NaiveDate, TimeZone};

use crate::config::{Config, Machine};
use crate::feed::{Feed, FeedCard};
use crate::herdr::{self, Agent, JUMP_TOKEN, Snapshot, Workspace};
use crate::model::{AgentInfo, Attention, Card, Priority, Slots, Source, Stage, Summary, Verdict};
use crate::poll::{Conn, MachineData};

/// Everything Kerdr knows about one machine.
#[derive(Clone, Debug)]
pub struct MachineState {
    pub machine: Machine,
    pub conn: Conn,
    /// The last read that worked: what the board shows, dimmed, while the machine is away.
    pub data: Option<MachineData>,
    pub feed: Option<Feed>,
    pub feed_error: Option<String>,
    pub last_ok: Option<DateTime<Local>>,
}

impl MachineState {
    pub fn new(machine: Machine) -> MachineState {
        let conn = if machine.hidden {
            Conn::Hidden
        } else {
            Conn::Connecting
        };
        MachineState {
            machine,
            conn,
            data: None,
            feed: None,
            feed_error: None,
            last_ok: None,
        }
    }

    /// The name the board uses: the config's, which for a picker entry is its SSH target until
    /// the machine says its hostname.
    pub fn name(&self) -> String {
        if self.machine.is_local() {
            return self.machine.name.clone();
        }
        match (&self.data, &self.machine.ssh) {
            (
                Some(MachineData {
                    host: Some(host), ..
                }),
                Some(ssh),
            ) if self.machine.name == *ssh => host.clone(),
            _ => self.machine.name.clone(),
        }
    }

    pub fn answers_to(&self, name: &str) -> bool {
        let name = name.trim();
        self.name().eq_ignore_ascii_case(name)
            || self.machine.name.eq_ignore_ascii_case(name)
            || self
                .data
                .as_ref()
                .and_then(|d| d.host.as_deref())
                .is_some_and(|h| h.eq_ignore_ascii_case(name))
            || self
                .machine
                .ssh
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(name))
    }

    pub fn is_shown(&self) -> bool {
        !matches!(self.conn, Conn::Hidden)
    }

    /// Set when the board's data for this machine isn't fresh.
    pub fn cached_at(&self) -> Option<DateTime<Local>> {
        if self.conn.is_live() {
            None
        } else {
            self.last_ok
        }
    }

    pub fn snapshot(&self, session: &str) -> Option<&Snapshot> {
        self.data.as_ref()?.snapshots.get(session)?.as_ref().ok()
    }
}

#[derive(Clone, Debug)]
pub struct SessionRow {
    pub machine: String,
    pub session: String,
    pub running: bool,
    pub slots: Slots,
    pub needs: usize,
    pub cards: usize,
    pub cached: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRow {
    pub machine: String,
    pub session: String,
    pub id: String,
    pub label: String,
    pub status: String,
    pub card: Option<String>,
    pub note: Option<String>,
    pub cached: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub cards: Vec<Card>,
    pub sessions: Vec<SessionRow>,
    pub workspaces: Vec<WorkspaceRow>,
    pub machine_order: Vec<String>,
}

struct WsRef<'a> {
    mi: usize,
    session: String,
    ws: &'a Workspace,
    agents: Vec<&'a Agent>,
    /// Whether `agents` is all of the workspace's, so herdr's status for the whole workspace
    /// speaks for them.
    whole: bool,
}

impl<'a> WsRef<'a> {
    /// The same workspace with only the agents `keep` keeps.
    fn only(&self, keep: impl Fn(&Agent) -> bool) -> WsRef<'a> {
        let agents: Vec<&'a Agent> = self.agents.iter().copied().filter(|a| keep(a)).collect();
        WsRef {
            mi: self.mi,
            session: self.session.clone(),
            ws: self.ws,
            whole: self.whole && agents.len() == self.agents.len(),
            agents,
        }
    }
}

fn workspaces<'a>(machines: &'a [MachineState], cfg: &Config) -> Vec<WsRef<'a>> {
    let mut out = Vec::new();
    for (mi, m) in machines.iter().enumerate() {
        if !m.is_shown() {
            continue;
        }
        let Some(data) = &m.data else { continue };
        for (session, snap) in &data.snapshots {
            if !cfg.board.wants(session) {
                continue;
            }
            let Ok(snap) = snap else { continue };
            for ws in &snap.workspaces {
                out.push(WsRef {
                    mi,
                    session: session.clone(),
                    ws,
                    agents: snap.agents_in(&ws.workspace_id).collect(),
                    whole: true,
                });
            }
        }
    }
    out
}

/// herdr's view of a workspace's agents, the most urgent first: blocked (asking), working,
/// done (finished, not yet seen), idle.
fn herdr_status(w: &WsRef) -> Option<String> {
    let statuses: Vec<&str> = w
        .agents
        .iter()
        .filter_map(|a| a.agent_status.as_deref())
        .collect();
    for want in ["blocked", "working", "done", "idle"] {
        if statuses.contains(&want) {
            return Some(want.to_string());
        }
    }
    if !w.agents.is_empty() {
        let whole = w.ws.agent_status.clone().filter(|_| w.whole);
        return whole.or(Some("unknown".into()));
    }
    None
}

fn running_title(w: &WsRef) -> Option<String> {
    w.agents
        .iter()
        .find(|a| a.agent_status.as_deref() == Some("working"))
        .and_then(|a| a.terminal_title_stripped.clone())
        .filter(|t| !t.trim().is_empty())
}

/// Decides what a card needs from you, from what the feed says and what herdr sees.
pub struct Signals<'a> {
    pub usage_waiting: bool,
    pub closed: bool,
    pub token: Option<&'a str>,
    pub herdr: Option<&'a str>,
    pub feed: Option<Attention>,
    pub feed_status: Option<&'a str>,
    pub needs: Option<&'a str>,
    pub blocker: Option<&'a str>,
    pub running_title: Option<&'a str>,
    pub from_feed: bool,
}

pub fn attention(s: &Signals) -> (Attention, String) {
    let status_or = |fallback: &str| s.feed_status.unwrap_or(fallback).to_string();
    if s.closed {
        return (Attention::Done, "done".into());
    }
    if let Some(att) = s.token.and_then(Attention::parse) {
        return (att, status_or(att.label()));
    }
    if s.herdr == Some("blocked") {
        return (
            Attention::NeedsYou,
            s.needs.unwrap_or("asking you something").to_string(),
        );
    }
    if s.feed == Some(Attention::NeedsYou) {
        return (
            Attention::NeedsYou,
            s.needs
                .map(str::to_string)
                .unwrap_or_else(|| status_or("waiting on you")),
        );
    }
    if s.feed == Some(Attention::Blocked) {
        return (
            Attention::Blocked,
            s.blocker.unwrap_or("blocked").to_string(),
        );
    }
    if s.herdr == Some("working") {
        let why = s.running_title.or(s.feed_status).unwrap_or("working");
        return (Attention::Running, why.to_string());
    }
    if s.feed == Some(Attention::Waiting) {
        return (Attention::Waiting, status_or("waiting"));
    }
    if s.usage_waiting && matches!(s.herdr, Some("done" | "idle")) {
        return (Attention::Waiting, "waiting for usage".into());
    }
    match s.herdr {
        // The silent wait: the agent stopped, and nothing it wrote says what for.
        Some("done") | Some("idle") if s.from_feed => (
            Attention::NeedsYou,
            "its turn ended; nothing says what it's waiting on".into(),
        ),
        Some("done") => (Attention::NeedsYou, "finished, not yet seen".into()),
        Some("idle") | Some("unknown") => (Attention::Idle, "idle".into()),
        _ => match s.feed {
            Some(Attention::Running) => (Attention::Running, status_or("under way")),
            Some(att) => (att, status_or(att.label())),
            None => (Attention::Idle, status_or("idle")),
        },
    }
}

fn usage_waiting(machine: &MachineState, workspace: &WsRef) -> bool {
    machine.feed.as_ref().is_some_and(|feed| {
        workspace.agents.iter().any(|agent| {
            feed.agents.iter().any(|a| {
                a.pane == agent.pane_id && a.session == workspace.session && a.waiting_for_usage()
            })
        })
    })
}

fn parse_time(s: &str) -> Option<DateTime<Local>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s.trim()) {
        return Some(t.with_timezone(&Local));
    }
    let date = NaiveDate::parse_from_str(s.trim().get(..10)?, "%Y-%m-%d").ok()?;
    Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0)?)
        .single()
}

fn last_two(id: &str) -> Option<String> {
    let digits: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.len() >= 2).then(|| digits[digits.len() - 2..].to_string())
}

pub fn build(machines: &[MachineState], cfg: &Config) -> Board {
    let all_ws = workspaces(machines, cfg);
    let mut claimed: HashSet<(usize, String, String)> = HashSet::new();
    let mut cards = Vec::new();

    // One copy of each feed card: the one from its home machine, else one that's live.
    let mut chosen: BTreeMap<String, (usize, &FeedCard, &Feed)> = BTreeMap::new();
    for (mi, m) in machines.iter().enumerate() {
        if !m.is_shown() {
            continue;
        }
        let Some(feed) = &m.feed else { continue };
        let source = feed.source.clone().unwrap_or_else(|| "feed".into());
        for fc in &feed.cards {
            let session = fc
                .workspace
                .as_ref()
                .and_then(|w| w.session.as_deref())
                .or_else(|| fc.home.as_ref().and_then(|h| h.session.as_deref()));
            if session.is_some_and(|s| !cfg.board.wants(s)) {
                continue;
            }
            let key = format!("{source}:{}", fc.id);
            let home_here = |mi: usize| {
                fc.home
                    .as_ref()
                    .and_then(|h| h.machine.as_deref())
                    .is_some_and(|h| machines[mi].answers_to(h))
            };
            let better = match chosen.get(&key) {
                None => true,
                Some((pmi, _, _)) => {
                    (home_here(mi) && !home_here(*pmi))
                        || (home_here(mi) == home_here(*pmi)
                            && m.conn.is_live()
                            && !machines[*pmi].conn.is_live())
                }
            };
            if better {
                chosen.insert(key, (mi, fc, feed));
            }
        }
    }

    // Each feed card's stage and home, and the workspace its feed names.
    struct Pending<'a> {
        key: &'a String,
        mi: usize,
        fc: &'a FeedCard,
        state: String,
        stage: Stage,
        home_mi: Option<usize>,
        home_session: Option<String>,
        ws: Option<usize>,
        /// The tab of that workspace the feed says the work runs in.
        tab: Option<String>,
    }
    let ws_key = |w: &WsRef| (w.mi, w.session.clone(), w.ws.workspace_id.clone());
    let mut pending: Vec<Pending> = Vec::new();
    for (key, (mi, fc, _feed)) in &chosen {
        let state = fc.state.as_deref().unwrap_or("active").to_ascii_lowercase();
        let stage = fc
            .stage
            .as_deref()
            .and_then(Stage::parse)
            .unwrap_or(if state == "closed" {
                Stage::Maintain
            } else if state == "queued" || state == "shelved" {
                Stage::Backlog
            } else {
                Stage::Build
            });
        let home_mi = fc
            .home
            .as_ref()
            .and_then(|h| h.machine.as_deref())
            .and_then(|h| {
                machines
                    .iter()
                    .position(|m| m.is_shown() && m.answers_to(h))
            });
        let (mut ws, mut tab) = (None, None);
        if let Some(fw) = &fc.workspace
            && let (Some(session), Some(id)) = (&fw.session, &fw.id)
        {
            ws = all_ws
                .iter()
                .position(|w| w.mi == *mi && &w.session == session && &w.ws.workspace_id == id);
            tab = ws.and(fw.tab.clone()).filter(|t| !t.trim().is_empty());
        }
        // Work in one tab of a workspace doesn't make the workspace its own: the workspace's card
        // can still be found by name, and agents in its other tabs still show.
        if let Some(i) = ws
            && tab.is_none()
        {
            claimed.insert(ws_key(&all_ws[i]));
        }
        pending.push(Pending {
            key,
            mi: *mi,
            fc,
            state,
            stage,
            home_mi,
            home_session: fc.home.as_ref().and_then(|h| h.session.clone()),
            ws,
            tab,
        });
    }

    // A workspace named for a card ("43-...") is only a guess, so it's taken only when it's sure:
    // the agent's pane title names the card, or exactly one open card in that session ends in
    // those digits and no other card has the workspace. (#43 and #143 both end in 43.)
    let open_work = |p: &Pending| !matches!(p.stage, Stage::Backlog | Stage::Maintain);
    for k in 0..pending.len() {
        let p = &pending[k];
        if p.ws.is_some() || !open_work(p) {
            continue;
        }
        let (Some(hmi), Some(session), Some(two)) =
            (p.home_mi, p.home_session.clone(), last_two(&p.fc.id))
        else {
            continue;
        };
        let prefix = format!("{two}-");
        let candidates: Vec<usize> = all_ws
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.mi == hmi
                    && w.session == session
                    && w.ws.label.starts_with(&prefix)
                    && !claimed.contains(&ws_key(w))
            })
            .map(|(i, _)| i)
            .collect();
        let named_as = format!("{} ", p.fc.reference());
        let named: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| {
                all_ws[i]
                    .agents
                    .iter()
                    .any(|a| a.title.as_deref().is_some_and(|t| t.contains(&named_as)))
            })
            .collect();
        let pick = if named.len() == 1 {
            Some(named[0])
        } else if candidates.len() == 1 {
            let rivals = pending
                .iter()
                .filter(|o| {
                    open_work(o)
                        && o.ws.is_none()
                        && o.home_mi == Some(hmi)
                        && o.home_session.as_deref() == Some(session.as_str())
                        && last_two(&o.fc.id).as_deref() == Some(two.as_str())
                })
                .count();
            (rivals == 1).then_some(candidates[0])
        } else {
            None
        };
        if let Some(i) = pick {
            claimed.insert(ws_key(&all_ws[i]));
            pending[k].ws = Some(i);
        }
    }

    // A card whose work runs in one tab of a shared workspace reads only that tab's agents, and
    // the workspace's own card (a feed's, or a card for the workspace itself) leaves out the tabs
    // such cards claim, so one agent's question shows on one card.
    let tabbed: HashSet<(usize, &str)> = pending
        .iter()
        .filter_map(|p| Some((p.ws?, p.tab.as_deref()?)))
        .collect();
    let untabbed = |i: usize| {
        all_ws[i].only(|a| {
            !a.tab_id
                .as_deref()
                .is_some_and(|at| tabbed.contains(&(i, at)))
        })
    };
    for p in &pending {
        let fc = p.fc;
        let closed = p.state == "closed";
        let own = p.ws.map(|i| match &p.tab {
            Some(tab) => all_ws[i].only(|a| a.tab_id.as_deref() == Some(tab.as_str())),
            None => untabbed(i),
        });
        let ws: Option<&WsRef> = own.as_ref();
        let card_mi = ws.map(|w| w.mi).or(p.home_mi).unwrap_or(p.mi);
        let herdr = ws.and_then(herdr_status);
        let title = ws.and_then(running_title);
        // What an agent reported for the workspace is the card's it belongs to, not a tab's.
        let token = ws
            .filter(|_| p.tab.is_none())
            .and_then(|w| w.ws.tokens.get("kerdr_status"))
            .map(String::as_str);
        let (att, why) = attention(&Signals {
            closed,
            usage_waiting: ws.is_some_and(|w| usage_waiting(&machines[w.mi], w)),
            token,
            herdr: herdr.as_deref(),
            feed: fc.attention.as_deref().and_then(Attention::parse),
            feed_status: fc.status.as_deref(),
            needs: fc.needs.as_deref(),
            blocker: fc.blockers.first().map(String::as_str),
            running_title: title.as_deref(),
            from_feed: true,
        });
        let session = ws
            .map(|w| w.session.clone())
            .or_else(|| fc.workspace.as_ref().and_then(|w| w.session.clone()))
            .or(p.home_session.clone());
        cards.push(Card {
            key: p.key.clone(),
            reference: fc.reference(),
            title: fc.title.clone(),
            stage: p.stage,
            attention: att,
            why,
            machine: machines[card_mi].name(),
            machine_index: card_mi,
            session,
            workspace_id: ws.map(|w| w.ws.workspace_id.clone()),
            workspace_label: ws.map(|w| w.ws.label.clone()),
            tab_id: p.tab.clone(),
            priority: fc
                .priority
                .as_deref()
                .map(Priority::parse)
                .unwrap_or_default(),
            background: fc.lane.as_deref() == Some("background") || p.state == "background",
            cached_at: machines[card_mi].cached_at(),
            updated: fc.updated.as_deref().and_then(parse_time),
            stale_days: fc.stale_days,
            state: fc.state.clone(),
            detail: Some(fc.clone()),
            agents: ws.map(agent_infos).unwrap_or_default(),
            source: Source::Feed,
            progress: fc.criteria_progress(),
        });
    }

    // Every other workspace with an agent, or with a stage an agent reported, is a card too. One
    // that only cards for its tabs claim is a card for the agents in its other tabs.
    for (i, w) in all_ws.iter().enumerate() {
        if claimed.contains(&ws_key(w)) {
            continue;
        }
        let w = &untabbed(i);
        let reported_stage = w.ws.tokens.get("kerdr_stage").and_then(|s| Stage::parse(s));
        if w.agents.is_empty() && reported_stage.is_none() {
            continue;
        }
        let m = &machines[w.mi];
        let herdr = herdr_status(w);
        let title = running_title(w);
        let token = w.ws.tokens.get("kerdr_status").map(String::as_str);
        let note = w.ws.tokens.get("kerdr_note").map(String::as_str);
        let (att, why) = attention(&Signals {
            closed: reported_stage == Some(Stage::Maintain),
            usage_waiting: usage_waiting(m, w),
            token,
            herdr: herdr.as_deref(),
            feed: None,
            feed_status: note,
            needs: note,
            blocker: note,
            running_title: title.as_deref(),
            from_feed: false,
        });
        let agent_title = w
            .agents
            .iter()
            .find_map(|a| a.title.as_deref().map(herdr::clean_title))
            .filter(|t| !t.is_empty())
            .or_else(|| {
                w.agents
                    .iter()
                    .find_map(|a| a.terminal_title_stripped.clone())
                    .filter(|t| !t.trim().is_empty())
            });
        let label = if w.ws.label.is_empty() {
            w.ws.workspace_id.clone()
        } else {
            w.ws.label.clone()
        };
        cards.push(Card {
            key: format!("ws:{}:{}:{}", m.name(), w.session, w.ws.workspace_id),
            reference: label.clone(),
            title: agent_title.unwrap_or_else(|| label.clone()),
            stage: Stage::Misc,
            attention: att,
            why,
            machine: m.name(),
            machine_index: w.mi,
            session: Some(w.session.clone()),
            workspace_id: Some(w.ws.workspace_id.clone()),
            workspace_label: Some(label),
            tab_id: None,
            priority: Priority::Normal,
            background: cfg.board.is_unlimited(&w.session),
            cached_at: m.cached_at(),
            updated: None,
            stale_days: None,
            state: None,
            detail: None,
            agents: agent_infos(w),
            source: Source::Workspace,
            progress: w
                .ws
                .tokens
                .get("kerdr_progress")
                .and_then(|p| parse_progress(p)),
        });
    }

    sort_cards(&mut cards);

    // Sessions and their slots: the feed's count when it gives one, else Kerdr's own.
    let mut sessions = Vec::new();
    let mut machine_order = Vec::new();
    for (mi, m) in machines.iter().enumerate() {
        if !m.is_shown() {
            continue;
        }
        let name = m.name();
        machine_order.push(name.clone());
        let Some(data) = &m.data else { continue };
        for info in data.sessions.iter().filter(|s| cfg.board.wants(&s.name)) {
            let unlimited = cfg.board.is_unlimited(&info.name);
            let feed_slots = m.feed.as_ref().and_then(|f| f.sessions.get(&info.name));
            let used_by_agents = all_ws
                .iter()
                .filter(|w| w.mi == mi && w.session == info.name && !w.agents.is_empty())
                .count() as u32;
            let slots = match feed_slots {
                Some(fs) => Slots {
                    used: fs.used,
                    limit: fs.limit,
                },
                None => Slots {
                    used: used_by_agents,
                    limit: if unlimited {
                        None
                    } else {
                        Some(cfg.board.slots)
                    },
                },
            };
            let in_session =
                |c: &&Card| c.machine == name && c.session.as_deref() == Some(info.name.as_str());
            let open_cards = cards
                .iter()
                .filter(in_session)
                .filter(|c| !matches!(c.stage, Stage::Backlog | Stage::Maintain | Stage::Misc))
                .count();
            sessions.push(SessionRow {
                machine: name.clone(),
                session: info.name.clone(),
                running: info.running,
                slots,
                needs: cards
                    .iter()
                    .filter(in_session)
                    .filter(|c| c.stage != Stage::Misc)
                    .filter(|c| c.needs_you())
                    .count(),
                cards: open_cards,
                cached: !m.conn.is_live(),
                error: data
                    .snapshots
                    .get(&info.name)
                    .and_then(|s| s.as_ref().err().cloned())
                    .or_else(|| m.feed_error.clone()),
            });
        }
    }

    // Each workspace's own card: a card for work in one of its tabs only when it has none.
    let mut card_for_ws: BTreeMap<(String, String, String), &Card> = BTreeMap::new();
    for c in &cards {
        if let (Some(session), Some(id)) = (&c.session, &c.workspace_id) {
            let key = (c.machine.clone(), session.clone(), id.clone());
            if c.tab_id.is_none() {
                card_for_ws.insert(key, c);
            } else {
                card_for_ws.entry(key).or_insert(c);
            }
        }
    }
    let workspaces = all_ws
        .iter()
        .map(|w| {
            let m = &machines[w.mi];
            let card = card_for_ws.get(&(m.name(), w.session.clone(), w.ws.workspace_id.clone()));
            WorkspaceRow {
                machine: m.name(),
                session: w.session.clone(),
                id: w.ws.workspace_id.clone(),
                label: w.ws.label.clone(),
                status: herdr_status(w).unwrap_or_else(|| "no agent".into()),
                card: card.map(|c| c.key.clone()),
                note: card
                    .filter(|c| c.attention == Attention::Done)
                    .map(|_| "its card is done".to_string()),
                cached: !m.conn.is_live(),
            }
        })
        .collect();

    Board {
        cards,
        sessions,
        workspaces,
        machine_order,
    }
}

fn agent_infos(w: &WsRef) -> Vec<AgentInfo> {
    w.agents
        .iter()
        .map(|a| AgentInfo {
            agent: a.agent.clone().unwrap_or_else(|| "agent".into()),
            status: a.agent_status.clone().unwrap_or_else(|| "unknown".into()),
            pane_id: a.pane_id.clone(),
            title: a.title.as_deref().map(herdr::clean_title),
        })
        .collect()
}

pub fn parse_progress(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('/')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (b > 0 && a <= b).then_some((a, b))
}

fn sort_cards(cards: &mut [Card]) {
    cards.sort_by(|a, b| {
        a.stage.cmp(&b.stage).then_with(|| match a.stage {
            Stage::Backlog => a
                .backlog_rank()
                .cmp(&b.backlog_rank())
                .then_with(|| a.reference.cmp(&b.reference)),
            Stage::Maintain => {
                let closed = |c: &Card| {
                    c.detail
                        .as_ref()
                        .and_then(|d| d.closed.clone())
                        .unwrap_or_default()
                };
                closed(b)
                    .cmp(&closed(a))
                    .then_with(|| b.reference.cmp(&a.reference))
            }
            _ => a
                .attention
                .cmp(&b.attention)
                .then_with(|| a.priority.cmp(&b.priority))
                .then_with(|| a.cached_at.is_some().cmp(&b.cached_at.is_some()))
                .then_with(|| a.reference.cmp(&b.reference)),
        })
    });
}

/// Which part of the board is in view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Machine(String),
    Session(String, String),
}

impl Scope {
    pub fn contains(&self, card: &Card) -> bool {
        match self {
            Scope::All => true,
            Scope::Machine(m) => &card.machine == m,
            Scope::Session(m, s) => {
                &card.machine == m && card.session.as_deref() == Some(s.as_str())
            }
        }
    }

    pub fn contains_session(&self, row: &SessionRow) -> bool {
        match self {
            Scope::All => true,
            Scope::Machine(m) => &row.machine == m,
            Scope::Session(m, s) => &row.machine == m && &row.session == s,
        }
    }
}

pub fn summarize(board: &Board, scope: &Scope, machines: &[MachineState]) -> Summary {
    let cards: Vec<&Card> = board
        .cards
        .iter()
        .filter(|c| scope.contains(c) && !c.dismissed() && c.stage != Stage::Misc)
        .collect();
    let open = |c: &&&Card| !matches!(c.stage, Stage::Backlog | Stage::Maintain);
    let mut s = Summary {
        needs_you: cards
            .iter()
            .filter(open)
            .filter(|c| c.attention == Attention::NeedsYou)
            .count(),
        running: cards
            .iter()
            .filter(open)
            .filter(|c| matches!(c.attention, Attention::Running | Attention::Waiting))
            .count(),
        blocked: cards
            .iter()
            .filter(open)
            .filter(|c| c.attention == Attention::Blocked)
            .count(),
        unreachable: cards
            .iter()
            .filter(open)
            .filter(|c| c.attention == Attention::Unreachable)
            .count(),
        cached: cards
            .iter()
            .filter(open)
            .filter(|c| c.cached_at.is_some())
            .count(),
        cached_since: cards.iter().filter_map(|c| c.cached_at).min(),
        backlog: cards.iter().filter(|c| c.stage == Stage::Backlog).count(),
        queued: cards
            .iter()
            .filter(|c| c.stage == Stage::Backlog && c.state.as_deref() != Some("shelved"))
            .count(),
        shelved: cards
            .iter()
            .filter(|c| c.state.as_deref() == Some("shelved"))
            .count(),
        ..Default::default()
    };
    let rows: Vec<&SessionRow> = board
        .sessions
        .iter()
        .filter(|r| scope.contains_session(r))
        .collect();
    for r in rows.iter().filter(|r| r.running) {
        if let Some(limit) = r.slots.limit {
            s.slots_used += r.slots.used;
            s.slots_limit += limit;
            if r.slots.is_full() {
                s.full_sessions.push(r.session.clone());
            }
        }
    }
    let _ = machines;
    let limited: Vec<&&SessionRow> = rows
        .iter()
        .filter(|r| r.slots.limit.is_some() && r.running)
        .collect();
    s.verdict = Some(if s.needs_you > 0 {
        Verdict::FinishFirst(s.needs_you)
    } else if limited.is_empty() {
        Verdict::Empty
    } else if let Some(best) = limited
        .iter()
        .filter(|r| !r.slots.is_full())
        .max_by_key(|r| {
            (
                r.slots.free().unwrap_or(0),
                std::cmp::Reverse(r.session.clone()),
            )
        })
    {
        let machines_in_view: std::collections::BTreeSet<&str> =
            limited.iter().map(|r| r.machine.as_str()).collect();
        let place = if machines_in_view.len() > 1 {
            format!("{} on {}", best.session, best.machine)
        } else {
            best.session.clone()
        };
        Verdict::Room {
            session: place,
            free: best.slots.free().unwrap_or(0),
        }
    } else {
        Verdict::Full
    });
    s
}

/// The `kerdr_jump` token one of Kerdr's in-herdr keys left on a workspace, if any.
pub fn jump_request(workspaces: &[Workspace]) -> Option<(String, String)> {
    workspaces.iter().find_map(|w| {
        w.tokens
            .get(JUMP_TOKEN)
            .map(|v| (w.workspace_id.clone(), v.clone()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_backlog_order_is_independent_of_affirmation_age() {
        let board = build(&crate::demo::machines(false), &Config::default());
        let seed = board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Backlog)
            .unwrap();
        let mut older = seed.clone();
        older.reference = "#1".into();
        older.stale_days = Some(99);
        older.detail.as_mut().unwrap().backlog_rank = Some(2);
        let mut first = seed.clone();
        first.reference = "#2".into();
        first.stale_days = Some(0);
        first.detail.as_mut().unwrap().backlog_rank = Some(1);
        let mut cards = vec![older, first];
        sort_cards(&mut cards);
        assert_eq!(cards[0].reference, "#2");
    }
    use crate::feed::{FeedSlots, FeedWorkspace, Home};
    use crate::herdr::SessionInfo;

    fn machine(name: &str, local: bool) -> Machine {
        Machine {
            name: name.into(),
            ssh: (!local).then(|| name.to_lowercase()),
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        }
    }

    fn ws(id: &str, label: &str) -> Workspace {
        Workspace {
            workspace_id: id.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    fn agent(ws: &str, status: &str) -> Agent {
        Agent {
            agent: Some("claude".into()),
            agent_status: Some(status.into()),
            pane_id: format!("{ws}:p1"),
            workspace_id: ws.into(),
            ..Default::default()
        }
    }

    fn live(machine: Machine, sessions: Vec<(&str, Snapshot)>, feed: Option<Feed>) -> MachineState {
        let mut m = MachineState::new(machine);
        m.conn = Conn::Live {
            latency: Default::default(),
        };
        m.data = Some(MachineData {
            host: Some(m.machine.name.clone()),
            sessions: sessions
                .iter()
                .map(|(n, _)| SessionInfo {
                    name: n.to_string(),
                    running: true,
                    default: false,
                })
                .collect(),
            snapshots: sessions
                .into_iter()
                .map(|(n, s)| (n.to_string(), Ok(s)))
                .collect(),
            ..Default::default()
        });
        m.feed = feed;
        m
    }

    fn feed_card(id: &str, stage: &str, state: &str) -> FeedCard {
        FeedCard {
            id: id.into(),
            title: format!("card {id}"),
            stage: Some(stage.into()),
            state: Some(state.into()),
            home: Some(Home {
                machine: Some("build-box".into()),
                session: Some("default".into()),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn tenant_background_sessions_have_unlimited_slots_and_background_cards() {
        let mut cfg = Config::default();
        cfg.board.unlimited = vec!["*-bg".into(), "background".into()];
        let sessions = ["pick4me-bg", "arknet-bg", "pick4me"]
            .into_iter()
            .map(|name| {
                (
                    name,
                    Snapshot {
                        workspaces: vec![ws("w1", name)],
                        agents: vec![agent("w1", "working")],
                        ..Default::default()
                    },
                )
            })
            .collect();
        let board = build(&[live(machine("laptop", true), sessions, None)], &cfg);
        for name in ["pick4me-bg", "arknet-bg", "pick4me"] {
            let background = name.ends_with("-bg");
            let session = board.sessions.iter().find(|s| s.session == name).unwrap();
            assert_eq!(session.slots.limit, if background { None } else { Some(3) });
            assert_eq!(session.slots.used, 1);
            let card = board
                .cards
                .iter()
                .find(|c| c.session.as_deref() == Some(name))
                .unwrap();
            assert_eq!(card.background, background);
        }
    }

    #[test]
    fn the_silent_wait_needs_you() {
        let (att, why) = attention(&Signals {
            usage_waiting: false,
            closed: false,
            token: None,
            herdr: Some("done"),
            feed: Some(Attention::Running),
            feed_status: Some("Under way"),
            needs: None,
            blocker: None,
            running_title: None,
            from_feed: true,
        });
        assert_eq!(att, Attention::NeedsYou);
        assert!(why.contains("turn ended"));
    }

    #[test]
    fn usage_wait_suppresses_only_silence_and_never_an_explicit_question() {
        let mut signals = Signals {
            usage_waiting: true,
            closed: false,
            token: None,
            herdr: Some("done"),
            feed: None,
            feed_status: None,
            needs: None,
            blocker: None,
            running_title: None,
            from_feed: true,
        };
        assert_eq!(attention(&signals).0, Attention::Waiting);
        signals.herdr = Some("blocked");
        assert_eq!(attention(&signals).0, Attention::NeedsYou);
        signals.herdr = Some("done");
        signals.token = Some("needs_you");
        assert_eq!(attention(&signals).0, Attention::NeedsYou);
        signals.token = None;
        signals.feed = Some(Attention::NeedsYou);
        assert_eq!(attention(&signals).0, Attention::NeedsYou);
        signals.feed = Some(Attention::Blocked);
        assert_eq!(attention(&signals).0, Attention::Blocked);
        signals.feed = None;
        signals.herdr = Some("working");
        assert_eq!(attention(&signals).0, Attention::Running);
    }

    #[test]
    fn waiting_on_a_review_is_not_needing_you() {
        let (att, why) = attention(&Signals {
            usage_waiting: false,
            closed: false,
            token: None,
            herdr: Some("idle"),
            feed: Some(Attention::Waiting),
            feed_status: Some("Waiting on Codex's review"),
            needs: None,
            blocker: None,
            running_title: None,
            from_feed: true,
        });
        assert_eq!(att, Attention::Waiting);
        assert_eq!(why, "Waiting on Codex's review");
    }

    #[test]
    fn an_agent_asking_always_needs_you() {
        let (att, _) = attention(&Signals {
            usage_waiting: false,
            closed: false,
            token: None,
            herdr: Some("blocked"),
            feed: Some(Attention::Waiting),
            feed_status: None,
            needs: None,
            blocker: None,
            running_title: None,
            from_feed: true,
        });
        assert_eq!(att, Attention::NeedsYou);
    }

    #[test]
    fn feed_cards_find_their_workspace_by_name_and_others_become_cards() {
        let snap = Snapshot {
            workspaces: vec![
                ws("w3", "43-next-flywheel"),
                ws("w4", "notes"),
                ws("w5", "omarchy-automation"),
            ],
            agents: vec![agent("w3", "done"), agent("w5", "working")],
            ..Default::default()
        };
        let mut c = feed_card("0143", "test", "active");
        c.attention = Some("running".into());
        let feed = Feed {
            kerdr_feed: 1,
            source: Some("api".into()),
            cards: vec![c, feed_card("0002", "backlog", "queued")],
            sessions: [(
                "default".to_string(),
                FeedSlots {
                    used: 3,
                    limit: Some(3),
                    max: Some(5),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let machines = vec![live(
            machine("build-box", true),
            vec![("default", snap)],
            Some(feed),
        )];
        let board = build(&machines, &Config::default());

        let flywheel = board.cards.iter().find(|c| c.reference == "#143").unwrap();
        assert_eq!(flywheel.workspace_id.as_deref(), Some("w3"));
        assert_eq!(flywheel.attention, Attention::NeedsYou);
        assert_eq!(flywheel.stage, Stage::Test);

        let automation = board
            .cards
            .iter()
            .find(|c| c.reference == "omarchy-automation")
            .unwrap();
        assert_eq!(automation.attention, Attention::Running);
        assert_eq!(automation.stage.id(), "misc");
        assert!(
            board.cards.iter().all(|c| c.reference != "notes"),
            "a workspace without an agent isn't a card"
        );

        let summary = summarize(&board, &Scope::All, &machines);
        assert_eq!(summary.needs_you, 1);
        assert_eq!(
            summary.running, 0,
            "an unlinked automation is not intent work"
        );
        assert_eq!(board.sessions[0].cards, 1);
        assert_eq!(summary.backlog, 1);
        assert_eq!(summary.slots_used, 3);
        assert_eq!(summary.full_sessions, ["default"]);
        assert_eq!(summary.verdict, Some(Verdict::FinishFirst(1)));
    }

    #[test]
    fn a_workspace_named_for_two_cards_goes_to_neither_unless_its_title_says() {
        let mut flywheel = feed_card("0143", "test", "active");
        flywheel.attention = Some("running".into());
        let old = feed_card("0043", "build", "active");
        let snap = |title: &str| Snapshot {
            workspaces: vec![ws("w3", "43-next-flywheel")],
            agents: vec![Agent {
                title: Some(title.into()),
                ..agent("w3", "working")
            }],
            ..Default::default()
        };
        let feed = Feed {
            kerdr_feed: 1,
            cards: vec![flywheel, old],
            ..Default::default()
        };
        let board = build(
            &[live(
                machine("build-box", true),
                vec![("default", snap("a shell"))],
                Some(feed.clone()),
            )],
            &Config::default(),
        );
        assert!(
            board
                .cards
                .iter()
                .filter(|c| c.reference.starts_with('#'))
                .all(|c| c.workspace_id.is_none())
        );
        let board = build(
            &[live(
                machine("build-box", true),
                vec![(
                    "default",
                    snap("📌 #143 Under way · Find the next flywheel"),
                )],
                Some(feed),
            )],
            &Config::default(),
        );
        let card = |r: &str| board.cards.iter().find(|c| c.reference == r).unwrap();
        assert_eq!(card("#143").workspace_id.as_deref(), Some("w3"));
        assert_eq!(card("#43").workspace_id, None);
    }

    #[test]
    fn a_card_in_a_tab_reads_that_tabs_agents_and_the_workspaces_card_the_rest() {
        let in_w1 = |tab: Option<&str>| FeedWorkspace {
            session: Some("default".into()),
            id: Some("w1".into()),
            label: None,
            tab: tab.map(Into::into),
        };
        let mut parent = feed_card("0237", "build", "active");
        parent.workspace = Some(in_w1(None));
        let mut child = feed_card("0246", "build", "active");
        child.workspace = Some(in_w1(Some("w1:t2")));
        // A card whose workspace isn't open has no tab to land on either.
        let mut gone = feed_card("0247", "build", "active");
        gone.workspace = Some(FeedWorkspace {
            id: Some("w9".into()),
            ..in_w1(Some("w9:t2"))
        });
        let board = |parent_status: &str, child_status: &str, token: Option<&str>| {
            let mut w1 = ws("w1", "37-parent");
            if let Some(t) = token {
                w1.tokens.insert("kerdr_status".into(), t.into());
            }
            let in_tab = |tab: &str, pane: &str, status: &str| Agent {
                pane_id: pane.into(),
                tab_id: Some(tab.into()),
                ..agent("w1", status)
            };
            let snap = Snapshot {
                workspaces: vec![w1],
                agents: vec![
                    in_tab("w1:t1", "w1:p1", parent_status),
                    in_tab("w1:t2", "w1:p2", child_status),
                ],
                ..Default::default()
            };
            let feed = Feed {
                kerdr_feed: 1,
                cards: vec![parent.clone(), child.clone(), gone.clone()],
                ..Default::default()
            };
            build(
                &[live(
                    machine("build-box", true),
                    vec![("default", snap)],
                    Some(feed),
                )],
                &Config::default(),
            )
        };
        let card = |b: &Board, r: &str| b.cards.iter().find(|c| c.reference == r).unwrap().clone();
        let panes = |c: &Card| {
            c.agents
                .iter()
                .map(|a| a.pane_id.clone())
                .collect::<Vec<_>>()
        };

        // The child's agent asks: only the child's card needs you, and Enter lands on its tab.
        let b = board("working", "blocked", None);
        let (p, c) = (card(&b, "#237"), card(&b, "#246"));
        assert_eq!(c.workspace_id.as_deref(), Some("w1"));
        assert_eq!(c.tab_id.as_deref(), Some("w1:t2"));
        assert_eq!(panes(&c), ["w1:p2"]);
        assert_eq!(c.attention, Attention::NeedsYou);
        assert_eq!(p.workspace_id.as_deref(), Some("w1"));
        assert_eq!(p.tab_id, None);
        assert_eq!(panes(&p), ["w1:p1"]);
        assert_ne!(p.attention, Attention::NeedsYou);
        assert_eq!(card(&b, "#247").tab_id, None);
        assert!(
            b.cards.iter().all(|c| c.reference != "37-parent"),
            "the shared workspace isn't a card of its own"
        );

        // What an agent reported for the workspace is the workspace card's, not the tab's.
        let b = board("working", "working", Some("needs_you"));
        assert_eq!(card(&b, "#237").attention, Attention::NeedsYou);
        assert_ne!(card(&b, "#246").attention, Attention::NeedsYou);
    }

    #[test]
    fn a_workspace_only_tab_cards_claim_keeps_its_other_agents_and_its_name() {
        let in_tab = |tab: Option<&str>, pane: &str, status: &str| Agent {
            pane_id: pane.into(),
            tab_id: tab.map(Into::into),
            ..agent("w1", status)
        };
        let snap = || Snapshot {
            workspaces: vec![Workspace {
                // herdr's status for the whole workspace: the child's question.
                agent_status: Some("blocked".into()),
                ..ws("w1", "37-parent")
            }],
            agents: vec![
                in_tab(Some("w1:t1"), "w1:p1", "working"),
                in_tab(Some("w1:t2"), "w1:p2", "starting"),
                in_tab(None, "w1:p3", "idle"),
            ],
            ..Default::default()
        };
        let mut child = feed_card("0246", "test", "active");
        child.workspace = Some(FeedWorkspace {
            session: Some("default".into()),
            id: Some("w1".into()),
            label: None,
            tab: Some("w1:t2".into()),
        });
        let board_of = |cards: Vec<FeedCard>| {
            build(
                &[live(
                    machine("build-box", true),
                    vec![("default", snap())],
                    Some(Feed {
                        kerdr_feed: 1,
                        cards,
                        ..Default::default()
                    }),
                )],
                &Config::default(),
            )
        };
        let panes = |c: &Card| {
            c.agents
                .iter()
                .map(|a| a.pane_id.clone())
                .collect::<Vec<_>>()
        };

        // No card for the workspace itself: its other agents are a card of their own.
        let b = board_of(vec![child.clone()]);
        let c = b.cards.iter().find(|c| c.reference == "#246").unwrap();
        assert_eq!(panes(c), ["w1:p2"]);
        assert_ne!(
            c.attention,
            Attention::NeedsYou,
            "the workspace's status isn't its tab's"
        );
        let rest = b.cards.iter().find(|c| c.reference == "37-parent").unwrap();
        assert_eq!(panes(rest), ["w1:p1", "w1:p3"]);
        assert_eq!(rest.tab_id, None);
        let row = b.workspaces.iter().find(|w| w.id == "w1").unwrap();
        assert_eq!(row.card.as_deref(), Some(rest.key.as_str()));

        // A parent card found by the workspace's name takes it, and the workspace list names the
        // parent's card, not the child's, whatever order they sort in.
        let b = board_of(vec![child, feed_card("0237", "build", "active")]);
        let p = b.cards.iter().find(|c| c.reference == "#237").unwrap();
        assert_eq!(p.workspace_id.as_deref(), Some("w1"));
        assert_eq!(panes(p), ["w1:p1", "w1:p3"]);
        assert!(b.cards.iter().all(|c| c.reference != "37-parent"));
        let row = b.workspaces.iter().find(|w| w.id == "w1").unwrap();
        assert_eq!(row.card.as_deref(), Some(p.key.as_str()));

        // What was reported for a workspace whose own agent has gone stays on its card, and the
        // card for the tab still left doesn't take it on.
        let mut w1 = ws("w1", "37-parent");
        for (k, v) in [("kerdr_stage", "test"), ("kerdr_status", "needs_you")] {
            w1.tokens.insert(k.into(), v.into());
        }
        let mut child = feed_card("0246", "build", "active");
        child.workspace = Some(FeedWorkspace {
            session: Some("default".into()),
            id: Some("w1".into()),
            label: None,
            tab: Some("w1:t2".into()),
        });
        let b = build(
            &[live(
                machine("build-box", true),
                vec![(
                    "default",
                    Snapshot {
                        workspaces: vec![w1],
                        agents: vec![in_tab(Some("w1:t2"), "w1:p2", "working")],
                        ..Default::default()
                    },
                )],
                Some(Feed {
                    kerdr_feed: 1,
                    cards: vec![child],
                    ..Default::default()
                }),
            )],
            &Config::default(),
        );
        let reported = b.cards.iter().find(|c| c.reference == "37-parent").unwrap();
        assert!(reported.agents.is_empty());
        assert_eq!(reported.attention, Attention::NeedsYou);
        let c = b.cards.iter().find(|c| c.reference == "#246").unwrap();
        assert_ne!(c.attention, Attention::NeedsYou);
    }

    #[test]
    fn the_home_machines_copy_of_a_card_wins() {
        let mut far = feed_card("0125", "build", "active");
        far.home = Some(Home {
            machine: Some("laptop".into()),
            session: Some("default".into()),
        });
        far.status = Some("from the server".into());
        let mut near = far.clone();
        near.status = Some("from the laptop".into());
        near.workspace = Some(FeedWorkspace {
            session: Some("default".into()),
            id: Some("w1".into()),
            label: None,
            tab: None,
        });
        let server = live(
            machine("build-box", true),
            vec![("default", Snapshot::default())],
            Some(Feed {
                kerdr_feed: 1,
                cards: vec![far],
                ..Default::default()
            }),
        );
        let laptop = live(
            machine("laptop", false),
            vec![(
                "default",
                Snapshot {
                    workspaces: vec![ws("w1", "25-quick-prompt")],
                    ..Default::default()
                },
            )],
            Some(Feed {
                kerdr_feed: 1,
                cards: vec![near],
                ..Default::default()
            }),
        );
        let board = build(&[server, laptop], &Config::default());
        let card = board.cards.iter().find(|c| c.reference == "#125").unwrap();
        assert_eq!(card.machine, "laptop");
        assert_eq!(card.workspace_id.as_deref(), Some("w1"));
        assert_eq!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("from the laptop")
        );
    }

    #[test]
    fn cards_from_a_machine_that_dropped_are_marked_cached() {
        let snap = Snapshot {
            workspaces: vec![ws("w1", "api")],
            agents: vec![agent("w1", "working")],
            ..Default::default()
        };
        let mut m = live(machine("box", false), vec![("default", snap)], None);
        m.last_ok = Some(Local::now());
        m.conn = Conn::Reconnecting {
            since: Local::now(),
            attempt: 1,
            retry_at: Local::now(),
            detail: "timed out".into(),
        };
        let board = build(&[m], &Config::default());
        assert!(board.cards[0].cached_at.is_some());
        assert!(board.sessions[0].cached);
    }

    #[test]
    fn verdict_points_at_the_session_with_most_room() {
        let mut board = Board::default();
        for (s, used) in [("default", 3), ("api", 1), ("web", 2)] {
            board.sessions.push(SessionRow {
                machine: "m".into(),
                session: s.into(),
                running: true,
                slots: Slots {
                    used,
                    limit: Some(3),
                },
                needs: 0,
                cards: 0,
                cached: false,
                error: None,
            });
        }
        let s = summarize(&board, &Scope::All, &[]);
        assert_eq!(
            s.verdict,
            Some(Verdict::Room {
                session: "api".into(),
                free: 2
            })
        );
        assert_eq!(s.verdict.unwrap().text(), "Room for 2 more in api");
    }

    #[test]
    fn reads_jump_requests() {
        let mut w = ws("w2", "x");
        assert_eq!(jump_request(std::slice::from_ref(&w)), None);
        w.tokens.insert(JUMP_TOKEN.into(), "next:123".into());
        assert_eq!(jump_request(&[w]), Some(("w2".into(), "next:123".into())));
        assert_eq!(parse_progress("1/3"), Some((1, 3)));
        assert_eq!(parse_progress("4/3"), None);
    }
}
