//! What part of the board is in view, and its columns (intent 0595). When a feed lists owners,
//! the top bar splits the board by whose work it is, then by team: `{ }` step through owners and
//! `[ ]` through the chosen owner's tabs (All, each team, No team), at every level. With no owners
//! listed, or `[board] split = "machines"`, it splits by machine and herdr session as it always
//! did. The columns between Backlog and Done come from the stage lists of the work in view.

use super::*;
use crate::config::Split;
use crate::owners::{self, OwnerInfo, UNOWNED};
use crate::stages::{self, Fold, Layout, Want};

/// Rows of the screen the board's header, strip and footer take, for how much room bands have.
const CHROME_ROWS: u16 = 10;

impl App {
    /// Whether the top bar splits by owner and team rather than machine and session.
    pub fn owner_split(&self) -> bool {
        !self.board.owners.is_empty() && self.cfg.board.split != Split::Machines
    }

    /// The owners the bar lists, in its order: those with work, grouped by organization, and the
    /// one chosen even when its work has gone; `Unowned` last while any work has no owner.
    pub fn listed_owners(&self) -> Vec<String> {
        let has_work = |id: &str| {
            self.board
                .cards
                .iter()
                .any(|c| c.owner == id && !c.dismissed())
        };
        let mut out: Vec<String> = owners::ordered(&self.board.owners)
            .into_iter()
            .filter(|o| has_work(&o.id) || self.owner_pick.as_deref() == Some(o.id.as_str()))
            .map(|o| o.id.clone())
            .collect();
        if has_work(UNOWNED) || self.owner_pick.as_deref() == Some(UNOWNED) {
            out.push(UNOWNED.into());
        }
        out
    }

    pub fn owner_info(&self, id: &str) -> Option<&OwnerInfo> {
        owners::find(&self.board.owners, id)
    }

    /// How the bar names an owner in a word: its code, else its name; `Unowned` for no owner.
    pub fn owner_short(&self, id: &str) -> String {
        match self.owner_info(id) {
            Some(o) => o.short().to_string(),
            None if id == UNOWNED => "Unowned".into(),
            None => id.to_string(),
        }
    }

    /// A team's name, from its owner's list.
    pub fn team_name(&self, owner: &str, team: Option<&str>) -> String {
        match team {
            None => "No team".into(),
            Some(t) => self
                .owner_info(owner)
                .and_then(|o| o.team(t))
                .map(|t| t.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| t.to_string()),
        }
    }

    /// The tab row on the owner split: the owners, on all of Kerdr; on an owner, its All tab, each
    /// of its teams (a retired one only while it holds a card) and, for an owner with teams, No
    /// team while any of its work has none. The first entry is the level itself.
    pub(super) fn owner_tabs(&self) -> Vec<Scope> {
        let Some(o) = self.owner_pick.clone() else {
            let mut tabs = vec![Scope::All];
            tabs.extend(self.listed_owners().into_iter().map(Scope::Owner));
            return tabs;
        };
        let mut tabs = vec![Scope::Owner(o.clone())];
        let mine: Vec<&Card> = self
            .board
            .cards
            .iter()
            .filter(|c| c.owner == o && !c.dismissed())
            .collect();
        if let Some(info) = self.owner_info(&o) {
            for t in &info.teams {
                if !t.retired
                    || mine
                        .iter()
                        .any(|c| c.team.as_deref() == Some(t.id.as_str()))
                {
                    tabs.push(Scope::Team(o.clone(), Some(t.id.clone())));
                }
            }
        }
        // An owner with no teams has only All: its No team would be the same.
        if mine.iter().any(|c| c.team.is_none()) && tabs.len() > 1 {
            tabs.push(Scope::Team(o, None));
        }
        tabs
    }

    /// `{ }` on the owner split: the previous or next owner, from wherever you are; from all of
    /// Kerdr the first or the last, and past either end all of Kerdr again.
    pub(super) fn step_owner(&mut self, dir: i32) {
        let choices: Vec<Option<String>> = std::iter::once(None)
            .chain(self.listed_owners().into_iter().map(Some))
            .collect();
        let pos = choices
            .iter()
            .position(|o| o == &self.owner_pick)
            .unwrap_or(0);
        let next = (pos as i32 + dir).rem_euclid(choices.len() as i32) as usize;
        self.pick_owner(choices[next].clone());
    }

    /// Goes to an owner (None: all of Kerdr), on its All tab.
    pub(super) fn pick_owner(&mut self, owner: Option<String>) {
        if owner != self.owner_pick {
            self.open_band = None;
            // A project is one owner's: it doesn't carry over to another.
            if owner.is_some() {
                self.project = None;
            }
        }
        self.owner_pick = owner;
        self.rebuild();
        self.set_tab(0);
    }

    /// Tab and Shift+Tab, and `[ ]`: the next or previous tab in the row. On all of Kerdr the row
    /// lists owners, so Tab goes down to one (`[ ]` have nothing to step through there).
    pub(super) fn step_row(&mut self, dir: i32, brackets: bool) {
        if self.owner_split() && self.owner_pick.is_none() {
            if !brackets {
                self.step_owner(dir);
            }
            return;
        }
        self.step_tab(dir);
    }

    /// `1` to `9`: tab n in the row. On all of Kerdr that's owner n, which it goes down to; on an
    /// owner tab 1 is All.
    pub(super) fn row_tab(&mut self, n: usize) {
        if !self.owner_split() {
            return self.session_tab(n);
        }
        if self.owner_pick.is_none() {
            match self.listed_owners().get(n.saturating_sub(1)).cloned() {
                Some(o) => self.pick_owner(Some(o)),
                None => self.say(format!("There's no owner {n}."), Tone::Info),
            }
            return;
        }
        if n >= 1 && n <= self.tabs.len() {
            self.set_tab(n - 1);
        } else {
            self.say(format!("There's no tab {n}."), Tone::Info);
        }
    }

    /// The tabs for the view: the owner split's, or a machine's sessions.
    pub(super) fn view_tabs(&mut self) -> Vec<Scope> {
        if self.owner_split() {
            self.machine = None;
            if let Some(o) = &self.owner_pick
                && o != UNOWNED
                && self.owner_info(o).is_none()
            {
                self.owner_pick = None;
            }
            return self.owner_tabs();
        }
        self.owner_pick = None;
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
        tabs
    }

    /// The machine the chosen owner's new work runs on, as its feed says, when Kerdr has it.
    pub fn owner_machine(&self) -> Option<usize> {
        let o = self.scope().owner()?;
        let name = self.owner_info(o)?.agent_machine.clone()?;
        self.machine_index(&name)
    }

    /// The lists the teams in view use, each with the title its band would have and who's on it.
    fn wants(&self) -> Vec<Want> {
        if !self.owner_split() {
            return vec![];
        }
        let lists = &self.board.lists;
        let scope = self.scope().clone();
        // (owner, team) in view: every team of the owners in view, and No team where work has none.
        let mut teams: Vec<(String, Option<String>)> = Vec::new();
        let owners_in_view: Vec<String> = match &scope {
            Scope::Owner(o) | Scope::Team(o, _) => vec![o.clone()],
            _ => self.listed_owners(),
        };
        for o in &owners_in_view {
            if let Scope::Team(_, t) = &scope {
                teams.push((o.clone(), t.clone()));
                continue;
            }
            if let Some(info) = self.owner_info(o) {
                for t in info.teams.iter().filter(|t| !t.retired) {
                    teams.push((o.clone(), Some(t.id.clone())));
                }
            }
            if self
                .board
                .cards
                .iter()
                .any(|c| &c.owner == o && c.team.is_none() && self.card_visible(c))
            {
                teams.push((o.clone(), None));
            }
        }
        let several_owners = owners_in_view.len() > 1;
        let mut wants: Vec<Want> = Vec::new();
        for (o, t) in &teams {
            let list = t
                .as_deref()
                .and_then(|t| self.owner_info(o)?.team(t)?.stages.clone())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| crate::model::STANDARD.to_string());
            let name = self.team_name(o, t.as_deref());
            let who = if several_owners {
                format!("{} {name}", self.owner_short(o))
            } else {
                name
            };
            match wants.iter_mut().find(|w| w.list == list) {
                Some(w) => w.who.push(who),
                None => {
                    let owner_name = stages::find(lists, &list)
                        .and_then(|l| l.owner.as_deref())
                        .and_then(|lo| self.owner_info(lo))
                        .map(|lo| lo.label().to_string());
                    wants.push(Want {
                        title: stages::list_title(lists, &list, owner_name.as_deref()),
                        list,
                        who: vec![who],
                    })
                }
            }
        }
        for w in &mut wants {
            // A list one team keeps for itself needs no "who": its title says whose it is.
            let own = stages::find(lists, &w.list).is_some_and(|l| {
                l.id != crate::model::STANDARD && w.who.len() == 1 && w.who[0].ends_with(&l.name)
            });
            // On all of Kerdr the standard list is everyone else's.
            if own {
                w.who.clear();
            } else if several_owners && w.list == crate::model::STANDARD {
                w.who = vec!["everyone else".into()];
            }
        }
        wants
    }

    /// The board's columns for the view: Backlog, each list's stages (in bands when the view
    /// mixes lists), Done.
    pub fn layout(&self) -> Layout {
        let lists: std::borrow::Cow<[crate::feed::StageList]> = if self.board.lists.is_empty() {
            std::borrow::Cow::Owned(vec![stages::builtin_standard()])
        } else {
            std::borrow::Cow::Borrowed(&self.board.lists)
        };
        let cards: Vec<(Stage, bool)> = self
            .board
            .cards
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.stage.is_working() && self.card_visible(c) && self.holder(*i).is_none()
            })
            .map(|(_, c)| (c.stage, c.needs_you()))
            .collect();
        let keep: Vec<String> = self
            .open_band
            .iter()
            .cloned()
            .chain(
                self.selected()
                    .and_then(|c| c.stage.list())
                    .map(str::to_string),
            )
            .collect();
        stages::layout(
            &lists,
            &self.wants(),
            &cards,
            &Fold {
                rows: self.height.saturating_sub(CHROME_ROWS),
                keep,
            },
        )
    }

    /// Columns drawn at this width: Backlog and Maintain give way first on a narrow screen, the
    /// Backlog only while nothing in it waits on you. A column the config hides
    /// (`[board] hide_columns`) never shows, its room going to the others (intent 0444), and a
    /// folded band's columns are its one line.
    pub fn visible_columns(&self) -> Vec<bool> {
        self.visible_in(&self.layout())
    }

    pub(crate) fn visible_in(&self, layout: &Layout) -> Vec<bool> {
        let narrow = self.width < 100;
        let mut v: Vec<bool> = layout
            .columns
            .iter()
            .map(|c| {
                !self.cfg.board.hides(c.stage) && c.band.is_none_or(|b| !layout.bands[b].folded)
            })
            .collect();
        if narrow && v[0] {
            v[0] = self.columns_in(layout)[0]
                .iter()
                .any(|&i| self.backlog_needs_you(i));
        }
        if narrow {
            let done = layout.done();
            v[done] = false;
        }
        v
    }

    pub fn columns(&self) -> Vec<Vec<usize>> {
        self.columns_in(&self.layout())
    }

    pub(crate) fn columns_in(&self, layout: &Layout) -> Vec<Vec<usize>> {
        let mut cols: Vec<Vec<usize>> = vec![Vec::new(); layout.columns.len()];
        for (i, c) in self.board.cards.iter().enumerate() {
            // What sits inside a card on the board is on its line, not in a column (intent 0349).
            if c.stage != Stage::Misc
                && self.card_visible(c)
                && self.holder(i).is_none()
                && let Some(ci) = layout.col_of(c.stage)
            {
                cols[ci].push(i);
            }
        }
        for (ci, col) in layout.columns.iter().enumerate() {
            self.arrange(col.stage, &mut cols[ci]);
        }
        cols
    }

    /// The column a card is in on the board as it's laid out now.
    pub fn col_of(&self, card: &Card) -> usize {
        self.layout().col_of(card.stage).unwrap_or(0)
    }

    /// The columns to start on, best first: every working stage but each band's first, then each
    /// band's first, then Backlog, then Done (the board always started past Plan).
    pub(super) fn start_order(layout: &Layout) -> Vec<usize> {
        let mut order = Vec::new();
        for b in &layout.bands {
            order.extend(b.first + 1..b.first + b.len);
        }
        order.extend(layout.bands.iter().filter(|b| b.len > 0).map(|b| b.first));
        order.push(0);
        order.push(layout.done());
        order
    }

    /// h and l: the next column with cards in the same band; past a band's ends, Backlog on the
    /// left and Done on the right; from those, into the band last moved in.
    pub(super) fn move_col(&mut self, dir: i32) {
        let layout = self.layout();
        let cols = self.columns_in(&layout);
        let vis = self.visible_in(&layout);
        let row = self.row();
        let ok = |c: usize| vis[c] && !cols[c].is_empty();
        let cur = self.sel.col.min(layout.columns.len() - 1);
        let done = layout.done();
        let mut scan: Vec<usize> = Vec::new();
        match layout.columns[cur].band {
            Some(b) => {
                let band = &layout.bands[b];
                let (lo, hi) = (band.first, band.first + band.len);
                if dir < 0 {
                    scan.extend((lo..cur).rev());
                    scan.push(0);
                } else {
                    scan.extend(cur + 1..hi);
                    scan.push(done);
                }
            }
            None => {
                let into = self.band_from_side(&layout, &cols, &vis);
                let band_cols: Vec<usize> = into
                    .map(|b| {
                        (layout.bands[b].first..layout.bands[b].first + layout.bands[b].len)
                            .collect()
                    })
                    .unwrap_or_default();
                if cur == 0 && dir > 0 {
                    scan.extend(band_cols);
                    scan.push(done);
                } else if cur == done && dir < 0 {
                    scan.extend(band_cols.into_iter().rev());
                    scan.push(0);
                }
            }
        }
        if let Some(c) = scan.into_iter().find(|&c| ok(c)) {
            let r = row.min(cols[c].len() - 1);
            self.select_in(&layout, &cols, c, r);
        }
    }

    /// The band h or l from Backlog or Done goes into: the one last moved in, else the first
    /// open one with cards.
    fn band_from_side(&self, layout: &Layout, cols: &[Vec<usize>], vis: &[bool]) -> Option<usize> {
        let with_cards = |b: usize| {
            let band = &layout.bands[b];
            (band.first..band.first + band.len).any(|c| vis[c] && !cols[c].is_empty())
        };
        self.last_band
            .as_ref()
            .and_then(|l| layout.bands.iter().position(|b| &b.list == l))
            .filter(|&b| with_cards(b))
            .or_else(|| (0..layout.bands.len()).find(|&b| with_cards(b)))
    }

    /// j and k: the next card in the column; past its ends on a board of bands, into the band
    /// below or above, which opens if it was folded.
    pub(super) fn move_row(&mut self, dir: i32) {
        let layout = self.layout();
        let cols = self.columns_in(&layout);
        let cur = self.sel.col.min(layout.columns.len() - 1);
        let len = cols[cur].len();
        if len == 0 {
            return;
        }
        let row = self.row() as i32 + dir;
        if (0..len as i32).contains(&row) {
            return self.select_in(&layout, &cols, cur, row as usize);
        }
        let Some(b) = layout.columns[cur].band else {
            return;
        };
        let offset = cur - layout.bands[b].first;
        let mut nb = b as i32 + dir;
        while (0..layout.bands.len() as i32).contains(&nb) {
            let band = &layout.bands[nb as usize];
            if band.open > 0 {
                // Opening it may change which bands fold, so lay the board out again.
                self.open_band = Some(band.list.clone());
                let list = band.list.clone();
                let layout = self.layout();
                let cols = self.columns_in(&layout);
                let vis = self.visible_in(&layout);
                let Some(band) = layout.bands.iter().find(|x| x.list == list) else {
                    return;
                };
                let near = offset.min(band.len.saturating_sub(1));
                let mut by_distance: Vec<usize> = (0..band.len).collect();
                by_distance.sort_by_key(|&k| k.abs_diff(near));
                if let Some(c) = by_distance
                    .into_iter()
                    .map(|k| band.first + k)
                    .find(|&c| vis[c] && !cols[c].is_empty())
                {
                    let r = if dir > 0 { 0 } else { cols[c].len() - 1 };
                    self.select_in(&layout, &cols, c, r);
                }
                return;
            }
            nb += dir;
        }
    }

    pub(super) fn select_index(&mut self, col: usize, row: usize) {
        let layout = self.layout();
        let cols = self.columns_in(&layout);
        self.select_in(&layout, &cols, col, row);
    }

    fn select_in(&mut self, layout: &Layout, cols: &[Vec<usize>], col: usize, row: usize) {
        if let Some(&i) = cols.get(col).and_then(|c| c.get(row)) {
            self.sel = Selection {
                col,
                key: Some(self.board.cards[i].key.clone()),
            };
            if let Some(b) = layout.columns[col].band {
                self.last_band = Some(layout.bands[b].list.clone());
            }
        }
    }

    pub(super) fn row(&self) -> usize {
        let cols = self.columns();
        self.sel
            .key
            .as_deref()
            .and_then(|k| {
                cols.get(self.sel.col)?
                    .iter()
                    .position(|&i| self.board.cards[i].key == k)
            })
            .unwrap_or(0)
    }

    /// A card's lifecycle rail: Backlog, its list's stages (its own too, when the list lacks it),
    /// Done.
    pub fn rail(&self, card: &Card) -> Vec<Stage> {
        let list = card.stage.list().unwrap_or(crate::model::STANDARD);
        let mut rail = vec![Stage::Backlog];
        match stages::find(&self.board.lists, list) {
            Some(l) => rail.extend(
                l.stages
                    .iter()
                    .filter(|e| !e.retired || card.stage.id() == e.id)
                    .map(|e| Stage::work(list, &e.id, &e.name)),
            ),
            None if list == crate::model::STANDARD => rail.extend(Stage::STANDARD),
            None => {}
        }
        if card.stage.is_working() && !rail.contains(&card.stage) {
            rail.push(card.stage);
        }
        rail.push(Stage::Maintain);
        rail
    }

    /// The view f's tree and the top bar both pick: a scope on the owner split.
    pub(super) fn choose_view(&mut self, scope: Scope) {
        match &scope {
            Scope::All => self.pick_owner(None),
            Scope::Owner(o) => self.pick_owner(Some(o.clone())),
            Scope::Team(o, _) => {
                if self.owner_pick.as_deref() != Some(o.as_str()) {
                    self.pick_owner(Some(o.clone()));
                }
                if let Some(tab) = self.tabs.iter().position(|t| *t == scope) {
                    self.set_tab(tab);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{Owner, Team};
    use crate::board::MachineState;
    use crate::config::Machine;
    use crate::feed::{Feed, FeedCard, StageEntry, StageList};
    use crate::poll::{Conn, MachineData};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn owner(id: &str, parent: Option<&str>, code: Option<&str>, teams: &[(&str, &str)]) -> Owner {
        Owner {
            id: id.into(),
            kind: if parent.is_some() {
                "tenant"
            } else {
                "organization"
            }
            .into(),
            name: match id {
                "tinyhost" => "Tinyhost".into(),
                "arkship" => "Arkship".into(),
                other => other.into(),
            },
            parent: parent.map(str::to_string),
            code: code.map(str::to_string),
            sessions: code.map(|c| vec![c.to_string()]).unwrap_or_default(),
            agent_machine: Some("lab".into()),
            teams: teams
                .iter()
                .map(|(t, list)| Team {
                    id: t.to_string(),
                    name: t[..1].to_uppercase() + &t[1..],
                    retired: false,
                    stages: Some(list.to_string()),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn card(
        id: &str,
        tenant: Option<&str>,
        org: &str,
        team: Option<&str>,
        stage: &str,
    ) -> FeedCard {
        FeedCard {
            id: id.into(),
            title: format!("card {id}"),
            tenant: tenant.map(str::to_string),
            organization: Some(org.into()),
            team: team.map(str::to_string),
            stage: Some(stage.into()),
            state: Some(
                if stage == "backlog" {
                    "queued"
                } else {
                    "active"
                }
                .into(),
            ),
            ..Default::default()
        }
    }

    /// Tinyhost with Dev on the standard list and Marketing on its own, Arkship with no teams.
    pub(crate) fn owner_app(marketing_list: bool) -> App {
        let marketing = if marketing_list {
            "tinyhost/marketing"
        } else {
            "standard"
        };
        let mut cards = vec![
            card("1", Some("tinyhost"), "arkship", Some("dev"), "build"),
            card("2", Some("tinyhost"), "arkship", None, "test"),
            card("3", None, "arkship", None, "build"),
            card("4", Some("tinyhost"), "arkship", Some("dev"), "backlog"),
        ];
        let mut m = card("5", Some("tinyhost"), "arkship", Some("marketing"), "learn");
        if marketing_list {
            m.stage_list = Some("tinyhost/marketing".into());
        } else {
            m.stage = Some("plan".into());
        }
        m.attention = Some("needs_you".into());
        cards.push(m);
        let feed = Feed {
            kerdr_feed: 1,
            source: Some("s".into()),
            cards,
            owners: vec![
                owner("arkship", None, Some("ARK"), &[]),
                owner(
                    "tinyhost",
                    Some("arkship"),
                    Some("TNY"),
                    &[("dev", "standard"), ("marketing", marketing)],
                ),
                owner("dunneshire", None, None, &[]),
            ],
            stage_lists: vec![StageList {
                id: "tinyhost/marketing".into(),
                name: "Marketing".into(),
                owner: Some("tinyhost".into()),
                rev: 1,
                stages: ["plan", "test", "execute", "learn"]
                    .iter()
                    .map(|s| StageEntry {
                        id: s.to_string(),
                        name: s[..1].to_uppercase() + &s[1..],
                        retired: false,
                    })
                    .collect(),
            }],
            create_fields: vec!["organization".into(), "tenant".into(), "team".into()],
            ..Default::default()
        };
        let mut st = MachineState::new(Machine {
            name: "lab".into(),
            ssh: None,
            herdr: "herdr".into(),
            feed: None,
            hidden: false,
        });
        st.conn = Conn::Live {
            latency: Default::default(),
        };
        st.data = Some(MachineData::default());
        st.feed = Some(feed);
        let mut app = App::new(Config::default(), vec![st]);
        app.width = 200;
        app.rebuild();
        app
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ids(app: &App) -> Vec<String> {
        app.columns()
            .iter()
            .flatten()
            .map(|&i| app.board.cards[i].key.clone())
            .collect()
    }

    #[test]
    fn a_feed_listing_owners_splits_the_bar_by_owner_with_work() {
        let app = owner_app(false);
        assert!(app.owner_split());
        assert_eq!(
            app.tabs,
            vec![
                Scope::All,
                Scope::Owner("arkship".into()),
                Scope::Owner("tinyhost".into())
            ],
            "Dunneshire has no work, so it isn't listed"
        );
    }

    #[test]
    fn braces_step_owners_and_brackets_step_the_owners_teams() {
        let mut app = owner_app(false);
        app.on_key(key('['));
        assert_eq!(*app.scope(), Scope::All, "nothing to step through yet");
        app.on_key(key('}'));
        assert_eq!(*app.scope(), Scope::Owner("arkship".into()));
        app.on_key(key('}'));
        assert_eq!(*app.scope(), Scope::Owner("tinyhost".into()));
        assert_eq!(
            app.tabs,
            vec![
                Scope::Owner("tinyhost".into()),
                Scope::Team("tinyhost".into(), Some("dev".into())),
                Scope::Team("tinyhost".into(), Some("marketing".into())),
                Scope::Team("tinyhost".into(), None),
            ]
        );
        app.on_key(key(']'));
        assert_eq!(
            *app.scope(),
            Scope::Team("tinyhost".into(), Some("dev".into()))
        );
        assert_eq!(ids(&app), ["s:4", "s:1"], "Dev's work only");
        app.on_key(key('4'));
        assert_eq!(*app.scope(), Scope::Team("tinyhost".into(), None));
        assert_eq!(ids(&app), ["s:2"]);
        app.on_key(key('}'));
        assert_eq!(
            *app.scope(),
            Scope::All,
            "past the last owner, all of Kerdr"
        );
        app.on_key(key('{'));
        assert_eq!(
            *app.scope(),
            Scope::Owner("tinyhost".into()),
            "from all, the last"
        );
        app.on_key(key('0'));
        assert_eq!(*app.scope(), Scope::All);
        app.on_key(key('2'));
        assert_eq!(*app.scope(), Scope::Owner("tinyhost".into()), "owner 2");
    }

    #[test]
    fn tab_goes_down_to_an_owner_then_steps_its_teams() {
        let mut app = owner_app(false);
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(*app.scope(), Scope::Owner("arkship".into()));
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            *app.scope(),
            Scope::Owner("arkship".into()),
            "Arkship has no teams"
        );
        app.on_key(key('}'));
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            *app.scope(),
            Scope::Team("tinyhost".into(), Some("dev".into()))
        );
    }

    #[test]
    fn a_team_on_the_standard_list_has_the_board_as_it_always_was() {
        let mut app = owner_app(false);
        app.choose_view(Scope::Owner("tinyhost".into()));
        let layout = app.layout();
        assert!(!layout.banded());
        assert_eq!(layout.columns.len(), 7);
    }

    #[test]
    fn a_team_with_its_own_list_has_its_own_columns() {
        let mut app = owner_app(true);
        app.choose_view(Scope::Team("tinyhost".into(), Some("marketing".into())));
        let layout = app.layout();
        assert!(!layout.banded());
        let names: Vec<&str> = layout.columns.iter().map(|c| c.stage.name()).collect();
        assert_eq!(
            names,
            ["Backlog", "Plan", "Test", "Execute", "Learn", "Maintain"]
        );
        assert_eq!(ids(&app), ["s:5"]);
    }

    #[test]
    fn an_owners_all_mixing_lists_stacks_bands_and_moves_between_them() {
        let mut app = owner_app(true);
        app.choose_view(Scope::Owner("tinyhost".into()));
        let layout = app.layout();
        assert!(layout.banded());
        assert_eq!(layout.bands[0].title, "Tinyhost Marketing's stages");
        assert_eq!(layout.bands[1].title, "Standard stages · Dev, No team");
        // The card needing you is selected first, in the Marketing band.
        assert_eq!(app.sel.key.as_deref(), Some("s:5"));
        app.on_key(key('j'));
        let now = app.selected().unwrap();
        assert_eq!(
            now.stage.list(),
            Some("standard"),
            "down into the next band"
        );
        app.on_key(key('k'));
        assert_eq!(app.sel.key.as_deref(), Some("s:5"), "and back up");
        app.on_key(key('h'));
        assert_eq!(
            app.sel.key.as_deref(),
            Some("s:4"),
            "past the band's left end, Backlog"
        );
        app.on_key(key('l'));
        assert_eq!(
            app.sel.key.as_deref(),
            Some("s:5"),
            "back into the band last moved in"
        );
    }

    #[test]
    fn all_of_kerdr_on_one_list_looks_as_it_always_did() {
        let app = owner_app(false);
        let layout = app.layout();
        assert!(!layout.banded());
        assert_eq!(ids(&app).len(), 5);
    }

    #[test]
    fn f_and_the_bar_are_one_choice() {
        let mut app = owner_app(false);
        let rows = app.filter_rows();
        let team = rows
            .iter()
            .position(|r| r.label == "Marketing")
            .expect("Tinyhost's teams are in f's tree");
        app.mode = Mode::Filter {
            query: String::new(),
            sel: team,
            typing: false,
            back: Box::new(Mode::Board),
        };
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            *app.scope(),
            Scope::Team("tinyhost".into(), Some("marketing".into()))
        );
        assert!(
            app.owner.is_none(),
            "never a second filter that can disagree"
        );
    }

    #[test]
    fn f_has_a_machine_row() {
        let mut app = owner_app(false);
        let rows = app.filter_rows();
        let lab = rows
            .iter()
            .position(|r| r.kind == "machine" && r.label == "lab")
            .expect("a Machine row for each machine");
        app.mode = Mode::Filter {
            query: String::new(),
            sel: lab,
            typing: false,
            back: Box::new(Mode::Board),
        };
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.machine_filter.as_deref(), Some("lab"));
        assert_eq!(ids(&app).len(), 5);
    }

    #[test]
    fn the_config_can_keep_the_machine_split() {
        let mut app = owner_app(false);
        app.cfg.board.split = Split::Machines;
        app.rebuild();
        assert!(!app.owner_split());
        app.on_key(key('}'));
        assert_eq!(*app.scope(), Scope::Machine("lab".into()));
    }

    #[test]
    fn a_feed_with_no_owners_keeps_the_machine_split() {
        let mut app = owner_app(false);
        app.machines[0].feed.as_mut().unwrap().owners.clear();
        app.rebuild();
        assert!(!app.owner_split());
        app.on_key(key('}'));
        assert_eq!(*app.scope(), Scope::Machine("lab".into()));
    }
}
