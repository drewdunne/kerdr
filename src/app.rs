//! Kerdr's state and its keys. The board owns the keyboard, so no key here needs a prefix;
//! the only keys that cross into herdr are herdr's own (see `herdr_keys`).

use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::board::{self, Board, MachineState, Scope};
use crate::config::Config;
use crate::model::{Attention, Card, CardSnapshot, Stage};
use crate::poll::{Conn, Event};

#[path = "account_controls.rs"]
pub mod account_controls;
#[path = "controls.rs"]
mod controls;
use account_controls::AccountScreen;
pub(crate) use controls::waiting_work;

/// A new intent in Andrew's own words. The agent that writes it up fills in everything else:
/// its title, who it's for, its project and its triage.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub words: String,
    pub machine: usize,
    pub session: String,
    /// Images pasted into it, each standing in the words as its `[Image #N]` tag.
    pub images: Vec<crate::images::Image>,
}

/// One new intent as sent: its id lets the adapter tell a resend of the same words from new
/// work. It's reused only for the same words to the same place, and only until an answer
/// names the record (or says plainly that nothing was made, when no earlier send went unheard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub id: String,
    pub words: String,
    pub images: Vec<crate::images::Image>,
    pub machine: usize,
    pub session: String,
    /// An earlier send of it got no answer, so it may have been made.
    pub unknown: bool,
}

/// One line of the filter list: an owner (organization or tenant), or one of its projects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterRow {
    pub label: String,
    pub kind: String,
    pub depth: u8,
    pub owner: Option<String>,
    pub project: Option<String>,
    pub open: usize,
    pub backlog: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
}

/// The background filter (b in the filter screen): every card, only background work (the cards
/// marked bg), or none of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    #[default]
    All,
    Only,
    Hide,
}

impl Background {
    pub const ALL: [Background; 3] = [Background::All, Background::Only, Background::Hide];

    pub fn next(self) -> Background {
        match self {
            Background::All => Background::Only,
            Background::Only => Background::Hide,
            Background::Hide => Background::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Background::All => "all",
            Background::Only => "bg only",
            Background::Hide => "no bg",
        }
    }

    pub fn shows(self, background: bool) -> bool {
        match self {
            Background::All => true,
            Background::Only => background,
            Background::Hide => !background,
        }
    }
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
    Accounts {
        screen: AccountScreen,
        back: Box<Mode>,
    },
    Palette {
        back: Box<Mode>,
        /// The card the menu opened on: its card keys act on that card or on none.
        target: Option<String>,
    },
    Compose {
        draft: Draft,
        back: Box<Mode>,
    },
    Filter {
        query: String,
        sel: usize,
        /// Letters go into the search (after /) rather than acting as the screen's keys.
        typing: bool,
        back: Box<Mode>,
    },
    Confirm {
        machine: usize,
        request: serde_json::Value,
        title: String,
        message: String,
        back: Box<Mode>,
        /// The card it asks about, as it was when the action started: a yes applies to that
        /// work as it was, never to another card or a changed one.
        target: Option<Box<CardSnapshot>>,
        /// A full session's other answer, for a reopen: wait in its queue (w).
        queue: bool,
        /// For a question that appears on its own (an action's answer): keys before this are
        /// ignored, so one held or typed before it appeared can't answer it.
        ready_at: Option<Instant>,
    },
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
    Misc {
        sel: usize,
    },
}

fn misc_selection(mode: &Mode) -> Option<usize> {
    match mode {
        Mode::Misc { sel } => Some(*sel),
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => misc_selection(back),
        _ => None,
    }
}

fn set_misc_selection(mode: &mut Mode, selected: usize) {
    match mode {
        Mode::Misc { sel } => *sel = selected,
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => set_misc_selection(back, selected),
        _ => {}
    }
}

fn backlog_selection(mode: &Mode) -> Option<usize> {
    match mode {
        Mode::Backlog { sel } => Some(*sel),
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => backlog_selection(back),
        _ => None,
    }
}

fn set_backlog_selection(mode: &mut Mode, selected: usize) {
    match mode {
        Mode::Backlog { sel } => *sel = selected,
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => set_backlog_selection(back, selected),
        _ => {}
    }
}

fn holds_peek(mode: &Mode) -> bool {
    match mode {
        Mode::Peek { .. } => true,
        Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => holds_peek(back),
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Mutate {
        machine: usize,
        request: serde_json::Value,
    },
    /// Read images into the new-intent box, away from the board's own thread.
    LoadImages(crate::images::Source),
    None,
    CopyLink(String),
    OpenLink(String),
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

/// A workspace a reopen just made, to go into as soon as its answer arrives: on the machine the
/// action ran on, before any card claims it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAfter {
    pub machine: usize,
    pub session: String,
    pub workspace: String,
    pub label: String,
    /// The card it was reopened from, for going on to the next that needs you.
    pub from: String,
}

/// A card to jump into as soon as its machine answers, and what to call it if the card is gone
/// by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingJump {
    pub key: String,
    pub workspace: String,
    pub machine: String,
}

pub struct App {
    pub cfg: Config,
    pub machines: Vec<MachineState>,
    pub board: Board,
    pub tabs: Vec<Scope>,
    pub tab: usize,
    /// None is the overview; session tabs belong only to this machine.
    pub machine: Option<String>,
    pub owner: Option<String>,
    pub project: Option<String>,
    pub provider: Option<String>,
    pub background: Background,
    pub wont_do: bool,
    pub busy: bool,
    pub count: String,
    /// Words typed into the new-intent box and not sent yet, kept for the next time it opens.
    pub draft: String,
    /// The images pasted with those words.
    pub draft_images: Vec<crate::images::Image>,
    /// The images being read for the new-intent box, by the number of that read; a read whose
    /// box closed meanwhile is dropped when it ends.
    pub image_load: Option<u64>,
    image_loads: u64,
    /// Where in the words that read's tags go: where the words ended when it began, so words
    /// typed while it reads come after them.
    image_at: usize,
    /// The new intent last sent, while its answer is still owed or unknown.
    pub submission: Option<Submission>,
    /// The intent just created here, selected in the backlog as soon as its card arrives.
    pub new_intent_id: Option<(usize, String)>,
    backlog_focus: Option<String>,
    pub sel: Selection,
    pub mode: Mode,
    pub message: Option<(String, Tone, Instant)>,
    /// A card to jump into as soon as its machine answers.
    pub pending_jump: Option<PendingJump>,
    /// The workspace a reopen just made, to go into now.
    pub open_after: Option<OpenAfter>,
    /// The card the running action was asked from, as it was then: its answer asks or takes you
    /// in only while you're still on that work. Esc lets it go.
    pub action_from: Option<CardSnapshot>,
    /// A card whose workspace turned out to have closed when you went in: reopen it next.
    pub pending_reopen: Option<String>,
    pub width: u16,
    pub demo: bool,
    /// Fixed in demo mode so screenshots don't change; the clock otherwise.
    pub fixed_now: Option<DateTime<Local>>,
    /// herdr's prefix, for the keys Kerdr names inside herdr.
    pub prefix: String,
    first_selection_done: bool,
    pub account_queue: std::collections::VecDeque<(usize, serde_json::Value)>,
    pub signin_open: Option<String>,
    pub account_batch_messages: Vec<String>,
    pub account_pending: Option<account_controls::Pending>,
}

impl App {
    pub fn new(cfg: Config, machines: Vec<MachineState>) -> App {
        let mut app = App {
            cfg,
            machines,
            board: Board::default(),
            tabs: vec![Scope::All],
            tab: 0,
            machine: None,
            owner: None,
            project: None,
            provider: None,
            background: Background::All,
            wont_do: false,
            busy: false,
            count: String::new(),
            draft: String::new(),
            draft_images: vec![],
            image_load: None,
            image_loads: 0,
            image_at: 0,
            new_intent_id: None,
            submission: None,
            backlog_focus: None,
            sel: Selection::default(),
            mode: Mode::Board,
            message: None,
            pending_jump: None,
            open_after: None,
            action_from: None,
            pending_reopen: None,
            width: 200,
            demo: false,
            fixed_now: None,
            prefix: "ctrl+b".to_string(),
            first_selection_done: false,
            account_queue: Default::default(),
            signin_open: None,
            account_batch_messages: vec![],
            account_pending: None,
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
                    m.feed_error = data.feed_failed_after.take();
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
        self.refresh_accounts();
        let current = self.scope().clone();
        self.board = board::build(&self.machines, &self.cfg);
        let mut tabs = vec![Scope::All];
        if let Some(name) = self.machine.clone() {
            if self
                .machines
                .iter()
                .any(|m| m.is_shown() && m.answers_to(&name))
            {
                tabs = vec![Scope::Machine(name.clone())];
                for r in self
                    .board
                    .sessions
                    .iter()
                    .filter(|r| r.machine == name && r.running)
                {
                    tabs.push(Scope::Session(name.clone(), r.session.clone()));
                }
            } else {
                self.machine = None;
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
        if let Some(sel) = misc_selection(&self.mode) {
            let items = self.misc_cards();
            let found = self
                .sel
                .key
                .as_ref()
                .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key));
            let selected = found.unwrap_or(sel.min(items.len().saturating_sub(1)));
            if !holds_peek(&self.mode) {
                self.sel.key = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
                self.sel.col = 0;
            }
            set_misc_selection(&mut self.mode, selected);
            return;
        }
        self.fix_selection();
        // Only once its card is in the backlog as shown: a filter may hide it for now.
        if let Some((machine, id)) = self.new_intent_id.clone()
            && let Some(&i) = self.backlog_cards().iter().find(|&&i| {
                let c = &self.board.cards[i];
                c.machine_index == machine && c.detail.as_ref().is_some_and(|d| d.id == id)
            })
        {
            self.backlog_focus = Some(self.board.cards[i].key.clone());
            self.new_intent_id = None;
        }
        if let Some(sel) = backlog_selection(&self.mode) {
            let items = self.backlog_cards();
            let found = self
                .backlog_focus
                .as_ref()
                .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key));
            let selected = found.unwrap_or(sel.min(items.len().saturating_sub(1)));
            // A pending command retains its original target even if that record disappeared.
            // Direct list navigation can move to the next available row.
            if found.is_some()
                || matches!(self.mode, Mode::Backlog { .. })
                || self.backlog_focus.is_none()
            {
                self.backlog_focus = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
            }
            set_backlog_selection(&mut self.mode, selected);
        }
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
            if c.stage != Stage::Misc && scope.contains(c) && self.card_visible(c) {
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
        if let Some(sel) = misc_selection(&self.mode) {
            if !holds_peek(&self.mode) {
                let items = self.misc_cards();
                let selected = self
                    .sel
                    .key
                    .as_ref()
                    .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key))
                    .unwrap_or(sel.min(items.len().saturating_sub(1)));
                self.sel.key = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
                set_misc_selection(&mut self.mode, selected);
            }
            return;
        }
        if holds_peek(&self.mode) {
            // A peek is an operation on one stable record, including when its column is hidden.
            // Losing the record must never silently point an action at a neighbor.
            if let Some(card) = self.selected() {
                self.sel.col = card.stage.index();
            }
            return;
        }
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

    /// The next card that needs you after `key`, skipping it and any in `skip` (workspaces found
    /// closed on the way): for jumping straight on.
    pub fn next_needs_you_after(&self, key: &str, skip: &[String]) -> Option<String> {
        let order = self.needs_you_order();
        let pos = order.iter().position(|o| o == key);
        let candidates: Vec<&String> = match pos {
            Some(p) => order[p + 1..].iter().chain(order[..p].iter()).collect(),
            None => order.iter().collect(),
        };
        candidates
            .into_iter()
            .filter(|k| !skip.contains(k))
            .find(|k| self.card(k).is_some_and(|c| c.is_open_workspace()))
            .cloned()
    }

    /// herdr has just said the workspace you went into, `tried`, has closed: work still under way,
    /// or done work, reopens next (done work after a yes), even before the board catches up; a card
    /// that has meanwhile moved to another workspace doesn't. Whether it will.
    pub fn closed_under_you(&mut self, key: &str, tried: Option<&str>) -> bool {
        let reopen = self.actions_on()
            && self.card(key).is_some_and(|c| {
                (c.under_way() || c.done())
                    && (c.workspace_id.is_none() || c.workspace_id.as_deref() == tried)
            });
        if reopen {
            self.pending_reopen = Some(key.to_string());
        }
        reopen
    }

    /// Waits for this card's machine to answer, then jumps in.
    pub fn wait_to_jump(&mut self, card: &Card) {
        self.pending_jump = Some(PendingJump {
            key: card.key.clone(),
            workspace: card.workspace_name(),
            machine: card.machine.clone(),
        });
    }

    /// The waiting jump, once its machine answers: the card to jump into now. None while it still
    /// waits, or once Kerdr has said why it can't: the workspace closed while the machine was
    /// away.
    pub fn pending_jump_ready(&mut self) -> Option<String> {
        let pending = self.pending_jump.clone()?;
        let open = match self.card(&pending.key) {
            Some(c) if !self.machines[c.machine_index].conn.is_live() => return None,
            Some(c) => Some(c.is_open_workspace()),
            None => None,
        };
        self.pending_jump = None;
        let PendingJump {
            key,
            workspace,
            machine,
        } = pending;
        match open {
            Some(true) => return Some(key),
            // Work still under way, or done, whose workspace closed meanwhile: reopen it instead.
            Some(false) if self.card(&key).is_some_and(|c| c.reopenable()) && self.actions_on() => {
                self.pending_reopen = Some(key);
            }
            Some(false) => self.say(
                format!("{workspace} closed while {machine} was away."),
                Tone::Warn,
            ),
            None => self.say(
                format!("{workspace} closed while {machine} was away; it's off the board now."),
                Tone::Warn,
            ),
        }
        None
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
        let choices: Vec<Option<String>> = std::iter::once(None)
            .chain(
                self.machines
                    .iter()
                    .filter(|m| m.is_shown())
                    .map(|m| Some(m.name())),
            )
            .collect();
        let pos = choices.iter().position(|m| m == &self.machine).unwrap_or(0);
        let next = (pos as i32 + dir).rem_euclid(choices.len() as i32) as usize;
        self.machine = choices[next].clone();
        self.rebuild();
        self.set_tab(0);
    }

    fn choose_scope(&mut self, scope: Scope) {
        self.machine = match &scope {
            Scope::All => None,
            Scope::Machine(m) | Scope::Session(m, _) => Some(m.clone()),
        };
        self.rebuild();
        if let Some(tab) = self.tabs.iter().position(|t| *t == scope) {
            self.set_tab(tab);
        }
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
        let Some(card) = self.card(&key).cloned() else {
            return Action::None;
        };
        if card.is_open_workspace() {
            return Action::Jump(key);
        }
        // Work under way whose workspace closed, or done work: reopen it rather than leave a dead
        // end. Done work is opened again only after a yes to the question its backend asks.
        if card.reopenable() {
            return self.reopen_action(&card);
        }
        if card.dismissed() {
            self.say(
                format!("{} is in won't do; u brings it back.", card.reference),
                Tone::Info,
            );
            return Action::None;
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
        // Esc anywhere lets a running action's answer go: it still finishes, but neither asks nor
        // takes you anywhere, and a reopen waiting its turn is dropped.
        let stopped = key.code == KeyCode::Esc
            && (self.pending_reopen.take().is_some()
                | (self.busy && self.action_from.take().is_some()));
        let action = self.handle_key(key);
        if stopped {
            self.say(
                "Stopped waiting: it still finishes, and won't take you anywhere.",
                Tone::Info,
            );
        }
        action
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(
                self.mode,
                Mode::Accounts {
                    screen: AccountScreen::Signin(_) | AccountScreen::PasteToken { .. },
                    ..
                }
            ) {
                return self
                    .account_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                    .unwrap_or(Action::None);
            }
            return Action::Quit;
        }
        if let Some(action) = self.account_key(key) {
            return action;
        }
        if let Some(action) = self.control_key(key) {
            return action;
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
            Mode::Misc { sel } => self.misc_key(key, sel),
            Mode::Accounts { .. }
            | Mode::Palette { .. }
            | Mode::Compose { .. }
            | Mode::Filter { .. }
            | Mode::Confirm { .. } => Action::None,
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
            KeyCode::Char('0') => self.choose_scope(Scope::All),
            KeyCode::Char(c @ '1'..='9') => self.session_tab(c as usize - '0' as usize),
            KeyCode::Char('/') => {
                self.mode = Mode::Search(Search {
                    query: String::new(),
                    filter: SearchFilter::Everything,
                    sel: 0,
                })
            }
            KeyCode::Char('b') => self.enter_backlog(false),
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
                self.mode = *back;
                if misc_selection(&self.mode).is_some() {
                    self.fix_selection();
                }
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
                } else if card.can_reopen() || (card.can_reopen_done() && self.actions_on()) {
                    self.mode = Mode::Board;
                    self.reopen_action(&card)
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
                self.choose_scope(scope);
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
                if !self.card_visible(c) {
                    continue;
                }
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
                let in_scope = match self.scope() {
                    Scope::All => true,
                    Scope::Machine(m) => &w.machine == m,
                    Scope::Session(m, s) => &w.machine == m && &w.session == s,
                };
                if !in_scope {
                    continue;
                }
                // Shown when any card on it is: its own, or one for work in one of its tabs.
                if self.filtering()
                    && !self.board.cards.iter().any(|c| {
                        c.machine == w.machine
                            && c.session.as_deref() == Some(w.session.as_str())
                            && c.workspace_id.as_deref() == Some(w.id.as_str())
                            && self.card_visible(c)
                    })
                {
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
                if !self.scope().contains_session(r) {
                    continue;
                }
                if self.filtering()
                    && !self.board.cards.iter().any(|c| {
                        c.machine == r.machine
                            && c.session.as_deref() == Some(r.session.as_str())
                            && self.card_visible(c)
                    })
                {
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
                        self.choose_scope(scope);
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
            .filter(|(_, c)| {
                (c.stage == Stage::Backlog || self.wont_do)
                    && scope.contains(c)
                    && self.card_visible(c)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn backlog_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        // Moving on your own ends the wait to select a just-made intent.
        self.new_intent_id = None;
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
                    self.backlog_focus = Some(key.clone());
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
        if let Mode::Backlog { sel } = self.mode {
            self.backlog_focus = items.get(sel).map(|&i| self.board.cards[i].key.clone());
        }
        Action::None
    }

    pub fn misc_cards(&self) -> Vec<usize> {
        self.board
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.stage == Stage::Misc && self.card_visible(c))
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn enter_misc(&mut self) {
        self.mode = Mode::Misc { sel: 0 };
        self.sel.col = 0;
        self.sel.key = self
            .misc_cards()
            .first()
            .map(|&i| self.board.cards[i].key.clone());
    }

    fn misc_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        let items = self.misc_cards();
        let mut selected = sel.min(items.len().saturating_sub(1));
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'i') => {
                self.mode = Mode::Board;
                self.fix_selection();
                return Action::None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(items.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => selected = 0,
            KeyCode::Char('G') | KeyCode::End => selected = items.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(&i) = items.get(selected) {
                    return self.jump_action(self.board.cards[i].key.clone());
                }
            }
            KeyCode::Char('m' | ' ') => {
                if !items.is_empty() {
                    self.sel.key = Some(self.board.cards[items[selected]].key.clone());
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Misc { sel: selected }),
                    };
                }
                return Action::None;
            }
            KeyCode::Char('[') | KeyCode::BackTab => {
                self.step_tab(-1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char(']') | KeyCode::Tab => {
                self.step_tab(1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char('{') => {
                self.step_machine(-1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char('}') => {
                self.step_machine(1);
                self.rebuild();
                return Action::None;
            }
            _ => {}
        }
        self.mode = Mode::Misc { sel: selected };
        self.sel.key = items
            .get(selected)
            .map(|&i| self.board.cards[i].key.clone());
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
    fn a_cycles_provider_directly_even_when_current_filter_leaves_board_empty() {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("claude"));
        app.project = Some("no-matching-project".into());
        assert!(app.columns().iter().all(Vec::is_empty));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("codex"));
        press(&mut app, 'a');
        assert_eq!(app.provider, None);
    }

    #[test]
    fn empty_board_still_switches_machines_and_sessions() {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        app.project = Some("no-matching-project".into());
        press(&mut app, '}');
        let machine = app.machine.clone().unwrap();
        press(&mut app, ']');
        assert!(matches!(app.scope(), Scope::Session(m, _) if m == &machine));
    }

    fn demo_app() -> App {
        App::new(Config::default(), crate::demo::machines(false))
    }

    fn press(app: &mut App, c: char) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    /// As if the question on screen had been up long enough to be answered.
    fn settle(app: &mut App) {
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
    }

    #[test]
    fn unlinked_workspace_goes_to_misc_and_enter_still_jumps() {
        let mut app = demo_app();
        let misc = app
            .board
            .cards
            .iter()
            .find(|c| c.source == crate::model::Source::Workspace)
            .unwrap()
            .clone();
        assert_eq!(misc.stage.id(), "misc");
        assert!(
            !app.columns()
                .iter()
                .flatten()
                .any(|&i| app.board.cards[i].key == misc.key)
        );
        press(&mut app, 'i');
        assert!(matches!(app.mode, Mode::Misc { .. }));
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Jump(misc.key)
        );
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert!(
            app.selected()
                .is_none_or(|c| c.source == crate::model::Source::Feed)
        );
    }

    #[test]
    fn misc_moves_to_its_intent_phase_once_and_survives_a_feed_error() {
        let mut app = demo_app();
        let misc = app
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Misc)
            .unwrap()
            .clone();
        let mi = misc.machine_index;
        let session = misc.session.clone().unwrap();
        let record = crate::feed::FeedCard {
            id: "1886".into(),
            title: "Real intent".into(),
            stage: Some("test".into()),
            workspace: Some(crate::feed::FeedWorkspace {
                session: Some(session),
                id: misc.workspace_id.clone(),
                label: None,
                tab: None,
            }),
            ..Default::default()
        };
        app.machines[mi].feed = Some(crate::feed::Feed {
            kerdr_feed: 1,
            cards: vec![record],
            ..Default::default()
        });
        app.rebuild();
        assert!(!app.board.cards.iter().any(|c| c.key == misc.key));
        let claimed: Vec<_> = app
            .board
            .cards
            .iter()
            .filter(|c| {
                c.machine_index == mi
                    && c.session == misc.session
                    && c.workspace_id == misc.workspace_id
            })
            .collect();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].stage, Stage::Test);
        let mut data = app.machines[mi].data.clone().unwrap();
        data.feed = Some(Err("temporary feed failure".into()));
        app.apply(Event {
            machine: mi,
            conn: app.machines[mi].conn.clone(),
            data: Some(data),
            at: Local::now(),
        });
        assert!(
            app.board.cards.iter().any(
                |c| c.detail.as_ref().is_some_and(|d| d.id == "1886") && c.stage == Stage::Test
            )
        );
        assert!(!app.board.cards.iter().any(|c| c.key == misc.key));
    }

    #[test]
    fn misc_menu_and_peek_preserve_selection_across_poll_and_filters() {
        let mut app = demo_app();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'i');
        assert!(matches!(app.mode, Mode::Misc { .. }));
        let original = app.selected().unwrap().clone();
        press(&mut app, 'm');
        app.rebuild();
        assert_eq!(app.selected().unwrap().key, original.key);
        assert!(matches!(app.mode, Mode::Peek { .. }));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Misc { .. }));
        app.provider = Some("no-such-provider".into());
        app.rebuild();
        assert!(app.misc_cards().is_empty());
        assert!(app.selected().is_none());
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn misc_provider_menu_retargets_peek_before_the_next_poll() {
        let mut app = demo_app();
        press(&mut app, 'i');
        let mut other = app.selected().unwrap().clone();
        for a in &mut other.agents {
            a.agent = "claude".into();
        }
        other.key = "misc-other".into();
        other.title = "Other provider".into();
        for c in &mut app.board.cards {
            if c.stage == Stage::Misc {
                for a in &mut c.agents {
                    a.agent = "codex".into();
                }
            }
        }
        app.board.cards.push(other);
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("claude"));
        assert_eq!(app.selected().unwrap().key, "misc-other");
        press(&mut app, 'm');
        assert_eq!(app.selected().unwrap().key, "misc-other");
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Jump("misc-other".into())
        );
    }

    #[test]
    fn misc_selection_survives_polling_while_composing_an_intent() {
        let mut app = demo_app();
        press(&mut app, 'i');
        let original = app.selected().unwrap().key.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        app.rebuild();
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Misc { .. }));
        assert_eq!(app.selected().unwrap().key, original);
    }

    #[test]
    fn overview_does_not_mix_all_machines_sessions_in_the_tab_bar() {
        let app = demo_app();
        assert_eq!(app.tabs, vec![Scope::All]);
    }

    #[test]
    fn braces_switch_machine_and_brackets_stay_in_its_sessions() {
        let mut app = demo_app();
        let first = app.machines[0].name();
        let second = app.machines[1].name();
        press(&mut app, '}');
        assert_eq!(app.scope(), &Scope::Machine(first.clone()));
        assert!(
            app.tabs
                .iter()
                .all(|t| matches!(t, Scope::Machine(m) | Scope::Session(m, _) if m == &first))
        );
        press(&mut app, ']');
        assert!(matches!(app.scope(), Scope::Session(m, _) if m == &first));
        press(&mut app, '}');
        assert_eq!(app.scope(), &Scope::Machine(second.clone()));
        assert!(
            app.tabs
                .iter()
                .all(|t| matches!(t, Scope::Machine(m) | Scope::Session(m, _) if m == &second))
        );
        press(&mut app, '0');
        assert_eq!(app.scope(), &Scope::All);
    }

    #[test]
    fn tenant_and_project_filters_compose_and_wont_do_has_its_own_view() {
        let mut app = demo_app();
        let key = app.board.cards[0].key.clone();
        let card = &mut app.board.cards[0];
        card.stage = Stage::Backlog;
        let d = card.detail.get_or_insert_with(Default::default);
        d.tenant = Some("shopfront".into());
        d.project = Some("website".into());
        app.owner = Some("tenant:shopfront".into());
        app.project = Some("website".into());
        assert!(
            app.columns()[0]
                .iter()
                .any(|&i| app.board.cards[i].key == key)
        );
        app.project = Some("another".into());
        assert!(app.columns().iter().all(Vec::is_empty));
        app.project = Some("website".into());
        app.board.cards[0].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(app.backlog_cards().is_empty());
        app.wont_do = true;
        assert_eq!(app.backlog_cards(), vec![0]);
    }

    #[test]
    fn dispatch_confirmation_cancels_without_action_and_only_yes_overrides() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        let home = app.card(&key).unwrap().machine_index;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        let request = serde_json::json!({"action":"dispatch","id":id});
        let response = crate::actions::Response {
            ok: false,
            message: "3 of 3 slots used".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            id: Some(id.clone()),
            ..Default::default()
        };
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        app.finish_action(home, request.clone(), Ok(response.clone()));
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::None
        );
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        app.finish_action(home, request, Ok(response));
        settle(&mut app);
        let Action::Mutate { machine, request } = press(&mut app, 'y') else {
            panic!("expected action")
        };
        assert_eq!(machine, home);
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["override"], true);
    }

    #[test]
    fn leader_affirm_and_counted_move_name_the_selected_backlog_record() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let i = app.backlog_cards()[0];
        let id = app.board.cards[i].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["action"], "refresh");
        assert_eq!(request["id"], id);
        press(&mut app, '1');
        press(&mut app, '0');
        let Action::Mutate { request, .. } = press(&mut app, 'K') else {
            panic!("expected reorder")
        };
        assert_eq!(request["action"], "move");
        assert_eq!(request["delta"], -10);
    }

    #[test]
    fn polling_preserves_peek_target_when_backlog_column_is_hidden() {
        let mut app = demo_app();
        app.width = 80;
        app.mode = Mode::Backlog { sel: 0 };
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let before = app.selected().unwrap().key.clone();
        app.rebuild();
        assert_eq!(app.selected().unwrap().key, before);
    }

    #[test]
    fn search_sessions_and_workspaces_respects_selected_machine() {
        let mut app = demo_app();
        press(&mut app, '}');
        for filter in [SearchFilter::Sessions, SearchFilter::Workspaces] {
            let search = Search {
                query: String::new(),
                filter,
                sel: 0,
            };
            for (hit, _) in app.search_hits(&search) {
                let machine = match hit {
                    Hit::Session(i) => &app.board.sessions[i].machine,
                    Hit::Workspace(i) => &app.board.workspaces[i].machine,
                    _ => continue,
                };
                assert_eq!(machine, app.machine.as_ref().unwrap());
            }
        }
    }

    #[test]
    fn owner_cycle_preserves_project_for_an_empty_intersection() {
        let mut app = demo_app();
        app.project = Some("website".into());
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 't');
        assert_eq!(app.project.as_deref(), Some("website"));
    }

    #[test]
    fn polling_reorder_never_retargets_a_palette_action_to_the_next_record() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let i = app.backlog_cards()[0];
        let id = app.board.cards[i].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        for machine in &mut app.machines {
            if let Some(feed) = &mut machine.feed {
                for card in &mut feed.cards {
                    if card.id == id {
                        card.backlog_rank = Some(99999);
                    }
                }
            }
        }
        app.rebuild();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
    }

    #[test]
    fn reopening_backlog_or_switching_machine_actions_the_visible_row() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'b');
        press(&mut app, 'j');
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, 'b');
        let id = app.board.cards[app.backlog_cards()[0]]
            .detail
            .as_ref()
            .unwrap()
            .id
            .clone();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
        press(&mut app, 'j');
        press(&mut app, '}');
        let id = app.board.cards[app.backlog_cards()[0]]
            .detail
            .as_ref()
            .unwrap()
            .id
            .clone();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
    }

    fn provider_app() -> App {
        let mut app = demo_app();
        let seed = app.board.cards[0].clone();
        app.board.cards = [
            vec!["claude"],
            vec!["codex"],
            vec!["claude", "codex"],
            vec!["future-agent"],
            vec![],
        ]
        .into_iter()
        .enumerate()
        .map(|(i, agents)| {
            let mut card = seed.clone();
            card.key = format!("provider-{i}");
            card.machine = "laptop".into();
            card.stage = if i == 4 { Stage::Backlog } else { Stage::Build };
            card.title = "Codex and Claude mentioned in title".into();
            card.agents = agents
                .into_iter()
                .map(|name| crate::model::AgentInfo {
                    agent: name.into(),
                    status: "working".into(),
                    pane_id: format!("p{i}"),
                    title: None,
                    conversation: None,
                })
                .collect();
            card
        })
        .collect();
        app
    }

    #[test]
    fn a_workspace_shows_under_a_filter_when_a_card_for_one_of_its_tabs_matches() {
        let mut app = demo_app();
        let i = app
            .board
            .cards
            .iter()
            .position(|c| c.workspace_id.is_some() && !c.agents.is_empty())
            .unwrap();
        for a in &mut app.board.cards[i].agents {
            a.agent = "claude".into();
        }
        let mut child = app.board.cards[i].clone();
        child.key = "child-in-a-tab".into();
        child.tab_id = Some("t2".into());
        child.agents[0].agent = "codex".into();
        child.agents.truncate(1);
        let ws = child.workspace_id.clone();
        app.board.cards.push(child);
        app.provider = Some("codex".into());
        let hits = app.search_hits(&Search {
            query: String::new(),
            filter: SearchFilter::Workspaces,
            sel: 0,
        });
        assert!(
            hits.iter().any(|(hit, _)| matches!(hit,
                Hit::Workspace(w) if Some(&app.board.workspaces[*w].id) == ws.as_ref())),
            "{hits:?}"
        );
    }

    fn provider_command(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(press(app, 'a'), Action::None);
    }

    fn visible_ids(app: &App) -> Vec<usize> {
        app.board
            .cards
            .iter()
            .enumerate()
            .filter(|(_, card)| app.card_visible(card))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn provider_cycle_uses_reported_agents_including_mixed_and_future_providers() {
        let mut app = provider_app();
        assert_eq!(visible_ids(&app), vec![0, 1, 2, 3, 4]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0, 2]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![1, 2]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![3]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn provider_filter_combines_with_machine_owner_project_and_dismissal() {
        let mut app = provider_app();
        for card in &mut app.board.cards {
            let detail = card.detail.get_or_insert_with(Default::default);
            detail.tenant = Some("shopfront".into());
            detail.project = Some("web".into());
        }
        app.board.cards[2].machine = "build-box".into();
        app.machine = Some("laptop".into());
        app.tabs = vec![Scope::Machine("laptop".into())];
        app.owner = Some("tenant:shopfront".into());
        app.project = Some("web".into());
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0]);
        app.project = Some("other".into());
        assert!(visible_ids(&app).is_empty());
        app.project = Some("web".into());
        app.board.cards[0].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(visible_ids(&app).is_empty());
        app.wont_do = true;
        assert_eq!(visible_ids(&app), vec![0]);
    }

    #[test]
    fn provider_search_excludes_unmatched_workspaces_and_sessions() {
        let mut app = demo_app();
        for card in &mut app.board.cards {
            for agent in &mut card.agents {
                agent.agent = "claude".into();
            }
        }
        let selected = app
            .board
            .cards
            .iter_mut()
            .find(|c| c.workspace_id.is_some() && !c.agents.is_empty())
            .unwrap();
        selected.agents[0].agent = "codex".into();
        let machine = selected.machine.clone();
        let session = selected.session.clone().unwrap();
        provider_command(&mut app);
        provider_command(&mut app);
        for filter in [SearchFilter::Workspaces, SearchFilter::Sessions] {
            let hits = app.search_hits(&Search {
                query: String::new(),
                filter,
                sel: 0,
            });
            assert!(!hits.is_empty());
            for (hit, _) in hits {
                match hit {
                    Hit::Workspace(i) => {
                        let w = &app.board.workspaces[i];
                        assert!(
                            w.card
                                .as_deref()
                                .and_then(|key| app.card(key))
                                .is_some_and(|c| c.agents.iter().any(|a| a.agent == "codex"))
                        );
                    }
                    Hit::Session(i) => {
                        assert_eq!(app.board.sessions[i].machine, machine);
                        assert_eq!(app.board.sessions[i].session, session);
                    }
                    _ => panic!("wrong search group"),
                }
            }
        }
    }

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

    /// The build box answers again, without this card's workspace.
    fn build_box_back_without(app: &mut App, card: &Card) {
        let mi = card.machine_index;
        let ws = card.workspace_id.clone().unwrap();
        let session = card.session.clone().unwrap();
        let mut data = app.machines[mi].data.clone().unwrap();
        if let Some(Ok(snap)) = data.snapshots.get_mut(&session) {
            snap.workspaces.retain(|w| w.workspace_id != ws);
            snap.agents.retain(|a| a.workspace_id != ws);
        }
        app.apply(Event {
            machine: mi,
            conn: Conn::Live {
                latency: Duration::from_millis(9),
            },
            data: Some(data),
            at: Local::now(),
        });
    }

    fn away_card(app: &App, source: crate::model::Source) -> Card {
        let mi = app.machine_index("build-box").unwrap();
        app.board
            .cards
            .iter()
            .find(|c| c.machine_index == mi && c.source == source && c.is_open_workspace())
            .unwrap()
            .clone()
    }

    #[test]
    fn a_waiting_jump_says_so_when_its_workspace_closed_while_the_machine_was_away() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        let card = away_card(&app, crate::model::Source::Workspace);
        app.wait_to_jump(&card);
        assert_eq!(app.pending_jump_ready(), None);
        assert!(
            app.pending_jump.is_some(),
            "still waiting while build-box is away"
        );
        build_box_back_without(&mut app, &card);
        assert!(app.card(&card.key).is_none());
        assert_eq!(app.pending_jump_ready(), None);
        assert!(app.pending_jump.is_none());
        assert_eq!(
            app.message(),
            Some((
                format!(
                    "Workspace {} closed while build-box was away; it's off the board now.",
                    card.reference
                )
                .as_str(),
                Tone::Warn
            ))
        );
    }

    #[test]
    fn a_waiting_jump_into_an_intent_whose_workspace_closed_says_so_and_keeps_the_card() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        let card = away_card(&app, crate::model::Source::Feed);
        app.wait_to_jump(&card);
        build_box_back_without(&mut app, &card);
        assert!(app.card(&card.key).is_some_and(|c| !c.is_open_workspace()));
        assert_eq!(app.pending_jump_ready(), None);
        assert!(app.pending_jump.is_none());
        let (text, _) = app.message().unwrap();
        assert_eq!(
            text,
            format!(
                "{}'s workspace closed while build-box was away.",
                card.reference
            )
        );
    }

    #[test]
    fn a_waiting_jump_goes_ahead_once_its_machine_answers_with_the_workspace() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        let card = away_card(&app, crate::model::Source::Workspace);
        app.wait_to_jump(&card);
        let mi = card.machine_index;
        let data = app.machines[mi].data.clone();
        app.apply(Event {
            machine: mi,
            conn: Conn::Live {
                latency: Duration::from_millis(9),
            },
            data,
            at: Local::now(),
        });
        assert_eq!(app.pending_jump_ready(), Some(card.key));
        assert!(app.pending_jump.is_none());
    }

    #[test]
    fn the_next_card_skips_workspaces_found_closed() {
        let app = demo_app();
        let open: Vec<String> = app
            .needs_you_order()
            .into_iter()
            .filter(|k| app.card(k).is_some_and(|c| c.is_open_workspace()))
            .collect();
        assert_eq!(open.len(), 2, "{open:?}");
        assert_eq!(
            app.next_needs_you_after(&open[0], &[]),
            Some(open[1].clone())
        );
        assert_eq!(
            app.next_needs_you_after(&open[1], &[]),
            Some(open[0].clone())
        );
        assert_eq!(
            app.next_needs_you_after(&open[0], std::slice::from_ref(&open[1])),
            None
        );
    }

    fn waiting_backlog_card(app: &App) -> (String, String) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && !c.dismissed()
                    && !c.is_open_workspace()
                    && c.state.as_deref() != Some("shelved")
            })
            .expect("a waiting backlog card in the demo");
        (card.key.clone(), card.detail.as_ref().unwrap().id.clone())
    }

    #[test]
    fn card_keys_work_on_the_board_with_the_same_letters_as_the_backlog() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        assert_eq!(
            app.backlog_card_for_keys().map(|c| c.key.clone()),
            Some(key)
        );
        for (c, action) in [
            ('y', "refresh"),
            ('d', "dispatch"),
            ('t', "move"),
            ('J', "move"),
        ] {
            let Action::Mutate { request, .. } = press(&mut app, c) else {
                panic!("{c} did nothing on the board")
            };
            assert_eq!(request["action"], action, "{c}");
            assert_eq!(request["id"], id, "{c}");
            app.busy = false;
        }
        // Digits still pick sessions on the board; counted moves are for the backlog.
        press(&mut app, '0');
        assert!(app.count.is_empty());
        assert!(matches!(press(&mut app, 'x'), Action::None));
        assert!(matches!(app.mode, Mode::Confirm { .. }));
    }

    #[test]
    fn card_keys_on_work_under_way_say_what_they_are_for() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Build && c.detail.is_some())
            .unwrap()
            .clone();
        app.sel = Selection {
            col: card.stage.index(),
            key: Some(card.key.clone()),
        };
        assert!(app.backlog_card_for_keys().is_none());
        assert_eq!(press(&mut app, 'd'), Action::None);
        let (text, _) = app.message().unwrap();
        assert!(
            text.contains(&card.reference) && text.contains("backlog work"),
            "{text}"
        );
    }

    #[test]
    fn a_new_intent_is_prose_sent_with_enter_and_kept_on_escape() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        for c in "Fix the wifi".chars() {
            press(&mut app, c);
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        app.on_paste("after sleep.\r\nIt bites \"every\" morning.");
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(
            app.draft,
            "Fix the wifi\n\nafter sleep.\nIt bites \"every\" morning."
        );
        // The kept words come back, and Enter sends them with the session in view.
        press(&mut app, 'c');
        let Action::Mutate { request, machine } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "create");
        assert_eq!(request["words"], app.draft.trim());
        assert_eq!(request["session"], "default");
        assert!(request.get("title").is_none() && request.get("tenant").is_none());
        // It worked: the words are gone and the new intent is picked out in the backlog.
        app.finish_action(
            0,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: "Saved as #9001.".into(),
                confirmation: false,
                used: None,
                limit: None,
                session: Some("default".into()),
                id: Some("9001".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty());
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        assert_eq!(app.new_intent_id, Some((0, "9001".to_string())));
    }

    #[test]
    fn images_go_in_as_tags_like_claude_code_and_travel_with_the_words() {
        use base64::Engine;
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("error shot.png");
        std::fs::write(&shot, &png).unwrap();
        let loaded = || crate::images::Loading {
            images: vec![crate::images::Loaded {
                media_type: "image/png",
                bytes: png.clone(),
            }],
            why: None,
        };
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        for c in "Moving failed:".chars() {
            press(&mut app, c);
        }
        let draft = |app: &App| match &app.mode {
            Mode::Compose { draft, .. } => draft.clone(),
            _ => panic!("the box closed"),
        };
        // Ctrl+V asks for the clipboard's image, read away from the board's thread; what it
        // finds goes in at the end as a tag.
        use crate::images::Source;
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_v), Action::LoadImages(Source::Clipboard));
        let (id, room) = app.begin_image_load(&Source::Clipboard).unwrap();
        assert_eq!(room, crate::images::Room::left(&[]));
        assert_eq!(app.message().unwrap().0, "Reading the clipboard…");
        // While it reads, Enter waits for it, and a second read waits its turn.
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert!(app.message().unwrap().0.contains("still being read"));
        assert!(app.begin_image_load(&Source::Clipboard).is_none());
        // Words typed while it reads come after the tag, where it was pasted.
        for c in " again".chars() {
            press(&mut app, c);
        }
        app.images_loaded(id, loaded());
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] again");
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.truncate("Moving failed: [Image #1] ".len());
        }
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] ");
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("Attached [Image #1]: a PNG image")
        );
        // A pasted (or dropped) image file's path attaches that file; an empty paste looks at
        // the clipboard; other text, blank lines and spaces too, stays words.
        let source = app.on_paste(&format!("'{}'", shot.display()));
        assert_eq!(
            source,
            Action::LoadImages(Source::Files(vec![shot.clone()]))
        );
        let Action::LoadImages(source) = source else {
            unreachable!()
        };
        let (id, room) = app.begin_image_load(&source).unwrap();
        app.images_loaded(id, source.load(room));
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] [Image #2] ");
        assert_eq!(app.on_paste(""), Action::LoadImages(Source::Clipboard));
        assert_eq!(app.on_paste("  \n"), Action::None);
        assert_eq!(
            draft(&app).words,
            "Moving failed: [Image #1] [Image #2]   \n"
        );
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.truncate(draft.words.len() - 3);
        }
        app.on_paste("and");
        app.attach_images(crate::images::Loading::refused(
            "There's no image on the clipboard.",
        ));
        assert_eq!(app.message().unwrap().1, Tone::Warn);
        app.attach_images(loaded());
        assert_eq!(draft(&app).images.len(), 3);
        // Backspace right after a tag takes the whole tag and its image out.
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        app.on_key(backspace);
        app.on_key(backspace);
        assert_eq!(
            draft(&app).words,
            "Moving failed: [Image #1] [Image #2] and "
        );
        assert_eq!(draft(&app).images.len(), 2);
        // Ctrl+W takes a tag whole too, and a tag deleted any other way leaves its image unsent.
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] ");
        assert_eq!(draft(&app).images.len(), 1);
        app.attach_images(loaded());
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] [Image #2] ");
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words = draft.words.replace("[Image #1]", "image one");
        }
        // A tag typed twice keeps its image until the last of them goes.
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.push_str("[Image #2]");
        }
        app.on_key(backspace);
        assert_eq!(draft(&app).images.len(), 2);
        // Esc keeps the images with the words for next time, and drops a read still going.
        let (late, room) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.draft_images.len(), 2);
        press(&mut app, 'c');
        app.images_loaded(late, Source::Files(vec![shot.clone()]).load(room));
        assert_eq!(
            draft(&app).images.len(),
            2,
            "a read for a closed box went in"
        );
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(request["words"], "Moving failed: image one [Image #2]");
        let sent = request["images"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["tag"], 2);
        assert_eq!(sent[0]["type"], "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(sent[0]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
        // A different image is new work: it gets a submission of its own.
        let first = request["submission"].clone();
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.images[1].bytes = std::sync::Arc::new([png.clone(), vec![1]].concat());
        }
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_ne!(request["submission"], first);
        app.finish_action(
            0,
            crate::actions::request_metadata(&request),
            Ok(crate::actions::Response {
                ok: true,
                message: "Saved as #9003.".into(),
                id: Some("9003".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty() && app.draft_images.is_empty());
    }

    #[test]
    fn images_past_the_limits_are_refused_by_name() {
        let mut app = demo_app();
        press(&mut app, 'c');
        let image = |n: usize| crate::images::Loaded {
            media_type: "image/png",
            bytes: [b"\x89PNG\r\n\x1a\n".to_vec(), vec![0; n]].concat(),
        };
        let some = |images| crate::images::Loading { images, why: None };
        app.attach_images(some(vec![
            image(9_000_000),
            image(9_000_000),
            image(3_000_000),
        ]));
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(text.contains("past 20 MB together"), "{text}");
        let Mode::Compose { draft, .. } = &app.mode else {
            panic!("the box closed")
        };
        assert_eq!(draft.images.len(), 2);
        app.attach_images(some((0..30).map(|_| image(10)).collect()));
        let Mode::Compose { draft, .. } = &app.mode else {
            panic!("the box closed")
        };
        assert_eq!(draft.images.len(), crate::images::MAX_IMAGES);
        assert!(app.message().unwrap().0.contains("at most 20 images"));
    }

    #[test]
    fn a_new_intent_saved_but_not_published_is_never_sent_twice() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.draft = "Make the dock wake the screens".into();
        press(&mut app, 'c');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        app.finish_action(
            0,
            request,
            Ok(crate::actions::Response {
                ok: false,
                message: "Saved as #9002 on this machine only; it couldn't be pushed.".into(),
                confirmation: false,
                used: None,
                limit: None,
                session: None,
                id: Some("9002".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty());
        assert!(!matches!(app.mode, Mode::Compose { .. }));
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    fn answer(ok: bool, id: Option<&str>) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok,
            message: if ok { "Saved." } else { "The store is locked" }.into(),
            confirmation: false,
            used: None,
            limit: None,
            session: None,
            id: id.map(str::to_string),
            ..Default::default()
        })
    }

    fn send_words(app: &mut App, words: &str) -> serde_json::Value {
        app.busy = false;
        app.draft = words.into();
        app.mode = Mode::Board;
        press(app, 'c');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        request
    }

    #[test]
    fn a_resend_after_a_lost_answer_names_the_same_submission() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        let id = first["submission"].as_str().unwrap().to_string();
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && id.len() <= 64);
        // No answer (a timeout): it may have been made, so the same words keep the same id,
        // even past a later refusal, which can't say what the first send did.
        app.finish_action(0, first, Err("timed out; the result is unknown".into()));
        settle(&mut app);
        let second = send_words(&mut app, "Back up the NAS");
        assert_eq!(second["submission"], id.as_str());
        app.finish_action(0, second, answer(false, None));
        settle(&mut app);
        let third = send_words(&mut app, "Back up the NAS");
        assert_eq!(third["submission"], id.as_str());
        // The answer names the record: settled, and the next words are new work.
        app.finish_action(0, third, answer(true, Some("9003")));
        settle(&mut app);
        let fourth = send_words(&mut app, "Back up the NAS");
        assert_ne!(fourth["submission"], id.as_str());
    }

    #[test]
    fn changed_words_after_a_lost_answer_are_new_work() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        let id = first["submission"].as_str().unwrap().to_string();
        app.finish_action(0, first, Err("timed out".into()));
        settle(&mut app);
        let edited = send_words(&mut app, "Back up the NAS and the laptop");
        assert_ne!(edited["submission"], id.as_str());
        // A refusal of a send nobody lost settles it.
        let edited_id = edited["submission"].as_str().unwrap().to_string();
        app.finish_action(0, edited, answer(false, None));
        settle(&mut app);
        let again = send_words(&mut app, "Back up the NAS and the laptop");
        assert_ne!(again["submission"], edited_id.as_str());
    }

    #[test]
    fn an_empty_new_intent_sends_nothing() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        press(&mut app, ' ');
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert!(matches!(app.mode, Mode::Compose { .. }));
    }

    #[test]
    fn the_menu_uses_the_direct_letters_and_n_is_not_new() {
        let mut app = demo_app();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Palette { .. }), "n left the menu");
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'f');
        assert!(matches!(app.mode, Mode::Filter { .. }));
    }

    #[test]
    fn the_filter_list_picks_an_owner_and_f_shows_everything_again() {
        let mut app = demo_app();
        press(&mut app, 'f');
        let rows = app.filter_rows();
        assert_eq!(rows[0].label, "All work");
        assert_eq!(rows[0].owner, None);
        let pick = rows
            .iter()
            .position(|r| r.owner.is_some() && r.project.is_none())
            .expect("an owner row");
        let owner = rows[pick].owner.clone();
        for _ in 0..pick {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(app.owner, owner);
        assert!(
            app.board
                .cards
                .iter()
                .filter(|c| app.card_visible(c))
                .all(|c| Some(c.owner_key()) == owner)
        );
        // After /, letters narrow the list rather than moving through it.
        press(&mut app, 'f');
        press(&mut app, '/');
        press(&mut app, 'j');
        let Mode::Filter { query, typing, .. } = &app.mode else {
            panic!("not in the filter list")
        };
        assert_eq!((query.as_str(), *typing), ("j", true));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let Mode::Filter { query, typing, .. } = &app.mode else {
            panic!("Esc while searching closed the filter list")
        };
        assert_eq!((query.as_str(), *typing), ("", false));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(app.owner, owner, "Esc changed the filter");
        app.provider = Some("codex".into());
        press(&mut app, 'F');
        assert_eq!(
            (app.owner.clone(), app.project.clone(), app.provider.clone()),
            (None, None, None)
        );
    }

    #[test]
    fn b_in_the_filter_list_cycles_background_work_and_f_clears_it() {
        let mut app = demo_app();
        let shown = |app: &App| -> Vec<bool> {
            app.board
                .cards
                .iter()
                .filter(|c| c.stage != Stage::Misc && app.card_visible(c))
                .map(|c| c.background)
                .collect()
        };
        let everything = shown(&app);
        assert!(everything.contains(&true) && everything.contains(&false));
        press(&mut app, 'f');
        press(&mut app, 'b');
        let Mode::Filter { query, .. } = &app.mode else {
            panic!("b left the filter list")
        };
        assert!(query.is_empty(), "b went into the search");
        assert_eq!(app.background, Background::Only);
        let only = shown(&app);
        assert!(!only.is_empty() && only.iter().all(|b| *b));
        // The list's counts follow it, so they say what picking a row would show.
        assert_eq!(app.filter_rows()[0].open + app.filter_rows()[0].backlog, {
            app.board
                .cards
                .iter()
                .filter(|c| {
                    c.background
                        && !c.dismissed()
                        && !matches!(c.stage, Stage::Maintain | Stage::Misc)
                })
                .count()
        });
        press(&mut app, 'b');
        assert_eq!(app.background, Background::Hide);
        let none = shown(&app);
        assert!(!none.is_empty() && none.iter().all(|b| !*b));
        assert!(
            app.needs_you_order()
                .iter()
                .all(|k| !app.card(k).unwrap().background)
        );
        press(&mut app, 'b');
        assert_eq!(app.background, Background::All);
        assert_eq!(shown(&app), everything);
        // While searching, b is a letter of the search.
        press(&mut app, '/');
        press(&mut app, 'b');
        let Mode::Filter { query, .. } = &app.mode else {
            panic!("left the filter list")
        };
        assert_eq!(query, "b");
        assert_eq!(app.background, Background::All);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, 'b');
        press(&mut app, 'f');
        assert!(
            matches!(app.mode, Mode::Board),
            "f didn't close the filter list"
        );
        assert_eq!(app.background, Background::Only);
        press(&mut app, 'F');
        assert_eq!(app.background, Background::All);
    }

    #[test]
    fn closing_the_filters_after_b_leaves_the_backlog_on_a_card_it_shows() {
        let mut app = demo_app();
        app.enter_backlog(false);
        let len = app.backlog_cards().len();
        assert!(len > 2, "the demo's backlog is too short for this test");
        for _ in 0..len {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert!(matches!(app.mode, Mode::Backlog { sel } if sel == len - 1));
        for close in [KeyCode::Esc, KeyCode::Char('f')] {
            press(&mut app, 'f');
            press(&mut app, 'b');
            app.on_key(KeyEvent::new(close, KeyModifiers::NONE));
            let Mode::Backlog { sel } = app.mode else {
                panic!("closing the filters left the backlog")
            };
            assert!(
                sel < app.backlog_cards().len().max(1),
                "row {sel} of {}",
                app.backlog_cards().len()
            );
        }
    }

    #[test]
    fn search_leaves_out_workspaces_and_sessions_the_background_filter_hides() {
        let mut app = demo_app();
        let everything = |app: &App, filter| {
            app.search_hits(&Search {
                query: String::new(),
                filter,
                sel: 0,
            })
            .len()
        };
        for filter in [SearchFilter::Workspaces, SearchFilter::Sessions] {
            let all = everything(&app, filter);
            app.background = Background::Only;
            let only = everything(&app, filter);
            app.background = Background::Hide;
            let hide = everything(&app, filter);
            app.background = Background::All;
            assert!(only < all && hide < all, "{filter:?}: {all} {only} {hide}");
        }
    }

    #[test]
    fn a_new_intent_is_selected_once_its_card_arrives() {
        let mut app = demo_app();
        let (key, id) = waiting_backlog_card(&app);
        let machine = app.card(&key).unwrap().machine_index;
        app.enter_backlog(false);
        // Another machine's record with the same id is someone else's.
        app.new_intent_id = Some((machine + 1, id.clone()));
        app.rebuild();
        assert!(app.new_intent_id.is_some());
        app.new_intent_id = Some((machine, id));
        app.rebuild();
        assert!(app.new_intent_id.is_none());
        let Mode::Backlog { sel } = app.mode else {
            panic!("not in the backlog")
        };
        assert_eq!(app.board.cards[app.backlog_cards()[sel]].key, key);
    }

    #[test]
    fn a_menu_opened_on_a_card_never_acts_on_another_after_a_refresh() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let first = app.backlog_cards()[0];
        let id = app.board.cards[first].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        // The card it opened on starts while the menu is open, and leaves the backlog.
        for machine in &mut app.machines {
            if let Some(feed) = &mut machine.feed {
                for card in &mut feed.cards {
                    if card.id == id {
                        card.stage = Some("build".into());
                        card.state = Some("active".into());
                    }
                }
            }
        }
        app.rebuild();
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    #[test]
    fn card_keys_refuse_finished_work_whatever_its_state_says() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].state = None;
        app.sel = Selection {
            col: Stage::Maintain.index(),
            key: Some(key),
        };
        assert!(app.backlog_card_for_keys().is_none());
        for c in ['d', 'y', 'x', 't', 'J', 'u'] {
            assert_eq!(
                press(&mut app, c),
                Action::None,
                "{c} acted on finished work"
            );
        }
    }

    #[test]
    fn deleting_a_word_never_splits_a_character() {
        let mut app = demo_app();
        for (words, left) in [
            ("one\u{2003}two", "one\u{2003}"),
            ("one\u{a0}two", "one\u{a0}"),
            ("ünïcödé wörds  ", "ünïcödé "),
            ("lone", ""),
        ] {
            app.draft = words.into();
            app.mode = Mode::Board;
            press(&mut app, 'c');
            app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
            let Mode::Compose { draft, .. } = &app.mode else {
                panic!("left the box")
            };
            assert_eq!(draft.words, left, "{words:?}");
        }
    }

    #[test]
    fn the_filter_list_applies_the_row_it_highlights() {
        let mut app = demo_app();
        press(&mut app, 'f');
        let rows = app.filter_rows();
        app.mode = Mode::Filter {
            query: String::new(),
            sel: rows.len() + 5,
            typing: false,
            back: Box::new(Mode::Board),
        };
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Mode::Filter { sel, .. } = app.mode else {
            panic!("left the list")
        };
        let shown = app.filter_matches("");
        assert_eq!(sel, shown.len() - 1);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.owner, shown[sel].owner);
        assert_eq!(app.project, shown[sel].project);
    }

    #[test]
    fn a_paste_reaches_the_keys_list_search() {
        let mut app = demo_app();
        app.mode = Mode::Keys {
            filter: String::new(),
            typing: true,
            scroll: 7,
        };
        app.on_paste("machine\nnow");
        let Mode::Keys { filter, scroll, .. } = &app.mode else {
            panic!("left the keys list")
        };
        assert_eq!((filter.as_str(), *scroll), ("machine now", 0));
    }

    #[test]
    fn a_menu_opened_on_no_card_acts_on_none_that_arrives_later() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.sel = Selection::default();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Palette { target: None, .. }));
        // A card turns up selected while the menu is open.
        let (key, _) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key),
        };
        assert_eq!(press(&mut app, 'd'), Action::None);
    }

    #[test]
    fn a_confirmation_rechecks_its_intent_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        // It starts elsewhere while the question is open.
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].state = Some("active".into());
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
        // Still waiting: yes sends it.
        app.board.cards[i].stage = Stage::Backlog;
        app.board.cards[i].state = Some("queued".into());
        press(&mut app, 'x');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (request["action"].as_str(), request["id"].as_str()),
            (Some("dismiss"), Some(id.as_str()))
        );
    }

    /// A demo card for work under way whose workspace has closed: in Build, with a record, no
    /// workspace, homed in session `web` on the first machine.
    fn closed_workspace_card(app: &mut App) -> (String, String) {
        let i = app
            .board
            .cards
            .iter()
            .position(|c| c.detail.is_some() && c.stage == Stage::Build)
            .expect("a Build card in the demo");
        let machine = app.machines[0].name();
        let card = &mut app.board.cards[i];
        card.workspace_id = None;
        card.session = None;
        card.attention = Attention::Idle;
        card.machine_index = 0;
        let d = card.detail.as_mut().unwrap();
        d.disposition = None;
        d.intake = None;
        d.home = Some(crate::feed::Home {
            machine: Some(machine),
            session: Some("web".into()),
        });
        d.resume = Some(crate::feed::Resume {
            agent: Some("codex".into()),
            conversation: Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41".into()),
        });
        let (key, id) = (card.key.clone(), d.id.clone());
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key.clone()),
        };
        (key, id)
    }

    fn enter(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn enter_reopens_work_whose_home_names_only_a_machine_there() {
        // A record with no home, its conversation on another machine: that machine's feed names
        // it, with no session, and Enter sends the reopen there, not to the card's own machine,
        // leaving the session to the backend.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let other = app.machines[1].name();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(other),
            session: None,
        });
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter didn't ask to reopen: {:?}", app.message())
        };
        assert_eq!(machine, 1);
        assert_eq!(request, serde_json::json!({"action": "reopen", "id": id}));
    }

    fn esc(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    /// Enter on the card, as you would: the request it sends, with Kerdr remembering the card.
    fn reopen_by_enter(app: &mut App) -> serde_json::Value {
        let Action::Mutate { request, .. } = enter(app) else {
            panic!("Enter didn't ask to reopen")
        };
        app.busy = true;
        request
    }

    fn reopen_reply(
        session: &str,
        ok: bool,
        confirmation: bool,
        token: &str,
    ) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok,
            message: if confirmation {
                format!("{session} has 3 workspaces, its usual limit. Reopen it anyway?")
            } else {
                "Reopened, resuming its Codex conversation.".into()
            },
            confirmation,
            used: confirmation.then_some(3),
            limit: confirmation.then_some(3),
            session: Some(session.into()),
            workspace: (ok && !confirmation).then(|| "w7".into()),
            label: (ok && !confirmation).then(|| "41-checkout-expired-cards".into()),
            confirm: confirmation.then(|| token.into()),
            ..Default::default()
        })
    }

    #[test]
    fn enter_on_work_whose_workspace_closed_asks_its_home_to_reopen_it() {
        let mut app = demo_app();
        let (key, id) = closed_workspace_card(&mut app);
        assert!(app.card(&key).unwrap().can_reopen());
        assert_eq!(
            app.card(&key).unwrap().resume_agent().as_deref(),
            Some("Codex")
        );
        // Without actions there's no one to ask: it says how to get there, and sends nothing.
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("[actions]"));
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter didn't ask to reopen")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["session"], "web");
        assert!(request.get("override").is_none());
        assert_eq!(
            app.action_from.as_ref().map(|f| f.key.as_str()),
            Some(key.as_str())
        );
        assert_eq!(
            app.working_on(&request),
            format!("Reopening {}…", app.card(&key).unwrap().reference)
        );
        // From a peek too, which closes on the way.
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        assert!(matches!(enter(&mut app), Action::Mutate { .. }));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn enter_on_waiting_or_unrecorded_work_still_reopens_nothing() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        for (stage, attention) in [
            (Stage::Backlog, Attention::Idle),
            (Stage::Build, Attention::Done),
        ] {
            app.board.cards[i].stage = stage;
            app.board.cards[i].attention = attention;
            assert!(!app.board.cards[i].reopenable());
        }
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].attention = Attention::Idle;
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        assert!(!app.board.cards[i].can_reopen());
        app.board.cards[i].detail.as_mut().unwrap().intake = None;
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(!app.board.cards[i].can_reopen());
        app.board.cards[i].detail = None;
        assert!(!app.board.cards[i].can_reopen());
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key),
        };
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("nothing to jump into"));
    }

    /// Intent 0289: the Wi-Fi outage (0117)'s case, work its agent left open with no workspace.
    /// C asks, then marks it done; nothing reopens or starts.
    #[test]
    fn c_marks_open_work_with_no_workspace_done_after_asking() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'C'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("C didn't ask first")
        };
        assert_eq!(title, &format!("Mark {reference} done?"));
        assert!(message.contains("nothing started"), "{message}");
        // No is no.
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Board));
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(machine, 0);
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
        // Backlog work that turns out to be done already closes the same way.
        let (key, id) = waiting_backlog_card(&app);
        app.busy = false;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key),
        };
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes on a backlog card sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
    }

    #[test]
    fn x_sets_aside_started_work_with_no_workspace_after_asking() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'x'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("x didn't ask first")
        };
        assert_eq!(title, &format!("Won't do {reference}?"));
        assert!(
            message.contains("Its record keeps where it was"),
            "{message}"
        );
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "dismiss", "id": id}));
    }

    #[test]
    fn c_and_x_refuse_work_with_a_workspace_done_work_and_won_t_do() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // Its workspace is open: an agent may be at it.
        app.board.cards[i].workspace_id = Some("w9".into());
        app.board.cards[i].session = Some("web".into());
        for c in ['C', 'x'] {
            assert_eq!(press(&mut app, c), Action::None);
            assert!(matches!(app.mode, Mode::Board));
            assert!(
                app.message().unwrap().0.contains("has a workspace open"),
                "{:?}",
                app.message()
            );
        }
        // Done.
        app.board.cards[i].workspace_id = None;
        app.board.cards[i].session = None;
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].attention = Attention::Done;
        app.sel.col = Stage::Maintain.index();
        press(&mut app, 'C');
        assert!(app.message().unwrap().0.ends_with("is done already."));
        press(&mut app, 'x');
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("won't do is for work that isn't")
        );
        // In won't do: brought back first.
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].attention = Attention::Idle;
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        app.wont_do = true;
        app.mode = Mode::Backlog { sel: 0 };
        let sel = app.backlog_cards().iter().position(|&j| j == i).unwrap();
        app.mode = Mode::Backlog { sel };
        press(&mut app, 'C');
        assert!(app.message().unwrap().0.contains("u brings it back"));
        assert!(matches!(app.mode, Mode::Backlog { .. }));
    }

    #[test]
    fn d_on_started_work_with_no_workspace_names_c_and_x() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        assert_eq!(press(&mut app, 'd'), Action::None);
        let said = app.message().unwrap().0;
        assert!(
            said.contains("C marks it done and x sets it aside"),
            "{said}"
        );
    }

    #[test]
    fn a_close_or_won_t_do_rechecks_the_card_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // A workspace opens on it while the question is up: its agent is back.
        for c in ['C', 'x'] {
            press(&mut app, c);
            settle(&mut app);
            app.board.cards[i].workspace_id = Some("w9".into());
            app.board.cards[i].session = Some("web".into());
            assert_eq!(enter(&mut app), Action::None);
            assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
            app.board.cards[i].workspace_id = None;
            app.board.cards[i].session = None;
        }
        // Won't do asked about backlog work whose home moved meanwhile: not the work asked about.
        let (bkey, _) = waiting_backlog_card(&app);
        let b = app.board.cards.iter().position(|c| c.key == bkey).unwrap();
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(bkey),
        };
        press(&mut app, 'x');
        settle(&mut app);
        app.board.cards[b].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("elsewhere".into()),
        });
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key),
        };
        // Won't do asked about started work, which went back to the backlog meanwhile.
        press(&mut app, 'x');
        settle(&mut app);
        app.board.cards[i].stage = Stage::Backlog;
        app.board.cards[i].state = Some("queued".into());
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    #[test]
    fn the_menu_offers_c_and_x_for_started_work_with_no_workspace() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        assert!(app.backlog_card_for_keys().is_none());
        assert_eq!(
            app.card_for_keys().map(|c| c.key.clone()),
            Some(key.clone())
        );
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(&app.mode, Mode::Palette { target: Some(t), .. } if *t == key));
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the menu's C sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
    }

    /// Work an agent is at (intent 0321): the demo's Build card, its workspace 07-build open in
    /// web on the first machine, a Claude agent mid-step in it.
    fn running_card(app: &mut App) -> (String, String) {
        let (key, id) = closed_workspace_card(app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        let card = &mut app.board.cards[i];
        card.workspace_id = Some("w7".into());
        card.workspace_label = Some("07-build".into());
        card.session = Some("web".into());
        card.attention = Attention::Running;
        card.agents = vec![crate::model::AgentInfo {
            agent: "claude".into(),
            status: "working".into(),
            pane_id: "w7:p1".into(),
            ..Default::default()
        }];
        (key, id)
    }

    #[test]
    fn x_upper_cancels_work_an_agent_is_at_after_saying_what_it_stops() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'X'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("X didn't ask first: {:?}", app.message())
        };
        assert_eq!(title, &format!("Cancel {reference}?"));
        assert!(
            message.starts_with("Stops its Claude agent (mid-step) and closes workspace 07-build (web), with every tab in it, then sets"),
            "{message}"
        );
        assert!(message.contains("aside in Won't do"), "{message}");
        assert!(message.contains("u brings it back"), "{message}");
        // No is no.
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Board));
        press(&mut app, 'X');
        settle(&mut app);
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(machine, 0);
        assert_eq!(
            request,
            serde_json::json!({"action": "cancel", "id": id, "session": "web", "workspace": "w7"})
        );
        assert!(app.working_on(&request).contains("stopping its agent"));
    }

    #[test]
    fn x_upper_on_a_child_closes_only_its_tab_and_a_parent_names_its_children() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].tab_id = Some("w7:t2".into());
        press(&mut app, 'X');
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("X didn't ask first")
        };
        assert!(
            message.contains(
                "closes its tab in 07-build (web), leaving the rest of that workspace running"
            ),
            "{message}"
        );
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(request["tab"], "w7:t2");
        assert_eq!(request["id"], serde_json::json!(id));
        // The parent: another card, the whole workspace, its child named.
        app.busy = false;
        let child = app.board.cards[i].reference.clone();
        let p = app
            .board
            .cards
            .iter()
            .position(|c| {
                c.key != key
                    && c.detail.is_some()
                    && !matches!(c.stage, Stage::Maintain | Stage::Misc | Stage::Backlog)
            })
            .unwrap();
        let card = &mut app.board.cards[p];
        card.workspace_id = Some("w7".into());
        card.workspace_label = Some("07-build".into());
        card.session = Some("web".into());
        card.machine_index = 0;
        card.tab_id = None;
        card.attention = Attention::Idle;
        card.agents.clear();
        card.state = Some("active".into());
        let d = card.detail.as_mut().unwrap();
        d.disposition = None;
        d.intake = None;
        app.sel = Selection {
            col: app.board.cards[p].stage.index(),
            key: Some(app.board.cards[p].key.clone()),
        };
        press(&mut app, 'X');
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("X on the parent didn't ask: {:?}", app.message())
        };
        assert!(
            message.starts_with("Stops whatever runs there"),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "{child} in a tab there stops too and stays open with no agent"
            )),
            "{message}"
        );
    }

    #[test]
    fn x_upper_rechecks_where_the_work_runs_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = running_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        press(&mut app, 'X');
        settle(&mut app);
        // Its workspace closed and another opened on it while the question was up.
        app.board.cards[i].workspace_id = Some("w9".into());
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    #[test]
    fn x_upper_refuses_work_no_agent_is_at_and_done_work() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        assert_eq!(press(&mut app, 'X'), Action::None);
        assert!(matches!(app.mode, Mode::Board));
        assert!(
            app.message().unwrap().0.contains(
                "has no workspace open, so no agent to stop: x sets it aside, C marks it done."
            ),
            "{:?}",
            app.message()
        );
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w7".into());
        app.board.cards[i].session = Some("web".into());
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].attention = Attention::Done;
        app.sel.col = Stage::Maintain.index();
        press(&mut app, 'X');
        assert!(app.message().unwrap().0.contains("done already"));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn the_menu_and_the_bottom_line_offer_x_and_s_for_work_an_agent_is_at() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        assert_eq!(
            app.card_for_keys().map(|c| c.key.clone()),
            Some(key.clone())
        );
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(&app.mode, Mode::Palette { target: Some(t), .. } if *t == key));
        press(&mut app, 'X');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the menu's X sent nothing")
        };
        assert_eq!(request["action"], "cancel");
        assert_eq!(request["id"], serde_json::json!(id));
    }

    /// A done card (intent 0278): the same work, in Maintain, its workspace gone with its wrap.
    fn done_card(app: &mut App) -> (String, String) {
        let (key, id) = closed_workspace_card(app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.stage = Stage::Maintain;
        card.attention = Attention::Done;
        app.sel = Selection {
            col: Stage::Maintain.index(),
            key: Some(key.clone()),
        };
        (key, id)
    }

    fn done_question(reference: &str, token: &str) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok: false,
            message: format!(
                "{reference} is done: a yes sets it open again and resumes its Codex conversation. Reopen it?"
            ),
            confirmation: true,
            session: Some("web".into()),
            confirm: Some(token.into()),
            ..Default::default()
        })
    }

    #[test]
    fn enter_on_a_done_card_asks_first_then_reopens_it_on_a_yes() {
        let mut app = demo_app();
        let (key, id) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let card = app.card(&key).unwrap();
        assert!(card.can_reopen_done() && card.reopenable() && !card.can_reopen());
        // Without actions there's no one to ask: it says how to get there, and sends nothing.
        assert_eq!(enter(&mut app), Action::None);
        let said = app.message().unwrap().0.to_string();
        assert!(
            said.contains("is done") && said.contains("[actions]"),
            "{said}"
        );
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter on a done card didn't ask to reopen it")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], id.as_str());
        assert!(
            request.get("override").is_none(),
            "the first ask is never a yes"
        );
        // The backend asks first, saying it's done: that's a question on screen, not a failure.
        app.busy = true;
        app.finish_action(0, request, done_question(&reference, "0123456789ab0008"));
        let Mode::Confirm {
            title,
            message,
            queue,
            ..
        } = &app.mode
        else {
            panic!("a done card's reopen didn't ask")
        };
        assert_eq!(title, &format!("Reopen {reference} in web?"));
        assert!(message.starts_with(&format!("{reference} is done: a yes sets it open again")));
        assert!(!*queue);
        // No: nothing is sent, and the board is as it was.
        settle(&mut app);
        assert_eq!(press(&mut app, 'n'), Action::None);
        assert!(matches!(app.mode, Mode::Board));
        assert!(app.card(&key).unwrap().can_reopen_done());
        // Yes, to the question asked again: the yes carries its token.
        let request = reopen_by_enter(&mut app);
        app.finish_action(0, request, done_question(&reference, "0123456789ab0008"));
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (
                sent["override"].as_bool(),
                sent["confirm"].as_str(),
                sent["session"].as_str()
            ),
            (Some(true), Some("0123456789ab0008"), Some("web"))
        );
        // Reopened: Kerdr goes in, while you're still on its card.
        app.busy = true;
        app.finish_action(0, sent, reopen_reply("web", true, false, ""));
        assert_eq!(
            app.open_after.as_ref().map(|o| o.workspace.as_str()),
            Some("w7")
        );
    }

    #[test]
    fn enter_on_a_done_card_is_refused_plainly_where_it_cant_reopen() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // Won't do: said by Kerdr itself, and nothing sent.
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(!app.board.cards[i].reopenable());
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(
            app.message().unwrap().0,
            format!("{reference} is in won't do; u brings it back.")
        );
        app.board.cards[i].detail.as_mut().unwrap().disposition = None;
        // Still being written up: nothing to reopen.
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("nothing to jump into: it's done")
        );
        app.board.cards[i].detail.as_mut().unwrap().intake = None;
        // Homed on a machine this Kerdr doesn't know: said, and nothing sent.
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some("elsewhere".into()),
            session: Some("web".into()),
        });
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("home machine elsewhere is not configured")
        );
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("web".into()),
        });
        // What only its home can tell (another account's, another machine's, one that never ran
        // there): its refusal is said as it comes, with no question.
        for refusal in [
            format!("{reference} belongs to another account; reopen it there."),
            format!("{reference} belongs on lab; reopen it there."),
            format!(
                "{reference} is done, with no record of running on this machine (no home session, no conversation found), so there's nothing to reopen. m peeks at it."
            ),
        ] {
            let request = reopen_by_enter(&mut app);
            app.finish_action(
                0,
                request,
                Ok(crate::actions::Response {
                    ok: false,
                    message: refusal.clone(),
                    ..Default::default()
                }),
            );
            assert!(matches!(app.mode, Mode::Board), "a refusal asked something");
            assert_eq!(app.message().unwrap().0, refusal);
            assert_eq!(app.open_after, None);
        }
    }

    #[test]
    fn a_yes_to_reopen_a_done_card_is_refused_once_it_has_a_workspace_again() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let request = reopen_by_enter(&mut app);
        app.finish_action(0, request, done_question(&reference, "0123456789ab0008"));
        settle(&mut app);
        // Opened meanwhile, from somewhere else: the yes no longer fits it.
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.workspace_id = Some("w9".into());
        card.session = Some("web".into());
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("changed while you were deciding")
        );
    }

    #[test]
    fn a_done_cards_workspace_closing_as_you_go_in_asks_to_reopen_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.workspace_id = Some("w9".into());
        card.session = Some("web".into());
        // Its wrap closed the pane just as you went in.
        assert!(app.closed_under_you(&key, Some("w9")));
        assert_eq!(app.pending_reopen.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn a_reopen_question_and_its_yes_are_for_what_was_asked() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        // The backend asks about another session than the request named: the yes is for that one.
        app.finish_action(
            0,
            request,
            reopen_reply("api", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let Mode::Confirm { title, queue, .. } = &app.mode else {
            panic!("no question")
        };
        assert_eq!(
            title,
            &format!("Reopen {} in api?", app.card(&key).unwrap().reference)
        );
        assert!(*queue, "a full session's question offers the queue");
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (
                sent["override"].as_bool(),
                sent["session"].as_str(),
                sent["confirm"].as_str()
            ),
            (Some(true), Some("api"), Some("0123456789ab0004"))
        );
        // Something changed since: the backend asks again, with a new token, and that's a new
        // question for a new yes, not a failure.
        app.busy = true;
        app.finish_action(
            0,
            sent,
            reopen_reply("api", false, true, "0123456789ab0005"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        let Action::Mutate { request: again, .. } = press(&mut app, 'y') else {
            panic!("the second yes sent nothing")
        };
        assert_eq!(again["confirm"], "0123456789ab0005");
        // The same answer to the yes, the same token: no loop of questions.
        app.busy = true;
        app.finish_action(
            0,
            again,
            reopen_reply("api", false, true, "0123456789ab0005"),
        );
        settle(&mut app);
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
    }

    #[test]
    fn a_full_session_can_queue_the_reopen_instead() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'w') else {
            panic!("w sent nothing")
        };
        assert_eq!(
            (sent["queue"].as_bool(), sent.get("override")),
            (Some(true), None)
        );
        // The token goes with it: queueing done work is a yes to what was asked, too.
        assert_eq!(sent["confirm"], "0123456789ab0004");
        app.finish_action(
            0,
            sent,
            Ok(crate::actions::Response {
                ok: true,
                message: "Queued in web.".into(),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert_eq!(app.open_after, None);
    }

    #[test]
    fn a_yes_is_refused_when_the_work_changed_since_it_was_asked_for() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // The feed names another conversation while the request runs: no question at all.
        let request = reopen_by_enter(&mut app);
        app.board.cards[i]
            .detail
            .as_mut()
            .unwrap()
            .resume
            .as_mut()
            .unwrap()
            .conversation = Some("9a8b7c6d-0000-4000-8000-000000000002".into());
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
        // Its agent changes while the question is up: the yes sends nothing.
        let request = reopen_by_enter(&mut app);
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        app.board.cards[i]
            .detail
            .as_mut()
            .unwrap()
            .resume
            .as_mut()
            .unwrap()
            .agent = Some("claude".into());
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
        // It gets a workspace of its own while the question is up: the yes sends nothing either.
        let (key, _) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w9".into());
        app.board.cards[i].session = Some("web".into());
        assert_eq!(press(&mut app, 'y'), Action::None);
    }

    #[test]
    fn a_late_answer_never_catches_keys_meant_for_another_view_or_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        // Searching when it arrives: no question, and the search keeps its keys.
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Search(Search {
            query: "chec".into(),
            filter: SearchFilter::Everything,
            sel: 0,
        });
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Search(_)));
        assert!(app.message().unwrap().0.contains("needs a yes first"));
        press(&mut app, 'y');
        assert!(matches!(&app.mode, Mode::Search(s) if s.query == "checy"));
        // Reopened from the board, then into the backlog: the board's selection still names it,
        // but you're looking at the backlog, so it doesn't pull you out.
        app.mode = Mode::Board;
        app.sel.key = Some(key.clone());
        let request = reopen_by_enter(&mut app);
        press(&mut app, 'b');
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        app.finish_action(0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        // A dispatch from the backlog, then moving to another backlog card: no question for it.
        app.mode = Mode::Backlog { sel: 0 };
        app.rebuild();
        let first = app.backlog_card_for_keys().map(|c| c.key.clone());
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        app.busy = true;
        press(&mut app, 'j');
        assert_ne!(app.backlog_card_for_keys().map(|c| c.key.clone()), first);
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Backlog { .. }));
    }

    #[test]
    fn a_reopen_takes_you_in_only_while_you_are_still_on_its_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .key
            .clone();
        // Moved to another card while it ran: it says so instead of taking you away.
        let request = reopen_by_enter(&mut app);
        app.sel.key = Some(other);
        app.finish_action(0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        assert!(app.message().unwrap().0.contains("on its card goes in"));
        // Esc while it runs, from its peek: it finishes, but takes you nowhere.
        app.sel.key = Some(key.clone());
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Board));
        app.finish_action(0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        // Still on it, in its peek: in it goes, remembering where it came from.
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        app.finish_action(0, request.clone(), reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(
            app.open_after.take(),
            Some(OpenAfter {
                machine: 0,
                session: "web".into(),
                workspace: "w7".into(),
                label: "41-checkout-expired-cards".into(),
                from: key.clone(),
            })
        );
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Good));
        // The board already shows the workspace it made by the time the answer comes: still in.
        app.mode = Mode::Board;
        let request = reopen_by_enter(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w7".into());
        app.board.cards[i].session = Some("web".into());
        app.finish_action(0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert!(app.open_after.take().is_some());
        // But a workspace other than the one it made is another matter.
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        app.board.cards[i].workspace_id = Some("w8".into());
        app.board.cards[i].session = Some("web".into());
        app.finish_action(0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        closed_workspace_card(&mut app);
        // A refusal goes nowhere.
        app.mode = Mode::Board;
        let request = reopen_by_enter(&mut app);
        app.finish_action(0, request, reopen_reply("web", false, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    #[test]
    fn a_workspace_found_closed_on_the_way_in_reopens_work_under_way() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // herdr says w5 closed, and the board hasn't caught up: it still shows w5. Reopen.
        app.board.cards[i].workspace_id = Some("w5".into());
        app.board.cards[i].session = Some("web".into());
        assert!(app.closed_under_you(&key, Some("w5")));
        assert_eq!(app.pending_reopen.take().as_deref(), Some(key.as_str()));
        // It has moved to another workspace meanwhile: that one's not closed, so no reopen.
        assert!(!app.closed_under_you(&key, Some("w4")));
        assert_eq!(app.pending_reopen, None);
        // Esc before its turn drops the reopen too.
        assert!(app.closed_under_you(&key, Some("w5")));
        esc(&mut app);
        assert_eq!(app.pending_reopen, None);
        // Its machine was away and its workspace closed meanwhile: reopen when it answers.
        app.board.cards[i].workspace_id = None;
        app.board.cards[i].session = None;
        let pending = |app: &App| PendingJump {
            key: key.clone(),
            workspace: "#41's workspace".into(),
            machine: app.card(&key).unwrap().machine.clone(),
        };
        app.pending_jump = Some(pending(&app));
        assert_eq!(app.pending_jump_ready(), None);
        assert_eq!(app.pending_reopen.as_deref(), Some(key.as_str()));
        // Without actions, it says what happened, as before.
        app.pending_reopen = None;
        app.cfg.actions.command = None;
        app.pending_jump = Some(pending(&app));
        assert_eq!(app.pending_jump_ready(), None);
        assert_eq!(app.pending_reopen, None);
        assert!(app.message().unwrap().0.contains("closed while"));
        assert!(!app.closed_under_you(&key, None));
    }

    #[test]
    fn a_question_that_appears_on_its_own_ignores_keys_held_or_typed_before_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        app.finish_action(
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        // Enter still held from opening it, a repeat of y, w: none answers it.
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert_eq!(press(&mut app, 'w'), Action::None);
        let mut held = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        held.kind = KeyEventKind::Repeat;
        settle(&mut app);
        assert_eq!(app.on_key(held), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        // A fresh y once it has been up a moment does.
        assert!(matches!(press(&mut app, 'y'), Action::Mutate { .. }));
    }

    #[test]
    fn a_start_work_yes_goes_where_it_was_asked_and_not_after_the_home_moved() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        let Action::Mutate { request, machine } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        app.busy = true;
        let asks = crate::actions::Response {
            ok: false,
            message: "web has 3 workspaces. Start anyway?".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            ..Default::default()
        };
        app.finish_action(machine, request.clone(), Ok(asks.clone()));
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (sent["override"].as_bool(), sent["session"].as_str()),
            (Some(true), Some("web"))
        );
        // Its home moves while the question is up: the yes sends nothing.
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        app.finish_action(machine, request, Ok(asks));
        settle(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("api".into()),
        });
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }
}
