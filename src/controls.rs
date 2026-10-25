//! Intent controls live apart from the navigation keys. Writes go through the configured
//! adapter, on the record's home machine; the board never edits canonical intent files.

use super::*;
use crate::actions::Response;
use crate::model::Summary;
use serde_json::{Value, json};

/// The most the new-intent box holds, kept under the actions' 64 KiB request limit.
const MAX_WORDS: usize = 48_000;

/// Work waiting to start: the only cards d, y, x, J, K and t change, so the bottom line, the
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
            Some("claude") => "Claude".into(),
            Some("codex") => "Codex".into(),
            Some(other) => other.into(),
        }
    }

    /// Metadata filters without the selected tab, so other session badges stay meaningful.
    pub(crate) fn card_matches_filters(&self, card: &Card) -> bool {
        self.owner.as_ref().is_none_or(|o| *o == card.owner_key())
            && self.project.as_ref().is_none_or(|p| p == card.project())
            && self
                .provider
                .as_ref()
                .is_none_or(|provider| card.agents.iter().any(|a| &a.agent == provider))
            && card.dismissed() == self.wont_do
    }

    pub fn card_visible(&self, card: &Card) -> bool {
        self.scope().contains(card) && self.card_matches_filters(card)
    }

    fn cycle_provider(&mut self) {
        let mut others: Vec<_> = self
            .board
            .cards
            .iter()
            .flat_map(|card| card.agents.iter())
            .map(|agent| agent.agent.clone())
            .filter(|name| !name.is_empty() && name != "claude" && name != "codex")
            .collect();
        others.sort();
        others.dedup();
        let providers: Vec<String> = ["claude".to_string(), "codex".to_string()]
            .into_iter()
            .chain(others)
            .collect();
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
        self.wont_do = wont_do;
        self.mode = Mode::Backlog { sel: 0 };
        self.backlog_focus = self
            .backlog_cards()
            .first()
            .map(|&i| self.board.cards[i].key.clone());
        self.fix_selection();
    }

    /// The filter list (f): every owner on the board, each with its projects, and how much of
    /// each is under way or waiting. Counts ignore the owner and project filters, so they say
    /// what picking a row would show.
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
        let sel = self
            .filter_rows()
            .iter()
            .position(|r| r.owner == self.owner && r.project == self.project)
            .unwrap_or(0);
        self.mode = Mode::Filter {
            query: String::new(),
            sel,
            back: Box::new(self.mode.clone()),
        };
    }

    fn set_filter(&mut self, owner: Option<String>, project: Option<String>) {
        self.owner = owner;
        self.project = project;
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
    }

    /// F: every owner, project and agent again.
    pub(super) fn clear_filters(&mut self) {
        let filtered = self.owner.is_some() || self.project.is_some() || self.provider.is_some();
        self.provider = None;
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
        back: Box<Mode>,
    ) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.filter_matches(&query).len();
        match key.code {
            KeyCode::Esc => {
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
            KeyCode::Backspace => {
                query.pop();
                sel = 0;
            }
            KeyCode::Char(c) if !ctrl && query.len() < 64 => {
                query.push(c);
                sel = 0;
            }
            _ => {}
        }
        let n = self.filter_matches(&query).len();
        let sel = sel.min(n.saturating_sub(1));
        self.mode = Mode::Filter { query, sel, back };
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

    fn selected_for_control(&self) -> Option<&Card> {
        match &self.mode {
            Mode::Backlog { sel } => self
                .backlog_cards()
                .get(*sel)
                .and_then(|&i| self.board.cards.get(i)),
            _ => self.selected(),
        }
    }

    fn can_write(&mut self, machine: usize) -> bool {
        if self.busy {
            self.say(
                "An action is still running. Wait for its result before another change.",
                Tone::Info,
            );
            return false;
        }
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

    fn emit_mutation(&mut self, machine: usize, request: Value) -> Action {
        if self.can_write(machine) {
            Action::Mutate { machine, request }
        } else {
            Action::None
        }
    }

    fn intent_action(&mut self, action: &str, extra: Value) -> Action {
        let Some(card) = self.selected_for_control().cloned() else {
            self.say("Select an intent first.", Tone::Info);
            return Action::None;
        };
        let Some(detail) = &card.detail else {
            self.say("This workspace has no intent record to change.", Tone::Info);
            return Action::None;
        };
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
        if !card.dismissed() && !waiting_work(&card) {
            self.say(
                format!(
                    "{} is already under way or done. d, y, x, J, K and t change backlog work.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        if card.dismissed() && action != "restore" {
            self.say(
                "Restore this intent before starting or reordering it.",
                Tone::Info,
            );
            return Action::None;
        }
        let target = detail.home.as_ref().and_then(|h| h.machine.as_deref());
        let machine = match target {
            Some(name) => match self.machine_index(name) {
                Some(i) => i,
                None => {
                    self.say(format!("The home machine {name} is not configured. Add it before changing this intent."), Tone::Warn);
                    return Action::None;
                }
            },
            None => card.machine_index,
        };
        if !self.can_write(machine) {
            return Action::None;
        }
        let mut request = json!({"action":action,"id":detail.id});
        if let Some(obj) = extra.as_object() {
            request.as_object_mut().unwrap().extend(obj.clone());
        }
        if action == "dismiss" {
            self.mode = Mode::Confirm {
                machine, request, title: format!("Won't do {}?", card.reference),
                message: "Keep the intent in Won't do, outside the active backlog. You can restore it later.".into(),
                back: Box::new(self.mode.clone()),
            };
            Action::None
        } else {
            Action::Mutate { machine, request }
        }
    }

    fn new_intent(&mut self) {
        if self.busy {
            self.say(
                "Wait for the current intent change before creating another.",
                Tone::Info,
            );
            return;
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
            KeyCode::Char('d' | 'y' | 'x' | 'u' | 't' | 'J' | 'K')
        );
        // A refresh while the menu was open may have moved or removed the card it opened on;
        // its keys then act on nothing rather than on whatever took its place.
        if card_key && self.backlog_card_for_keys().map(|c| &c.key) != target.as_ref() {
            self.say("The selected intent changed while the menu was open. Select it again before changing it.", Tone::Warn);
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {}
            KeyCode::Char('c') => self.new_intent(),
            KeyCode::Char(c @ ('d' | 'y' | 'x' | 'u' | 't' | 'J' | 'K')) => {
                return self.card_action(c, 1);
            }
            KeyCode::Char('b') => self.enter_backlog(false),
            KeyCode::Char('w') => self.enter_backlog(!self.wont_do),
            KeyCode::Char('i') => self.enter_misc(),
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
    fn card_action(&mut self, key: char, count: i64) -> Action {
        match key {
            'd' => self.intent_action("dispatch", json!({})),
            'y' => self.intent_action("refresh", json!({})),
            'x' => self.intent_action("dismiss", json!({})),
            'u' => self.intent_action("restore", json!({})),
            't' => self.intent_action("move", json!({"top":true})),
            'J' => self.intent_action("move", json!({"delta":count})),
            'K' => self.intent_action("move", json!({"delta":-count})),
            _ => Action::None,
        }
    }

    /// The new-intent box: Andrew's own words, nothing else. Enter sends them; Shift+Enter,
    /// Alt+Enter or Ctrl+J starts a new line; Esc keeps them for the next time it opens.
    fn compose_key(&mut self, key: KeyEvent, mut draft: Draft, back: Box<Mode>) -> Action {
        if key.code == KeyCode::Esc {
            self.draft = draft.words;
            self.mode = *back;
            return Action::None;
        }
        if self.busy {
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
                } else {
                    let words = draft.words.trim().to_string();
                    let same = self.submission.as_ref().is_some_and(|s| {
                        s.words == words && s.machine == draft.machine && s.session == draft.session
                    });
                    if !same {
                        self.submission = Some(Submission {
                            id: new_submission(),
                            words: words.clone(),
                            machine: draft.machine,
                            session: draft.session.clone(),
                            unknown: false,
                        });
                    }
                    let id = self.submission.as_ref().map(|s| s.id.clone());
                    let request = json!({"action":"create","words":words,"session":draft.session,"submission":id});
                    let machine = draft.machine;
                    self.mode = Mode::Compose { draft, back };
                    return self.emit_mutation(machine, request);
                }
            }
            KeyCode::Backspace => {
                draft.words.pop();
            }
            KeyCode::Char('u') if ctrl => draft.words.clear(),
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

    /// Pasted text arrives whole, line breaks and all, so a pasted paragraph never sends itself.
    pub fn on_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let busy = self.busy;
        match &mut self.mode {
            Mode::Compose { draft, .. } if !busy => {
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
            Mode::Filter { query, sel, .. } => {
                query.push_str(&text.replace('\n', " "));
                *sel = 0;
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
    }

    pub(super) fn control_key(&mut self, key: KeyEvent) -> Option<Action> {
        match self.mode.clone() {
            Mode::Compose { draft, back } => return Some(self.compose_key(key, draft, back)),
            Mode::Filter { query, sel, back } => {
                return Some(self.filter_key(key, query, sel, back));
            }
            Mode::Confirm {
                machine,
                mut request,
                back,
                ..
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                        self.mode = *back;
                    }
                    KeyCode::Enter | KeyCode::Char('y') => {
                        self.mode = *back;
                        if !self.confirm_still_applies(machine, &request) {
                            self.say(
                                "That intent changed while you were deciding, so nothing was sent. Select it again.",
                                Tone::Warn,
                            );
                            return Some(Action::None);
                        }
                        if request["action"] == "dispatch" {
                            request["override"] = json!(true);
                        }
                        return Some(self.emit_mutation(machine, request));
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
                Mode::Board | Mode::Peek { .. } | Mode::Backlog { .. } | Mode::Misc { .. }
            )
        {
            let target = self.backlog_card_for_keys().map(|c| c.key.clone());
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
            Mode::Board | Mode::Backlog { .. } | Mode::Misc { .. }
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
            KeyCode::Char(c @ ('d' | 'y' | 'x' | 'u' | 't' | 'J' | 'K')) => {
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

    pub fn finish_action(
        &mut self,
        machine: usize,
        request: Value,
        result: Result<Response, String>,
    ) {
        self.busy = false;
        match result {
            Ok(response)
                if response.confirmation
                    && request["action"] == "dispatch"
                    && request["override"] != true =>
            {
                let title = format!(
                    "Start {} in {}?",
                    request["id"].as_str().unwrap_or("this intent"),
                    response.session.as_deref().unwrap_or("its home session")
                );
                let message = match (response.used, response.limit) {
                    (Some(used), Some(limit)) => format!(
                        "{used} active workspaces; the usual limit is {limit}. {}",
                        response.message
                    ),
                    _ => response.message,
                };
                self.mode = Mode::Confirm {
                    machine,
                    request,
                    title,
                    message,
                    back: Box::new(self.mode.clone()),
                };
            }
            Ok(response) => {
                let mut message = response.message.clone();
                if request["action"] == "create"
                    && (response.id.is_some()
                        || !self.submission.as_ref().is_some_and(|s| s.unknown))
                {
                    // Settled: the record is named, or nothing was made and no earlier send of
                    // these words went unheard. The next words get an id of their own.
                    self.submission = None;
                }
                // A failure that names an intent still made one (saved here but not pushed,
                // say), so its words must not be sent again.
                if request["action"] == "create" && (response.ok || response.id.is_some()) {
                    self.draft.clear();
                    self.new_intent_id = response.id.clone().map(|id| (machine, id));
                    self.enter_backlog(false);
                    if self.owner.is_some() || self.project.is_some() || self.provider.is_some() {
                        message.push_str(
                            " A filter may hide it until it's written up: F shows everything.",
                        );
                    }
                }
                self.say(message, if response.ok { Tone::Good } else { Tone::Warn });
            }
            Err(error) => {
                if request["action"] == "create"
                    && let Some(s) = self.submission.as_mut()
                {
                    s.unknown = true;
                }
                self.say(error, Tone::Warn)
            }
        }
    }

    /// Whether a confirmed change still fits its record: a refresh while the question was open
    /// may have started, finished or restored it.
    fn confirm_still_applies(&self, machine: usize, request: &Value) -> bool {
        let Some(id) = request["id"].as_str() else {
            return false;
        };
        let name = self.machines.get(machine).map(|m| m.name());
        self.board.cards.iter().any(|c| {
            c.detail.as_ref().is_some_and(|d| {
                d.id == id
                    && (c.machine_index == machine
                        || d.home.as_ref().and_then(|h| h.machine.as_deref()) == name.as_deref())
            }) && match request["action"].as_str() {
                Some("restore") => c.dismissed(),
                _ => waiting_work(c),
            }
        })
    }
}
