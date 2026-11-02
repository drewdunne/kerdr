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

    /// Card i's direct pieces in its screen's order (sorted once per board, `family::summaries`).
    fn children(&self, i: usize) -> Vec<usize> {
        self.board.down.get(i).cloned().unwrap_or_default()
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
            // Your order, but what waits on you before it (a question asked before the work
            // starts): far down the list, nobody sees it (intent 0420).
            Stage::Backlog => col.sort_by_key(|&i| !self.backlog_needs_you(i)),
            Stage::Misc => {}
            // What's waiting on you first, then the rest as the board sorted it, cards as last
            // seen from a machine out of reach at the end (intent 0401).
            _ => col.sort_by_key(|&i| {
                let c = &self.board.cards[i];
                let on_you = crate::family::on_you(c, &self.holds(i));
                (on_you.is_none(), on_you, c.cached_at.is_some())
            }),
        }
    }

    /// A backlog card waits on you: itself (questions before it starts), or a piece it holds.
    pub(super) fn backlog_needs_you(&self, i: usize) -> bool {
        let c = &self.board.cards[i];
        !c.dismissed() && (c.needs_you() || self.holds(i).needs > 0)
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
            focus,
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
            if let Some(g) = group
                && !opened.contains(&g)
            {
                opened.push(g);
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
                if !foci.contains(&focus) {
                    self.say(
                        "That row has left this card; j and k choose another.",
                        Tone::Info,
                    );
                    return Action::None;
                }
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
            // A card on the board, from a card's screen: the board itself, on it, wherever the
            // screens were opened from.
            None if matches!(self.mode, Mode::Card { .. }) => {
                self.mode = Mode::Board;
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
        } else if let Some(tab) = self.live_own_tab(card) {
            (Some(tab), true)
        } else {
            (None, false)
        }
    }

    /// With no own tab on record (herdr doesn't know its agent's conversation), the one tab in the
    /// card's workspace holding an agent that is neither a helper's nor a card's inside it, and
    /// only when it is also the workspace's first tab with any agent in it, since the agent you
    /// talk to starts first and children and helpers open after it. None otherwise, where Enter
    /// asks instead of guessing (intent 0411).
    fn live_own_tab(&self, card: &Card) -> Option<String> {
        let ws = card.workspace_id.as_deref()?;
        let snap = self
            .machines
            .get(card.machine_index)?
            .snapshot(card.session.as_deref()?)?;
        let inside: Vec<String> = self
            .tabs_inside(card)
            .iter()
            .filter_map(|c| c.tab_id.clone())
            .collect();
        let mut tabs: Vec<&str> = snap
            .agents
            .iter()
            .filter(|a| a.workspace_id == ws && !a.helper)
            .filter_map(|a| a.tab_id.as_deref())
            .filter(|t| !inside.iter().any(|i| i == t))
            .collect();
        tabs.sort_unstable();
        tabs.dedup();
        let [one] = tabs.as_slice() else {
            return None;
        };
        let number = |t: &str| t.rsplit_once(":t").and_then(|(_, n)| n.parse::<u32>().ok());
        let first = snap
            .agents
            .iter()
            .filter(|a| a.workspace_id == ws)
            .filter_map(|a| a.tab_id.as_deref().and_then(number))
            .min()?;
        (number(one)? == first).then(|| (*one).to_string())
    }

    /// Enter on a card whose workspace holds other work's tabs while its records name no tab of
    /// its own, and herdr shows no single tab that can only be its own: it doesn't guess, and
    /// offers to reopen the card's own conversation instead.
    pub(super) fn own_tab_unknown(&mut self, card: &Card) -> Option<Action> {
        if card.tab_id.is_some()
            || card.own_tab.is_some()
            || self.tabs_inside(card).is_empty()
            || self.live_own_tab(card).is_some()
        {
            return None;
        }
        Some(self.offer_own_reopen(card, false))
    }

    /// herdr found the tab Enter was for has closed. A piece's reopens, as a card whose workspace
    /// closed does; a card's own offers to reopen its conversation, never landing on another tab.
    pub fn tab_gone(&mut self, key: &str) {
        let Some(card) = self.card(key).cloned() else {
            return;
        };
        if card.tab_id.is_some() {
            if self.actions_on() && (card.under_way() || card.done()) {
                self.owe_reopen(key, "tab");
            } else {
                self.say(format!("{}'s tab has closed.", card.reference), Tone::Warn);
            }
        } else {
            self.offer_own_reopen(&card, true);
        }
    }

    /// Offers to reopen a card's own agent: its tab has `closed`, or Kerdr can't tell which tab
    /// is its own (herdr doesn't know that agent's conversation).
    fn offer_own_reopen(&mut self, card: &Card, closed: bool) -> Action {
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
        let said = if closed {
            format!(
                "{reference}'s own agent has closed its tab; ⏎ won't open another tab in its workspace, which may hold {tabs}."
            )
        } else {
            format!(
                "Kerdr can't tell which tab is {reference}'s own agent's (herdr doesn't know that agent's conversation), and it won't open {tabs} instead."
            )
        };
        if !self.actions_on() || card.detail.is_none() {
            self.say(format!("{said} o shows what's inside."), Tone::Info);
            return Action::None;
        }
        // Another action runs: this ⏎ waits its turn, and asks then (intent 0411).
        if self.busy {
            return self.enter_while_busy(card);
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
    /// families with no own tab on record and two tabs herdr can't tell apart: the parent's and
    /// another non-helper agent's, so Enter can't know which is the card's own.
    fn two_unknown_tabs() -> App {
        let mut app = families(None);
        let data = app.machines[0].data.as_mut().unwrap();
        let snap = data
            .snapshots
            .values_mut()
            .next()
            .unwrap()
            .as_mut()
            .unwrap();
        snap.agents.push(agent("w2K:p3", "w2K:t3", "idle"));
        app.rebuild();
        app
    }

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

    fn live() -> Conn {
        Conn::Live {
            latency: Default::default(),
        }
    }

    /// A read of `m`'s session `session` as `snap`, made at `at`, with no feed this time.
    fn read(
        m: &MachineState,
        session: &str,
        snap: Snapshot,
        at: chrono::DateTime<chrono::Local>,
        i: usize,
    ) -> crate::poll::Event {
        let mut data = m.data.clone().unwrap();
        data.snapshots = [(session.to_string(), Ok(snap))].into_iter().collect();
        crate::poll::Event {
            machine: i,
            conn: live(),
            data: Some(data),
            at,
        }
    }

    /// Heimdall (ark-sdlc 0328, intent 0420): its agent's turn ended on questions for you, its
    /// pinboard said Waiting on Andrew, and a workflow in its pane kept herdr saying working.
    #[test]
    fn a_card_waiting_on_you_stays_yours_while_a_workflow_keeps_its_agent_working() {
        let label = "28-screen-watch-flywheel";
        let snap = |heimdall: &str| Snapshot {
            workspaces: vec![
                Workspace {
                    workspace_id: "w2P".into(),
                    label: label.into(),
                    ..Default::default()
                },
                Workspace {
                    workspace_id: "w3A".into(),
                    label: "29-busy".into(),
                    ..Default::default()
                },
            ],
            agents: vec![
                agent("w2P:p1", "w2P:t1", heimdall),
                agent("w3A:p1", "w3A:t1", "working"),
            ],
            ..Default::default()
        };
        let t0 = chrono::Local::now();
        let mut laptop = machine(
            "AMD-LZM4",
            false,
            "KBZ",
            snap("working"),
            vec![
                card("0329", "Busy", "build", |c| {
                    c.attention = Some("running".into());
                    c.workspace = in_ws("w3A", "29-busy", None, Some("w3A:t1"));
                }),
                card(
                    "0328",
                    "Screen-watching improvement flywheel",
                    "build",
                    |c| {
                        c.attention = Some("needs_you".into());
                        c.status = Some("Waiting on Andrew".into());
                        c.workspace = in_ws("w2P", label, None, Some("w2P:t1"));
                    },
                ),
            ],
        );
        laptop.feed.as_mut().unwrap().generated_at =
            Some((t0 - chrono::Duration::seconds(30)).to_rfc3339());
        let mut app = App::new(Config::default(), vec![laptop]);
        // A later read: still working, as it was before the feed said it waits on you.
        let ev = read(&app.machines[0], "KBZ", snap("working"), t0, 0);
        app.apply(ev);
        let heimdall = key(&app, "0328");
        assert_eq!(app.card(&heimdall).unwrap().attention, Attention::NeedsYou);
        // A yellow box at the top of its column, and the next to jump to.
        assert_eq!(column_ids(&app)[Stage::Build.index()][0], "0328");
        assert_eq!(
            app.next_needs_you_after(&key(&app, "0329"), &[]),
            Some(heimdall.clone())
        );
        let s = crate::board::summarize(&app.board, &Scope::All, &app.machines);
        assert_eq!(s.needs_you, 1);

        // You answer: its turn ends, then it goes to work on your answer. That shows at once.
        let ev = read(
            &app.machines[0],
            "KBZ",
            snap("idle"),
            t0 + chrono::Duration::seconds(2),
            0,
        );
        app.apply(ev);
        assert_eq!(app.card(&heimdall).unwrap().attention, Attention::NeedsYou);
        let ev = read(
            &app.machines[0],
            "KBZ",
            snap("working"),
            t0 + chrono::Duration::seconds(4),
            0,
        );
        app.apply(ev);
        assert_eq!(app.card(&heimdall).unwrap().attention, Attention::Running);
    }

    /// Launching a Tinyhost world (ark-sdlc 0408, intent 0420): queued with two questions for you
    /// before it starts, homed on another machine, 76th of 80 in the backlog.
    #[test]
    fn a_queued_card_with_questions_for_you_tops_the_backlog_and_counts() {
        let empty = Snapshot::default();
        let mut cards: Vec<FeedCard> = (1..=12)
            .map(|n| {
                card(&format!("{:04}", 100 + n), "Queued", "backlog", |c| {
                    c.backlog_rank = Some(n);
                })
            })
            .collect();
        let world = card("0408", "Launch a Tinyhost world", "backlog", |c| {
            c.backlog_rank = Some(408);
            c.attention = Some("needs_you".into());
            c.needs = Some("2 questions before it starts".into());
            c.home = Some(Home {
                machine: Some("LAB-Server".into()),
                session: Some("TNY".into()),
            });
        });
        cards.push(world.clone());
        // A queued card holding a piece already at work that asks you something.
        cards.push(card("0450", "Queued, holding a question", "backlog", |c| {
            c.backlog_rank = Some(450);
        }));
        cards.push(card("0451", "Its piece, asking", "build", |c| {
            c.parent = Some("0450".into());
            c.attention = Some("needs_you".into());
        }));
        let laptop = machine("AMD-LZM4", false, "KBZ", empty.clone(), cards);
        let lab = machine("LAB-Server", true, "default", empty, vec![world]);
        let mut app = App::new(Config::default(), vec![laptop, lab]);
        app.rebuild();
        let k = key(&app, "0408");
        let c = app.card(&k).unwrap();
        assert_eq!(c.attention, Attention::NeedsYou);
        assert_eq!(c.why, "2 questions before it starts");
        assert_eq!(c.machine, "LAB-Server");
        // First in the backlog column; the backlog's own list keeps your order.
        let backlog = &column_ids(&app)[Stage::Backlog.index()];
        assert_eq!(backlog[..3], ["0408", "0450", "0101"]);
        let order: Vec<String> = app
            .backlog_order()
            .iter()
            .map(|&i| app.board.cards[i].detail.as_ref().unwrap().id.clone())
            .collect();
        assert_eq!(order.last().map(String::as_str), Some("0450"));
        // Counted with what waits on you (the piece counts itself), and n reaches it.
        let s = crate::board::summarize(&app.board, &Scope::All, &app.machines);
        assert_eq!(s.needs_you, 2);
        assert_eq!(s.verdict, Some(crate::model::Verdict::FinishFirst(2)));
        assert_eq!(
            app.needs_you_order(),
            [k.clone(), key(&app, "0451")],
            "n goes to the queued card, then the piece inside the other"
        );
        // A narrow screen keeps the Backlog column while something in it waits on you, and the
        // row keeps its number, the note giving way first.
        for w in [80, 60] {
            app.width = w;
            assert!(app.visible_columns()[Stage::Backlog.index()]);
            let screen = crate::ui::tests_render(&app, w, 30);
            assert!(screen.contains("● #408"), "at {w} columns:\n{screen}");
        }
        for m in &mut app.machines {
            for c in &mut m.feed.as_mut().unwrap().cards {
                if c.id == "0408" {
                    c.attention = None;
                }
            }
        }
        app.rebuild();
        // Only the piece held inside a queued card asks now: the column still stays.
        assert!(app.visible_columns()[Stage::Backlog.index()]);
        for m in &mut app.machines {
            for c in &mut m.feed.as_mut().unwrap().cards {
                c.attention = None;
            }
        }
        app.rebuild();
        assert!(!app.visible_columns()[Stage::Backlog.index()]);
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
        let mut app = two_unknown_tabs();
        select(&mut app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(title, "Reopen #256's own agent?");
        assert!(message.contains("#262's tab"), "{message}");
        assert!(message.contains("can't tell which tab"), "{message}");
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn with_no_tab_of_its_own_on_record_enter_while_busy_asks_in_its_turn() {
        let mut app = two_unknown_tabs();
        app.busy = true;
        app.busy_id = Some("0999".into());
        select(&mut app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert!(matches!(app.mode, Mode::Board), "{:?}", app.mode);
        let key = app.sel.key.clone().unwrap();
        assert_eq!(app.pending_enter.as_deref(), Some(key.as_str()));
        let said = app.message().unwrap().0.to_string();
        assert!(said.contains("#256 opens once"), "{said}");
        assert!(!said.contains("still running"), "{said}");
        // Its turn: the question it would have asked.
        app.busy = false;
        app.busy_id = None;
        assert_eq!(app.next_owed(), Action::None);
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(title, "Reopen #256's own agent?");
    }

    #[test]
    fn with_no_own_tab_on_record_enter_lands_on_the_one_tab_only_it_can_be() {
        // herdr's integration doesn't know the parent's conversation, but its workspace holds one
        // agent's tab besides its child's: Enter goes there, never to the child's tab.
        let mut app = families(None);
        select(&mut app, "0256");
        let k = key(&app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Jump(k.clone()));
        let card = app.card(&k).cloned().unwrap();
        assert_eq!(app.jump_tab(&card), (Some("w2K:t1".to_string()), true));
        // A helper's tab doesn't count as a second candidate.
        let mut app = families(None);
        let data = app.machines[0].data.as_mut().unwrap();
        let snap = data
            .snapshots
            .values_mut()
            .next()
            .unwrap()
            .as_mut()
            .unwrap();
        let mut reviewer = agent("w2K:p3", "w2K:t3", "working");
        reviewer.helper = true;
        snap.agents.push(reviewer);
        app.rebuild();
        select(&mut app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Jump(k));
        // The parent's own agent has ended and a child the feed doesn't show yet runs in a later
        // tab: that tab isn't the first with an agent, so Enter asks rather than going there.
        let mut app = families(None);
        let data = app.machines[0].data.as_mut().unwrap();
        let snap = data
            .snapshots
            .values_mut()
            .next()
            .unwrap()
            .as_mut()
            .unwrap();
        snap.agents
            .retain(|a| a.tab_id.as_deref() != Some("w2K:t1"));
        snap.agents.push(agent("w2K:p3", "w2K:t3", "working"));
        app.rebuild();
        select(&mut app, "0256");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(title, "Reopen #256's own agent?");
    }

    #[test]
    fn search_enter_on_a_parent_with_no_own_tab_asks_as_the_board_does() {
        let mut app = two_unknown_tabs();
        let i = app
            .board
            .cards
            .iter()
            .position(|c| c.detail.as_ref().is_some_and(|d| d.id == "0256"))
            .unwrap();
        let search = Search {
            query: "#256".into(),
            filter: SearchFilter::Everything,
            sel: 0,
        };
        assert_eq!(app.open_hit(Hit::Card(i), search), Action::None);
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(title, "Reopen #256's own agent?");
        // With its own tab on record, search goes in as the board does.
        let mut app = families(Some("w2K:t1"));
        let search = Search {
            query: "#256".into(),
            filter: SearchFilter::Everything,
            sel: 0,
        };
        let k = key(&app, "0256");
        assert_eq!(app.open_hit(Hit::Card(i), search), Action::Jump(k));
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

    /// Intent 0404: ⏎ on a row nobody started starts that row's work, as d does, never the
    /// card the screen is for.
    #[test]
    fn enter_on_a_not_started_row_starts_that_work() {
        let mut app = families(Some("w2K:t1"));
        select(&mut app, "0256");
        press(&mut app, KeyCode::Char('o'));
        let want = key(&app, "0332");
        for _ in 0..12 {
            let Mode::Card { focus, .. } = &app.mode else {
                panic!("{:?}", app.mode)
            };
            if *focus == want {
                break;
            }
            let code = if focus.starts_with("group:") {
                KeyCode::Enter
            } else {
                KeyCode::Char('j')
            };
            press(&mut app, code);
        }
        assert_eq!(app.sel.key.as_deref(), Some(want.as_str()));
        let Action::Mutate { machine, request } = press(&mut app, KeyCode::Enter) else {
            panic!("{:?}", app.message())
        };
        assert_eq!(app.machines[machine].name(), "AMD-LZM4");
        assert_eq!(
            request,
            serde_json::json!({"action":"dispatch","id":"0332"})
        );
        // Past the limit it asks, and a feed rebuild while the question is open keeps the row
        // selected, so the yes and any newer question are still for it.
        let over = crate::actions::Response {
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("KBZ".into()),
            message: "3 of 3 slots used".into(),
            ..Default::default()
        };
        app.finish_action(machine, request, Ok(over.clone()));
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
        app.rebuild();
        assert_eq!(app.sel.key.as_deref(), Some(want.as_str()));
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
        let Action::Mutate { machine, request } = press(&mut app, KeyCode::Enter) else {
            panic!("{:?}", app.mode)
        };
        assert_eq!(request["id"], "0332");
        assert_eq!(request["override"], true);
        assert!(matches!(app.mode, Mode::Card { .. }));
        assert_eq!(app.sel.key.as_deref(), Some(want.as_str()));
        // A newer question after the yes still asks.
        let newer = crate::actions::Response {
            confirm: Some("0123456789ab0001".into()),
            ..over
        };
        app.finish_action(machine, request, Ok(newer));
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
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
    fn a_childs_closed_tab_reopens_once_the_running_action_answers() {
        let mut app = families(Some("w2K:t1"));
        let k = key(&app, "0262");
        let i = app.board.cards.iter().position(|c| c.key == k).unwrap();
        app.board.cards[i].attention = crate::model::Attention::Idle;
        assert!(app.card(&k).unwrap().tab_id.is_some());
        assert!(app.card(&k).unwrap().under_way());
        select(&mut app, "0262");
        app.busy = true;
        app.busy_since = Some(std::time::Instant::now());
        app.tab_gone(&k);
        assert_eq!(app.pending_reopen.as_deref(), Some(k.as_str()));
        assert_eq!(app.next_owed(), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("tab has closed; it reopens as soon as"),
            "{:?}",
            app.message()
        );
        app.busy = false;
        let Action::Mutate { request, .. } = app.next_owed() else {
            panic!("the owed reopen sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], "0262");
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

    #[test]
    fn n_to_a_card_on_the_board_lands_on_the_board_from_the_backlog_and_a_peek_too() {
        for start in ["backlog", "peek"] {
            let mut app = families(Some("w2K:t1"));
            let f = app.machines[0].feed.as_mut().unwrap();
            f.cards.push(card("0400", "Piece of a piece", "build", |c| {
                c.parent = Some("0262".into())
            }));
            f.cards
                .push(card("0500", "Unrelated, asking", "build", |c| {
                    c.attention = Some("needs_you".into())
                }));
            f.cards
                .push(card("0600", "Waiting, holding work", "backlog", |_| {}));
            f.cards.push(card("0601", "Its piece", "backlog", |c| {
                c.from = Some("0600".into())
            }));
            app.rebuild();
            let base = if start == "backlog" {
                app.enter_backlog(false);
                Mode::Backlog { sel: 0 }
            } else {
                Mode::Board
            };
            app.mode = base;
            app.open_inside(&key(&app, "0256"));
            press(&mut app, KeyCode::Char('j'));
            press(&mut app, KeyCode::Char('o'));
            if start == "peek" {
                press(&mut app, KeyCode::Char(' '));
                assert!(matches!(app.mode, Mode::Peek { .. }));
                press(&mut app, KeyCode::Char('N'));
                let Mode::Peek { back, .. } = &app.mode else {
                    panic!("{:?}", app.mode)
                };
                assert!(matches!(**back, Mode::Board), "{start}: {back:?}");
            } else {
                assert_eq!(app.step_needs_you(-1), Some(key(&app, "0500")));
                assert!(matches!(app.mode, Mode::Board), "{start}: {:?}", app.mode);
            }
            assert_eq!(app.sel.key, Some(key(&app, "0500")), "{start}");
        }
    }

    #[test]
    fn a_selected_folded_group_that_vanishes_or_unfolds_leaves_nothing_selected() {
        let mut app = families(Some("w2K:t1"));
        let f = app.machines[0].feed.as_mut().unwrap();
        for n in 700..704 {
            f.cards
                .push(card(&n.to_string(), "Done piece", "maintain", |c| {
                    c.parent = Some("0143".into());
                    c.attention = Some("done".into());
                }));
        }
        app.rebuild();
        select(&mut app, "0143");
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('j'));
        let focus = |app: &App| match &app.mode {
            Mode::Card { focus, .. } => focus.clone(),
            m => panic!("{m:?}"),
        };
        assert_eq!(focus(&app), "group:done");
        // One piece goes: three left, so the group unfolds and its heading can't be selected.
        app.machines[0]
            .feed
            .as_mut()
            .unwrap()
            .cards
            .retain(|c| c.id != "700");
        app.rebuild();
        assert_eq!(focus(&app), "group:done");
        assert_eq!(app.sel.key, None);
        assert_eq!(press(&mut app, KeyCode::Char('X')), Action::None);
        assert!(matches!(app.mode, Mode::Card { .. }), "{:?}", app.mode);
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(focus(&app), "group:done", "Enter opened nothing");
        let text = rendered(&mut app, 176, 44);
        assert!(text.contains("has left this card"), "{text}");
        assert!(text.contains("nothing selected"), "{text}");
        assert!(!text.contains("opens the list"), "{text}");
        // All of them go.
        app.machines[0]
            .feed
            .as_mut()
            .unwrap()
            .cards
            .retain(|c| !c.id.starts_with('7'));
        app.rebuild();
        assert_eq!(app.sel.key, None);
        let text = rendered(&mut app, 176, 44);
        assert!(text.contains("has left this card"), "{text}");
        assert!(!text.contains("opens the list"), "{text}");
    }

    #[test]
    fn a_long_chain_is_summed_once_per_board() {
        let mut app = families(Some("w2K:t1"));
        let f = app.machines[0].feed.as_mut().unwrap();
        for n in 1000..4000 {
            f.cards.push(card(&n.to_string(), "Link", "build", |c| {
                if n > 1000 {
                    c.parent = Some((n - 1).to_string());
                }
            }));
        }
        let at = std::time::Instant::now();
        app.rebuild();
        let top = app.card_index(&key(&app, "1000")).unwrap();
        assert_eq!(app.holds(top).count, 2999);
        let _ = app.columns();
        assert!(
            at.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            at.elapsed()
        );
    }
}
