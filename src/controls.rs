//! Intent controls live apart from the navigation keys. Writes go through the configured
//! adapter, on the record's home machine; the board never edits canonical intent files.

use super::*;
use crate::actions::Response;
use crate::model::Summary;
use serde_json::{Value, json};

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

    fn cycle_owner(&mut self) {
        let mut owners: Vec<_> = self.board.cards.iter().map(Card::owner_key).collect();
        owners.sort();
        owners.dedup();
        let pos = self
            .owner
            .as_ref()
            .and_then(|o| owners.iter().position(|v| v == o));
        self.owner = match pos {
            Some(i) => owners.get(i + 1).cloned(),
            None => owners.first().cloned(),
        };
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
    }

    fn cycle_project(&mut self) {
        let mut projects: Vec<_> = self
            .board
            .cards
            .iter()
            .filter(|c| self.owner.as_ref().is_none_or(|o| *o == c.owner_key()))
            .map(|c| c.project().to_string())
            .collect();
        projects.sort();
        projects.dedup();
        let pos = self
            .project
            .as_ref()
            .and_then(|o| projects.iter().position(|v| v == o));
        self.project = match pos {
            Some(i) => projects.get(i + 1).cloned(),
            None => projects.first().cloned(),
        };
        self.backlog_focus = None;
        set_backlog_selection(&mut self.mode, 0);
        self.fix_selection();
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
        if !card.dismissed()
            && (card.is_open_workspace()
                || matches!(
                    card.state.as_deref(),
                    Some("closed" | "active" | "background")
                ))
        {
            self.say("This work is already active or finished. Backlog controls apply to work waiting to start.", Tone::Info);
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
        let machine = self
            .machine
            .as_deref()
            .and_then(|m| self.machine_index(m))
            .unwrap_or(0);
        let organization = self
            .owner
            .as_deref()
            .and_then(|o| o.strip_prefix("org:"))
            .unwrap_or("")
            .to_string();
        let tenant = self
            .owner
            .as_deref()
            .and_then(|o| o.strip_prefix("tenant:"))
            .unwrap_or("")
            .to_string();
        let organization = if organization.is_empty() && !tenant.is_empty() {
            self.board
                .cards
                .iter()
                .find(|c| c.owner_key() == format!("tenant:{tenant}"))
                .and_then(|c| c.detail.as_ref())
                .and_then(|d| d.organization.clone())
                .unwrap_or_default()
        } else {
            organization
        };
        let session = match self.scope() {
            Scope::Session(_, s) => s.clone(),
            _ if !tenant.is_empty() => tenant.clone(),
            _ => "default".into(),
        };
        self.mode = Mode::Compose {
            form: IntentForm {
                organization,
                tenant,
                project: self.project.clone().unwrap_or_default(),
                machine,
                session,
                ..Default::default()
            },
            back: Box::new(self.mode.clone()),
        };
    }

    fn palette_key(&mut self, key: KeyEvent, back: Box<Mode>) -> Action {
        let pending_backlog = backlog_selection(&back).is_some();
        let lost_target = pending_backlog
            && self
                .backlog_focus
                .as_ref()
                .is_some_and(|key| !self.card(key).is_some_and(|c| self.card_visible(c)));
        self.mode = (*back).clone();
        if lost_target && matches!(key.code, KeyCode::Char('d' | 'f' | 'y' | 'x' | 'u' | 'g')) {
            self.say("The selected intent changed while the menu was open. Select it again before changing it.", Tone::Warn);
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {}
            KeyCode::Char('n') => self.new_intent(),
            KeyCode::Char('d') => return self.intent_action("dispatch", json!({})),
            KeyCode::Char('f' | 'y') => return self.intent_action("refresh", json!({})),
            KeyCode::Char('x') => return self.intent_action("dismiss", json!({})),
            KeyCode::Char('u') => return self.intent_action("restore", json!({})),
            KeyCode::Char('g') => return self.intent_action("move", json!({"top":true})),
            KeyCode::Char('t') => self.cycle_owner(),
            KeyCode::Char('p') => self.cycle_project(),
            KeyCode::Char('a') => self.cycle_provider(),
            KeyCode::Char('m' | 'i') => self.enter_misc(),
            KeyCode::Char('w') => {
                self.enter_backlog(!self.wont_do);
            }
            KeyCode::Char('b') => {
                self.enter_backlog(false);
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Keys {
                    filter: String::new(),
                    typing: false,
                    scroll: 0,
                }
            }
            _ => self.mode = Mode::Palette { back },
        }
        Action::None
    }

    fn compose_key(&mut self, key: KeyEvent, mut form: IntentForm, back: Box<Mode>) -> Action {
        if key.code == KeyCode::Esc {
            self.mode = *back;
            return Action::None;
        }
        if self.busy {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Enter && (ctrl || form.field == 7) {
            if form.title.trim().is_empty()
                || form.words.trim().is_empty()
                || form.session.trim().is_empty()
            {
                self.say(
                    "Add a title, request and target session before creating the intent.",
                    Tone::Warn,
                );
            } else {
                let request = json!({"action":"create","title":form.title.trim(),"words":form.words.trim(),
                    "organization":form.organization.trim(),"tenant":form.tenant.trim(),"project":form.project.trim(),"session":form.session.trim()});
                self.mode = Mode::Compose {
                    form: form.clone(),
                    back: back.clone(),
                };
                return self.emit_mutation(form.machine, request);
            }
        } else {
            match key.code {
                KeyCode::Tab => form.field = (form.field + 1) % 8,
                KeyCode::BackTab => form.field = (form.field + 7) % 8,
                KeyCode::Left | KeyCode::Right if form.field == 5 => {
                    let n = self.machines.len().max(1);
                    form.machine = if key.code == KeyCode::Left {
                        (form.machine + n - 1) % n
                    } else {
                        (form.machine + 1) % n
                    };
                }
                _ => {
                    let text = match form.field {
                        0 => Some(&mut form.title),
                        1 => Some(&mut form.words),
                        2 => Some(&mut form.organization),
                        3 => Some(&mut form.tenant),
                        4 => Some(&mut form.project),
                        6 => Some(&mut form.session),
                        _ => None,
                    };
                    if let Some(text) = text {
                        match key.code {
                            KeyCode::Backspace => {
                                text.pop();
                            }
                            KeyCode::Char('u') if ctrl => text.clear(),
                            KeyCode::Char(c) if !ctrl && text.len() < 16000 => text.push(c),
                            KeyCode::Enter if form.field == 1 && text.len() < 16000 => {
                                text.push('\n')
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.mode = Mode::Compose { form, back };
        Action::None
    }

    pub(super) fn control_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.code == KeyCode::Char('i') && matches!(self.mode, Mode::Board | Mode::Backlog { .. })
        {
            self.enter_misc();
            return Some(Action::None);
        }
        match self.mode.clone() {
            Mode::Compose { form, back } => return Some(self.compose_key(key, form, back)),
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
                        if request["action"] == "dispatch" {
                            request["override"] = json!(true);
                        }
                        self.mode = *back;
                        return Some(self.emit_mutation(machine, request));
                    }
                    _ => {}
                }
                return Some(Action::None);
            }
            Mode::Palette { back } => return Some(self.palette_key(key, back)),
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
            if let Some(card) = self.selected_for_control() {
                self.backlog_focus = Some(card.key.clone());
            }
            self.mode = Mode::Palette {
                back: Box::new(self.mode.clone()),
            };
            return Some(Action::None);
        }
        if key.code == KeyCode::Char('a')
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && matches!(
                self.mode,
                Mode::Board | Mode::Backlog { .. } | Mode::Misc { .. }
            )
        {
            self.cycle_provider();
            return Some(Action::None);
        }
        if matches!(self.mode, Mode::Board | Mode::Backlog { .. }) && key.code == KeyCode::Char('w')
        {
            self.enter_backlog(!self.wont_do);
            return Some(Action::None);
        }
        if matches!(self.mode, Mode::Backlog { .. } | Mode::Peek { .. }) {
            if let KeyCode::Char(c @ '0'..='9') = key.code {
                if (c != '0' || !self.count.is_empty()) && self.count.len() < 5 {
                    self.count.push(c);
                }
                return Some(Action::None);
            }
            let count: i64 = std::mem::take(&mut self.count).parse().unwrap_or(1);
            match key.code {
                KeyCode::Char('J') => {
                    return Some(self.intent_action("move", json!({"delta":count})));
                }
                KeyCode::Char('K') => {
                    return Some(self.intent_action("move", json!({"delta":-count})));
                }
                KeyCode::Char('y') => return Some(self.intent_action("refresh", json!({}))),
                KeyCode::Char('d') => return Some(self.intent_action("dispatch", json!({}))),
                KeyCode::Char('x') => return Some(self.intent_action("dismiss", json!({}))),
                KeyCode::Char('u') => return Some(self.intent_action("restore", json!({}))),
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
                if response.ok && request["action"] == "create" {
                    self.enter_backlog(false);
                }
                self.say(
                    response.message,
                    if response.ok { Tone::Good } else { Tone::Warn },
                );
            }
            Err(error) => self.say(error, Tone::Warn),
        }
    }
}
