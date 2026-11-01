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
        let mut best = family::group(&self.board.cards[i]);
        let mut stack = self.board.down.get(i).cloned().unwrap_or_default();
        let mut seen = 0;
        while let Some(k) = stack.pop() {
            seen += 1;
            if seen > self.board.cards.len() {
                break;
            }
            best = best.min(family::group(&self.board.cards[k]));
            stack.extend(self.board.down.get(k).cloned().unwrap_or_default());
        }
        best
    }

    /// Every card inside card i, nested ones too, each followed by its own.
    pub fn pieces(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.walk(i, 0, &mut out);
        out.into_iter().map(|(k, _)| k).collect()
    }

    fn walk(&self, i: usize, depth: usize, out: &mut Vec<(usize, usize)>) {
        for k in self.children(i) {
            if out.len() > self.board.cards.len() || out.iter().any(|(o, _)| *o == k) {
                return;
            }
            out.push((k, depth));
            self.walk(k, depth + 1, out);
        }
    }

    /// What card i holds, for its line on the board.
    pub fn holds(&self, i: usize) -> Holds {
        Holds::of(self.pieces(i).iter().map(|&k| &self.board.cards[k]))
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
                _ => focus = String::new(),
            }
            if !focusable(self, &opened).contains(&focus) {
                focus = String::new();
            }
        }
        let selected = if focus.is_empty() {
            Some(card.clone())
        } else if focus.starts_with("group:") {
            None
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
                let key = if focus.is_empty() { card } else { focus };
                return self.jump_action(key);
            }
            KeyCode::Char('m' | ' ') => {
                if !focus.starts_with("group:") {
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
        let leave = |mode: &Mode| match mode {
            Mode::Card { back, .. } => (**back).clone(),
            other => other.clone(),
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
            (Some(tab.clone()), !self.tabs_inside(card).is_empty())
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
            [one] => format!("{one}'s tab"),
            many => format!("tabs for {}", many.join(", ")),
        };
        let reference = card.reference.clone();
        let said = format!(
            "{reference}'s own agent {why}, and its workspace holds {tabs}: ⏎ won't open another's tab."
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
