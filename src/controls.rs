//! Intent controls live apart from the navigation keys. Writes go through the configured
//! adapter, on the record's home machine; the board never edits canonical intent files.

use super::*;
use crate::actions::Response;
use crate::images;
use crate::model::{CardSnapshot, Summary};
use crate::resolve::Effect;
use serde_json::{Value, json};

/// The most the new-intent box holds, kept under the actions' 64 KiB request limit.
const MAX_WORDS: usize = 48_000;

/// The most of an action's answer the board shows: a backend's error can run to thousands of
/// characters, which would fill the screen (intent 0411).
const MAX_ANSWER: usize = 300;

/// An action's answer cut to what the board shows, ending in "…" when it was longer.
pub(super) fn short(text: String) -> String {
    match text.char_indices().nth(MAX_ANSWER) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    }
}

/// Work waiting to start: the only cards d, y, x, J, K, t and B change, so the bottom line, the
/// menu and the keys themselves agree.
pub(crate) fn waiting_work(card: &Card) -> bool {
    card.detail.is_some()
        && !card.dismissed()
        && card.stage == Stage::Backlog
        && !card.is_open_workspace()
        && !matches!(
            card.state.as_deref(),
            Some("closed" | "active" | "background")
        )
}

/// Takes the `[Image #N]` tag the words end with out of them, and its image with it: at the
/// very end for Backspace, or before trailing spaces for Ctrl+W. False when they don't end
/// with an attached image's tag.
fn take_tag(draft: &mut Draft, past_spaces: bool) -> bool {
    let end = if past_spaces {
        draft.words.trim_end().len()
    } else {
        draft.words.len()
    };
    let Some(i) = draft
        .images
        .iter()
        .position(|i| draft.words[..end].ends_with(&images::tag(i.tag)))
    else {
        return false;
    };
    let tag = images::tag(draft.images[i].tag);
    draft.words.truncate(end - tag.len());
    // The same tag typed twice is one image: it goes only with the last of them.
    if !draft.words.contains(&tag) {
        draft.images.remove(i);
    }
    true
}

/// Puts images into a new intent's words, each as the next `[Image #N]` tag at `at` (where the
/// words ended when the read began), or at the end when the words have since been cut shorter
/// than that. What went in, and why anything didn't, to say.
fn attach_into(draft: &mut Draft, got: images::Loading, at: Option<usize>) -> Vec<String> {
    let mut at = at
        .filter(|&a| a <= draft.words.len() && draft.words.is_char_boundary(a))
        .unwrap_or(draft.words.len());
    let mut said = vec![];
    for image in got.images {
        if draft.images.len() >= images::MAX_IMAGES {
            said.push(images::too_many());
            break;
        }
        let total: usize = draft.images.iter().map(|i| i.bytes.len()).sum();
        if total + image.bytes.len() > images::MAX_IMAGES_BYTES {
            said.push(images::too_much());
            break;
        }
        let n = draft.images.iter().map(|i| i.tag).max().unwrap_or(0) + 1;
        let tag = images::tag(n);
        if draft.words.len() + tag.len() + 2 > MAX_WORDS {
            said.push("The box is full; there's no room for another image's tag.".into());
            break;
        }
        // A space either side keeps the tag apart from the words around it.
        let mut piece = String::new();
        if draft.words[..at]
            .chars()
            .last()
            .is_some_and(|c| !c.is_whitespace())
        {
            piece.push(' ');
        }
        piece.push_str(&tag);
        if !draft.words[at..].starts_with(char::is_whitespace) {
            piece.push(' ');
        }
        draft.words.insert_str(at, &piece);
        at += piece.len();
        said.push(format!(
            "Attached {tag}: a {} image, {}.",
            images::kind_name(image.media_type),
            images::size(image.bytes.len())
        ));
        draft.images.push(images::Image {
            tag: n,
            media_type: image.media_type,
            bytes: std::sync::Arc::new(image.bytes),
        });
    }
    said.extend(got.why);
    said
}

/// An id for one new intent's words, so an adapter can tell a resend from new work.
fn new_submission() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("kerdr-{nanos:x}-{:x}", std::process::id())
}

impl App {
    pub fn owner_label(&self) -> String {
        match self.owner.as_deref() {
            None => "All work".into(),
            Some("unassigned") => "Unassigned".into(),
            Some(s) => match s.split_once(':') {
                Some(("org", n)) => format!("{n} (organization)"),
                Some((_, n)) => n.to_string(),
                None => s.to_string(),
            },
        }
    }

    pub fn project_label(&self) -> String {
        self.project
            .clone()
            .unwrap_or_else(|| "all projects".into())
    }

    pub fn provider_label(&self) -> String {
        match self.provider.as_deref() {
            None => "All agents".into(),
            Some(id) => self.all_providers().program_label(id),
        }
    }

    /// The programs the agents filter cycles through: those the feeds' providers and accounts
    /// name, in their order, then any other agent herdr reports on a card, alphabetically.
    pub(crate) fn agent_programs(&self) -> Vec<String> {
        let mut programs = self.all_providers().programs();
        let mut others: Vec<String> = self
            .board
            .cards
            .iter()
            .flat_map(|card| card.agents.iter())
            .map(|agent| agent.agent.clone())
            .filter(|name| !name.is_empty() && !programs.contains(name))
            .collect();
        others.sort();
        others.dedup();
        programs.extend(others);
        programs
    }

    /// Whether f's filters (owner, project, agents or background) hide any work.
    pub(crate) fn filtering(&self) -> bool {
        self.owner.is_some()
            || self.project.is_some()
            || self.provider.is_some()
            || self.background != Background::All
    }

    /// Metadata filters without the selected tab, so other session badges stay meaningful.
    pub(crate) fn card_matches_filters(&self, card: &Card) -> bool {
        self.owner.as_ref().is_none_or(|o| *o == card.owner_key())
            && self.project.as_ref().is_none_or(|p| p == card.project())
            && self
                .provider
                .as_ref()
                .is_none_or(|provider| card.agents.iter().any(|a| &a.agent == provider))
            && self.background.shows(card.background)
            && card.dismissed() == self.wont_do
    }

    pub fn card_visible(&self, card: &Card) -> bool {
        self.scope().contains(card) && self.card_matches_filters(card)
    }

    fn cycle_provider(&mut self) {
        let providers = self.agent_programs();
        self.provider = match self
            .provider
            .as_ref()
            .and_then(|p| providers.iter().position(|name| name == p))
        {
            Some(index) => providers.get(index + 1).cloned(),
            None => providers.first().cloned(),
        };
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
    }

    pub fn summary(&self) -> Summary {
        let mut filtered = self.board.clone();
        filtered.cards.retain(|c| self.card_visible(c));
        board::summarize(&filtered, self.scope(), &self.machines)
    }

    pub(super) fn enter_backlog(&mut self, wont_do: bool) {
        // A search of the backlog stays across sessions and machines, but not into Won't do.
        if wont_do != self.wont_do || !matches!(self.mode, Mode::Backlog { .. }) {
            self.view_search = None;
        }
        self.wont_do = wont_do;
        self.mode = Mode::Backlog { sel: 0 };
        self.backlog_focus = self
            .backlog_cards()
            .first()
            .map(|&i| self.board.cards[i].key.clone());
        self.fix_selection();
    }

    /// The filter list (f): every owner on the board, each with its projects, and how much of
    /// each is under way or waiting. Counts ignore the owner and project filters but follow the
    /// agents and background ones, so they say what picking a row would show.
    pub fn filter_rows(&self) -> Vec<FilterRow> {
        let cards: Vec<&Card> = self
            .board
            .cards
            .iter()
            .filter(|c| {
                c.stage != Stage::Misc
                    && !c.dismissed()
                    && self.scope().contains(c)
                    && self
                        .provider
                        .as_ref()
                        .is_none_or(|p| c.agents.iter().any(|a| &a.agent == p))
                    && self.background.shows(c.background)
            })
            .collect();
        let row = |label: String,
                   kind: String,
                   depth: u8,
                   owner: Option<String>,
                   project: Option<String>| {
            let picked = cards.iter().filter(|c| {
                owner.as_ref().is_none_or(|o| *o == c.owner_key())
                    && project.as_ref().is_none_or(|p| p == c.project())
            });
            let (mut open, mut backlog) = (0, 0);
            for c in picked {
                match c.stage {
                    Stage::Backlog => backlog += 1,
                    Stage::Maintain | Stage::Misc => {}
                    _ => open += 1,
                }
            }
            FilterRow {
                label,
                kind,
                depth,
                owner,
                project,
                open,
                backlog,
            }
        };
        let projects = |owner: &str| {
            let mut v: Vec<String> = cards
                .iter()
                .filter(|c| c.owner_key() == owner && !c.project().is_empty())
                .map(|c| c.project().to_string())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let mut owners: Vec<String> = cards.iter().map(|c| c.owner_key()).collect();
        owners.sort();
        owners.dedup();
        let tenant_org = |tenant: &str| {
            cards
                .iter()
                .find(|c| c.owner_key() == format!("tenant:{tenant}"))
                .and_then(|c| c.detail.as_ref())
                .and_then(|d| d.organization.clone())
                .filter(|o| !o.is_empty())
        };
        let mut orgs: Vec<String> = owners
            .iter()
            .filter_map(|o| o.strip_prefix("org:").map(str::to_string))
            .chain(
                owners
                    .iter()
                    .filter_map(|o| o.strip_prefix("tenant:"))
                    .filter_map(&tenant_org),
            )
            .collect();
        orgs.sort();
        orgs.dedup();
        let mut rows = vec![row("All work".into(), String::new(), 0, None, None)];
        let tenant_rows = |rows: &mut Vec<FilterRow>, tenant: &str, depth: u8, kind: String| {
            let key = format!("tenant:{tenant}");
            rows.push(row(
                tenant.to_string(),
                kind,
                depth,
                Some(key.clone()),
                None,
            ));
            for p in projects(&key).into_iter().filter(|p| p != tenant) {
                rows.push(row(
                    p.clone(),
                    "project".into(),
                    depth + 1,
                    Some(key.clone()),
                    Some(p),
                ));
            }
        };
        for org in &orgs {
            let key = format!("org:{org}");
            rows.push(row(
                org.clone(),
                "organization".into(),
                0,
                Some(key.clone()),
                None,
            ));
            for p in projects(&key) {
                rows.push(row(
                    p.clone(),
                    "project".into(),
                    1,
                    Some(key.clone()),
                    Some(p),
                ));
            }
            for tenant in owners.iter().filter_map(|o| o.strip_prefix("tenant:")) {
                if tenant_org(tenant).as_deref() == Some(org.as_str()) {
                    tenant_rows(&mut rows, tenant, 1, format!("{org} tenant"));
                }
            }
        }
        for tenant in owners.iter().filter_map(|o| o.strip_prefix("tenant:")) {
            if tenant_org(tenant).is_none() {
                tenant_rows(&mut rows, tenant, 0, "tenant".into());
            }
        }
        if owners.iter().any(|o| o == "unassigned") {
            rows.push(row(
                "Unassigned".into(),
                String::new(),
                0,
                Some("unassigned".into()),
                None,
            ));
        }
        rows
    }

    /// The filter list's rows that match what's typed into it.
    pub fn filter_matches(&self, query: &str) -> Vec<FilterRow> {
        let q = query.trim().to_lowercase();
        self.filter_rows()
            .into_iter()
            .filter(|r| {
                q.is_empty()
                    || format!("{} {}", r.label, r.kind)
                        .to_lowercase()
                        .contains(&q)
            })
            .collect()
    }

    pub(super) fn enter_filter(&mut self) {
        self.mode = Mode::Filter {
            query: String::new(),
            sel: self.filter_row_shown(),
            typing: false,
            back: Box::new(self.mode.clone()),
        };
    }

    /// The filter list's row for what the board shows now.
    fn filter_row_shown(&self) -> usize {
        self.filter_rows()
            .iter()
            .position(|r| r.owner == self.owner && r.project == self.project)
            .unwrap_or(0)
    }

    /// b in the filter screen: every card, only background work, none of it, then every card.
    fn cycle_background(&mut self) {
        self.background = self.background.next();
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
    }

    fn set_filter(&mut self, owner: Option<String>, project: Option<String>) {
        self.owner = owner;
        self.project = project;
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
    }

    /// F: every owner, project, agent and background card again.
    pub(super) fn clear_filters(&mut self) {
        let filtered = self.filtering();
        self.provider = None;
        self.background = Background::All;
        self.set_filter(None, None);
        if filtered {
            self.say("Showing everything.", Tone::Info);
        }
    }

    fn filter_key(
        &mut self,
        key: KeyEvent,
        mut query: String,
        mut sel: usize,
        mut typing: bool,
        back: Box<Mode>,
    ) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.filter_matches(&query).len();
        match key.code {
            // Esc stops a search, then closes.
            KeyCode::Esc if typing => {
                typing = false;
                query.clear();
                sel = self.filter_row_shown();
            }
            KeyCode::Esc => {
                self.mode = *back;
                return Action::None;
            }
            KeyCode::Char('f') if !typing && !ctrl => {
                self.mode = *back;
                return Action::None;
            }
            KeyCode::Enter => {
                self.mode = *back;
                // The row drawn highlighted: the list may have shrunk since it was picked.
                if let Some(row) = self
                    .filter_matches(&query)
                    .into_iter()
                    .nth(sel.min(n.saturating_sub(1)))
                {
                    self.set_filter(row.owner, row.project);
                }
                return Action::None;
            }
            KeyCode::Down => sel = (sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => sel = sel.saturating_sub(1),
            KeyCode::Char('j' | 'n') if ctrl => sel = (sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k' | 'p') if ctrl => sel = sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                query.clear();
                sel = 0;
            }
            KeyCode::Tab => self.cycle_provider(),
            KeyCode::Backspace if typing && query.is_empty() => typing = false,
            KeyCode::Backspace => {
                query.pop();
                sel = 0;
            }
            KeyCode::Char(c) if typing && !ctrl => {
                if query.len() < 64 {
                    query.push(c);
                    sel = 0;
                }
            }
            // Not searching, letters are the screen's keys.
            KeyCode::Char('/') if !ctrl => typing = true,
            KeyCode::Char('j') if !ctrl => sel = (sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') if !ctrl => sel = sel.saturating_sub(1),
            KeyCode::Char('a') if !ctrl => self.cycle_provider(),
            KeyCode::Char('b') if !ctrl => self.cycle_background(),
            KeyCode::Char('F') if !ctrl => {
                self.clear_filters();
                query.clear();
                sel = 0;
            }
            _ => {}
        }
        let n = self.filter_matches(&query).len();
        let sel = sel.min(n.saturating_sub(1));
        // b, a and F reset the backlog's selection in the screen behind this one: keep that.
        let back = match &self.mode {
            Mode::Filter { back, .. } => back.clone(),
            _ => back,
        };
        self.mode = Mode::Filter {
            query,
            sel,
            typing,
            back,
        };
        Action::None
    }

    /// The selected card when the card keys apply to it: work waiting in the backlog, or work
    /// set aside as won't do. The bottom line and the menu name its keys.
    pub fn backlog_card_for_keys(&self) -> Option<&Card> {
        let card = match &self.mode {
            Mode::Palette { target, .. } => target.as_ref().and_then(|k| self.card(k)),
            Mode::Backlog { sel } => self
                .backlog_cards()
                .get(*sel)
                .and_then(|&i| self.board.cards.get(i)),
            _ => self.selected(),
        }?;
        (card.dismissed() || waiting_work(card)).then_some(card)
    }

    /// The selected card when any card key applies to it: backlog_card_for_keys's, or open work
    /// with no workspace, which `C` marks done and `x` sets aside (intent 0289). The menu's
    /// THIS CARD and its keys are for it.
    pub fn card_for_keys(&self) -> Option<&Card> {
        self.backlog_card_for_keys()
            .or_else(|| self.started_card_for_keys())
            .or_else(|| self.running_card_for_keys())
    }

    /// The selected card when an agent may be at it, its workspace open: `X` cancels it, and `s`
    /// moves its agent to another account (intent 0321). The menu's THIS CARD names both.
    pub fn running_card_for_keys(&self) -> Option<&Card> {
        let card = match &self.mode {
            Mode::Palette { target, .. } => target.as_ref().and_then(|k| self.card(k)),
            Mode::Backlog { sel } => self
                .backlog_cards()
                .get(*sel)
                .and_then(|&i| self.board.cards.get(i)),
            _ => self.selected(),
        }?;
        card.cancellable().then_some(card)
    }

    /// The selected card when it's work already started with no workspace open: `C` and `x` act
    /// on it, and ⏎ reopens it. None for backlog work, which backlog_card_for_keys covers.
    pub fn started_card_for_keys(&self) -> Option<&Card> {
        let card = match &self.mode {
            Mode::Palette { target, .. } => target.as_ref().and_then(|k| self.card(k)),
            Mode::Backlog { sel } => self
                .backlog_cards()
                .get(*sel)
                .and_then(|&i| self.board.cards.get(i)),
            _ => self.selected(),
        }?;
        (card.closable() && !waiting_work(card)).then_some(card)
    }

    pub(super) fn selected_for_control(&self) -> Option<&Card> {
        match &self.mode {
            Mode::Backlog { sel } => self
                .backlog_cards()
                .get(*sel)
                .and_then(|&i| self.board.cards.get(i)),
            _ => self.selected(),
        }
    }

    pub(super) fn can_write(&mut self, machine: usize) -> bool {
        if self
            .cfg
            .actions
            .command
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            self.say(
                "Intent controls are not configured for this board.",
                Tone::Info,
            );
            return false;
        }
        if !self.machines.get(machine).is_some_and(|m| m.conn.is_live()) {
            self.say(
                "That machine is not answering. Reconnect before changing its intents.",
                Tone::Warn,
            );
            return false;
        }
        true
    }

    pub(super) fn emit_mutation(&mut self, machine: usize, request: Value) -> Action {
        if self.can_write(machine) {
            Action::Mutate { machine, request }
        } else {
            Action::None
        }
    }

    /// Where a refusal said a card's record lives now, while the feed still shows the home it had
    /// when the refusal came; once the feed says anything else about its home, the feed wins.
    pub(super) fn moved_target(&self, card: &Card) -> Option<usize> {
        let detail = card.detail.as_ref()?;
        let home = detail.home.as_ref().and_then(|h| h.machine.as_deref());
        self.moved_home
            .iter()
            .find(|(id, _, from)| *id == detail.id && from.as_deref() == home)
            .map(|(_, to, _)| *to)
    }

    /// The machine a card's record lives on, where its actions run: its home, or the machine its
    /// card came from. None, said, when its home isn't one of Kerdr's machines.
    pub(super) fn home_machine(&mut self, card: &Card) -> Option<usize> {
        // A refusal said it lives elsewhere now, before the board has caught up.
        if let Some(to) = self.moved_target(card) {
            return Some(to);
        }
        let target = card
            .detail
            .as_ref()
            .and_then(|d| d.home.as_ref())
            .and_then(|h| h.machine.clone());
        match target {
            Some(name) => {
                let found = self.machine_index(&name);
                if found.is_none() {
                    self.say(format!("The home machine {name} is not configured. Add it before changing this intent."), Tone::Warn);
                }
                found
            }
            None => Some(card.machine_index),
        }
    }

    /// Enter on work under way whose workspace is gone, or on done work: the action backend on its
    /// home machine reopens it, resuming its agent's conversation, and Kerdr goes in once it
    /// answers. Done work it opens again only after asking, so the yes is Andrew's (intent 0278).
    pub fn reopen_action(&mut self, card: &Card) -> Action {
        let Some(detail) = &card.detail else {
            return Action::None;
        };
        if self.still_saving(card) || self.still_changing(card, "reopen") {
            return Action::None;
        }
        if self
            .cfg
            .actions
            .command
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            let what = if card.done() {
                "is done"
            } else {
                "has no herdr workspace open"
            };
            self.say(
                format!(
                    "{} {what}; with [actions] configured, ⏎ reopens it. m peeks at it.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        let Some(machine) = self.home_machine(card) else {
            return Action::None;
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        // Its home session, else the session tab you're on, if it's that machine's.
        let session = detail
            .home
            .as_ref()
            .and_then(|h| h.session.clone())
            .or_else(|| match self.scope() {
                Scope::Session(m, s) if self.machine_index(m) == Some(machine) => Some(s.clone()),
                _ => None,
            });
        let mut request = json!({"action":"reopen","id":detail.id});
        if let Some(session) = session {
            request["session"] = json!(session);
        }
        self.action_from = Some(CardSnapshot::of(card));
        Action::Mutate { machine, request }
    }

    /// Whether you're still on this work, in the view you're looking at (the board, a peek, the
    /// backlog or Misc), and it hasn't changed since the action started: where a late answer may
    /// ask you something or take you in. Typing a search, a filter or a new intent isn't.
    pub(super) fn still_on(&self, from: &CardSnapshot, made: Option<&str>) -> bool {
        matches!(
            self.mode,
            Mode::Board
                | Mode::Peek { .. }
                | Mode::Backlog { .. }
                | Mode::Misc { .. }
                | Mode::Done { .. }
                | Mode::Card { .. }
        ) && !self.typing_view_search()
            && self
                .selected_for_control()
                .is_some_and(|c| c.key == from.key && from.same_work_or(c, made))
    }

    /// Whether a list view's search is being typed: its letters are words, so no question may
    /// take them over as answers.
    pub(super) fn typing_view_search(&self) -> bool {
        matches!(
            self.mode,
            Mode::Backlog { .. } | Mode::Misc { .. } | Mode::Done { .. }
        ) && self.view_search.as_ref().is_some_and(|s| s.typing)
    }

    /// Whether intent actions are configured, so ⏎ can reopen and d start work.
    pub fn actions_on(&self) -> bool {
        self.cfg
            .actions
            .command
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    }

    /// What the board says while an action runs.
    pub fn working_on(&self, request: &Value) -> String {
        if request["action"]
            .as_str()
            .is_some_and(|a| a.starts_with("account-"))
        {
            return if request["action"] == "account-signin" {
                "Signing in…".into()
            } else {
                "Updating accounts…".into()
            };
        }
        if request["action"] == "reopen" {
            let id = request["id"].as_str().unwrap_or_default();
            format!("Reopening {}…", self.reference_for(id))
        } else if request["action"] == "close" {
            let id = request["id"].as_str().unwrap_or_default();
            format!("Marking {} done…", self.reference_for(id))
        } else if request["action"] == "dispatch" {
            let id = request["id"].as_str().unwrap_or_default();
            format!("Starting {}…", self.reference_for(id))
        } else if request["action"] == "kickoff" {
            let id = request["id"].as_str().unwrap_or_default();
            format!("Checking {}'s questions…", self.reference_for(id))
        } else if request["action"] == "cancel" {
            let id = request["id"].as_str().unwrap_or_default();
            format!("Cancelling {}: stopping its agent…", self.reference_for(id))
        } else {
            "Changing the intent…".into()
        }
    }

    /// How a message names the record with this id: its card's reference, or the id.
    pub(super) fn reference_for(&self, id: &str) -> String {
        self.board
            .cards
            .iter()
            .find(|c| c.detail.as_ref().is_some_and(|d| d.id == id))
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn intent_action(&mut self, action: &str, extra: Value) -> Action {
        let Some(card) = self.selected_for_control().cloned() else {
            self.say("Select an intent first.", Tone::Info);
            return Action::None;
        };
        self.intent_action_on(card, action, extra)
    }

    /// An intent action on this card, which needn't be the selected one: ⏎ on a search hit, a
    /// row inside a card or the misc list starts that card's work (intent 0404).
    pub(super) fn intent_action_on(&mut self, card: Card, action: &str, extra: Value) -> Action {
        let Some(detail) = &card.detail else {
            self.say("This workspace has no intent record to change.", Tone::Info);
            return Action::None;
        };
        if self.still_saving(&card) || self.still_changing(&card, action) {
            return Action::None;
        }
        // d on work still being written up from his words: it starts as soon as that's done,
        // rather than be refused until then. The runner holds the start until its home's feed
        // shows it written up, so it goes even while he's inside a workspace (intent 0445).
        if action == "dispatch" && card.intake() == Some("pending") && waiting_work(&card) {
            if self.resolver.waiting_for_write_up(&card.key).is_none() {
                let Some(machine) = self.home_machine(&card) else {
                    return Action::None;
                };
                if !self.can_write(machine) {
                    return Action::None;
                }
                let request = json!({"action":"dispatch","id":detail.id});
                let effect = self.effect_for(&card.key, &request);
                let n = self.resolver.add_gated(
                    machine,
                    request,
                    Some(CardSnapshot::of(&card)),
                    Some(card.key.clone()),
                    effect,
                );
                self.note_workspaces(n, machine);
            }
            self.say(
                format!(
                    "{} is still being written up from your words; it starts as soon as that's done. Esc on the board drops the start.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        self.backlog_focus = Some(card.key.clone());
        if action == "restore" && !card.dismissed() {
            self.say(
                format!(
                    "u brings back work from won't do; {} isn't there.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        // C and x on work already started (intent 0289): only with no workspace open, since that
        // is where its agent would be; the backend checks for one running elsewhere too.
        let ends = matches!(action, "close" | "dismiss");
        let started = !card.dismissed() && !waiting_work(&card);
        if action == "close" && card.dismissed() {
            self.say(
                format!(
                    "{} is in won't do; u brings it back before it can be marked done.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        // d on work in a working column that no agent has run and nothing records a state for
        // (another agent's note in its log moved it there): it does what ⏎ does, whose backend
        // starts it when it never ran and resumes it when it did, so d and ⏎ agree (intent 0411).
        if action == "dispatch"
            && started
            && card.no_agent()
            && card.state.is_none()
            && card.under_way()
            && card.outside_herdr().is_none()
        {
            return self.reopen_action(&card);
        }
        if started && !(ends && card.closable()) {
            let reference = &card.reference;
            let said = if ends && card.done() || card.state.as_deref() == Some("closed") {
                if action == "close" {
                    format!("{reference} is done already.")
                } else {
                    format!("{reference} is done; won't do is for work that isn't.")
                }
            } else if ends && card.is_open_workspace() {
                format!(
                    "{reference} has a workspace open, so an agent may be at it: C and x are for work with none. ⏎ goes in."
                )
            } else if ends && card.intake() == Some("pending") {
                format!("{reference} is still being written up.")
            } else if card.closable() {
                format!(
                    "{reference} is already under way. d, y, J, K, t and B change backlog work; with no agent at it, C marks it done and x sets it aside."
                )
            } else {
                format!(
                    "{reference} is already under way or done. d, y, x, J, K, t and B change backlog work."
                )
            };
            self.say(said, Tone::Info);
            return Action::None;
        }
        if card.dismissed() && action != "restore" {
            self.say(
                "Restore this intent before starting or reordering it.",
                Tone::Info,
            );
            return Action::None;
        }
        let Some(machine) = self.home_machine(&card) else {
            return Action::None;
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        let mut request = json!({"action":action,"id":detail.id});
        if let Some(obj) = extra.as_object() {
            request.as_object_mut().unwrap().extend(obj.clone());
        }
        if ends {
            let (title, message) = match (action, started) {
                ("close", _) => (
                    format!("Mark {} done?", card.reference),
                    "It closes as done with nothing started: no agent, no tokens. Its log says you closed it from Kerdr, and ⏎ on it later asks to reopen it.".to_string(),
                ),
                (_, true) => (
                    format!("Won't do {}?", card.reference),
                    "Set it aside in Won't do, off the board, with nothing started. Its record keeps where it was; u brings it back.".to_string(),
                ),
                _ => (
                    format!("Won't do {}?", card.reference),
                    "Keep the intent in Won't do, outside the active backlog. You can restore it later.".to_string(),
                ),
            };
            self.mode = Mode::Confirm {
                machine,
                request,
                title,
                message,
                back: Box::new(self.mode.clone()),
                target: Some(Box::new(CardSnapshot::of(&card))),
                queue: false,
                ready_at: None,
                plain: false,
            };
            Action::None
        } else {
            // Its questions are read ahead: the box opens now, and the start waits for his answers.
            if let Some(check) = self.open_preloaded_kickoff(&card, machine, &request) {
                return check;
            }
            self.action_from = Some(CardSnapshot::of(&card));
            Action::Mutate { machine, request }
        }
    }

    /// X (intent 0321): the agent at the selected card's work stopped, mid-step or idle, by
    /// closing its workspace (only its tab, for a child running in its parent's workspace), and the
    /// work set aside in Won't do, its state kept, so u brings it back and ⏎ then resumes its
    /// conversation. It asks first, saying what it will stop and how it leaves the intent; the
    /// request names the workspace and tab the yes was for, so the backend closes nothing else.
    fn cancel_action(&mut self) -> Action {
        let Some(card) = self.selected_for_control().cloned() else {
            self.say("Select an intent first.", Tone::Info);
            return Action::None;
        };
        let Some(detail) = &card.detail else {
            self.say(
                "This workspace has no intent record: X cancels an intent's work. Close the workspace in herdr instead.",
                Tone::Info,
            );
            return Action::None;
        };
        if self.still_saving(&card) || self.still_changing(&card, "cancel") {
            return Action::None;
        }
        self.backlog_focus = Some(card.key.clone());
        let reference = card.reference.clone();
        if !card.cancellable() {
            let said = if card.dismissed() {
                format!("{reference} is in won't do already.")
            } else if card.done()
                || card.state.as_deref() == Some("closed")
                || card.attention == Attention::Done
            {
                format!("{reference} is done already; there's nothing to cancel.")
            } else if card.intake() == Some("pending") {
                format!("{reference} is still being written up.")
            } else if card.closable() {
                format!(
                    "{reference} has no workspace open, so no agent to stop: x sets it aside, C marks it done."
                )
            } else {
                format!("X cancels work an agent is at; {reference} has none.")
            };
            self.say(said, Tone::Info);
            return Action::None;
        }
        // Its workspace is where the card came from: that machine's herdr closes it.
        let machine = card.machine_index;
        if !self.can_write(machine) {
            return Action::None;
        }
        let (Some(session), Some(workspace)) = (card.session.clone(), card.workspace_id.clone())
        else {
            return Action::None;
        };
        let mut request =
            json!({"action":"cancel","id":detail.id,"session":session,"workspace":workspace});
        if let Some(tab) = &card.tab_id {
            request["tab"] = json!(tab);
        }
        let place = card
            .workspace_label
            .clone()
            .unwrap_or_else(|| workspace.clone());
        let what = match card.agents.as_slice() {
            [] => "whatever runs there".to_string(),
            [a] => format!(
                "its {} agent{}",
                self.providers(machine).program_label(&a.agent),
                match a.status.as_str() {
                    "working" => " (mid-step)",
                    "blocked" => " (at a question)",
                    "idle" | "done" => " (idle)",
                    _ => "",
                }
            ),
            agents => format!("its {} agents", agents.len()),
        };
        let closes = if card.tab_id.is_some() {
            format!("its tab in {place} ({session}), leaving the rest of that workspace running")
        } else {
            format!("workspace {place} ({session}), with every tab in it")
        };
        // Children in tabs of a workspace that closes stop too, and stay open with no agent.
        let children: Vec<String> = if card.tab_id.is_none() {
            self.board
                .cards
                .iter()
                .filter(|c| {
                    c.key != card.key
                        && c.machine_index == card.machine_index
                        && c.session == card.session
                        && c.workspace_id == card.workspace_id
                        && c.tab_id.is_some()
                })
                .map(|c| c.reference.clone())
                .collect()
        } else {
            vec![]
        };
        let mut message = format!(
            "Stops {what} and closes {closes}, then sets {reference} aside in Won't do. Anything it hadn't committed stays as it left it. Its log says you cancelled it; u brings it back, and ⏎ then resumes its conversation."
        );
        if !children.is_empty() {
            message.push_str(&format!(
                " {} in a tab there {} too and {} open with no agent; x or C ends {}.",
                children.join(", "),
                if children.len() == 1 { "stops" } else { "stop" },
                if children.len() == 1 { "stays" } else { "stay" },
                if children.len() == 1 { "it" } else { "them" }
            ));
        }
        self.mode = Mode::Confirm {
            machine,
            request,
            title: format!("Cancel {reference}?"),
            message,
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(&card))),
            queue: false,
            ready_at: None,
            plain: false,
        };
        Action::None
    }

    fn new_intent(&mut self) {
        // The box opens while earlier ones are still saving (intent 0445). Words a failed save
        // gave back come first, once nothing else waits in it.
        if self.draft.trim().is_empty()
            && let Some(back) = self.returned.pop_front()
        {
            self.draft = back.words;
            self.draft_images = back.images;
            self.draft_came_back = back.from;
        }
        let machine = match self.scope() {
            Scope::Session(m, _) => self.machine_index(m),
            _ => self.machine.as_deref().and_then(|m| self.machine_index(m)),
        }
        .unwrap_or(0);
        let tenant = self
            .owner
            .as_deref()
            .and_then(|o| o.strip_prefix("tenant:"));
        let session = match (self.scope(), tenant) {
            (Scope::Session(_, s), _) => s.clone(),
            (_, Some(t)) => t.to_string(),
            _ => "default".into(),
        };
        self.mode = Mode::Compose {
            draft: Draft {
                words: self.draft.clone(),
                machine,
                session,
                images: self.draft_images.clone(),
                came_back: self.draft_came_back.clone(),
            },
            back: Box::new(self.mode.clone()),
        };
    }

    /// Ctrl+Space: the same letters as the direct keys, listed, so the menu is a reminder and
    /// never a second set of keys to learn.
    fn palette_key(&mut self, key: KeyEvent, back: Box<Mode>, target: Option<String>) -> Action {
        self.mode = (*back).clone();
        let card_key = matches!(
            key.code,
            KeyCode::Char('d' | 'e' | 'y' | 'x' | 'u' | 't' | 'B' | 'J' | 'K' | 'C' | 'X' | 's')
        );
        // A refresh while the menu was open may have moved or removed the card it opened on;
        // its keys then act on nothing rather than on whatever took its place.
        if card_key && self.card_for_keys().map(|c| &c.key) != target.as_ref() {
            self.say("The selected intent changed while the menu was open. Select it again before changing it.", Tone::Warn);
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {}
            KeyCode::Char('c') => self.new_intent(),
            KeyCode::Char(
                c @ ('d' | 'e' | 'y' | 'x' | 'u' | 't' | 'B' | 'J' | 'K' | 'C' | 'X'),
            ) => {
                return self.card_action(c, 1);
            }
            // The menu's s is the board's: the selected card's agent to another account.
            KeyCode::Char('s') => {
                return self
                    .account_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
                    .unwrap_or(Action::None);
            }
            KeyCode::Char('b') => self.enter_backlog(false),
            KeyCode::Char('w') => self.enter_backlog(!self.wont_do),
            KeyCode::Char('i') => self.enter_misc(),
            KeyCode::Char('D') => self.enter_done(),
            KeyCode::Char('f') => self.enter_filter(),
            KeyCode::Char('F') => self.clear_filters(),
            KeyCode::Char('a') => self.cycle_provider(),
            KeyCode::Char('M') => self.mode = Mode::Machines { sel: 0 },
            KeyCode::Char('?') => {
                self.mode = Mode::Keys {
                    filter: String::new(),
                    typing: false,
                    scroll: 0,
                }
            }
            _ => self.mode = Mode::Palette { back, target },
        }
        Action::None
    }

    /// The keys that change the selected backlog intent, the same wherever one is selected.
    pub(super) fn card_action(&mut self, key: char, count: i64) -> Action {
        match key {
            'd' => self.intent_action("dispatch", json!({})),
            'e' => self.edit_action(),
            'y' => self.intent_action("refresh", json!({})),
            'x' => self.intent_action("dismiss", json!({})),
            'C' => self.intent_action("close", json!({})),
            'X' => self.cancel_action(),
            'u' => self.intent_action("restore", json!({})),
            't' => self.intent_action("move", json!({"top":true})),
            'B' => self.intent_action("move", json!({"bottom":true})),
            'J' => self.intent_action("move", json!({"delta":count})),
            'K' => self.intent_action("move", json!({"delta":-count})),
            _ => Action::None,
        }
    }

    /// The new-intent box: Andrew's own words, nothing else. Enter sends them; Shift+Enter,
    /// Alt+Enter or Ctrl+J starts a new line; Ctrl+V attaches the clipboard's image as an
    /// `[Image #N]` tag, as Claude Code does; Esc keeps them for the next time it opens.
    fn compose_key(&mut self, key: KeyEvent, mut draft: Draft, back: Box<Mode>) -> Action {
        if key.code == KeyCode::Esc {
            self.draft = draft.words;
            self.draft_images = draft.images;
            self.draft_came_back = draft.came_back;
            // A read for words already sent carries on for them.
            if self.box_loading().is_some() {
                self.image_load = None;
            }
            self.mode = *back;
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let newline = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        let room = draft.words.len() < MAX_WORDS;
        match key.code {
            KeyCode::Enter if newline => {
                if room {
                    draft.words.push('\n');
                }
            }
            KeyCode::Char('j') if ctrl => {
                if room {
                    draft.words.push('\n');
                }
            }
            KeyCode::Enter => {
                if draft.words.trim().is_empty() {
                    self.say("Write what you want first.", Tone::Info);
                } else if !self.can_write(draft.machine) {
                    // Said why; the box stays open with the words.
                } else if let Some(load) = self.box_loading() {
                    // The image still being read goes with them: the box closes now, and they're
                    // sent once it's in (intent 0445). What the box kept for next time is these
                    // words, now on their way: it opens empty, and nothing else typed meanwhile
                    // is cleared when they go.
                    self.clear_kept_draft();
                    self.send_after_load = Some((load, draft));
                    self.closed_on_send = true;
                    self.enter_backlog(false);
                    self.say(
                        "Reading the image; your new intent is sent once it's in.",
                        Tone::Info,
                    );
                    return Action::None;
                } else {
                    // The box closes at once onto the backlog, where the new intent's card shows
                    // while it's saved (intent 0445).
                    self.clear_kept_draft();
                    self.closed_on_send = true;
                    self.enter_backlog(false);
                    return self.send_draft(draft);
                }
            }
            KeyCode::Char('v') if ctrl => {
                self.mode = Mode::Compose { draft, back };
                return Action::LoadImages(images::Source::Clipboard);
            }
            // A tag goes as one, and its image with it.
            KeyCode::Backspace if take_tag(&mut draft, false) => {}
            KeyCode::Backspace => {
                draft.words.pop();
            }
            KeyCode::Char('u') if ctrl => {
                draft.words.clear();
                draft.images.clear();
            }
            KeyCode::Char('w') if ctrl && take_tag(&mut draft, true) => {}
            KeyCode::Char('w') if ctrl => {
                let kept = draft.words.trim_end().len();
                draft.words.truncate(kept);
                let cut = draft
                    .words
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_whitespace())
                    .map_or(0, |(i, c)| i + c.len_utf8());
                draft.words.truncate(cut);
            }
            KeyCode::Char(c) if !ctrl && room => draft.words.push(c),
            _ => {}
        }
        self.mode = Mode::Compose { draft, back };
        Action::None
    }

    /// Sends a new intent's words. Each send keeps its own words, images and submission until
    /// its answer comes, so the box is free for the next at once (intent 0445). Words whose last
    /// send went unanswered may be saved already: they go again where they went then, with the
    /// same submission, wherever you are now, so the adapter finds the first one rather than
    /// making a second.
    fn send_draft(&mut self, mut draft: Draft) -> Action {
        // Its machine stopped answering meanwhile (a read that took a while): nothing is lost.
        if !self.can_write(draft.machine) {
            self.give_back(draft.words, draft.images, draft.came_back);
            return Action::None;
        }
        let words = draft.words.trim().to_string();
        // Only the images whose tags are still in the words go with them.
        let images: Vec<_> = draft
            .images
            .iter()
            .filter(|i| words.contains(&images::tag(i.tag)))
            .cloned()
            .collect();
        // Words a save gave back, unchanged: they go again as the send they came back from, even
        // when another send of the same words has since been answered (intent 0445).
        if let Some(s) = draft
            .came_back
            .take()
            .filter(|s| s.words == words && s.images == images)
        {
            draft.machine = s.machine;
            draft.session = s.session.clone();
            if !self.submissions.iter().any(|k| k.id == s.id) {
                self.submissions.push(s);
            }
        } else if let Some(s) = self
            .submissions
            .iter()
            .find(|s| s.unknown && s.words == words && s.images == images)
        {
            draft.machine = s.machine;
            draft.session = s.session.clone();
        }
        let id = match self.submissions.iter().find(|s| {
            s.words == words
                && s.images == images
                && s.machine == draft.machine
                && s.session == draft.session
        }) {
            Some(s) => s.id.clone(),
            None => {
                let id = new_submission();
                self.submissions.push(Submission {
                    id: id.clone(),
                    words: words.clone(),
                    images: images.clone(),
                    machine: draft.machine,
                    session: draft.session.clone(),
                    unknown: false,
                });
                id
            }
        };
        // The same words already on their way: one send, not two, so a failure of one can't
        // give back words the other has saved (intent 0445).
        if self.resolver.saving(&id).is_some() {
            self.say(
                "Those words are already being saved; their card shows in the backlog.",
                Tone::Info,
            );
            return Action::None;
        }
        let mut request =
            json!({"action":"create","words":words,"session":draft.session,"submission":id});
        if !images.is_empty() {
            request["images"] = Value::Array(images::request_images(&words, &images));
        }
        self.emit_mutation(draft.machine, request)
    }

    /// Empties what the box keeps for next time: Enter has taken those words.
    fn clear_kept_draft(&mut self) {
        self.draft.clear();
        self.draft_images.clear();
        self.draft_came_back = None;
    }

    /// The image read for the box that's open, not one for words already sent.
    fn box_loading(&self) -> Option<u64> {
        self.image_load
            .filter(|l| self.send_after_load.as_ref().is_none_or(|(s, _)| s != l))
    }

    /// Words a failed save gives back: into the box for the next c when it's free, else kept
    /// for a later c. Whether they went into the box.
    fn give_back(
        &mut self,
        words: String,
        images: Vec<images::Image>,
        from: Option<crate::app::Submission>,
    ) -> bool {
        let open = matches!(self.mode, Mode::Compose { .. }) || self.send_after_load.is_some();
        if !open && self.draft.trim().is_empty() {
            self.draft = words;
            self.draft_images = images;
            self.draft_came_back = from;
            true
        } else {
            self.returned.push_back(crate::app::Returned {
                words,
                images,
                from,
            });
            false
        }
    }

    /// The card key a new intent's card has while it's saved.
    pub(super) fn placeholder_key(&self, machine: usize, submission: &str) -> String {
        format!(
            "{}:{}",
            self.source_of(machine),
            crate::resolve::Placeholder::id_for(submission)
        )
    }

    /// The source of a machine's feed, which its cards' keys start with.
    pub(super) fn source_of(&self, machine: usize) -> String {
        self.machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .and_then(|f| f.source.clone())
            .unwrap_or_else(|| "feed".into())
    }

    /// Pasted text arrives whole, line breaks and all, so a pasted paragraph never sends itself.
    /// In the new-intent box, a paste that is only an image file's path attaches that image,
    /// and an empty one (a terminal pasting a clipboard that holds only an image) looks for one.
    pub fn on_paste(&mut self, text: &str) -> Action {
        self.closed_on_send = false;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.account_paste(&text) {
            return Action::None;
        }
        if matches!(self.mode, Mode::Compose { .. }) {
            // Spaces and blank lines are words; only a paste with nothing in it means an image.
            if text.is_empty() {
                return Action::LoadImages(images::Source::Clipboard);
            }
            if let Some(paths) = images::paths_in_paste(&text) {
                return Action::LoadImages(images::Source::Files(paths));
            }
        }
        if self.typing_view_search() {
            if let Some(s) = self.view_search.as_mut() {
                let room = 64usize.saturating_sub(s.query.chars().count());
                s.query.extend(text.replace('\n', " ").chars().take(room));
            }
            self.settle_view_selection(false);
            return Action::None;
        }
        match &mut self.mode {
            Mode::Kickoff { form, .. } => App::kickoff_paste(form, &text),
            Mode::Compose { draft, .. } => {
                let room = MAX_WORDS.saturating_sub(draft.words.len());
                let mut end = text.len().min(room);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                draft.words.push_str(&text[..end]);
            }
            Mode::Search(s) => {
                s.query.push_str(&text.replace('\n', " "));
                s.sel = 0;
            }
            Mode::Filter {
                query, sel, typing, ..
            } => {
                query.push_str(&text.replace('\n', " "));
                *sel = 0;
                *typing = true;
            }
            Mode::Keys {
                filter,
                typing: true,
                scroll,
            } => {
                filter.push_str(&text.replace('\n', " "));
                *scroll = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// Starts reading images for the new-intent box: the read's number and the room its draft
    /// has left, or None (saying why) when there's no box to read them into.
    pub fn begin_image_load(&mut self, source: &images::Source) -> Option<(u64, images::Room)> {
        let Mode::Compose { draft, .. } = &self.mode else {
            return None;
        };
        if self.image_load.is_some() {
            self.say("Still reading the last image.", Tone::Info);
            return None;
        }
        let room = images::Room::left(&draft.images);
        self.image_at = draft.words.len();
        self.image_loads += 1;
        self.image_load = Some(self.image_loads);
        let what = match source {
            images::Source::Clipboard => "Reading the clipboard…",
            images::Source::Files(_) => "Reading the image…",
        };
        self.say(what, Tone::Info);
        Some((self.image_loads, room))
    }

    /// A read started by begin_image_load has ended: its images go in, unless the box it was
    /// for has closed since. When Enter sent the words while it read, they go now, with it: the
    /// action that sends them (intent 0445).
    pub fn images_loaded(&mut self, id: u64, loading: images::Loading) -> Action {
        if self.image_load != Some(id) {
            return Action::None;
        }
        self.image_load = None;
        if let Some((load, mut draft)) = self.send_after_load.take() {
            if load == id {
                let said = attach_into(&mut draft, loading, Some(self.image_at));
                if said.iter().any(|s| !s.starts_with("Attached")) {
                    // Not everything read went in (none found, the box full, too many or too
                    // big): not sent, words and what did go in kept, and why said.
                    let into_box = self.give_back(draft.words, draft.images, draft.came_back);
                    let back = if into_box {
                        "c opens it again with your words."
                    } else {
                        "Its words are kept, and c brings them back once the box is empty."
                    };
                    self.fail(format!(
                        "Your new intent wasn't sent: {} {back}",
                        said.join(" ")
                    ));
                    return Action::None;
                }
                return self.send_draft(draft);
            }
            self.send_after_load = Some((load, draft));
        }
        self.attach_images_at(loading, Some(self.image_at));
        Action::None
    }

    /// Puts images read from the clipboard or a pasted path into the new-intent box, each as
    /// the next `[Image #N]` tag at the end of the words, and says what went in and why
    /// anything didn't.
    pub fn attach_images(&mut self, got: images::Loading) {
        self.attach_images_at(got, None);
    }

    /// As attach_images, with the tags at `at` in the words (where they ended when the read
    /// began), or at the end when the words have since been cut shorter than that.
    fn attach_images_at(&mut self, got: images::Loading, at: Option<usize>) {
        let Mode::Compose { draft, .. } = &mut self.mode else {
            return;
        };
        let said = attach_into(draft, got, at);
        if said.is_empty() {
            return;
        }
        let good = said.iter().all(|s| s.starts_with("Attached"));
        self.say(said.join(" "), if good { Tone::Good } else { Tone::Warn });
    }

    pub(super) fn control_key(&mut self, key: KeyEvent) -> Option<Action> {
        match self.mode.clone() {
            Mode::Compose { draft, back } => return Some(self.compose_key(key, draft, back)),
            Mode::Kickoff { form, back, target } => {
                return Some(self.kickoff_key(key, form, back, target));
            }
            Mode::Filter {
                query,
                sel,
                typing,
                back,
            } => {
                return Some(self.filter_key(key, query, sel, typing, back));
            }
            Mode::Confirm {
                machine,
                mut request,
                back,
                target,
                queue,
                ready_at,
                plain,
                ..
            } => {
                // A question that just appeared, or a key held down, never answers it.
                let early = key.kind == KeyEventKind::Repeat
                    || ready_at.is_some_and(|at| Instant::now() < at);
                if early && matches!(key.code, KeyCode::Enter | KeyCode::Char('y' | 'w')) {
                    return Some(Action::None);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                        self.mode = *back;
                    }
                    // A full session's other answer: the reopen waits in its queue.
                    KeyCode::Char('w') if queue => {
                        self.mode = *back;
                        if !self.confirm_still_applies(machine, &request, target.as_deref()) {
                            self.say(
                                "That intent changed while you were deciding, so nothing was sent. Select it again.",
                                Tone::Warn,
                            );
                            return Some(Action::None);
                        }
                        request["queue"] = json!(true);
                        let action = self.emit_mutation(machine, request);
                        if matches!(action, Action::Mutate { .. }) {
                            self.action_from = target.map(|t| *t);
                        }
                        return Some(action);
                    }
                    KeyCode::Enter | KeyCode::Char('y') => {
                        self.mode = *back;
                        if !self.confirm_still_applies(machine, &request, target.as_deref()) {
                            self.say(
                                "That intent changed while you were deciding, so nothing was sent. Select it again.",
                                Tone::Warn,
                            );
                            return Some(Action::None);
                        }
                        if !plain
                            && matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
                        {
                            request["override"] = json!(true);
                        }
                        let action = self.emit_mutation(machine, request);
                        if matches!(action, Action::Mutate { .. }) {
                            self.action_from = target.map(|t| *t);
                        }
                        return Some(action);
                    }
                    _ => {}
                }
                return Some(Action::None);
            }
            Mode::Palette { back, target } => return Some(self.palette_key(key, back, target)),
            _ => {}
        }
        let leader = key.code == KeyCode::Null
            || (matches!(key.code, KeyCode::Char(' ' | '\0'))
                && key.modifiers.contains(KeyModifiers::CONTROL));
        if leader
            && matches!(
                self.mode,
                Mode::Board
                    | Mode::Peek { .. }
                    | Mode::Backlog { .. }
                    | Mode::Misc { .. }
                    | Mode::Done { .. }
            )
        {
            let target = self.card_for_keys().map(|c| c.key.clone());
            if let Some(key) = &target {
                self.backlog_focus = Some(key.clone());
            }
            self.mode = Mode::Palette {
                back: Box::new(self.mode.clone()),
                target,
            };
            return Some(Action::None);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let lists = matches!(
            self.mode,
            Mode::Board | Mode::Backlog { .. } | Mode::Misc { .. } | Mode::Done { .. }
        );
        match key.code {
            KeyCode::Char('c') if lists || matches!(self.mode, Mode::Peek { .. }) => {
                self.new_intent();
                return Some(Action::None);
            }
            KeyCode::Char('f') if lists => {
                self.enter_filter();
                return Some(Action::None);
            }
            KeyCode::Char('F') if lists => {
                self.clear_filters();
                return Some(Action::None);
            }
            KeyCode::Char('a') if lists => {
                self.cycle_provider();
                return Some(Action::None);
            }
            KeyCode::Char('i') if matches!(self.mode, Mode::Board | Mode::Backlog { .. }) => {
                self.enter_misc();
                return Some(Action::None);
            }
            KeyCode::Char('w') if matches!(self.mode, Mode::Board | Mode::Backlog { .. }) => {
                self.enter_backlog(!self.wont_do);
                return Some(Action::None);
            }
            _ => {}
        }
        if !matches!(
            self.mode,
            Mode::Board | Mode::Backlog { .. } | Mode::Peek { .. }
        ) {
            return None;
        }
        // On the board the digits pick sessions, so counted moves (10J) belong to the backlog
        // and a peek.
        if !matches!(self.mode, Mode::Board)
            && let KeyCode::Char(c @ '0'..='9') = key.code
        {
            if (c != '0' || !self.count.is_empty()) && self.count.len() < 5 {
                self.count.push(c);
            }
            return Some(Action::None);
        }
        let count: i64 = std::mem::take(&mut self.count).parse().unwrap_or(1);
        match key.code {
            KeyCode::Char(
                c @ ('d' | 'e' | 'y' | 'x' | 'u' | 't' | 'B' | 'J' | 'K' | 'C' | 'X'),
            ) => {
                return Some(self.card_action(c, count));
            }
            KeyCode::Char('{') | KeyCode::Char('}')
                if matches!(self.mode, Mode::Backlog { .. }) =>
            {
                self.step_machine(if key.code == KeyCode::Char('{') {
                    -1
                } else {
                    1
                });
                self.enter_backlog(self.wont_do);
                return Some(Action::None);
            }
            KeyCode::Char('[') | KeyCode::Char(']') | KeyCode::Tab | KeyCode::BackTab
                if matches!(self.mode, Mode::Backlog { .. }) =>
            {
                self.step_tab(
                    if matches!(key.code, KeyCode::Char('[') | KeyCode::BackTab) {
                        -1
                    } else {
                        1
                    },
                );
                self.enter_backlog(self.wont_do);
                return Some(Action::None);
            }
            _ => {}
        }
        None
    }

    /// Takes a change to a card into the action resolution tool (intent 0416): the board shows it
    /// at once, and its command runs in the background.
    pub fn queue_action(&mut self, machine: usize, request: Value) {
        if request["action"] == "create" {
            return self.queue_create(machine, request);
        }
        let from = self.action_from.take();
        let id = request["id"].as_str().unwrap_or_default().to_string();
        // An item's new text is for the card whose text Kerdr holds (intent 0414).
        let held = (request["action"] == "edit")
            .then(|| self.edit_card(&id, machine))
            .flatten();
        let key = from.as_ref().map(|f| f.key.clone()).or(held).or_else(|| {
            let mine = |c: &&Card| c.detail.as_ref().is_some_and(|d| d.id == id);
            let cards = &self.board.cards;
            cards
                .iter()
                .filter(mine)
                .find(|c| c.machine_index == machine)
                .or_else(|| cards.iter().find(mine))
                .map(|c| c.key.clone())
        });
        let effect = key.as_deref().and_then(|k| self.effect_for(k, &request));
        let shows = effect.is_some();
        let starts = matches!(request["action"].as_str(), Some("dispatch" | "reopen"));
        let asking = (!shows).then(|| self.asking_line(&request)).flatten();
        let n = self.resolver.add(machine, request, from, key, effect);
        if let Some(line) = asking {
            self.resolver.mark_asking(n);
            self.say(line, Tone::Info);
        }
        if starts {
            self.note_workspaces(n, machine);
        }
        if shows {
            self.rebuild();
        }
    }

    /// Notes the workspaces a start's machine has now: one that opens meanwhile is the start's
    /// (intent 0445).
    fn note_workspaces(&mut self, n: u64, machine: usize) {
        if let Some(name) = self.machines.get(machine).map(|m| m.name()) {
            let before = self
                .board
                .workspaces
                .iter()
                .filter(|w| w.machine == name)
                .map(|w| (w.session.clone(), w.id.clone()))
                .collect();
            self.resolver.note_workspaces(n, before);
        }
    }

    /// A new intent: its card shows at once in the backlog it went to, saying it's being saved,
    /// and the backlog lands on it when Enter just sent it (intent 0445).
    fn queue_create(&mut self, machine: usize, request: Value) {
        let submission = request["submission"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let key = self.placeholder_key(machine, &submission);
        // A second send of the same words while the first is on its way shows no second card.
        let effect = self.resolver.saving(&submission).is_none().then(|| {
            Effect::Saving(crate::resolve::Placeholder {
                id: crate::resolve::Placeholder::id_for(&submission),
                title: crate::resolve::Placeholder::title_for(
                    request["words"].as_str().unwrap_or_default(),
                ),
                machine: self
                    .machines
                    .get(machine)
                    .map(|m| m.name())
                    .unwrap_or_default(),
                session: request["session"].as_str().unwrap_or("default").to_string(),
                source: self.source_of(machine),
                made: None,
            })
        });
        self.resolver
            .add(machine, request, None, Some(key.clone()), effect);
        if self.closed_on_send && matches!(self.mode, Mode::Backlog { .. }) {
            self.backlog_focus = Some(key.clone());
        }
        self.rebuild();
        // Its card can't show where you are (a filter hides it, or its machine has no feed yet):
        // say so instead.
        if !self
            .backlog_cards()
            .iter()
            .any(|&i| self.board.cards[i].key == key)
        {
            self.say("Saving your new intent…", Tone::Info);
        }
    }

    /// What a start or reopen that shows nothing on its card says instead until it's answered.
    fn asking_line(&self, request: &Value) -> Option<String> {
        if !matches!(request["action"].as_str(), Some("dispatch" | "reopen")) {
            return None;
        }
        let reference = self.reference_for(request["id"].as_str()?);
        Some(if request["queue"] == true {
            format!("Queuing {reference}…")
        } else {
            format!("Asking about {reference}…")
        })
    }

    /// Whether a start or reopen can only come back as a question: done work always asks, and so
    /// does work for a session the feed says is at its limit, unless it goes to the background
    /// lane, which has none. Until it does, the card mustn't claim it's starting (intent 0491).
    fn asks_first(&self, key: &str, request: &Value) -> bool {
        if request.get("override").is_some() {
            return false;
        }
        let Some(card) = self.card(key) else {
            return false;
        };
        if request["action"] == "reopen" {
            return card.done();
        }
        let Some(detail) = card.detail.as_ref() else {
            return false;
        };
        if detail.lane.as_deref() == Some("background") {
            return false;
        }
        let home = detail.home.as_ref();
        let machine = home
            .and_then(|h| h.machine.as_deref())
            .map_or(Some(card.machine_index), |name| self.machine_index(name));
        let session = request["session"]
            .as_str()
            .or(home.and_then(|h| h.session.as_deref()));
        let (Some(machine), Some(session)) = (machine, session) else {
            return false;
        };
        self.machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .and_then(|f| f.sessions.get(session))
            .is_some_and(|s| s.limit.is_some_and(|limit| s.used >= limit))
    }

    /// What the board shows for a change while it's on its way: what the backend will make of the
    /// record, as far as Kerdr can tell. A request that waits in a queue shows nothing, nor one
    /// that can only come back as a question (intent 0491).
    pub(super) fn effect_for(&self, key: &str, request: &Value) -> Option<Effect> {
        Some(match request["action"].as_str()? {
            "dispatch" | "reopen" if request["queue"] == true || self.asks_first(key, request) => {
                return None;
            }
            "dispatch" => Effect::Starting,
            "reopen" => Effect::Reopening,
            "close" => Effect::Close,
            "dismiss" | "cancel" => Effect::SetAside,
            "restore" => Effect::Restore,
            "refresh" => Effect::Refresh(chrono::Local::now().date_naive().to_string()),
            // A save of a backlog item's text; its read shows nothing (intent 0414).
            "edit" => Effect::Text(request["text"].as_str()?.to_string()),
            "move" => {
                // As the backend orders it: the card's own group of open backlog work, by rank
                // and then number, the card moved, everything numbered again from 0.
                let card = self.card(key)?;
                let source = key.rsplit_once(':').map(|(s, _)| s)?;
                let mut peers: Vec<&Card> = self
                    .board
                    .cards
                    .iter()
                    .filter(|c| {
                        c.stage == Stage::Backlog
                            && !c.dismissed()
                            && c.owner_key() == card.owner_key()
                            && c.key.rsplit_once(':').map(|(s, _)| s) == Some(source)
                    })
                    .collect();
                let number = |c: &Card| {
                    c.detail
                        .as_ref()
                        .and_then(|d| d.id.parse::<i64>().ok())
                        .unwrap_or(i64::MAX)
                };
                peers.sort_by_key(|c| (c.backlog_rank(), number(c)));
                let at = peers.iter().position(|c| c.key == key)?;
                let to = if request["top"] == true {
                    0
                } else if request["bottom"] == true {
                    peers.len() - 1
                } else {
                    let delta = request["delta"].as_i64()?;
                    (at as i64)
                        .saturating_add(delta)
                        .clamp(0, peers.len() as i64 - 1) as usize
                };
                let moved = peers.remove(at);
                peers.insert(to, moved);
                Effect::Order(
                    peers
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (c.key.clone(), i as i64))
                        .collect(),
                )
            }
            _ => return None,
        })
    }

    /// A change's answer, from the action resolution tool: one that went as shown keeps showing
    /// until the feed has it; any other takes its effect with it, so the card is back as it really
    /// is before anything is said or asked.
    pub fn finish_job(&mut self, n: u64, request: Value, result: Result<Response, String>) {
        let Some(job) = self.resolver.take(n) else {
            return;
        };
        let mut job = job;
        let machine = job.machine;
        self.action_from = job.from.clone();
        self.answer_went_in = job.entered;
        if job.asking
            && let Some(line) = self.asking_line(&job.request)
        {
            self.unsay(&line);
        }
        let mut refused = None;
        if let Ok(response) = &result
            && response.ok
            && !response.confirmation
        {
            // It asked nothing after all (a slot came free since the feed was read): from here it
            // shows as any start or reopen does.
            let guessed_wrong = job.asking && job.key.is_some() && job.request["queue"] != true;
            if guessed_wrong {
                job.effect = Some(if job.request["action"] == "reopen" {
                    Effect::Reopening
                } else {
                    Effect::Starting
                });
            }
            match job.effect.as_mut() {
                // Its card stays until the feed has the record it made; nothing made, it goes.
                Some(Effect::Saving(p)) => {
                    p.made = response.id.clone();
                    if p.made.is_none() {
                        job.effect = None;
                        self.rebuild();
                    }
                }
                // Asked for on another machine, where it starts within minutes: it shows as
                // starting there until that start reaches this copy (intent 0445).
                Some(Effect::Starting)
                    if response.machine.as_deref().is_some_and(|m| {
                        self.machines
                            .get(machine)
                            .is_some_and(|here| !here.answers_to(m))
                    }) =>
                {
                    job.effect = Some(Effect::StartingOn(
                        response.machine.clone().unwrap_or_default(),
                    ));
                }
                _ => {}
            }
            // It stays as shown until the feed has it; a start answered yes moves on now.
            let changes = guessed_wrong
                || matches!(job.effect, Some(Effect::Starting | Effect::StartingOn(_)));
            self.resolver.settle(job);
            if changes {
                self.rebuild();
            }
        } else {
            refused = Some(job);
        }
        let resends = self.resend.len();
        let moved_on = refused
            .as_ref()
            .is_some_and(|j| j.request["action"] == "move");
        let shown = refused.as_ref().is_some_and(|j| j.effect.is_some());
        let redirected = result.as_ref().is_ok_and(Response::sends_on);
        if !redirected {
            if moved_on {
                self.resolver.forget_moves_after(machine, n);
            }
            if shown {
                self.rebuild();
            }
        }
        self.finish_action(machine, request, result);
        // Sent on to the machine its refusal named: back in its place in the queue, still shown,
        // so a later change to the same record can't overtake it (intent 0416).
        if self.resend.len() > resends
            && let Some(job) = refused
            && let Some((to, request, from)) = self.resend.pop_back()
        {
            self.resolver.retarget(job, to, request, from);
        } else if redirected {
            if moved_on {
                self.resolver.forget_moves_after(machine, n);
            }
            if shown {
                self.rebuild();
            }
        }
        // Not asked or taken anywhere: nothing else is owed to that card.
        self.action_from = None;
        self.answer_went_in = false;
    }

    /// Whether this card's record has a start or a reopen on its way, said when so, in words for
    /// the key pressed (`action`): nothing else may change it until that's answered, as the
    /// backend would refuse it.
    pub(crate) fn still_changing(&mut self, card: &Card, action: &str) -> bool {
        let Some(id) = card.detail.as_ref().map(|d| d.id.clone()) else {
            return false;
        };
        let Some(job) = self.resolver.starting(&id) else {
            return false;
        };
        let open = card.is_open_workspace();
        if open {
            // Answered, and the copy shown has its workspace: it's under way, and the usual
            // rules speak (a start that ran on another machine, while this copy catches up).
            if job.settled.is_some() {
                return false;
            }
            // X stops it: its cancel goes once the start has answered, as every change to a
            // record waits for the one before it (intent 0445).
            if action == "cancel" {
                return false;
            }
        }
        let verb = if job.request["action"] == "reopen" {
            "reopening"
        } else {
            "starting up"
        };
        let secs = job.asked.elapsed().as_secs();
        let reference = &card.reference;
        // Nothing has started: a question or a place in a queue is all that can come (intent 0491).
        if job.asking {
            let what = if job.request["queue"] == true {
                "queuing"
            } else {
                "asking about"
            };
            self.say(
                format!("Kerdr is still {what} {reference} ({secs} s so far)."),
                Tone::Info,
            );
            return true;
        }
        let text = if open {
            format!(
                "{reference}'s workspace is open and its agent is still taking its brief ({secs} s so far): ⏎ goes in, and X stops it."
            )
        } else if matches!(action, "enter" | "dispatch" | "reopen") {
            format!(
                "{reference} is {verb} ({secs} s so far); ⏎ goes in as soon as its workspace opens."
            )
        } else {
            format!(
                "{reference} is {verb} ({secs} s so far), so nothing else changes it yet. Once its workspace opens, X stops it and sets it aside."
            )
        };
        self.say(text, Tone::Info);
        true
    }

    /// Whether this is a new intent's card while its words are still being saved, said when so:
    /// there's no record to change yet (intent 0445).
    pub(crate) fn still_saving(&mut self, card: &Card) -> bool {
        if card.intake() != Some(crate::resolve::SAVING) {
            return false;
        }
        self.say(
            "This new intent is still being saved; try again in a moment, once it has its number.",
            Tone::Info,
        );
        true
    }

    /// ⏎ on a card whose start or reopen is on its way and no workspace shows on it yet: into the
    /// workspace a feed names for its record that has opened meanwhile (ark-dispatch opens it
    /// before its agent takes its brief, and answers only then), else says it's starting up and
    /// for how long. None when no
    /// start is on its way (intent 0445).
    pub(crate) fn enter_starting(&mut self, card: &Card) -> Option<Action> {
        let id = card.detail.as_ref()?.id.clone();
        if card.is_open_workspace() {
            return None;
        }
        let job = self.resolver.starting(&id)?;
        // Where it asked, and where a start that ran on another machine runs (intent 0445).
        let mut machines = vec![self.machines.get(job.machine).map(|m| m.name())?];
        if let Some(Effect::StartingOn(there)) = &job.effect
            && let Some(i) = self.machine_index(there)
        {
            machines.push(self.machines[i].name());
        }
        // Only a workspace a feed names for this very record: herdr's names carry two digits of
        // its number, which 0045 and 0145 share, so a name alone never says it's this one's.
        let named: Vec<(String, String, String)> = self
            .machines
            .iter()
            .filter(|m| machines.contains(&m.name()))
            .filter_map(|m| m.feed.as_ref().map(|f| (m.name(), f)))
            .flat_map(|(name, f)| {
                let source = f.source.clone().unwrap_or_else(|| "feed".into());
                f.cards
                    .iter()
                    .filter(move |c| format!("{source}:{}", c.id) == card.key)
                    .filter_map(move |c| {
                        let w = c.workspace.as_ref()?;
                        Some((name.clone(), w.session.clone()?, w.id.clone()?))
                    })
            })
            .collect();
        let opened = self.board.workspaces.iter().find(|w| {
            named
                .iter()
                .any(|(m, s, i)| *m == w.machine && *s == w.session && *i == w.id)
                && !job
                    .before
                    .iter()
                    .any(|(s, i)| *s == w.session && *i == w.id)
        });
        if let Some(w) = opened {
            let action = Action::JumpWorkspace {
                machine: w.machine.clone(),
                session: w.session.clone(),
                id: w.id.clone(),
            };
            // In by his own ⏎: its answer takes him nowhere later.
            self.resolver.went_in(&id);
            return Some(action);
        }
        self.still_changing(card, "enter");
        Some(Action::None)
    }

    /// Going into a card's workspace yourself: a start or reopen still on its way for it neither
    /// asks nor takes you in when it answers, since you're already there (intent 0445).
    pub fn went_into(&mut self, key: &str) {
        if let Some(id) = self
            .card(key)
            .and_then(|c| c.detail.as_ref())
            .map(|d| d.id.clone())
        {
            self.resolver.went_in(&id);
        }
    }

    /// The same, going into a workspace by its id: the card it's for, if any.
    pub fn went_into_workspace(&mut self, machine: usize, session: &str, id: &str) {
        let Some(name) = self.machines.get(machine).map(|m| m.name()) else {
            return;
        };
        let keys: Vec<String> = self
            .board
            .workspaces
            .iter()
            .filter(|w| w.machine == name && w.session == session && w.id == id)
            .filter_map(|w| w.card.clone())
            .chain(
                self.board
                    .cards
                    .iter()
                    .filter(|c| {
                        c.machine_index == machine
                            && c.session.as_deref() == Some(session)
                            && c.workspace_id.as_deref() == Some(id)
                    })
                    .map(|c| c.key.clone()),
            )
            .collect();
        for key in keys {
            self.went_into(&key);
        }
    }

    /// The runner has started a change's command: a start that waited for its write-up shows
    /// from now, and one Esc dropped a moment too late says it carries on (review of intent 0445).
    pub fn job_started(&mut self, n: u64) {
        let started = self.resolver.started(n);
        if started.too_late {
            let reference = self
                .resolver
                .job(n)
                .and_then(|j| j.id())
                .map(|id| self.reference_for(id))
                .unwrap_or_else(|| "The waiting card".into());
            self.say(
                format!("{reference} had already begun starting when Esc came, so it carries on."),
                Tone::Info,
            );
        }
        if started.shows_now || started.too_late {
            self.rebuild();
        }
    }

    /// Whether a start d asked for waits for this card's write-up.
    pub fn starts_when_written(&self, key: &str) -> bool {
        self.resolver.waiting_for_write_up(key).is_some()
    }

    /// What a card's own screen and its peek say while a change to it is on its way: "◐ Starting
    /// up · 12 s so far · ⏎ goes in as soon as its workspace opens", "◐ Marking it done… 2 s"
    /// (intent 0445).
    pub fn starting_line(&self, card: &Card) -> Option<String> {
        let id = card.detail.as_ref()?.id.as_str();
        if card.intake() == Some(crate::resolve::SAVING) {
            return Some("Saving your new intent… it gets its number in a moment.".into());
        }
        if self.starts_when_written(&card.key) {
            return Some(
                "Being written up from your words; it starts as soon as that's done.".into(),
            );
        }
        let Some(job) = self.resolver.starting(id) else {
            // Any other change still on its way: shown as done, but not yet saved.
            let job = self.resolver.unanswered_on(&card.key)?;
            let secs = job.asked.elapsed().as_secs();
            let what = match job.request["action"].as_str() {
                Some("close") => "Marking it done",
                Some("dismiss") => "Setting it aside",
                Some("cancel") => "Setting it aside: stopping its agent",
                Some("restore") => "Bringing it back",
                Some("refresh") => "Saving that you still want it",
                Some("move") => "Moving it",
                Some("edit") => "Saving its new text",
                _ => "Saving the change",
            };
            return Some(format!("◐ {what}… {secs} s"));
        };
        if card.is_open_workspace() {
            return None;
        }
        let secs = job.asked.elapsed().as_secs();
        let what = match &job.effect {
            Some(crate::resolve::Effect::StartingOn(m)) => format!("Starting up on {m}"),
            _ if job.request["action"] == "reopen" => "Reopening".to_string(),
            _ => "Starting up".to_string(),
        };
        Some(format!(
            "◐ {what} · {secs} s so far · ⏎ goes in as soon as its workspace opens"
        ))
    }

    /// What holds Kerdr open on q, said: a change not yet sent, which would never be, or a new
    /// intent whose words may still be on their way to its command (intent 0445).
    pub fn quit_held(&self) -> Option<String> {
        let held = self.resolver.holding_quit();
        if held.is_empty() {
            return None;
        }
        if held.iter().any(|j| j.request["action"] == "create") {
            return Some(
                "Your new intent is still being saved. Wait a moment before quitting.".into(),
            );
        }
        let first = held[0];
        let reference = first
            .id()
            .map(|id| self.reference_for(id))
            .unwrap_or_else(|| "A card".into());
        // Started, but its command doesn't have all of it yet: quitting now would cut it off.
        if first.running.is_some() {
            return Some(format!(
                "{reference}'s change is still being handed to its command. Wait a moment before quitting."
            ));
        }
        Some(format!(
            "{reference}'s change hasn't been sent yet; it goes once the one before it answers. Wait a moment before quitting."
        ))
    }

    /// What quitting would drop that Kerdr alone keeps, for q to say first, a second q quitting:
    /// a start waiting for its write-up, which Kerdr sends; words waiting for their image; words
    /// a failed save gave back, or written and not yet sent (intent 0445); your version of an
    /// item's text, kept until a save of it lands (intent 0414). Each is named.
    pub fn quit_loses(&self) -> Option<String> {
        let mut lost = vec![];
        let awaited = self.resolver.write_ups_awaited();
        if let Some(first) = awaited.first() {
            let reference = first
                .id()
                .map(|id| self.reference_for(id))
                .unwrap_or_else(|| "A card".into());
            let more = match awaited.len() {
                1 => String::new(),
                n => format!(" ({} more like it)", n - 1),
            };
            lost.push(format!(
                "{reference} starts once it's written up{more}, and only Kerdr sends that start: quitting drops it."
            ));
        }
        if self.send_after_load.is_some() {
            lost.push(
                "Your new intent is sent once its image is in, in a moment: quitting drops it."
                    .into(),
            );
        }
        if !self.returned.is_empty() || !self.draft.trim().is_empty() {
            lost.push(
                "Words for a new intent are kept only while Kerdr is open (c brings them back): quitting drops them."
                    .into(),
            );
        }
        lost.extend(self.edits_lost());
        (!lost.is_empty()).then(|| lost.join(" "))
    }

    /// What's still running and carries on without Kerdr, for q to say before it quits: "#41 is
    /// still starting; its command has the whole request and carries on after Kerdr quits." None
    /// when nothing is.
    pub fn still_running(&self) -> Option<String> {
        let running = self.resolver.running();
        let first = running.first()?;
        let reference = first
            .id()
            .map(|id| self.reference_for(id))
            .unwrap_or_else(|| "A change".into());
        let what = match first.request["action"].as_str() {
            Some("dispatch") => "is still starting",
            Some("reopen") => "is still reopening",
            _ => "is still being saved",
        };
        let more = match running.len() {
            1 => String::new(),
            n => format!(" ({} more running)", n - 1),
        };
        Some(format!(
            "{reference} {what}{more}; its command has the whole request and carries on after Kerdr quits."
        ))
    }

    /// How a failure names the change that didn't happen: "#41 wasn't marked done".
    fn what_failed(&self, request: &Value) -> Option<String> {
        let id = request["id"].as_str()?;
        let reference = self.reference_for(id);
        Some(match request["action"].as_str()? {
            "dispatch" => format!("{reference} didn't start"),
            "reopen" => format!("{reference} didn't reopen"),
            "close" => format!("{reference} wasn't marked done"),
            "dismiss" => format!("{reference} wasn't set aside"),
            "cancel" => format!("{reference} wasn't cancelled"),
            "restore" => format!("{reference} wasn't brought back"),
            "refresh" => format!("{reference} wasn't refreshed"),
            "move" => format!("{reference} didn't move"),
            _ => format!("{reference} didn't change"),
        })
    }

    pub fn finish_action(
        &mut self,
        machine: usize,
        request: Value,
        result: Result<Response, String>,
    ) {
        self.answer_arrives(machine, &request, &result);
        self.take_answer(machine, request, result);
        self.answer_done();
    }

    fn take_answer(&mut self, machine: usize, request: Value, result: Result<Response, String>) {
        if request["action"]
            .as_str()
            .is_some_and(|a| a.starts_with("account-"))
        {
            self.busy = false;
            self.busy_id = None;
            self.busy_since = None;
            self.busy_label.clear();
            self.finish_account_action(machine, request, result);
            return;
        }
        self.answered_at = Some(Instant::now());
        if request["action"] == "create" {
            return self.create_answered(machine, &request, result);
        }
        let from = self.action_from.take();
        let id = request["id"].as_str().map(str::to_string);
        // Only this answer's own resend: another record's, still on its way, stays guarded.
        let was_resent = match self
            .resent
            .iter()
            .position(|(m, r)| *m == machine && Some(r) == id.as_ref())
        {
            Some(i) => {
                self.resent.remove(i);
                true
            }
            None => false,
        };
        // Refused because the intent lives on another machine than the one asked (a copy that
        // was behind named the wrong home): the same request goes there, once, and its actions
        // go there from now on. When it can't go, Kerdr says why in its own words, since the
        // backend's "⏎ again opens it there" holds only when Kerdr sends it on (intent 0411).
        if let Ok(response) = &result
            && !response.ok
            && !response.confirmation
            && let Some(id) = id
            && let Some(home) = response.home.as_deref()
        {
            let (host, session) = home.split_once('/').unwrap_or((home, ""));
            let reference = self.reference_for(&id);
            let asked = self
                .machines
                .get(machine)
                .map(|m| m.name())
                .unwrap_or_default();
            // A save of an item's text that can't go on: his version stays (intent 0414).
            let kept = self.edit_kept(&request);
            match self.machine_index(host) {
                None => {
                    self.fail(
                        format!("{reference} lives on {host} now, which isn't one of this board's machines, so Kerdr can't change it there.{kept}"),
                    );
                    return;
                }
                Some(to) if to == machine => {}
                Some(_) if was_resent => {
                    self.moved_home.retain(|(i, ..)| *i != id);
                    self.fail(
                        format!("{reference}'s copies on {asked} and {host} disagree about where it lives; one is behind. Wait a minute for it to catch up, then try again.{kept}"),
                    );
                    return;
                }
                Some(to) => {
                    let old_home = self
                        .board
                        .cards
                        .iter()
                        .filter_map(|c| c.detail.as_ref())
                        .find(|d| d.id == id)
                        .and_then(|d| d.home.as_ref())
                        .and_then(|h| h.machine.clone());
                    self.moved_home.retain(|(i, ..)| *i != id);
                    self.moved_home.push((id.clone(), to, old_home));
                    let name = self.machines[to].name();
                    if !self.machines[to].conn.is_live() {
                        self.fail(
                            format!("{reference} lives on {name} now, and {name} isn't answering; nothing changed. Try again once it's back.{kept}"),
                        );
                        return;
                    }
                    let mut request = request;
                    if request["action"] == "reopen" && !session.is_empty() {
                        request["session"] = json!(session);
                    }
                    // A save's text came back only counted: it goes again as he wrote it.
                    if request["action"] == "edit" {
                        self.edit_resend(&mut request);
                    }
                    // Sent on quietly: the card still shows the change while it goes there.
                    self.resent.push((to, id));
                    self.resend.push_back((to, request, from));
                    return;
                }
            }
        }
        if request["action"] == "edit" {
            return self.finish_edit(machine, request, result, from);
        }
        if request["action"] == "kickoff" {
            return self.finish_kickoff_check(machine, &request, result);
        }
        // A question, or after a yes a new one: something changed, so it carries a new token.
        let asks = |response: &Response| {
            (response.confirmation || !response.questions.is_empty())
                && matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
                && (request["override"] != true
                    || response
                        .confirm
                        .as_deref()
                        .is_some_and(|t| request["confirm"].as_str() != Some(t)))
        };
        match result {
            Ok(response) if asks(&response) => {
                let id = request["id"].as_str().unwrap_or("this intent").to_string();
                let card = from.as_ref().and_then(|f| self.card(&f.key)).cloned();
                let reference = card
                    .as_ref()
                    .map(|c| c.reference.clone())
                    .unwrap_or_else(|| self.reference_for(&id));
                // Only while you're still on that work: a question that pops up over typing would
                // take the next key for its answer.
                if !from.as_ref().is_some_and(|f| self.still_on(f, None)) {
                    let key = if request["action"] == "reopen" {
                        "⏎"
                    } else {
                        "d"
                    };
                    if !response.questions.is_empty() {
                        self.fail(format!(
                            "{reference} didn't start: it has kickoff questions for you first, and {key} on it again shows them."
                        ));
                        return;
                    }
                    self.fail(format!(
                        "{reference} didn't {}: it needs a yes first, and {key} on it again asks.",
                        if request["action"] == "reopen" {
                            "reopen"
                        } else {
                            "start"
                        }
                    ));
                    return;
                }
                let title = card.as_ref().map(|c| c.title.clone()).unwrap_or_default();
                let kept = self
                    .kickoff_kept
                    .get(request["id"].as_str().unwrap_or_default())
                    .cloned()
                    .unwrap_or_default();
                if let Some(form) = Kickoff::from_answer(
                    machine,
                    &request,
                    &response,
                    reference.clone(),
                    title,
                    &kept,
                ) {
                    self.mode = Mode::Kickoff {
                        form,
                        back: Box::new(self.mode.clone()),
                        target: from.map(Box::new),
                    };
                    return;
                }
                let mut request = request;
                request.as_object_mut().map(|o| o.remove("override"));
                // The yes goes back for what was asked: the session it named and its token.
                if let Some(session) = &response.session {
                    request["session"] = json!(session);
                }
                if let Some(token) = &response.confirm {
                    request["confirm"] = json!(token);
                }
                let session = response.session.as_deref().unwrap_or("its home session");
                let (is_reopen, full) = (request["action"] == "reopen", response.used.is_some());
                // A reopen's answer says every reason itself: a copy open elsewhere, a full session.
                // At the most a session holds, a yes can't open it there: w, its queue, is the
                // answer the question offers, so its title says so.
                let at_most = is_reopen
                    && response.used.is_some_and(|used| {
                        self.machines
                            .get(machine)
                            .and_then(|m| m.feed.as_ref())
                            .and_then(|f| f.sessions.get(session))
                            .and_then(|s| s.max)
                            .is_some_and(|most| used >= most)
                    });
                let (title, message) = if at_most {
                    (format!("Queue {reference} in {session}?"), response.message)
                } else if request["action"] == "reopen" {
                    (
                        format!("Reopen {reference} in {session}?"),
                        response.message,
                    )
                } else {
                    let message = match (response.used, response.limit) {
                        (Some(used), Some(limit)) => format!(
                            "{used} active workspaces; the usual limit is {limit}. {}",
                            response.message
                        ),
                        _ => response.message,
                    };
                    (format!("Start {id} in {session}?"), message)
                };
                self.mode = Mode::Confirm {
                    machine,
                    request,
                    title,
                    message,
                    back: Box::new(self.mode.clone()),
                    target: from.map(Box::new),
                    queue: is_reopen && full,
                    ready_at: Some(Instant::now() + Duration::from_millis(500)),
                    plain: false,
                };
            }
            Ok(response) => {
                let mut message = response.message.clone();
                // What a change that went as shown still says: only what you'd need to find it.
                let mut note = None;
                if request["action"] == "dispatch" && response.ok {
                    self.started = request["id"]
                        .as_str()
                        .map(|id| (id.to_string(), Instant::now()));
                }
                // A start asked for on another machine says where (intent 0445).
                if request["action"] == "dispatch"
                    && response.ok
                    && response.machine.as_deref().is_some_and(|m| {
                        self.machines
                            .get(machine)
                            .is_some_and(|here| !here.answers_to(m))
                    })
                {
                    note = Some(message.clone());
                }
                // A reopen queued in a full session (w) says where it waits.
                if request["action"] == "reopen" && request["queue"] == true && response.ok {
                    note = Some(message.clone());
                }
                // A reopen takes you in only while you're still on its card; otherwise it says so.
                if request["action"] == "reopen"
                    && response.ok
                    && let (Some(session), Some(workspace)) =
                        (response.session.clone(), response.workspace.clone())
                {
                    // The board may already show the workspace it just made: that's still the card.
                    match from
                        .filter(|f| self.still_on(f, Some(&workspace)))
                        .map(|f| f.key)
                    {
                        Some(from) => {
                            self.open_after = Some(OpenAfter {
                                machine,
                                label: response.label.clone().unwrap_or_else(|| workspace.clone()),
                                session,
                                workspace,
                                tab: response.tab.clone(),
                                from,
                            })
                        }
                        // He went in himself while it was on its way: nothing to say.
                        None if self.answer_went_in => {}
                        None => {
                            message.push_str(" ⏎ on its card goes in.");
                            note = Some(message.clone());
                        }
                    }
                }
                // Answers kept on leaving the Kickoff Questions box: how many are left.
                if request["action"] == "answer" && response.ok {
                    note = Some(message.clone());
                }
                // Started: its answers are saved, and what was kept of them is done with.
                if request["action"] == "dispatch"
                    && response.ok
                    && let Some(id) = request["id"].as_str()
                {
                    self.kickoff_kept.remove(id);
                }
                if !response.ok {
                    // The card is back as it really is; say what didn't happen, and why.
                    let what = self.what_failed(&request);
                    let kept = self.kickoff_kept_note(&request);
                    self.fail(short(match what {
                        Some(what) => format!("{what}: {message}{kept}"),
                        None => format!("{message}{kept}"),
                    }));
                } else if let Some(note) = note {
                    // Done as shown: only what you'd need to find it is said.
                    self.say(short(note), Tone::Info);
                }
            }
            Err(error) => {
                // Its result may be unknown (a timeout): the error says so in its own words.
                let kept = self.kickoff_kept_note(&request);
                match request["id"].as_str() {
                    Some(id) => {
                        let reference = self.reference_for(id);
                        self.fail(short(format!("{reference}: {error}{kept}")))
                    }
                    None => self.fail(short(error)),
                }
            }
        }
    }

    /// A new intent's answer. Made: its words are never sent again, and its card takes the place
    /// of the one shown while it saved. Not made, or unknown: its words come back for c, and a
    /// resend of them keeps its submission when it may have been made (intent 0445).
    fn create_answered(
        &mut self,
        machine: usize,
        request: &Value,
        result: Result<Response, String>,
    ) {
        self.action_from = None;
        let submission = request["submission"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let at = self.submissions.iter().position(|s| s.id == submission);
        let words = match at {
            Some(i) => (
                self.submissions[i].words.clone(),
                self.submissions[i].images.clone(),
            ),
            None => (
                request["words"].as_str().unwrap_or_default().to_string(),
                vec![],
            ),
        };
        let shown = self.placeholder_key(machine, &submission);
        // On its card: the one shown while it saved, or the record's, which may have taken its
        // place before this answer came (intent 0445).
        let on_it = [self.backlog_focus.as_deref(), self.sel.key.as_deref()]
            .into_iter()
            .flatten()
            .any(|k| {
                k == shown
                    || self
                        .card(k)
                        .and_then(|c| c.detail.as_ref())
                        .is_some_and(|d| d.submission.as_deref() == Some(submission.as_str()))
            });
        let back = |into_box: bool| {
            if into_box {
                "c opens it again with your words".to_string()
            } else {
                "its words are kept, and c brings them back once the box is empty".to_string()
            }
        };
        match result {
            // A failure that names an intent still made one (saved here but not pushed, say), so
            // its words must not be sent again.
            Ok(response) if response.ok || response.id.is_some() => {
                if let Some(i) = at {
                    self.submissions.remove(i);
                }
                // Words an earlier send of it gave back are saved now: never offered again.
                self.returned
                    .retain(|r| r.from.as_ref().is_none_or(|s| s.id != submission));
                if self
                    .draft_came_back
                    .as_ref()
                    .is_some_and(|s| s.id == submission && s.words == self.draft.trim())
                {
                    self.clear_kept_draft();
                }
                if on_it {
                    self.new_intent_id = response.id.clone().map(|id| (machine, id));
                }
                if !response.ok {
                    return self.fail(short(response.message));
                }
                let mut message = response.message;
                if self.filtering() {
                    message.push_str(
                        " A filter may hide it until it's written up: F shows everything.",
                    );
                } else if on_it && matches!(self.mode, Mode::Backlog { .. }) {
                    // Saved as shown: its card says so.
                    return;
                } else {
                    message.push_str(" b shows it in the backlog.");
                }
                self.say(short(message), Tone::Info);
            }
            Ok(response) => {
                // Nothing made. Settled, unless an earlier send of these words went unheard.
                let unheard = at
                    .map(|i| self.submissions[i].clone())
                    .filter(|s| s.unknown);
                if let Some(i) = at.filter(|_| unheard.is_none()) {
                    self.submissions.remove(i);
                }
                let into_box = self.give_back(words.0, words.1, unheard);
                self.fail(short(format!(
                    "Your new intent wasn't saved: {} {}.",
                    response.message,
                    back(into_box)
                )));
            }
            Err(error) => {
                let unheard = at.map(|i| {
                    self.submissions[i].unknown = true;
                    self.submissions[i].clone()
                });
                let into_box = self.give_back(words.0, words.1, unheard);
                self.fail(format!(
                    "Your new intent may not have been saved ({error}). {}; sending them again never makes a second one.",
                    back(into_box)
                ));
            }
        }
    }

    /// Whether a confirmed change still fits the card it asked about, as it was: a refresh while
    /// the question was open may have started, finished, restored or rehomed it.
    pub(super) fn confirm_still_applies(
        &self,
        machine: usize,
        request: &Value,
        target: Option<&CardSnapshot>,
    ) -> bool {
        let Some(id) = request["id"].as_str() else {
            return false;
        };
        let name = self.machines.get(machine).map(|m| m.name());
        // Asked where a refusal said it lives now, though the board still shows the old home.
        self.board.cards.iter().any(|c| {
            target.is_none_or(|t| c.key == t.key)
                && c.detail.as_ref().is_some_and(|d| {
                    d.id == id
                        && (self.moved_target(c) == Some(machine)
                            || c.machine_index == machine
                            || d.home.as_ref().and_then(|h| h.machine.as_deref())
                                == name.as_deref())
                })
                && match request["action"].as_str() {
                    Some("restore") => c.dismissed(),
                    Some("reopen") => {
                        (c.under_way() || c.done()) && target.is_some_and(|t| t.same_work(c))
                    }
                    // Started where it was asked: its home hasn't moved since.
                    Some("dispatch") => waiting_work(c) && target.is_none_or(|t| t.same_work(c)),
                    // Still open with no workspace, the same work (intent 0289).
                    Some("close") => c.closable() && target.is_some_and(|t| t.same_work(c)),
                    // Still the same work, open where the yes was given (intent 0321).
                    Some("cancel") => {
                        c.cancellable()
                            && target.is_some_and(|t| t.same_work(c))
                            && c.session.as_deref() == request["session"].as_str()
                            && c.workspace_id.as_deref() == request["workspace"].as_str()
                            && c.tab_id.as_deref() == request["tab"].as_str()
                    }
                    // Started work set aside only while it's still that, with no workspace;
                    // backlog work only while it still waits.
                    Some("dismiss") => match target {
                        Some(t) if !t.waiting => c.closable() && !waiting_work(c) && t.same_work(c),
                        _ => waiting_work(c) && target.is_none_or(|t| t.same_work(c)),
                    },
                    _ => waiting_work(c),
                }
        })
    }
}
