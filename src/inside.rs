//! Work inside a card: what an agent made for a card (its children, its undertaker, its
//! follow-ups) leaves the board's columns and sits inside that card, counted on its line, a key
//! away on the card's own screen (`o`), and still stopped on by `n` and Ctrl+Space Space.

use super::*;
use crate::family::{self, Group, Holds};

/// One line of a card's own screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// The card itself, first.
    Itself(usize),
    Blank,
    /// A group's heading. A folded one is a line you can select, and Enter opens it.
    Heading {
        group: Group,
        members: Vec<usize>,
        folded: bool,
    },
    /// A piece, `depth` deep: a piece of a piece shows under its own card's row.
    Piece {
        index: usize,
        depth: usize,
    },
}

impl Row {
    /// What a selection on this row is called in `Mode::Card`'s focus: "" for the card itself, a
    /// card's key, or `group:<id>` for a folded group. None for a line that can't be selected.
    pub fn focus(&self, app: &App) -> Option<String> {
        match self {
            Row::Itself(_) => Some(String::new()),
            Row::Heading {
                group,
                folded: true,
                ..
            } => Some(format!("group:{}", group.id())),
            Row::Piece { index, .. } => app.board.cards.get(*index).map(|c| c.key.clone()),
            _ => None,
        }
    }
}

/// More pieces than this, and every group but what needs you starts folded.
const MANY: usize = 12;

impl App {
    pub fn card_index(&self, key: &str) -> Option<usize> {
        self.board.cards.iter().position(|c| c.key == key)
    }

    /// Whether a card shows on the board as it's filtered now, column or not.
    pub fn shown_on_board(&self, i: usize) -> bool {
        self.board
            .cards
            .get(i)
            .is_some_and(|c| c.stage != Stage::Misc && self.card_visible(c))
    }

    /// The card this one sits inside on the board: the topmost of its family that's shown.
    pub fn holder(&self, i: usize) -> Option<usize> {
        let (mut cur, mut top) = (i, None);
        for _ in 0..self.board.cards.len() {
            match self.board.up.get(cur).copied().flatten() {
                Some(up) => {
                    if self.shown_on_board(up) {
                        top = Some(up);
                    }
                    cur = up;
                }
                None => break,
            }
        }
        top
    }

    /// A card shown on its own although its record sits in another card, out of view here (on
    /// another machine's tab, filtered out, or in no feed): that card's handle, for "in #256".
    pub fn held_elsewhere(&self, i: usize) -> Option<String> {
        let card = self.board.cards.get(i)?;
        if self.holder(i).is_some() {
            return None;
        }
        card.link.as_ref().map(|(r, _)| r.clone())
    }

    fn children(&self, i: usize) -> Vec<usize> {
        let mut kids = self.board.down.get(i).cloned().unwrap_or_default();
        let key = |k: &usize| {
            let c = &self.board.cards[*k];
            (
                self.subtree_group(*k),
                family::group(c),
                c.attention,
                c.reference.len(),
                c.reference.clone(),
            )
        };
        kids.sort_by_key(key);
        kids
    }

    /// The most urgent group among a card and everything inside it.
    fn subtree_group(&self, i: usize) -> Group {
        self.board
            .subtree
            .get(i)
            .copied()
            .filter(|_| self.board.subtree.len() == self.board.cards.len())
            .unwrap_or_else(|| family::group(&self.board.cards[i]))
    }

    /// Every card inside card i, nested ones too, each followed by its own.
    pub fn pieces(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.walk(i, 0, &mut out);
        out.into_iter().map(|(k, _)| k).collect()
    }

    fn walk(&self, i: usize, depth: usize, out: &mut Vec<(usize, usize)>) {
        for k in self.children(i) {
            // The tree has no loops; this only bounds a board changed under it.
            if out.len() > self.board.cards.len() {
                return;
            }
            out.push((k, depth));
            self.walk(k, depth + 1, out);
        }
    }

    /// What card i holds, for its line on the board.
    pub fn holds(&self, i: usize) -> Holds {
        match self.board.holds.get(i) {
            Some(h) if self.board.holds.len() == self.board.cards.len() => h.clone(),
            _ => Holds::of(self.pieces(i).iter().map(|&k| &self.board.cards[k])),
        }
    }

    /// The lines of card i's own screen: itself, then what's inside by group. Done folds past
    /// three, and past MANY pieces every group but what needs you folds, unless opened.
    pub fn card_rows(&self, i: usize, opened: &[Group]) -> Vec<Row> {
        let mut rows = vec![Row::Itself(i)];
        let many = self.pieces(i).len() > MANY;
        for g in Group::ALL {
            let mut members = Vec::new();
            for k in self.children(i) {
                if self.subtree_group(k) == g {
                    members.push((k, 0));
                    self.walk(k, 1, &mut members);
                }
            }
            if members.is_empty() {
                continue;
            }
            let folded = g != Group::NeedsYou
                && !opened.contains(&g)
                && (many || (g == Group::Done && members.len() > 3));
            rows.push(Row::Blank);
            rows.push(Row::Heading {
                group: g,
                members: members.iter().map(|(k, _)| *k).collect(),
                folded,
            });
            if !folded {
                rows.extend(
                    members
                        .into_iter()
                        .map(|(index, depth)| Row::Piece { index, depth }),
                );
            }
        }
        rows
    }

    /// Card i's pieces in the order its screen shows them, every group open.
    pub fn piece_order(&self, i: usize) -> Vec<usize> {
        self.card_rows(i, &Group::ALL)
            .into_iter()
            .filter_map(|r| match r {
                Row::Piece { index, .. } => Some(index),
                _ => None,
            })
            .collect()
    }

    /// Board order for the cards in a column: a card whose piece needs you sorts as needing you,
    /// and in Maintain, done cards with work still open inside go first, those needing you first.
    pub(super) fn arrange(&self, stage: Stage, col: &mut [usize]) {
        match stage {
            Stage::Maintain => col.sort_by_key(|&i| {
                let h = self.holds(i);
                if h.needs > 0 {
                    0
                } else if h.open > 0 {
                    1
                } else {
                    2
                }
            }),
            Stage::Backlog | Stage::Misc => {}
            _ => col.sort_by_key(|&i| {
                let a = self.board.cards[i].attention;
                if self.holds(i).needs > 0 {
                    a.min(Attention::NeedsYou)
                } else {
                    a
                }
            }),
        }
    }

    /// Opens card i's own screen on `focus` ("" for the card itself), going back to `back`.
    pub(super) fn open_card(&mut self, i: usize, focus: Option<String>, back: Mode) {
        let Some(card) = self.board.cards.get(i) else {
            return;
        };
        self.mode = Mode::Card {
            card: card.key.clone(),
            focus: focus.unwrap_or_default(),
            opened: vec![],
            back: Box::new(back),
        };
        self.fix_card_screen();
    }

    /// `o`: card i's own screen. A card shown on its own while its record sits in a card out of
    /// view opens that card's screen, on it; on a card's screen, a row's own.
    pub(super) fn open_inside(&mut self, key: &str) {
        let Some(i) = self.card_index(key) else {
            return;
        };
        let back = self.mode.clone();
        if !matches!(self.mode, Mode::Card { .. })
            && self.holder(i).is_none()
            && let Some(up) = self.board.up.get(i).copied().flatten()
        {
            return self.open_card(up, Some(key.to_string()), back);
        }
        self.open_card(i, None, back);
    }

    /// Keeps the card screen's focus on a row it shows (opening a folded group a piece is in),
    /// and the selection on what that row is. False when not on a card's screen, or its card has
    /// gone, which goes back.
    pub(super) fn fix_card_screen(&mut self) -> bool {
        let Mode::Card {
            card,
            mut focus,
            mut opened,
            back,
        } = self.mode.clone()
        else {
            return false;
        };
        let Some(i) = self.card_index(&card) else {
            self.mode = *back;
            return false;
        };
        let focusable = |app: &App, opened: &[Group]| -> Vec<String> {
            app.card_rows(i, opened)
                .iter()
                .filter_map(|r| r.focus(app))
                .collect()
        };
        if !focusable(self, &opened).contains(&focus) {
            let group = self.card_rows(i, &Group::ALL).iter().find_map(|r| match r {
                Row::Heading { group, members, .. }
                    if members.iter().any(|&k| self.board.cards[k].key == focus) =>
                {
                    Some(*group)
                }
                _ => None,
            });
            match group {
                Some(g) if !opened.contains(&g) => opened.push(g),
                // A folded group that's gone, or none: its card itself.
                _ if focus.starts_with("group:") => focus = String::new(),
                _ => {}
            }
        }
        // A piece that has left this card (closed out of the feed, or moved to another family)
        // stays the focus, with nothing selected, so no key acts on a card you didn't choose
        // until you move.
        let gone = !focusable(self, &opened).contains(&focus);
        let selected = if gone || focus.starts_with("group:") {
            None
        } else if focus.is_empty() {
            Some(card.clone())
        } else {
            Some(focus.clone())
        };
        self.sel.key = selected;
        if let Some(c) = self.selected() {
            self.sel.col = c.stage.index();
        }
        self.mode = Mode::Card {
            card,
            focus,
            opened,
            back,
        };
        true
    }

    /// Esc on a card's screen: back where it was opened from, on the card.
    fn leave_card(&mut self, card: &str, focus: &str, back: Mode) {
        self.mode = back;
        let mut tries: Vec<usize> = Vec::new();
        for k in [card, focus] {
            if let Some(i) = self.card_index(k) {
                tries.push(i);
                tries.extend(self.holder(i));
            }
        }
        if let Some(&i) = tries
            .iter()
            .find(|&&i| self.shown_on_board(i) && self.holder(i).is_none())
        {
            let c = &self.board.cards[i];
            self.sel = Selection {
                col: c.stage.index(),
                key: Some(c.key.clone()),
            };
            if matches!(self.mode, Mode::Backlog { .. }) {
                self.backlog_focus = Some(c.key.clone());
            }
        }
        self.fix_selection();
    }

    pub(super) fn card_key(
        &mut self,
        key: KeyEvent,
        card: String,
        focus: String,
        mut opened: Vec<Group>,
        back: Box<Mode>,
    ) -> Action {
        let Some(i) = self.card_index(&card) else {
            self.mode = *back;
            return Action::None;
        };
        let rows = self.card_rows(i, &opened);
        let foci: Vec<String> = rows.iter().filter_map(|r| r.focus(self)).collect();
        let at = foci.iter().position(|f| *f == focus).unwrap_or(0);
        let go = |app: &mut App, focus: String, opened: Vec<Group>| {
            app.mode = Mode::Card {
                card: card.clone(),
                focus,
                opened,
                back: back.clone(),
            };
            app.fix_card_screen();
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.leave_card(&card, &focus, *back.clone());
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let next = foci
                    .get((at + 1).min(foci.len().saturating_sub(1)))
                    .cloned();
                go(self, next.unwrap_or_default(), opened);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let next = foci.get(at.saturating_sub(1)).cloned();
                go(self, next.unwrap_or_default(), opened);
            }
            KeyCode::Enter => {
                if let Some(id) = focus.strip_prefix("group:") {
                    if let Some(g) = Group::ALL.into_iter().find(|g| g.id() == id) {
                        opened.push(g);
                        // The first of its rows, once it's open.
                        let first = self.card_rows(i, &opened).iter().find_map(|r| match r {
                            Row::Heading { group, members, .. } if *group == g => {
                                members.first().map(|&k| self.board.cards[k].key.clone())
                            }
                            _ => None,
                        });
                        go(self, first.unwrap_or_default(), opened);
                    }
                    return Action::None;
                }
                if !foci.contains(&focus) {
                    self.say(
                        "That row has left this card; j and k choose another.",
                        Tone::Info,
                    );
                    return Action::None;
                }
                let key = if focus.is_empty() { card } else { focus };
                return self.jump_action(key);
            }
            KeyCode::Char('m' | ' ') => {
                if !focus.starts_with("group:") && self.sel.key.is_some() {
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(self.mode.clone()),
                    };
                }
            }
            KeyCode::Char('o') => {
                if let Some(k) = self.sel.key.clone().filter(|_| !focus.is_empty())
                    && let Some(p) = self.card_index(&k)
                {
                    if self.board.down.get(p).is_some_and(|d| !d.is_empty()) {
                        self.open_inside(&k);
                    } else {
                        let r = self.board.cards[p].reference.clone();
                        self.say(format!("{r} holds nothing of its own."), Tone::Info);
                    }
                }
            }
            KeyCode::Char('n') => {
                self.step_needs_you(1);
            }
            KeyCode::Char('N') => {
                self.step_needs_you(-1);
            }
            KeyCode::Char(c @ ('d' | 'y' | 'x' | 'C' | 'X')) => {
                return self.card_action(c, 1);
            }
            _ => {}
        }
        Action::None
    }

    /// Cards and the pieces inside them that need you, in board order: a card, then what inside
    /// it needs you, in its screen's order.
    pub(super) fn needs_order(&self) -> Vec<String> {
        let cols = self.columns();
        let vis = self.visible_columns();
        let mut out = Vec::new();
        for ci in 0..7 {
            if !vis[ci] {
                continue;
            }
            for &i in &cols[ci] {
                let card = &self.board.cards[i];
                if card.needs_you() {
                    out.push(card.key.clone());
                }
                for p in self.piece_order(i) {
                    let piece = &self.board.cards[p];
                    if family::needs(piece) && !out.contains(&piece.key) {
                        out.push(piece.key.clone());
                    }
                }
            }
        }
        out
    }

    /// n and N: the next (or previous) card or piece that needs you. A piece opens its card's
    /// screen on it; a card on the board selects it. Its key.
    pub(super) fn needs_step(&mut self, dir: i32) -> Option<String> {
        let order = self.needs_you_order();
        if order.is_empty() {
            self.say("Nothing here needs you right now.", Tone::Good);
            return None;
        }
        let len = order.len() as i32;
        let pos = self
            .sel
            .key
            .as_ref()
            .and_then(|k| order.iter().position(|o| o == k));
        let next = match pos {
            Some(p) => (p as i32 + dir).rem_euclid(len) as usize,
            None => self
                .needs_from_card_screen(&order, dir)
                .unwrap_or(if dir > 0 { 0 } else { order.len() - 1 }),
        };
        let key = order[next].clone();
        self.go_to(&key);
        Some(key)
    }

    /// On a card's screen, on a row that doesn't need you: where n goes from there, the next of
    /// its rows that does in that direction, else on from its last.
    fn needs_from_card_screen(&self, order: &[String], dir: i32) -> Option<usize> {
        let Mode::Card { card, focus, .. } = &self.mode else {
            return None;
        };
        let i = self.card_index(card)?;
        let rows: Vec<String> = std::iter::once(card.clone())
            .chain(
                self.piece_order(i)
                    .into_iter()
                    .map(|p| self.board.cards[p].key.clone()),
            )
            .collect();
        let here = rows.iter().position(|r| r == focus).unwrap_or(0);
        let mine: Vec<(usize, usize)> = order
            .iter()
            .enumerate()
            .filter_map(|(o, k)| rows.iter().position(|r| r == k).map(|r| (o, r)))
            .collect();
        let len = order.len() as i32;
        if dir > 0 {
            mine.iter()
                .find(|(_, r)| *r > here)
                .map(|(o, _)| *o)
                .or_else(|| {
                    mine.last()
                        .map(|(o, _)| ((*o as i32 + 1).rem_euclid(len)) as usize)
                })
        } else {
            mine.iter()
                .rev()
                .find(|(_, r)| *r < here)
                .map(|(o, _)| *o)
                .or_else(|| {
                    mine.first()
                        .map(|(o, _)| ((*o as i32 - 1).rem_euclid(len)) as usize)
                })
        }
    }

    /// Goes to a card or piece from n: a piece on its card's screen, a card on the board.
    fn go_to(&mut self, key: &str) {
        let Some(i) = self.card_index(key) else {
            return;
        };
        let card = &self.board.cards[i];
        self.sel = Selection {
            col: card.stage.index(),
            key: Some(card.key.clone()),
        };
        // Out of every card screen at once, to where the first was opened from.
        let leave = |mode: &Mode| {
            let mut m = mode.clone();
            while let Mode::Card { back, .. } = m {
                m = *back;
            }
            m
        };
        match self.holder(i) {
            Some(h) if matches!(self.mode, Mode::Board | Mode::Card { .. }) => {
                if let Mode::Card { card, opened, .. } = &mut self.mode
                    && *card == self.board.cards[h].key
                {
                    let opened = opened.clone();
                    let back = leave(&self.mode);
                    self.mode = Mode::Card {
                        card: self.board.cards[h].key.clone(),
                        focus: key.to_string(),
                        opened,
                        back: Box::new(back),
                    };
                    self.fix_card_screen();
                    return;
                }
                let back = leave(&self.mode);
                self.open_card(h, Some(key.to_string()), back);
            }
            None if matches!(self.mode, Mode::Card { .. }) => {
                self.mode = leave(&self.mode);
                self.sel = Selection {
                    col: self.board.cards[i].stage.index(),
                    key: Some(key.to_string()),
                };
            }
            _ => {}
        }
    }

    /// Other cards running in tabs of this card's workspace: what Enter must never land on.
    pub fn tabs_inside(&self, card: &Card) -> Vec<&Card> {
        if card.workspace_id.is_none() {
            return vec![];
        }
        self.board
            .cards
            .iter()
            .filter(|c| {
                c.key != card.key
                    && c.tab_id.is_some()
                    && c.machine_index == card.machine_index
                    && c.session == card.session
                    && c.workspace_id == card.workspace_id
            })
            .collect()
    }

    /// The tab Enter lands on for a card, and whether only that tab will do: a card confined to a
    /// tab, or a card's own tab while other work runs in its workspace's other tabs. Elsewhere a
    /// tab that has closed leaves herdr on the workspace, as before.
    pub fn jump_tab(&self, card: &Card) -> (Option<String>, bool) {
        if let Some(tab) = &card.tab_id {
            (Some(tab.clone()), true)
        } else if let Some(tab) = &card.own_tab {
            // A recorded own tab is never guessed past: when it's gone, Enter offers a reopen.
            (Some(tab.clone()), true)
        } else {
            (None, false)
        }
    }

    /// Enter on a card whose workspace holds other work's tabs while its records name no tab of
    /// its own: it never guesses, and offers to reopen the card's own conversation instead.
    pub(super) fn own_tab_unknown(&mut self, card: &Card) -> Option<Action> {
        if card.tab_id.is_some() || card.own_tab.is_some() || self.tabs_inside(card).is_empty() {
            return None;
        }
        Some(self.offer_own_reopen(card, "has no tab of its own on record"))
    }

    /// herdr found the tab Enter was for has closed. A piece's reopens, as a card whose workspace
    /// closed does; a card's own offers to reopen its conversation, never landing on another tab.
    pub fn tab_gone(&mut self, key: &str) {
        let Some(card) = self.card(key).cloned() else {
            return;
        };
        if card.tab_id.is_some() {
            if self.actions_on() && (card.under_way() || card.done()) {
                self.pending_reopen = Some(key.to_string());
            } else {
                self.say(format!("{}'s tab has closed.", card.reference), Tone::Warn);
            }
        } else {
            self.offer_own_reopen(&card, "has closed its tab");
        }
    }

    fn offer_own_reopen(&mut self, card: &Card, why: &str) -> Action {
        let others: Vec<String> = self
            .tabs_inside(card)
            .iter()
            .map(|c| c.reference.clone())
            .collect();
        let tabs = match others.as_slice() {
            [] => "other tabs".to_string(),
            [one] => format!("{one}'s tab"),
            many => format!("tabs for {}", many.join(", ")),
        };
        let reference = card.reference.clone();
        let said = format!(
            "{reference}'s own agent {why}; ⏎ won't open another tab in its workspace, which may hold {tabs}."
        );
        if !self.actions_on() || card.detail.is_none() {
            self.say(format!("{said} o shows what's inside."), Tone::Info);
            return Action::None;
        }
        let back = self.mode.clone();
        match self.reopen_action(card) {
            Action::Mutate { machine, request } => {
                // Asked here first; the yes sends the reopen as ⏎ would, with the backend's own
                // questions after it.
                let target = self.action_from.take().map(Box::new);
                self.mode = Mode::Confirm {
                    machine,
                    request,
                    title: format!("Reopen {reference}'s own agent?"),
                    message: format!(
                        "{said} y reopens {reference}'s conversation, as ⏎ does for work whose workspace closed; your backend asks anything it needs to."
                    ),
                    back: Box::new(back),
                    target,
                    queue: false,
                    ready_at: None,
                    plain: true,
                };
            }
            other => return other,
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::MachineState;
    use crate::config::Machine;
    use crate::feed::{Feed, FeedCard, FeedWorkspace, Home, WaitsOn};
    use crate::herdr::{Agent, SessionInfo, Snapshot, Workspace};
    use crate::poll::{Conn, MachineData};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn machine(
        name: &str,
        ssh: bool,
        session: &str,
        snap: Snapshot,
        cards: Vec<FeedCard>,
    ) -> MachineState {
        let mut m = MachineState::new(Machine {
            name: name.into(),
            ssh: ssh.then(|| name.to_lowercase()),
            herdr: "herdr".into(),
            feed: Some("ark-cards".into()),
            hidden: false,
        });
        m.conn = Conn::Live {
            latency: Default::default(),
        };
        m.data = Some(MachineData {
            host: Some(name.into()),
            version: Some("0.9.5".into()),
            sessions: vec![SessionInfo {
                name: session.into(),
                running: true,
                default: false,
            }],
            snapshots: [(session.to_string(), Ok(snap))].into_iter().collect(),
            keys: true,
            feed: None,
            feed_failed_after: None,
        });
        m.feed = Some(Feed {
            kerdr_feed: 1,
            source: Some("ark-sdlc".into()),
            machine: Some(name.into()),
            cards,
            ..Default::default()
        });
        m
    }

    fn card(id: &str, title: &str, stage: &str, f: impl FnOnce(&mut FeedCard)) -> FeedCard {
        let mut c = FeedCard {
            id: id.into(),
            title: title.into(),
            stage: Some(stage.into()),
            state: Some(
                match stage {
                    "backlog" => "queued",
                    "maintain" => "closed",
                    _ => "active",
                }
                .into(),
            ),
            home: Some(Home {
                machine: Some("AMD-LZM4".into()),
                session: Some("KBZ".into()),
            }),
            ..Default::default()
        };
        f(&mut c);
        c
    }

    fn agent(pane: &str, tab: &str, status: &str) -> Agent {
        Agent {
            agent: Some("claude".into()),
            agent_status: Some(status.into()),
            pane_id: pane.into(),
            tab_id: Some(tab.into()),
            workspace_id: pane.split(':').next().unwrap().into(),
            ..Default::default()
        }
    }

    fn in_ws(id: &str, label: &str, tab: Option<&str>, own: Option<&str>) -> Option<FeedWorkspace> {
        Some(FeedWorkspace {
            session: Some("KBZ".into()),
            id: Some(id.into()),
            label: Some(label.into()),
            tab: tab.map(Into::into),
            own_tab: own.map(Into::into),
        })
    }

    /// The laptop holds 0256 (its own agent in tab 1, its child 0262 asking in tab 2, 0277 and
    /// 0332 not started) and 0139, done with three follow-ups open; arknet-one runs 0331, a
    /// follow-up of 0256's.
    fn families(own_tab: Option<&str>) -> App {
        let laptop_snap = Snapshot {
            workspaces: vec![Workspace {
                workspace_id: "w2K".into(),
                label: "56-agents-on-the-tenant-server".into(),
                ..Default::default()
            }],
            agents: vec![
                agent("w2K:p1", "w2K:t1", "idle"),
                agent("w2K:p2", "w2K:t2", "blocked"),
            ],
            ..Default::default()
        };
        let label = "56-agents-on-the-tenant-server";
        let laptop = machine(
            "AMD-LZM4",
            false,
            "KBZ",
            laptop_snap,
            vec![
                card("0256", "Agents on the tenant server", "test", |c| {
                    c.attention = Some("waiting".into());
                    c.workspace = in_ws("w2K", label, None, own_tab);
                }),
                card("0262", "Agent machine setup, as code", "deploy", |c| {
                    c.parent = Some("0256".into());
                    c.workspace = in_ws("w2K", label, Some("w2K:t2"), None);
                    c.needs = Some("It's kept open until you say wrap.".into());
                }),
                card("0277", "Agents' glab token and rotation", "backlog", |c| {
                    c.parent = Some("0256".into());
                    c.waits_on = vec![WaitsOn {
                        reference: Some("#262".into()),
                        open: Some(true),
                        ..Default::default()
                    }];
                }),
                card("0332", "Live check: arknet-one asks LAB", "backlog", |c| {
                    c.from = Some("0256".into());
                }),
                card("0139", "knowledge browser web app", "maintain", |c| {
                    c.attention = Some("done".into());
                    c.closed = Some("2026-10-31".into());
                }),
                card("0287", "Browser tests on today's ark-tenant", "test", |c| {
                    c.from = Some("0139".into());
                    c.no_agent = true;
                    c.lane = Some("background".into());
                }),
                card(
                    "0348",
                    "Knowledge browser live access checks",
                    "backlog",
                    |c| {
                        c.from = Some("0139".into());
                    },
                ),
                card(
                    "0350",
                    "Browser key-node shapes design pass",
                    "backlog",
                    |c| {
                        c.from = Some("0139".into());
                    },
                ),
                card("0143", "Find the next flywheel", "maintain", |c| {
                    c.attention = Some("done".into());
                    c.closed = Some("2026-10-31".into());
                }),
            ],
        );
        let one_snap = Snapshot {
            workspaces: vec![Workspace {
                workspace_id: "w1".into(),
                label: "31-live-check".into(),
                ..Default::default()
            }],
            agents: vec![agent("w1:p1", "w1:t1", "working")],
            ..Default::default()
        };
        let one = machine(
            "arknet-one",
            true,
            "ARK-Background",
            one_snap,
            vec![card(
                "0331",
                "Live check: Arkship work on arknet-one",
                "build",
                |c| {
                    c.from = Some("0256".into());
                    c.lane = Some("background".into());
                    c.home = Some(Home {
                        machine: Some("arknet-one".into()),
                        session: Some("ARK-Background".into()),
                    });
                    c.workspace = Some(FeedWorkspace {
                        session: Some("ARK-Background".into()),
                        id: Some("w1".into()),
                        label: Some("31-live-check".into()),
                        ..Default::default()
                    });
                },
            )],
        );
        let mut cfg = Config::default();
        cfg.actions.command = Some("ark-card-action".into());
        let mut app = App::new(cfg, vec![laptop, one]);
        app.fixed_now = Some(crate::demo::now());
        app
    }

    fn key(app: &App, id: &str) -> String {
        format!("ark-sdlc:{id}").tap(|k| assert!(app.card(k).is_some(), "{k} on the board"))
    }

    trait Tap: Sized {
        fn tap(self, f: impl FnOnce(&Self)) -> Self {
            f(&self);
            self
        }
    }
    impl<T> Tap for T {}

    fn press(app: &mut App, code: KeyCode) -> Action {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn select(app: &mut App, id: &str) {
        let k = key(app, id);
        let col = app.card(&k).unwrap().stage.index();
        app.sel = Selection { col, key: Some(k) };
    }

    fn column_ids(app: &App) -> Vec<Vec<String>> {
        app.columns()
            .iter()
            .map(|c| {
                c.iter()
                    .map(|&i| app.board.cards[i].detail.as_ref().unwrap().id.clone())
                    .collect()
            })
            .collect()
    }

    fn rendered(app: &mut App, w: u16, h: u16) -> String {
        app.width = w;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        t.draw(|f| crate::ui::draw(f, app)).unwrap();
        let b = t.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| b[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    const SIZES: [(u16, u16); 3] = [(176, 44), (120, 40), (100, 30)];

    #[test]
    fn pieces_leave_the_columns_for_their_cards_line_across_machines() {
        let app = families(Some("w2K:t1"));
        let cols = column_ids(&app);
        let all: Vec<&String> = cols.iter().flatten().collect();
        for piece in ["0262", "0277", "0331", "0332", "0287", "0348", "0350"] {
            assert!(
                !all.iter().any(|id| *id == piece),
                "{piece} is inside its card"
            );
        }
        // Column counts count a card once.
        assert_eq!(cols[Stage::Test.index()], ["0256"]);
        assert!(cols[Stage::Backlog.index()].is_empty());
        let i = app.card_index(&key(&app, "0256")).unwrap();
        let holds = app.holds(i);
        assert_eq!((holds.count, holds.needs), (4, 1));
        assert_eq!(holds.first_needing.as_deref(), Some("#262"));
        // 0331 on arknet-one is inside 0256 on the laptop.
        let one = app.card_index(&key(&app, "0331")).unwrap();
        assert_eq!(app.holder(one), Some(i));
    }

    #[test]
    fn a_finished_card_with_open_work_goes_first_in_maintain_with_its_count() {
        let mut app = families(Some("w2K:t1"));
        let cols = column_ids(&app);
        assert_eq!(cols[Stage::Maintain.index()], ["0139", "0143"]);
        for (w, h) in SIZES {
            let text = rendered(&mut app, w, h);
            if w >= 120 {
                assert!(text.contains("3 open"), "{w}x{h}:\n{text}");
                assert!(text.contains("#139▸"), "{w}x{h}");
            }
        }
    }

    #[test]
    fn the_cards_line_names_what_needs_you_and_its_border_turns_yellow() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0143");
        for (w, h) in SIZES {
            let text = rendered(&mut app, w, h);
            assert!(text.contains("#262"), "{w}x{h}:\n{text}");
            if w >= 176 {
                assert!(text.contains("4 ▲●◇· #262 needs you"), "{w}x{h}:\n{text}");
            }
        }
        // The yellow border: the card's own attention is waiting, a piece needs you.
        app.width = 176;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(176, 44)).unwrap();
        t.draw(|f| crate::ui::draw(f, &app)).unwrap();
        let b = t.backend().buffer().clone();
        let (x, y) = (0..44u16)
            .flat_map(|y| (0..176u16).map(move |x| (x, y)))
            .find(|&(x, y)| {
                b[(x, y)].symbol() == "#"
                    && b[(x + 1, y)].symbol() == "2"
                    && b[(x + 2, y)].symbol() == "5"
                    && b[(x + 3, y)].symbol() == "6"
            })
            .expect("#256 on the board");
        assert_eq!(
            b[(x - 2, y)].fg,
            crate::theme::YELLOW,
            "its border is yellow"
        );
    }

    #[test]
    fn enter_on_a_card_goes_to_its_own_tab_never_a_childs() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0256");
        let k = key(&app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Jump(k.clone()));
        let card = app.card(&k).unwrap().clone();
        assert_eq!(app.jump_tab(&card), (Some("w2K:t1".into()), true));
        // A child lands on its own tab.
        let child = app.card(&key(&app, "0262")).unwrap().clone();
        assert_eq!(app.jump_tab(&child), (Some("w2K:t2".into()), true));
        // herdr finds the card's own tab gone: it asks to reopen the card, landing nowhere.
        app.tab_gone(&k);
        let Mode::Confirm { request, plain, .. } = &app.mode else {
            panic!("asks: {:?}", app.mode)
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], "0256");
        assert!(plain);
        // The yes sends the reopen as it is, never as an override past the backend's questions.
        let action = press(&mut app, KeyCode::Char('y'));
        let Action::Mutate { request, .. } = action else {
            panic!("{action:?}")
        };
        assert_eq!(request["action"], "reopen");
        assert!(request.get("override").is_none());
    }

    #[test]
    fn with_no_tab_of_its_own_on_record_enter_offers_a_reopen_instead_of_guessing() {
        let mut app = families(None);
        select(&mut app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(title, "Reopen #256's own agent?");
        assert!(message.contains("#262's tab"), "{message}");
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn o_opens_the_cards_screen_and_enter_goes_where_each_row_runs() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0256");
        press(&mut app, KeyCode::Char('o'));
        assert!(matches!(app.mode, Mode::Card { .. }));
        for (w, h) in SIZES {
            let text = rendered(&mut app, w, h);
            for r in [
                "#256",
                "#262",
                "#277",
                "#331",
                "#332",
                "NEEDS YOU",
                "UNDER WAY",
                "NOT STARTED",
            ] {
                assert!(text.contains(r), "{w}x{h} shows {r}:\n{text}");
            }
            assert!(
                text.contains("the card itself") || w < 110,
                "{w}x{h}:\n{text}"
            );
        }
        // The card itself first, then what needs you.
        let rows = |app: &App| {
            let Mode::Card { focus, .. } = &app.mode else {
                panic!()
            };
            focus.clone()
        };
        assert_eq!(rows(&app), "");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Jump(key(&app, "0256"))
        );
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(rows(&app), key(&app, "0262"));
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Action::Jump(key(&app, "0262"))
        );
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(rows(&app), key(&app, "0331"));
        let one = key(&app, "0331");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Jump(one.clone()));
        assert_eq!(app.card(&one).unwrap().machine, "arknet-one");
        // Esc goes back to the board, on the card.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(app.sel.key, Some(key(&app, "0256")));
    }

    #[test]
    fn a_done_cards_screen_lists_its_follow_ups_and_c_and_x_act_on_them() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0139");
        press(&mut app, KeyCode::Char('o'));
        let text = rendered(&mut app, 176, 44);
        for r in ["#287", "#348", "#350", "3 inside"] {
            assert!(text.contains(r), "{r}:\n{text}");
        }
        assert!(text.contains("no agent at it"), "{text}");
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.sel.key, Some(key(&app, "0287")));
        press(&mut app, KeyCode::Char('C'));
        let Mode::Confirm { request, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(request["action"], "close");
        assert_eq!(request["id"], "0287");
    }

    #[test]
    fn n_stops_on_a_piece_that_needs_you_opening_its_cards_screen_on_it() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0143");
        let order = app.needs_you_order();
        assert!(order.contains(&key(&app, "0262")));
        assert!(order.contains(&key(&app, "0287")));
        let next = app.step_needs_you(1).unwrap();
        assert_eq!(next, key(&app, "0262"));
        let Mode::Card { card, focus, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(
            (card.as_str(), focus.as_str()),
            ("ark-sdlc:0256", "ark-sdlc:0262")
        );
        // The next is inside another card: its screen, on it.
        app.step_needs_you(1);
        let Mode::Card { card, focus, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(
            (card.as_str(), focus.as_str()),
            ("ark-sdlc:0139", "ark-sdlc:0287")
        );
        // Ctrl+Space Space goes straight to a piece's own tab: only those with a workspace open.
        assert_eq!(
            app.next_needs_you_after(&key(&app, "0287"), &[]),
            Some(key(&app, "0262"))
        );
    }

    #[test]
    fn a_piece_whose_card_is_out_of_view_shows_on_its_own_marked_in_that_card() {
        let mut app = families(Some("w2K:t1"));
        app.choose_scope(Scope::Machine("arknet-one".into()));
        let cols = column_ids(&app);
        assert_eq!(cols[Stage::Build.index()], ["0331"]);
        let i = app.card_index(&key(&app, "0331")).unwrap();
        assert_eq!(app.held_elsewhere(i).as_deref(), Some("#256"));
        let text = rendered(&mut app, 176, 44);
        assert!(text.contains("in #256"), "{text}");
        // o on it opens #256's screen, on it.
        select(&mut app, "0331");
        press(&mut app, KeyCode::Char('o'));
        let Mode::Card { card, focus, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(
            (card.as_str(), focus.as_str()),
            ("ark-sdlc:0256", "ark-sdlc:0331")
        );
    }

    #[test]
    fn cancelling_a_parent_names_its_childs_tab_and_never_a_follow_up_elsewhere() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0256");
        press(&mut app, KeyCode::Char('X'));
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert!(message.contains("#262"), "{message}");
        assert!(!message.contains("#331"), "{message}");
    }

    #[test]
    fn many_pieces_fold_quiet_groups_and_enter_opens_one() {
        let mut app = families(Some("w2K:t1"));
        let mut extra: Vec<FeedCard> = (400..440)
            .map(|n| {
                card(&n.to_string(), "Bug from the swarm", "maintain", |c| {
                    c.parent = Some("0143".into());
                    c.attention = Some("done".into());
                })
            })
            .collect();
        app.machines[0]
            .feed
            .as_mut()
            .unwrap()
            .cards
            .append(&mut extra);
        app.rebuild();
        select(&mut app, "0143");
        press(&mut app, KeyCode::Char('o'));
        let text = rendered(&mut app, 176, 44);
        assert!(text.contains("DONE  40 ▸"), "{text}");
        press(&mut app, KeyCode::Char('j'));
        let Mode::Card { focus, .. } = &app.mode else {
            panic!()
        };
        assert_eq!(focus, "group:done");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        for (w, h) in SIZES {
            let text = rendered(&mut app, w, h);
            assert!(text.contains("#400"), "{w}x{h}:\n{text}");
            assert!(
                text.contains("below"),
                "past a screenful it scrolls, {w}x{h}:\n{text}"
            );
        }
    }

    #[test]
    fn a_loop_of_links_leaves_every_card_on_the_board() {
        let mut app = families(Some("w2K:t1"));
        let f = app.machines[0].feed.as_mut().unwrap();
        f.cards.push(card("0500", "a", "build", |c| {
            c.parent = Some("0501".into())
        }));
        f.cards.push(card("0501", "b", "build", |c| {
            c.parent = Some("0500".into())
        }));
        app.rebuild();
        let cols = column_ids(&app);
        let on_board = cols
            .iter()
            .flatten()
            .filter(|id| *id == "0500" || *id == "0501")
            .count();
        assert_eq!(on_board, 1, "one heads the family, holding the other");
    }

    #[test]
    fn a_recorded_own_tab_is_strict_even_with_no_other_cards_tabs_beside_it() {
        let mut app = families(Some("w2K:t1"));
        let f = app.machines[0].feed.as_mut().unwrap();
        f.cards.retain(|c| c.id != "0262");
        app.rebuild();
        let card = app.card(&key(&app, "0256")).unwrap().clone();
        assert!(app.tabs_inside(&card).is_empty());
        assert_eq!(app.jump_tab(&card), (Some("w2K:t1".into()), true));
        // Gone, it offers a reopen, never another tab.
        app.tab_gone(&card.key);
        assert!(
            matches!(app.mode, Mode::Confirm { plain: true, .. }),
            "{:?}",
            app.mode
        );
    }

    #[test]
    fn n_from_a_nested_card_screen_to_a_card_on_the_board_leaves_every_screen() {
        let mut app = families(Some("w2K:t1"));
        let f = app.machines[0].feed.as_mut().unwrap();
        f.cards.push(card("0400", "Piece of a piece", "build", |c| {
            c.parent = Some("0262".into())
        }));
        f.cards
            .push(card("0500", "Unrelated, asking", "build", |c| {
                c.attention = Some("needs_you".into())
            }));
        app.rebuild();
        select(&mut app, "0256");
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.sel.key, Some(key(&app, "0262")));
        // o on a row holding work opens its own screen, over its card's.
        press(&mut app, KeyCode::Char('o'));
        let Mode::Card { card, back, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(card, "ark-sdlc:0262");
        assert!(matches!(**back, Mode::Card { .. }));
        // N goes back to 0500, a card on the board: out of both screens, on it.
        assert_eq!(app.step_needs_you(-1), Some(key(&app, "0500")));
        assert!(matches!(app.mode, Mode::Board), "{:?}", app.mode);
        assert_eq!(app.sel.key, Some(key(&app, "0500")));
        // Enter acts on 0500 itself (no workspace: its reopen), not on the screen left behind.
        let Action::Mutate { request, .. } = press(&mut app, KeyCode::Enter) else {
            panic!()
        };
        assert_eq!(request["id"], "0500");
    }

    #[test]
    fn a_selected_piece_that_leaves_its_card_leaves_nothing_selected() {
        for change in ["gone", "moved"] {
            let mut app = families(Some("w2K:t1"));
            select(&mut app, "0256");
            press(&mut app, KeyCode::Char('o'));
            press(&mut app, KeyCode::Char('j'));
            assert_eq!(app.sel.key, Some(key(&app, "0262")));
            let f = app.machines[0].feed.as_mut().unwrap();
            if change == "gone" {
                f.cards.retain(|c| c.id != "0262");
            } else if let Some(c) = f.cards.iter_mut().find(|c| c.id == "0262") {
                c.parent = Some("0139".into());
            }
            app.rebuild();
            let Mode::Card { card, focus, .. } = &app.mode else {
                panic!("{:?}", app.mode)
            };
            assert_eq!(
                (card.as_str(), focus.as_str()),
                ("ark-sdlc:0256", "ark-sdlc:0262")
            );
            assert_eq!(app.sel.key, None, "{change}: not its parent");
            // X and Enter act on nothing, rather than on the parent's whole workspace.
            assert_eq!(press(&mut app, KeyCode::Char('X')), Action::None);
            assert!(
                matches!(app.mode, Mode::Card { .. }),
                "{change}: {:?}",
                app.mode
            );
            assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
            let text = rendered(&mut app, 176, 44);
            assert!(text.contains("has left this card"), "{text}");
            // Moving chooses a row again.
            press(&mut app, KeyCode::Char('k'));
            assert_eq!(app.sel.key, Some(key(&app, "0256")));
        }
    }
}
