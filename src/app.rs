//! Kerdr's state and its keys. The board owns the keyboard, so no key here needs a prefix;
//! the only keys that cross into herdr are herdr's own (see `herdr_keys`).

use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::board::{self, Board, MachineState, Scope};
use crate::config::Config;
use crate::model::{Attention, Card, Stage};
use crate::poll::{Conn, Event};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFilter {
    Everything,
    Cards,
    Workspaces,
    Sessions,
    DoneToo,
}

impl SearchFilter {
    pub const ALL: [SearchFilter; 5] = [
        SearchFilter::Everything,
        SearchFilter::Cards,
        SearchFilter::Workspaces,
        SearchFilter::Sessions,
        SearchFilter::DoneToo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchFilter::Everything => "everything",
            SearchFilter::Cards => "cards",
            SearchFilter::Workspaces => "workspaces",
            SearchFilter::Sessions => "sessions",
            SearchFilter::DoneToo => "done too",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Search {
    pub query: String,
    pub filter: SearchFilter,
    pub sel: usize,
}

#[derive(Clone, Debug)]
pub enum Mode {
    Board,
    Peek {
        scroll: u16,
        back: Box<Mode>,
    },
    Search(Search),
    Keys {
        filter: String,
        typing: bool,
        scroll: u16,
    },
    Machines {
        sel: usize,
    },
    Backlog {
        sel: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    /// Open herdr full screen on this card's workspace.
    Jump(String),
    /// Open herdr full screen on a workspace no card claims.
    JumpWorkspace {
        machine: String,
        session: String,
        id: String,
    },
    /// Read a machine again now: one, or every machine that isn't answering.
    Retry(Option<usize>),
    /// Run SSH in the foreground so you can answer its prompt.
    Answer(usize),
    /// Run herdr's own setup for a machine, which asks before changing anything.
    Setup(usize),
    ToggleHidden(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Card(usize),
    Workspace(usize),
    Session(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub col: usize,
    pub key: Option<String>,
}

pub struct App {
    pub cfg: Config,
    pub machines: Vec<MachineState>,
    pub board: Board,
    pub tabs: Vec<Scope>,
    pub tab: usize,
    pub sel: Selection,
    pub mode: Mode,
    pub message: Option<(String, Tone, Instant)>,
    /// A card to jump into as soon as its machine answers.
    pub pending_jump: Option<String>,
    pub width: u16,
    pub demo: bool,
    /// Fixed in demo mode so screenshots don't change; the clock otherwise.
    pub fixed_now: Option<DateTime<Local>>,
    /// herdr's prefix, for the keys Kerdr names inside herdr.
    pub prefix: String,
    first_selection_done: bool,
}

impl App {
    pub fn new(cfg: Config, machines: Vec<MachineState>) -> App {
        let mut app = App {
            cfg,
            machines,
            board: Board::default(),
            tabs: vec![Scope::All],
            tab: 0,
            sel: Selection::default(),
            mode: Mode::Board,
            message: None,
            pending_jump: None,
            width: 200,
            demo: false,
            fixed_now: None,
            prefix: "ctrl+b".to_string(),
            first_selection_done: false,
        };
        app.rebuild();
        app
    }

    pub fn now(&self) -> DateTime<Local> {
        self.fixed_now.unwrap_or_else(Local::now)
    }

    pub fn this_machine(&self) -> String {
        self.machines.first().map(|m| m.name()).unwrap_or_default()
    }

    pub fn scope(&self) -> &Scope {
        self.tabs.get(self.tab).unwrap_or(&Scope::All)
    }

    pub fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.message = Some((text.into(), tone, Instant::now()));
    }

    pub fn message(&self) -> Option<(&str, Tone)> {
        self.message
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(8))
            .map(|(t, tone, _)| (t.as_str(), *tone))
    }

    pub fn apply(&mut self, ev: Event) {
        let Some(m) = self.machines.get_mut(ev.machine) else {
            return;
        };
        if matches!(m.conn, Conn::Hidden) {
            return;
        }
        m.conn = ev.conn;
        if let Some(mut data) = ev.data {
            match data.feed.take() {
                Some(Ok(feed)) => {
                    m.feed = Some(feed);
                    m.feed_error = None;
                }
                Some(Err(e)) => m.feed_error = Some(e),
                None => {}
            }
            m.data = Some(data);
            m.last_ok = Some(ev.at);
        }
        self.rebuild();
    }

    pub fn rebuild(&mut self) {
        let current = self.scope().clone();
        self.board = board::build(&self.machines, &self.cfg);
        let mut tabs = vec![Scope::All];
        for m in self.machines.iter().filter(|m| m.is_shown()) {
            let name = m.name();
            tabs.push(Scope::Machine(name.clone()));
            // A session that isn't running has nothing on the board; herdr also leaves one behind
            // on this machine for every session it has attached to remotely.
            for r in self
                .board
                .sessions
                .iter()
                .filter(|r| r.machine == name && r.running)
            {
                tabs.push(Scope::Session(name.clone(), r.session.clone()));
            }
        }
        self.tab = tabs
            .iter()
            .position(|t| *t == current)
            .or_else(|| match &current {
                Scope::Session(m, _) => tabs.iter().position(|t| *t == Scope::Machine(m.clone())),
                _ => None,
            })
            .unwrap_or(0);
        self.tabs = tabs;
        self.fix_selection();
    }

    /// Columns drawn at this width: Backlog and Maintain give way first on a narrow screen.
    pub fn visible_columns(&self) -> [bool; 7] {
        let narrow = self.width < 100;
        let mut v = [true; 7];
        if narrow {
            v[Stage::Backlog.index()] = false;
            v[Stage::Maintain.index()] = false;
        }
        v
    }

    pub fn columns(&self) -> [Vec<usize>; 7] {
        let mut cols: [Vec<usize>; 7] = Default::default();
        let scope = self.scope();
        for (i, c) in self.board.cards.iter().enumerate() {
            if scope.contains(c) {
                cols[c.stage.index()].push(i);
            }
        }
        cols
    }

    pub fn card(&self, key: &str) -> Option<&Card> {
        self.board.cards.iter().find(|c| c.key == key)
    }

    pub fn selected(&self) -> Option<&Card> {
        self.sel.key.as_deref().and_then(|k| self.card(k))
    }

    fn fix_selection(&mut self) {
        let cols = self.columns();
        let vis = self.visible_columns();
        if let Some(key) = &self.sel.key {
            for (ci, col) in cols.iter().enumerate() {
                if vis[ci] && col.iter().any(|&i| &self.board.cards[i].key == key) {
                    self.sel.col = ci;
                    return;
                }
            }
        }
        // Start on the first card that needs you: that's what you came to the board for.
        let order = [2usize, 3, 4, 5, 1, 0, 6];
        if !self.first_selection_done || self.sel.key.is_some() {
            for ci in [1usize, 2, 3, 4, 5] {
                if let Some(&i) = cols[ci].iter().find(|&&i| self.board.cards[i].needs_you()) {
                    self.sel = Selection {
                        col: ci,
                        key: Some(self.board.cards[i].key.clone()),
                    };
                    self.first_selection_done = true;
                    return;
                }
            }
        }
        let col = if vis[self.sel.col] && !cols[self.sel.col].is_empty() {
            Some(self.sel.col)
        } else {
            order
                .iter()
                .copied()
                .find(|&ci| vis[ci] && !cols[ci].is_empty())
        };
        match col {
            Some(ci) => {
                self.sel = Selection {
                    col: ci,
                    key: Some(self.board.cards[cols[ci][0]].key.clone()),
                };
                if !self.board.cards.is_empty() {
                    self.first_selection_done = true;
                }
            }
            None => self.sel.key = None,
        }
    }

    fn select_index(&mut self, col: usize, row: usize) {
        let cols = self.columns();
        if let Some(&i) = cols[col].get(row) {
            self.sel = Selection {
                col,
                key: Some(self.board.cards[i].key.clone()),
            };
        }
    }

    fn row(&self) -> usize {
        let cols = self.columns();
        self.sel
            .key
            .as_deref()
            .and_then(|k| {
                cols[self.sel.col]
                    .iter()
                    .position(|&i| self.board.cards[i].key == k)
            })
            .unwrap_or(0)
    }

    fn move_col(&mut self, dir: i32) {
        let cols = self.columns();
        let vis = self.visible_columns();
        let row = self.row();
        let mut ci = self.sel.col as i32;
        loop {
            ci += dir;
            if !(0..7).contains(&ci) {
                return;
            }
            let c = ci as usize;
            if vis[c] && !cols[c].is_empty() {
                let r = row.min(cols[c].len() - 1);
                self.select_index(c, r);
                return;
            }
        }
    }

    fn move_row(&mut self, dir: i32) {
        let cols = self.columns();
        let len = cols[self.sel.col].len();
        if len == 0 {
            return;
        }
        let row = (self.row() as i32 + dir).clamp(0, len as i32 - 1) as usize;
        self.select_index(self.sel.col, row);
    }

    /// Cards that need you, in board order: column by column, top to bottom.
    pub fn needs_you_order(&self) -> Vec<String> {
        let cols = self.columns();
        let vis = self.visible_columns();
        let mut out = Vec::new();
        for ci in 0..7 {
            if !vis[ci] {
                continue;
            }
            for &i in &cols[ci] {
                if self.board.cards[i].needs_you() {
                    out.push(self.board.cards[i].key.clone());
                }
            }
        }
        out
    }

    /// Moves to the next (or previous) card that needs you. Returns its key.
    pub fn step_needs_you(&mut self, dir: i32) -> Option<String> {
        let order = self.needs_you_order();
        if order.is_empty() {
            self.say("Nothing here needs you right now.", Tone::Good);
            return None;
        }
        let pos = self
            .sel
            .key
            .as_ref()
            .and_then(|k| order.iter().position(|o| o == k));
        let next = match pos {
            Some(p) => (p as i32 + dir).rem_euclid(order.len() as i32) as usize,
            None if dir > 0 => 0,
            None => order.len() - 1,
        };
        let key = order[next].clone();
        if let Some(card) = self.card(&key) {
            self.sel = Selection {
                col: card.stage.index(),
                key: Some(key.clone()),
            };
        }
        Some(key)
    }

    /// The next card that needs you after `key`, skipping it: for jumping straight on.
    pub fn next_needs_you_after(&self, key: &str) -> Option<String> {
        let order = self.needs_you_order();
        let pos = order.iter().position(|o| o == key);
        let candidates: Vec<&String> = match pos {
            Some(p) => order[p + 1..].iter().chain(order[..p].iter()).collect(),
            None => order.iter().collect(),
        };
        candidates
            .into_iter()
            .find(|k| self.card(k).is_some_and(|c| c.is_open_workspace()))
            .cloned()
    }

    fn set_tab(&mut self, tab: usize) {
        if tab < self.tabs.len() {
            self.tab = tab;
            self.fix_selection();
        }
    }

    fn step_tab(&mut self, dir: i32) {
        let n = self.tabs.len() as i32;
        self.set_tab((self.tab as i32 + dir).rem_euclid(n) as usize);
    }

    fn step_machine(&mut self, dir: i32) {
        let machine_tabs: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t, Scope::All | Scope::Machine(_)))
            .map(|(i, _)| i)
            .collect();
        let current = match self.scope() {
            Scope::Session(m, _) => self
                .tabs
                .iter()
                .position(|t| *t == Scope::Machine(m.clone())),
            _ => Some(self.tab),
        }
        .unwrap_or(0);
        let pos = machine_tabs.iter().position(|&t| t == current).unwrap_or(0) as i32;
        let next = (pos + dir).rem_euclid(machine_tabs.len() as i32) as usize;
        self.set_tab(machine_tabs[next]);
    }

    fn session_tab(&mut self, n: usize) {
        let idx = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t, Scope::Session(..)))
            .nth(n.saturating_sub(1))
            .map(|(i, _)| i);
        match idx {
            Some(i) => self.set_tab(i),
            None => self.say(format!("There's no session tab {n}."), Tone::Info),
        }
    }

    pub fn machine_index(&self, name: &str) -> Option<usize> {
        self.machines.iter().position(|m| m.answers_to(name))
    }

    fn jump_action(&mut self, key: String) -> Action {
        let Some(card) = self.card(&key) else {
            return Action::None;
        };
        if card.is_open_workspace() {
            return Action::Jump(key);
        }
        let what = match card.stage {
            Stage::Backlog => "it hasn't started",
            Stage::Maintain => "it's done",
            _ => "it has no herdr workspace open",
        };
        let msg = format!(
            "{} has nothing to jump into: {what}. m peeks at it.",
            card.reference
        );
        self.say(msg, Tone::Info);
        Action::None
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        match self.mode.clone() {
            Mode::Board => self.board_key(key),
            Mode::Peek { scroll, back } => self.peek_key(key, scroll, back),
            Mode::Search(s) => self.search_key(key, s),
            Mode::Keys {
                filter,
                typing,
                scroll,
            } => self.keys_key(key, filter, typing, scroll),
            Mode::Machines { sel } => self.machines_key(key, sel),
            Mode::Backlog { sel } => self.backlog_key(key, sel),
        }
    }

    fn board_key(&mut self, key: KeyEvent) -> Action {
        let ca = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('j' | 'J') if ca => {
                if shift || key.code == KeyCode::Char('J') {
                    self.move_row(1);
                } else {
                    self.step_needs_you(1);
                }
            }
            KeyCode::Char('k' | 'K') if ca => {
                if shift || key.code == KeyCode::Char('K') {
                    self.move_row(-1);
                } else {
                    self.step_needs_you(-1);
                }
            }
            KeyCode::Char('h' | 'H') if ca => {
                if shift || key.code == KeyCode::Char('H') {
                    self.step_machine(-1)
                } else {
                    self.step_tab(-1)
                }
            }
            KeyCode::Char('l' | 'L') if ca => {
                if shift || key.code == KeyCode::Char('L') {
                    self.step_machine(1)
                } else {
                    self.step_tab(1)
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_col(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_col(1),
            KeyCode::Char('j') | KeyCode::Down => self.move_row(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_row(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select_index(self.sel.col, 0),
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.columns()[self.sel.col].len();
                if len > 0 {
                    self.select_index(self.sel.col, len - 1);
                }
            }
            KeyCode::Enter => {
                if let Some(k) = self.sel.key.clone() {
                    return self.jump_action(k);
                }
            }
            KeyCode::Char('m') | KeyCode::Char(' ') => {
                if self.sel.key.is_some() {
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Board),
                    };
                }
            }
            KeyCode::Char('n') => {
                self.step_needs_you(1);
            }
            KeyCode::Char('N') => {
                self.step_needs_you(-1);
            }
            KeyCode::Char(']') | KeyCode::Tab => self.step_tab(1),
            KeyCode::Char('[') | KeyCode::BackTab => self.step_tab(-1),
            KeyCode::Char('}') => self.step_machine(1),
            KeyCode::Char('{') => self.step_machine(-1),
            KeyCode::Char('0') => self.set_tab(0),
            KeyCode::Char(c @ '1'..='9') => self.session_tab(c as usize - '0' as usize),
            KeyCode::Char('/') => {
                self.mode = Mode::Search(Search {
                    query: String::new(),
                    filter: SearchFilter::Everything,
                    sel: 0,
                })
            }
            KeyCode::Char('b') => self.mode = Mode::Backlog { sel: 0 },
            KeyCode::Char('M') => self.mode = Mode::Machines { sel: 0 },
            KeyCode::Char('r') => {
                let target = self
                    .selected()
                    .map(|c| c.machine_index)
                    .filter(|&i| !self.machines[i].conn.is_live());
                return Action::Retry(target);
            }
            KeyCode::Char('R') => return Action::Retry(None),
            KeyCode::Char('?') => {
                self.mode = Mode::Keys {
                    filter: String::new(),
                    typing: false,
                    scroll: 0,
                }
            }
            KeyCode::Esc => {
                self.message = None;
                if self.pending_jump.take().is_some() {
                    self.say("Stopped waiting.", Tone::Info);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn peek_key(&mut self, key: KeyEvent, scroll: u16, back: Box<Mode>) -> Action {
        let ca = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') | KeyCode::Char(' ') => {
                self.mode = *back
            }
            KeyCode::Enter => {
                if let Some(k) = self.sel.key.clone() {
                    let action = self.jump_action(k);
                    if action != Action::None {
                        self.mode = *back;
                    }
                    return action;
                }
            }
            KeyCode::Char('j') if ca => {
                self.step_needs_you(1);
                self.mode = Mode::Peek { scroll: 0, back };
            }
            KeyCode::Char('k') if ca => {
                self.step_needs_you(-1);
                self.mode = Mode::Peek { scroll: 0, back };
            }
            KeyCode::Char('n') => {
                self.step_needs_you(1);
                self.mode = Mode::Peek { scroll: 0, back };
            }
            KeyCode::Char('N') => {
                self.step_needs_you(-1);
                self.mode = Mode::Peek { scroll: 0, back };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Peek {
                    scroll: scroll.saturating_add(1),
                    back,
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Peek {
                    scroll: scroll.saturating_sub(1),
                    back,
                }
            }
            _ => {}
        }
        Action::None
    }

    fn search_key(&mut self, key: KeyEvent, mut s: Search) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let hits_len = self.search_hits(&s).len();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Board;
                return Action::None;
            }
            KeyCode::Enter => {
                let hits = self.search_hits(&s);
                if let Some((hit, _)) = hits.get(s.sel) {
                    return self.open_hit(*hit, s);
                }
                return Action::None;
            }
            KeyCode::Down => s.sel = (s.sel + 1).min(hits_len.saturating_sub(1)),
            KeyCode::Up => s.sel = s.sel.saturating_sub(1),
            KeyCode::Char('n' | 'j') if ctrl => s.sel = (s.sel + 1).min(hits_len.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => s.sel = s.sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                s.query.clear();
                s.sel = 0;
            }
            KeyCode::Char('o') if ctrl => {
                let hits = self.search_hits(&s);
                if let Some((Hit::Card(i), _)) = hits.get(s.sel) {
                    let card = &self.board.cards[*i];
                    self.sel = Selection {
                        col: card.stage.index(),
                        key: Some(card.key.clone()),
                    };
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Search(s)),
                    };
                    return Action::None;
                }
            }
            KeyCode::Tab => {
                let i = SearchFilter::ALL
                    .iter()
                    .position(|f| *f == s.filter)
                    .unwrap_or(0);
                s.filter = SearchFilter::ALL[(i + 1) % SearchFilter::ALL.len()];
                s.sel = 0;
            }
            KeyCode::BackTab => {
                let i = SearchFilter::ALL
                    .iter()
                    .position(|f| *f == s.filter)
                    .unwrap_or(0);
                s.filter =
                    SearchFilter::ALL[(i + SearchFilter::ALL.len() - 1) % SearchFilter::ALL.len()];
                s.sel = 0;
            }
            KeyCode::Backspace => {
                s.query.pop();
                s.sel = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                s.query.push(c);
                s.sel = 0;
            }
            _ => {}
        }
        self.mode = Mode::Search(s);
        Action::None
    }

    fn open_hit(&mut self, hit: Hit, s: Search) -> Action {
        match hit {
            Hit::Card(i) => {
                let card = self.board.cards[i].clone();
                self.sel = Selection {
                    col: card.stage.index(),
                    key: Some(card.key.clone()),
                };
                if card.is_open_workspace() {
                    self.mode = Mode::Board;
                    Action::Jump(card.key)
                } else {
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Search(s)),
                    };
                    Action::None
                }
            }
            Hit::Workspace(i) => {
                let w = self.board.workspaces[i].clone();
                self.mode = Mode::Board;
                if let Some(key) = w.card.clone()
                    && self.card(&key).is_some_and(|c| c.is_open_workspace())
                {
                    return Action::Jump(key);
                }
                Action::JumpWorkspace {
                    machine: w.machine,
                    session: w.session,
                    id: w.id,
                }
            }
            Hit::Session(i) => {
                let r = &self.board.sessions[i];
                let scope = Scope::Session(r.machine.clone(), r.session.clone());
                if let Some(t) = self.tabs.iter().position(|t| *t == scope) {
                    self.set_tab(t);
                }
                self.mode = Mode::Board;
                Action::None
            }
        }
    }

    /// Search across cards, workspaces and sessions on every machine, best matches first.
    pub fn search_hits(&self, s: &Search) -> Vec<(Hit, i64)> {
        let q = s.query.trim().to_lowercase();
        let mut hits: Vec<(Hit, i64)> = Vec::new();
        let want = |f: SearchFilter| s.filter == SearchFilter::Everything || s.filter == f;
        if want(SearchFilter::Cards) || s.filter == SearchFilter::DoneToo {
            for (i, c) in self.board.cards.iter().enumerate() {
                let done = c.attention == Attention::Done;
                if done && s.filter != SearchFilter::DoneToo && q.is_empty() {
                    continue;
                }
                if let Some(score) = card_score(&q, c) {
                    hits.push((Hit::Card(i), score));
                }
            }
        }
        if want(SearchFilter::Workspaces) {
            for (i, w) in self.board.workspaces.iter().enumerate() {
                if q.is_empty() && s.filter == SearchFilter::Everything {
                    continue;
                }
                let hay = format!("{} {} {} {}", w.label, w.session, w.machine, w.id);
                if let Some(score) = score(&q, &hay) {
                    hits.push((Hit::Workspace(i), 1000 + score));
                }
            }
        }
        if want(SearchFilter::Sessions) {
            for (i, r) in self.board.sessions.iter().enumerate() {
                if q.is_empty() && s.filter == SearchFilter::Everything {
                    continue;
                }
                let hay = format!("{} {}", r.session, r.machine);
                if let Some(score) = score(&q, &hay) {
                    hits.push((Hit::Session(i), 2000 + score));
                }
            }
        }
        hits.sort_by_key(|(h, score)| {
            let group = match h {
                Hit::Card(_) => 0,
                Hit::Workspace(_) => 1,
                Hit::Session(_) => 2,
            };
            (group, *score)
        });
        hits
    }

    fn keys_key(&mut self, key: KeyEvent, mut filter: String, typing: bool, scroll: u16) -> Action {
        if typing {
            match key.code {
                KeyCode::Esc => filter.clear(),
                KeyCode::Enter => {}
                KeyCode::Backspace => {
                    filter.pop();
                }
                KeyCode::Char(c) => {
                    filter.push(c);
                    self.mode = Mode::Keys {
                        filter,
                        typing: true,
                        scroll: 0,
                    };
                    return Action::None;
                }
                _ => {}
            }
            let keep = key.code == KeyCode::Backspace;
            self.mode = Mode::Keys {
                filter,
                typing: keep,
                scroll: 0,
            };
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.mode = Mode::Board,
            KeyCode::Char('/') => {
                self.mode = Mode::Keys {
                    filter,
                    typing: true,
                    scroll: 0,
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Keys {
                    filter,
                    typing: false,
                    scroll: scroll.saturating_add(1),
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Keys {
                    filter,
                    typing: false,
                    scroll: scroll.saturating_sub(1),
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Every machine, in the order Kerdr reads them: this one first, hidden ones included.
    pub fn machine_rows(&self) -> Vec<usize> {
        (0..self.machines.len()).collect()
    }

    fn machines_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        let rows = self.machine_rows();
        let n = rows.len().max(1);
        let i = rows.get(sel.min(n - 1)).copied().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => self.mode = Mode::Board,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('}') => {
                self.mode = Mode::Machines {
                    sel: (sel + 1).min(n - 1),
                }
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('{') => {
                self.mode = Mode::Machines {
                    sel: sel.saturating_sub(1),
                }
            }
            KeyCode::Char('r') => return Action::Retry(Some(i)),
            KeyCode::Char('R') => return Action::Retry(None),
            KeyCode::Char('x') => {
                if self.machines[i].machine.is_local() {
                    self.say("This machine can't be hidden.", Tone::Info);
                } else {
                    return Action::ToggleHidden(i);
                }
            }
            KeyCode::Char('u') => {
                if self.machines[i].machine.is_local() {
                    self.say("Update herdr here with: herdr update", Tone::Info);
                } else {
                    return Action::Setup(i);
                }
            }
            KeyCode::Enter => {
                let m = &self.machines[i];
                match &m.conn {
                    Conn::HostKey { .. } | Conn::Auth { .. } => return Action::Answer(i),
                    Conn::HerdrMissing | Conn::HerdrTooOld { .. } if !m.machine.is_local() => {
                        return Action::Setup(i);
                    }
                    Conn::Hidden => return Action::ToggleHidden(i),
                    _ => {
                        let scope = Scope::Machine(m.name());
                        if let Some(t) = self.tabs.iter().position(|t| *t == scope) {
                            self.set_tab(t);
                        }
                        self.mode = Mode::Board;
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    /// The backlog in the order its view lists it.
    pub fn backlog_cards(&self) -> Vec<usize> {
        let scope = self.scope();
        self.board
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.stage == Stage::Backlog && scope.contains(c))
            .map(|(i, _)| i)
            .collect()
    }

    fn backlog_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        let items = self.backlog_cards();
        let n = items.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('b') => self.mode = Mode::Board,
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Backlog {
                    sel: (sel + 1).min(n.saturating_sub(1)),
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Backlog {
                    sel: sel.saturating_sub(1),
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Search(Search {
                    query: String::new(),
                    filter: SearchFilter::Everything,
                    sel: 0,
                })
            }
            KeyCode::Enter | KeyCode::Char('m') | KeyCode::Char(' ') => {
                if let Some(&i) = items.get(sel) {
                    let key = self.board.cards[i].key.clone();
                    self.sel = Selection {
                        col: 0,
                        key: Some(key),
                    };
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Backlog { sel }),
                    };
                }
            }
            _ => {}
        }
        Action::None
    }
}

/// Ranks a card for a query: matches in its reference or title beat matches in its session or
/// machine, and more urgent cards come first among equals. Lower is better.
fn card_score(q: &str, c: &Card) -> Option<i64> {
    let urgency = c.attention as i64;
    let fields = [
        (0, c.reference.as_str()),
        (1, c.title.as_str()),
        (2, c.workspace_label.as_deref().unwrap_or("")),
        (3, c.why.as_str()),
        (4, c.session.as_deref().unwrap_or("")),
        (4, c.machine.as_str()),
    ];
    fields
        .iter()
        .filter_map(|(rank, text)| {
            score(q, text).map(|s| rank * 100_000 + urgency * 1000 + s.min(999))
        })
        .min()
}

/// Lower is better; None when it doesn't match. Plain substring first, then the letters in order.
pub fn score(query: &str, haystack: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let hay = haystack.to_lowercase();
    if let Some(pos) = hay.find(query) {
        return Some(pos as i64);
    }
    let mut it = hay.chars().enumerate();
    let mut first = None;
    let mut last = 0;
    for qc in query.chars().filter(|c| !c.is_whitespace()) {
        let (i, _) = it.find(|(_, hc)| *hc == qc)?;
        first.get_or_insert(i);
        last = i;
    }
    Some(500 + (last - first.unwrap_or(0)) as i64)
}

/// Where the query matches in a piece of text, for highlighting: (start, end) in chars.
pub fn match_range(query: &str, text: &str) -> Option<(usize, usize)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    let byte = lower.find(&q)?;
    let start = lower[..byte].chars().count();
    Some((start, start + q.chars().count()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_substrings_before_scattered_letters() {
        assert_eq!(score("flyw", "#143 Find the next flywheel"), Some(19));
        assert!(score("ftnf", "#143 Find the next flywheel").unwrap() >= 500);
        assert_eq!(score("zzz", "#143 Find the next flywheel"), None);
        assert_eq!(
            match_range("shop", "On the shop's Spin screen"),
            Some((7, 11))
        );
    }

    #[test]
    fn a_capital_that_lowercases_to_two_letters_still_searches() {
        // "İ" lowercases to two characters, so a range can run past the original text; the
        // drawing code checks it (ui::tests::highlighting_never_slices_past_the_text).
        let (a, b) = match_range("move", "İstanbul office move").unwrap();
        assert!(a <= b);
        assert!(score("move", "İstanbul office move").is_some());
    }
}
