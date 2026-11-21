//! Intent controls live apart from the navigation keys. Writes go through the configured
//! adapter, on the record's home machine; the board never edits canonical intent files.

use super::*;
use crate::actions::Response;
use crate::images;
use crate::model::{CardSnapshot, Summary};
use crate::resolve::Effect;
use serde_json::{Value, json};

/// The most the new-intent box holds for a backend without fix-input, kept under its 64 KiB
/// request limit.
const MAX_WORDS: usize = 48_000;

/// The most it holds for a backend that fixes input, which saves long words whole in a file beside
/// the intent (intent 0591, release 5): a megabyte, well inside a 32 MiB request.
const FIX_WORDS: usize = 1_000_000;

/// The most the new-intent box holds for a backend that fixes input or not.
pub(crate) fn max_words(fix: bool) -> usize {
    if fix { FIX_WORDS } else { MAX_WORDS }
}

/// The most of an action's answer the board shows: a backend's error can run to thousands of
/// characters, which would fill the screen (intent 0411).
const MAX_ANSWER: usize = 300;

/// What the busy line of an account request adds while it's sent again by itself (intent 0591).
const RETRYING_TAIL: &str = " trying again…";

/// How long ⏎ on a card just started waits for its workspace to show, and how long a second ⏎
/// or d meanwhile starts nothing again (intent 0591, K1-43): a start may wait for a place, or
/// take minutes to open on a busy machine.
const START_WAIT: Duration = Duration::from_secs(600);
/// The backend's words when a yes doesn't fit the question it answers: nothing started.
const YES_NOT_MATCHED: &str = "couldn't be matched to the question";

/// What a feed lists in `supports` when its backend does each key's step first (intent 0591).
pub(crate) const STEPS: &str = "steps";

/// What a backend must list in `supports` to take this request as Kerdr means it (Codex's fifth
/// look): a cancel only a newer backend knows, which an older one would read as X's; or a request
/// sent on the strength of its backend doing each key's step first, judged by the card it was
/// asked on (`from`), which an older backend would refuse or do without its question. When it
/// goes on to another machine than the one that asked (`forwarded`), a choice or a save's token
/// counts too, since only such a backend gives those. None when every backend takes it alike.
pub(crate) fn needs(
    request: &Value,
    from: Option<&CardSnapshot>,
    forwarded: bool,
) -> Option<&'static str> {
    use crate::app::cancels::{CANCEL_BACKLOG, CANCEL_WORKSPACE, is_bare_cancel, is_stop_start};
    if is_bare_cancel(request) {
        return Some(CANCEL_WORKSPACE);
    }
    if is_stop_start(request) {
        return Some(CANCEL_BACKLOG);
    }
    let action = request["action"].as_str().unwrap_or_default();
    // X on work an agent is at, naming no place for its backend to ask about (the end-to-end
    // check's fourth failure): only a backend that does the step first asks it.
    if names_no_place(request) {
        return Some(STEPS);
    }
    if forwarded
        && (request.get("choice").is_some()
            || (action == "edit" && request.get("confirm").is_some()))
    {
        return Some(STEPS);
    }
    let f = from?;
    let steps = match action {
        // C or x on work an agent may be at, C on work in Won't do, x on done work.
        "close" | "dismiss" => f.dismissed || (!f.waiting && (f.done || f.workspace.is_some())),
        // d or ⏎ on work in Won't do: brought back and started in one request.
        "dispatch" | "reopen" => f.dismissed,
        // X on done work, or on work in Won't do, whose workspace is still open.
        "cancel" => f.done || f.dismissed,
        // e's save on work that isn't waiting in the backlog.
        "edit" => request.get("text").is_some() && !f.waiting,
        _ => false,
    };
    steps.then_some(STEPS)
}

/// X's cancel of a card that names no place: neither the workspace Kerdr's own question named, nor
/// the window outside herdr, nor Esc's stop of a start. Its backend asks where its agent is.
pub(crate) fn names_no_place(request: &Value) -> bool {
    request["action"] == "cancel"
        && request.get("id").is_some()
        && request.get("workspace").is_none()
        && request.get("outside_since").is_none()
        && request.get("to").is_none()
}

/// An action's answer cut to what the board shows, ending in "…" when it was longer.
pub(super) fn short(text: String) -> String {
    match text.char_indices().nth(MAX_ANSWER) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    }
}

/// Whether a backend's words already name the card by `reference` ("#21", or a workspace's
/// name): as a whole word, so "#2" isn't found in "#21", and "#21's" counts.
pub(super) fn names(words: &str, reference: &str) -> bool {
    let reference = reference.trim();
    if reference.is_empty() {
        return false;
    }
    words.match_indices(reference).any(|(at, _)| {
        let before = words[..at].chars().next_back();
        let after = words[at + reference.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '#')
            && !after.is_some_and(char::is_alphanumeric)
    })
}

/// What didn't happen and the backend's words why, as one line: "#41 wasn't marked done: it's
/// locked." Words that already name the card are said alone, so a line never names it twice,
/// as "#21 didn't start: #21 didn't start yet (…)" did.
pub(super) fn what_and_why(what: &str, reference: &str, why: &str) -> String {
    if names(why, reference) {
        why.to_string()
    } else {
        format!("{what}: {why}")
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

/// What putting a read into a new intent's words did: what to say of it, the images too large to
/// send (for a backend that fixes input, to ask about smaller copies of), and how much went in at
/// the place it went.
#[derive(Debug, Default)]
struct Attached {
    said: Vec<String>,
    /// Copied text that didn't fit the box: kept for c, never dropped (Codex's review 85, 5).
    left_over: Option<String>,
    /// Something read didn't go in: refused, or past a limit.
    refused: bool,
    oversized: Vec<images::Loaded>,
    inserted: usize,
    at: usize,
}

/// Puts what a read found into a new intent's words at `at` (where the words ended when the read
/// began), or at the end when the words have since been cut shorter than that: text first, as any
/// paste would put it (intent 0591, K3-61), then each image as the next `[Image #N]` tag. For a
/// backend that fixes input (`fix`) there's no count, and no image is refused for the words'
/// room; one past the 20 MB its images may be together is held back for a question.
fn attach_into(draft: &mut Draft, got: images::Loading, at: Option<usize>, fix: bool) -> Attached {
    let mut at = at
        .filter(|&a| a <= draft.words.len() && draft.words.is_char_boundary(a))
        .unwrap_or(draft.words.len());
    let before = draft.words.len();
    let mut out = Attached {
        at,
        said: got.notes,
        ..Attached::default()
    };
    if let Some(text) = got.text {
        let room = max_words(fix).saturating_sub(draft.words.len());
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut piece = String::new();
        if draft.words[..at]
            .chars()
            .last()
            .is_some_and(|c| !c.is_whitespace())
            && !text.starts_with(char::is_whitespace)
        {
            piece.push(' ');
        }
        piece.push_str(&text[..end]);
        draft.words.insert_str(at, &piece);
        at += piece.len();
        if end < text.len() {
            // What didn't fit is kept, and said: never cut short and sent unseen.
            out.said.push(format!(
                "Only the first {} of the copied text fit the box, which holds {}; the rest is kept, and c brings it back once these words have gone.",
                images::size(end),
                images::size(max_words(fix))
            ));
            out.left_over = Some(text[end..].to_string());
            out.refused = true;
        } else {
            out.said.push("Pasted the copied text.".to_string());
        }
    }
    for image in got.images {
        let total: usize = draft.images.iter().map(|i| i.bytes.len()).sum();
        if fix {
            if total + image.bytes.len() > images::MAX_IMAGES_BYTES
                || image.bytes.len() > images::FIX_IMAGE_BYTES
            {
                out.oversized.push(image);
                continue;
            }
        } else {
            if draft.images.len() >= images::MAX_IMAGES {
                out.said.push(images::too_many());
                out.refused = true;
                break;
            }
            if total + image.bytes.len() > images::MAX_IMAGES_BYTES {
                out.said.push(images::too_much());
                out.refused = true;
                break;
            }
        }
        let n = draft.images.iter().map(|i| i.tag).max().unwrap_or(0) + 1;
        let tag = images::tag(n);
        if !fix && draft.words.len() + tag.len() + 2 > MAX_WORDS {
            out.said
                .push("The box is full; there's no room for another image's tag.".into());
            out.refused = true;
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
        out.said.push(format!(
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
    out.refused |= got.why.is_some();
    out.said.extend(got.why);
    out.inserted = draft.words.len() - before;
    out
}

/// A new intent's `create` request: its words, the images whose tags are in them, where it goes
/// and who it's filed under, and its submission.
fn create_request(
    id: &str,
    words: &str,
    images: &[images::Image],
    session: &str,
    filed: &crate::app::Filed,
) -> Value {
    let mut request = json!({"action":"create","words":words,"session":session,"submission":id});
    for (field, value) in [
        ("organization", &filed.organization),
        ("tenant", &filed.tenant),
        ("team", &filed.team),
    ] {
        if let Some(v) = value {
            request[field] = json!(v);
        }
    }
    if !images.is_empty() {
        request["images"] = Value::Array(images::request_images(words, images));
    }
    request
}

/// What the board says while a read goes.
fn reading(source: &images::Source) -> &'static str {
    match source {
        images::Source::Clipboard => "Reading the clipboard…",
        images::Source::Files(_) => "Reading the image…",
        images::Source::Smaller(..) => "Making a smaller copy…",
    }
}

/// Names as a sentence lists them.
fn and_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Why images didn't go with words Enter sent: past what one intent may carry, and how to send a
/// smaller copy.
fn too_large_to_send(images: &[images::Loaded]) -> String {
    let each: Vec<String> = images
        .iter()
        .map(|i| format!("{} is {}", i.from, images::size(i.bytes.len())))
        .collect();
    format!(
        "{}, past the {} one intent's images may be together; pasted again in the box, Kerdr offers a smaller copy.",
        and_names(&each),
        images::size(images::MAX_IMAGES_BYTES)
    )
}

/// The question whether smaller copies go in, naming each image, its size and the room left.
fn smaller_question(images: &[images::Loaded], left: usize) -> String {
    let each: Vec<String> = images
        .iter()
        .map(|i| {
            format!(
                "{} ({}, {})",
                i.from,
                images::kind_name(i.media_type),
                images::size(i.bytes.len())
            )
        })
        .collect();
    let room = if left >= images::MAX_IMAGES_BYTES {
        format!(
            "past the {} one intent's images may be together",
            images::size(images::MAX_IMAGES_BYTES)
        )
    } else {
        format!(
            "past the {} this intent's images have room for",
            images::size(left)
        )
    };
    if images.len() == 1 {
        format!(
            "{} is {room}. Attach a smaller copy instead? It may lose fine detail; the image itself stays as it is. n leaves it out.",
            each[0]
        )
    } else {
        format!(
            "{} are {room}. Attach smaller copies instead? They may lose fine detail; the images themselves stay as they are. n leaves them out.",
            and_names(&each)
        )
    }
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

    /// Whether f's filters (owner, project, machine, agents or background) hide any work.
    pub(crate) fn filtering(&self) -> bool {
        self.owner.is_some()
            || self.machine_filter.is_some()
            || self.project.is_some()
            || self.provider.is_some()
            || self.background != Background::All
    }

    /// Metadata filters without the selected tab, so other session badges stay meaningful.
    pub(crate) fn card_matches_filters(&self, card: &Card) -> bool {
        self.owner.as_ref().is_none_or(|o| *o == card.owner_key())
            && self
                .machine_filter
                .as_ref()
                .is_none_or(|m| *m == card.machine)
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
        if self.owner_split() {
            return self.owner_filter_rows();
        }
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
                pick: Pick::Filter,
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
            .position(|r| self.filter_row_on(r))
            .unwrap_or(0)
    }

    /// Whether a filter row is what the board shows now: its check mark.
    pub fn filter_row_on(&self, r: &FilterRow) -> bool {
        match &r.pick {
            Pick::Filter => r.owner == self.owner && r.project == self.project,
            Pick::View(scope) => scope == self.scope(),
            Pick::Project(p) => *p == self.project,
            Pick::Machine(m) => *m == self.machine_filter,
        }
    }

    /// f's list on the owner split (intent 0595): the tree of owners and their teams, grouped by
    /// organization, each picking the top bar's view; then the chosen owner's projects, and each
    /// machine. Counts follow the agents and background filters, as on the machine split.
    fn owner_filter_rows(&self) -> Vec<FilterRow> {
        let counted = |c: &&Card| {
            c.stage != Stage::Misc
                && !c.dismissed()
                && self
                    .provider
                    .as_ref()
                    .is_none_or(|p| c.agents.iter().any(|a| &a.agent == p))
                && self.background.shows(c.background)
                && self.machine_filter.as_ref().is_none_or(|m| *m == c.machine)
        };
        let count = |keep: &dyn Fn(&Card) -> bool| {
            let (mut open, mut backlog) = (0, 0);
            for c in self.board.cards.iter().filter(counted).filter(|c| keep(c)) {
                match c.stage {
                    Stage::Backlog => backlog += 1,
                    Stage::Maintain | Stage::Misc => {}
                    _ => open += 1,
                }
            }
            (open, backlog)
        };
        let row = |label: String, kind: &str, depth: u8, scope: Scope| {
            let (open, backlog) = count(&|c: &Card| scope.contains(c));
            FilterRow {
                label,
                kind: kind.to_string(),
                depth,
                owner: None,
                project: None,
                open,
                backlog,
                pick: Pick::View(scope),
            }
        };
        let mut rows = vec![row("All work".into(), "", 0, Scope::All)];
        // An organization heads its tenants in the tree even with no work of its own, so a tenant
        // is never drawn under another organization.
        let listed = self.listed_owners();
        let mut tree: Vec<String> = Vec::new();
        for id in &listed {
            if let Some(org) = self
                .owner_info(id)
                .map(|o| o.organization(&self.board.owners).id.clone())
                .filter(|org| org != id && !tree.contains(org))
            {
                tree.push(org);
            }
            if !tree.contains(id) {
                tree.push(id.clone());
            }
        }
        for id in tree {
            let Some(o) = self.owner_info(&id).cloned() else {
                rows.push(row("Unowned".into(), "", 0, Scope::Owner(id)));
                continue;
            };
            let tenant = o.parent.is_some();
            let kind = if tenant {
                format!(
                    "{} tenant",
                    self.owner_info(o.parent.as_deref().unwrap_or_default())
                        .map(|p| p.label().to_string())
                        .unwrap_or_default()
                )
            } else {
                "organization".into()
            };
            let depth = u8::from(tenant);
            let label = match o.code.as_deref() {
                Some(code) => format!("{} · {code}", o.label()),
                None => o.label().to_string(),
            };
            rows.push(row(label, kind.trim(), depth, Scope::Owner(id.clone())));
            let tabs: Vec<Scope> = {
                let mut v = Vec::new();
                for t in o.teams.iter() {
                    let has = self
                        .board
                        .cards
                        .iter()
                        .any(|c| c.owner == id && c.team.as_deref() == Some(t.id.as_str()));
                    if !t.retired || has {
                        v.push(Scope::Team(id.clone(), Some(t.id.clone())));
                    }
                }
                if self
                    .board
                    .cards
                    .iter()
                    .any(|c| c.owner == id && c.team.is_none() && !c.dismissed())
                    && !o.teams.is_empty()
                {
                    v.push(Scope::Team(id.clone(), None));
                }
                v
            };
            for scope in tabs {
                let Scope::Team(_, t) = &scope else { continue };
                let name = self.team_name(&id, t.as_deref());
                rows.push(row(name, "team", depth + 1, scope));
            }
        }
        // The chosen owner's projects.
        if let Some(owner) = self.scope().owner().map(str::to_string) {
            let mut projects: Vec<String> = self
                .board
                .cards
                .iter()
                .filter(|c| c.owner == owner && !c.project().is_empty())
                .map(|c| c.project().to_string())
                .collect();
            projects.sort();
            projects.dedup();
            if !projects.is_empty() {
                let (open, backlog) = count(&|c: &Card| c.owner == owner);
                rows.push(FilterRow {
                    label: "All projects".into(),
                    kind: format!("{}'s", self.owner_short(&owner)),
                    depth: 0,
                    owner: None,
                    project: None,
                    open,
                    backlog,
                    pick: Pick::Project(None),
                });
                for p in projects {
                    let (open, backlog) = count(&|c: &Card| c.owner == owner && c.project() == p);
                    rows.push(FilterRow {
                        label: p.clone(),
                        kind: "project".into(),
                        depth: 1,
                        owner: None,
                        project: None,
                        open,
                        backlog,
                        pick: Pick::Project(Some(p)),
                    });
                }
            }
        }
        // Each machine: only its work, for the times you want one machine's.
        let machine_count = |m: Option<&str>| {
            let (mut open, mut backlog) = (0, 0);
            for c in self.board.cards.iter().filter(|c| {
                c.stage != Stage::Misc
                    && !c.dismissed()
                    && self.background.shows(c.background)
                    && m.is_none_or(|m| c.machine == m)
            }) {
                match c.stage {
                    Stage::Backlog => backlog += 1,
                    Stage::Maintain | Stage::Misc => {}
                    _ => open += 1,
                }
            }
            (open, backlog)
        };
        let (open, backlog) = machine_count(None);
        rows.push(FilterRow {
            label: "All machines".into(),
            kind: String::new(),
            depth: 0,
            owner: None,
            project: None,
            open,
            backlog,
            pick: Pick::Machine(None),
        });
        for m in self.machines.iter().filter(|m| m.is_shown()) {
            let name = m.name();
            let (open, backlog) = machine_count(Some(&name));
            rows.push(FilterRow {
                label: name.clone(),
                kind: "machine".into(),
                depth: 1,
                owner: None,
                project: None,
                open,
                backlog,
                pick: Pick::Machine(Some(name)),
            });
        }
        rows
    }

    /// Enter on a filter row: what it picks.
    fn apply_filter_row(&mut self, row: FilterRow) {
        match row.pick {
            Pick::Filter => self.set_filter(row.owner, row.project),
            Pick::View(scope) => {
                self.choose_view(scope);
                self.backlog_focus = None;
                set_backlog_selection(&mut self.mode, 0);
                self.fix_selection();
            }
            Pick::Project(p) => {
                let owner = self.owner.clone();
                self.set_filter(owner, p);
            }
            Pick::Machine(m) => {
                self.machine_filter = m;
                self.backlog_focus = None;
                set_backlog_selection(&mut self.mode, 0);
                self.fix_selection();
            }
        }
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

    /// F: every owner, project, machine, agent and background card again. On the owner split the
    /// owner is the top bar's, which stays.
    pub(super) fn clear_filters(&mut self) {
        let filtered = self.filtering();
        self.provider = None;
        self.machine_filter = None;
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
                    self.apply_filter_row(row);
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
                "Kerdr has no action backend here to change intents: set [actions] command in its config.",
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
        // Only a change to a card is held or sent through another machine (intent 0591, release
        // 4): a new intent or an account request needs its machine answering.
        let routed = request["action"]
            .as_str()
            .is_some_and(|a| a != "create" && !a.starts_with("account-"));
        let can = if routed {
            self.can_change(machine)
        } else {
            self.can_write(machine)
        };
        if can {
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
            .find(|(key, _, from)| *key == card.key && from.as_deref() == home)
            .map(|(_, to, _)| *to)
    }

    /// Kerdr's own question before C on a card in Won't do (intent 0591, K1-19), or X on work with
    /// nothing running, whose end is Won't do (K1-30, K1-31): its yes sends the request as it is,
    /// and its backend still checks nothing runs it under its lock.
    pub(super) fn ask_to_end(&mut self, card: &Card, action: &str) -> Action {
        let Some(detail) = &card.detail else {
            return Action::None;
        };
        let Some(machine) = self.home_machine(card) else {
            return Action::None;
        };
        if !self.can_change(machine) {
            return Action::None;
        }
        let reference = &card.reference;
        let (title, message) = if action == "close" {
            (
                format!("Mark {reference} done?"),
                "It's in Won't do: yes brings it back and marks it done, with nothing started. Its log says you closed it from Kerdr, and ⏎ on it later asks to reopen it.".to_string(),
            )
        } else {
            (
                format!("Won't do {reference}?"),
                format!(
                    "Nothing is running for {reference}, so X sets it aside in Won't do, with nothing stopped or started. Its record keeps where it was; u brings it back."
                ),
            )
        };
        self.mode = Mode::Confirm {
            machine,
            request: json!({"action": action, "id": detail.id}),
            title,
            message,
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
            queue: false,
            ready_at: None,
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
        Action::None
    }

    /// Whether the backend where this card's record lives does each key's step first and asks its
    /// own questions for C, x and X on work an agent may be at: its feed's `supports` has "steps"
    /// (intent 0591). Any other gets 0.33.0's behaviour: Kerdr's own question or refusal first. A
    /// home on a machine Kerdr isn't set up for is no backend that does steps: the card's own
    /// machine never answers for it (Codex's sixth look, swept).
    pub(crate) fn does_steps(&self, card: &Card) -> bool {
        let home = card
            .detail
            .as_ref()
            .and_then(|d| d.home.as_ref())
            .and_then(|h| h.machine.as_deref());
        let machine = match (self.moved_target(card), home) {
            (Some(to), _) => Some(to),
            (None, Some(name)) => self.machine_index(name),
            (None, None) => Some(card.machine_index),
        };
        machine.is_some_and(|m| self.steps_on(m))
    }

    /// Whether this machine's backend does each key's step first: for a request sent there rather
    /// than to the card's home, such as X's, which goes where its workspace runs (Codex's review).
    pub(crate) fn steps_on(&self, machine: usize) -> bool {
        self.machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .is_some_and(|f| f.supports(STEPS))
    }

    /// What this request needs that `machine`'s backend doesn't list in `supports`, checked where
    /// it actually goes: a fresh send, a question's yes, or a resend to the machine a refusal named
    /// (`forwarded`). None when it takes it (Codex's fifth look).
    pub(crate) fn missing_on(
        &self,
        machine: usize,
        request: &Value,
        from: Option<&CardSnapshot>,
        forwarded: bool,
    ) -> Option<&'static str> {
        // A stop once its workspace opens waits behind the start, and names that workspace when
        // it goes to a backend that doesn't ask first (K1-41).
        if crate::app::behind::is_cancel_once_open(request)
            && let Some(f) = from
            && self
                .resolver
                .starting(&f.key, request["id"].as_str().unwrap_or_default())
                .is_some()
        {
            return None;
        }
        needs(request, from, forwarded).filter(|what| {
            !self
                .machines
                .get(machine)
                .and_then(|m| m.feed.as_ref())
                .is_some_and(|f| f.supports(what))
        })
    }

    /// Whether `machine`'s backend starts this on the account it names, by its feed's `starts_on`,
    /// checked where it goes as `missing_on` is: true for anything that names none, or isn't a
    /// start (Codex's fifth look). An older backend would start it on its owner's list without a
    /// word (intent 0580).
    pub(crate) fn starts_named_account_on(&self, machine: usize, request: &Value) -> bool {
        let action = request["action"].as_str().unwrap_or_default();
        let names = match action {
            "dispatch" | "reopen" => request.get("account").is_some(),
            "account-start" | crate::app::resume::RESUME => true,
            _ => false,
        };
        !names
            || self
                .machines
                .get(machine)
                .and_then(|m| m.feed.as_ref())
                .is_some_and(|f| f.starts_on(action))
    }

    /// A request that wasn't sent because its backend lacks `what`: said as a backend without it
    /// would have it, and the key again asks there as that backend takes it.
    pub(crate) fn not_sent_for(
        &mut self,
        machine: usize,
        request: &Value,
        from: Option<&CardSnapshot>,
        what: &str,
        forwarded: bool,
    ) {
        use crate::app::cancels::{CANCEL_BACKLOG, CANCEL_WORKSPACE, NO_INTENT};
        let reference = from
            .and_then(|f| self.card(&f.key))
            .map(|c| c.reference.clone())
            .unwrap_or_else(|| self.reference_for(request["id"].as_str().unwrap_or_default()));
        let name = self
            .machines
            .get(machine)
            .map(|m| m.name())
            .unwrap_or_default();
        let (said, tone) = match (what, forwarded) {
            (CANCEL_WORKSPACE, _) => (NO_INTENT.to_string(), Tone::Info),
            (CANCEL_BACKLOG, true) => (
                format!(
                    "{reference} lives on {name} now, whose backend can't put it back in the backlog, so it wasn't stopped: X on its card stops it and sets it aside."
                ),
                Tone::Warn,
            ),
            (CANCEL_BACKLOG, false) => (
                format!(
                    "{reference} wasn't stopped: {name}'s backend can't put it back in the backlog. X on its card stops it and sets it aside."
                ),
                Tone::Warn,
            ),
            (_, true) => (
                format!(
                    "{reference} lives on {name} now, whose backend doesn't do that step first, so nothing was sent there. The same key on its card asks again, as that backend takes it."
                ),
                Tone::Warn,
            ),
            (_, false) => (
                format!(
                    "{name}'s backend doesn't do that step first, so nothing was sent for {reference}. The same key on its card asks again, as that backend takes it."
                ),
                Tone::Warn,
            ),
        };
        self.say(said, tone);
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
                // Not one of Kerdr's machines: a machine that forwards sends it there (intent
                // 0591, release 4; route names it in `forward_to`).
                if found.is_none()
                    && let Some(via) = self.forwarder_for(Some(&name))
                {
                    return Some(via);
                }
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
        self.reopen_action_with(card, json!({}))
    }

    /// A reopen with more in its request: `s`'s `account`, the one to resume it on (intent 0580).
    pub(super) fn reopen_action_with(&mut self, card: &Card, extra: Value) -> Action {
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
                    "{} {what}, and Kerdr has no action backend here to reopen it (set [actions] command in its config). m peeks at it.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        let Some(machine) = self.home_machine(card) else {
            return Action::None;
        };
        if !self.can_change(machine) {
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
        if let (Some(fields), Some(more)) = (request.as_object_mut(), extra.as_object()) {
            fields.extend(more.clone());
        }
        self.action_from = Some(CardSnapshot::of(card));
        Action::Mutate { machine, request }
    }

    /// Whether you're still on this work, in the view you're looking at (the board, a peek, the
    /// backlog or Misc), and it hasn't changed since the action started: where a late answer may
    /// ask you something or take you in. Typing a search, a filter or a new intent isn't.
    pub(super) fn still_on(&self, from: &CardSnapshot, made: Option<(&str, &str)>) -> bool {
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
            } else if request["action"] == "account-start" {
                "Starting a fresh agent…".into()
            } else if request["action"] == crate::app::resume::RESUME {
                "Resuming its conversation…".into()
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
            self.say(
                "Nothing is selected for that key to act on: pick a card first.",
                Tone::Info,
            );
            return Action::None;
        };
        self.intent_action_on(card, action, extra)
    }

    /// An intent action on this card, which needn't be the selected one: ⏎ on a search hit, a
    /// row inside a card or the misc list starts that card's work (intent 0404).
    pub(super) fn intent_action_on(&mut self, card: Card, action: &str, extra: Value) -> Action {
        let Some(detail) = &card.detail else {
            self.say(
                "This workspace has no intent, so there's nothing to start, reorder or change; ⏎ goes in.",
                Tone::Info,
            );
            return Action::None;
        };
        // A new intent still saving: its key waits for its number (intent 0591, K1-42).
        if self.still_changing(&card, action) {
            return Action::None;
        }
        // C or x while its start is on its way, with nothing open yet: a stop once it opens
        // (K1-41).
        if matches!(action, "close" | "dismiss") && self.cancel_once_open(&card) {
            return Action::None;
        }
        // The question this key asked while you were elsewhere, waiting on it (intent 0591).
        if self.key_opens_waiting(&card.key, action) {
            return Action::None;
        }
        // d on work still being written up from his words: it starts as soon as that's done,
        // rather than be refused until then. The runner holds the start until its home's feed
        // shows it written up, so it goes even while he's inside a workspace (intent 0445).
        if action == "dispatch" && card.intake() == Some("pending") && waiting_work(&card) {
            if self
                .resolver
                .start_waiting_for_write_up(&card.key)
                .is_none()
            {
                let Some(machine) = self.home_machine(&card) else {
                    return Action::None;
                };
                if !self.can_change(machine) {
                    return Action::None;
                }
                let mut request = json!({"action":"dispatch","id":detail.id});
                // s's account goes with the held start too (intent 0580).
                if let (Some(fields), Some(more)) = (request.as_object_mut(), extra.as_object()) {
                    fields.extend(more.clone());
                }
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
                    "{} isn't set aside, so there's nothing to bring back.",
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
        // Its backend does each key's step first, asking its own questions (intent 0591).
        let steps = self.does_steps(&card);
        let finished = card.done() || card.state.as_deref() == Some("closed");
        if action == "close" && card.dismissed() {
            // C on a card in Won't do means finishing it: asked as any C is, then brought back
            // and marked done in one request (intent 0591, K1-19).
            if steps {
                return self.ask_to_end(&card, action);
            }
            self.say(
                format!(
                    "{} is in won't do; u brings it back before it can be marked done.",
                    card.reference
                ),
                Tone::Info,
            );
            return Action::None;
        }
        // d on work under way or done does what ⏎ does: goes in when its workspace is open,
        // reopens it when it isn't, and asks first for done work (intent 0591, K1-24, K1-25). That
        // covers work in a working column no agent ran (intent 0411), whose reopen starts it.
        if action == "dispatch" && started && card.intake().is_none() {
            return self.jump_action(card.key.clone());
        }
        // C or x on work an agent may be at, or x on done work: to a backend that does the step
        // first, sent as it is, and the backend asks, naming what stops and offering to have its
        // agent wrap up, or to move done work to Won't do (intent 0591, K1-21, K1-22).
        let asks_itself = steps
            && started
            && card.intake().is_none()
            && ((ends && card.is_open_workspace() && !finished)
                || (action == "dismiss" && finished));
        if started && !(ends && card.closable()) && !asks_itself {
            let reference = &card.reference;
            let ordering = matches!(action, "move" | "refresh");
            let said = if ends && finished {
                if action == "close" {
                    format!("{reference} is done already.")
                } else {
                    format!("{reference} is done; won't do is for work that isn't.")
                }
            } else if ends && card.is_open_workspace() {
                format!(
                    "{reference} has a workspace open, so an agent may be at it: C and x are for work with none. ⏎ goes in."
                )
            } else if ordering && finished {
                format!("{reference} is done, so it has no place in the backlog order.")
            } else if ordering {
                format!("{reference} has started, so it has no place in the backlog order.")
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
            let reference = &card.reference;
            let said = match action {
                // d on a card in Won't do means starting it: its backend brings it back and starts
                // it in one request, with every usual check (intent 0591, K1-26).
                "dispatch" if steps => None,
                "dispatch" => Some(format!(
                    "{reference} is in won't do; u brings it back, then d starts it."
                )),
                "dismiss" => Some(format!("{reference} is in won't do already.")),
                _ => Some(format!(
                    "{reference} is in won't do, so it has no place in the backlog order; u brings it back."
                )),
            };
            if let Some(said) = said {
                self.say(said, Tone::Info);
                return Action::None;
            }
        }
        let Some(machine) = self.home_machine(&card) else {
            return Action::None;
        };
        if !self.can_change(machine) {
            return Action::None;
        }
        let mut request = json!({"action":action,"id":detail.id});
        if let Some(obj) = extra.as_object() {
            request.as_object_mut().unwrap().extend(obj.clone());
        }
        // Its conversation runs in a terminal outside herdr: C and x go straight to its backend,
        // which stops it there first and asks before it does, naming where it runs (intent 0591):
        // that question is the one he answers, not a second of Kerdr's. `outside_since` is the
        // window the card shows; an older backend refuses as before.
        if ends
            && started
            && let Some((_, since)) = self.outside_said(&card)
        {
            request["outside_since"] = json!(since);
            self.action_from = Some(CardSnapshot::of(&card));
            return Action::Mutate { machine, request };
        }
        if asks_itself {
            self.action_from = Some(CardSnapshot::of(&card));
            return Action::Mutate { machine, request };
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
                choices: vec![],
                enter_queues: false,
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

    /// How a question names a conversation running outside herdr, and the `since` Kerdr sends with
    /// a stop of it (the feed's own words, or ""): "its Codex conversation running in a foot window
    /// since 13:02". None when the card's conversation runs in herdr or nowhere.
    pub(super) fn outside_said(&self, card: &Card) -> Option<(String, String)> {
        let outside = card.outside_herdr()?;
        let program = card
            .resume_agent()
            .map(|a| format!("{} ", self.providers(card.machine_index).program_label(a)))
            .unwrap_or_default();
        let since = outside
            .since
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| format!(" since {}", self.when(t.with_timezone(&chrono::Local))))
            .unwrap_or_default();
        Some((
            format!(
                "its {program}conversation running in {}{since}",
                outside.place()
            ),
            outside.since.clone().unwrap_or_default(),
        ))
    }

    /// X (intent 0321): the agent at the selected card's work stopped, mid-step or idle, by
    /// closing its workspace (only its tab, for a child running in its parent's workspace), and the
    /// work set aside in Won't do, its state kept, so u brings it back and ⏎ then resumes its
    /// conversation. It asks first, saying what it will stop and how it leaves the intent; the
    /// request names the workspace and tab the yes was for, so the backend closes nothing else.
    fn cancel_action(&mut self) -> Action {
        let Some(card) = self.selected_for_control().cloned() else {
            self.say(
                "Nothing is selected for that key to act on: pick a card first.",
                Tone::Info,
            );
            return Action::None;
        };
        // A workspace with no intent: closed, after its question, by a backend that can (intent
        // 0591, K1-27).
        let Some(detail) = &card.detail else {
            return self.bare_cancel(&card);
        };
        if self.still_changing(&card, "cancel") || self.cancel_once_open(&card) {
            return Action::None;
        }
        if self.key_opens_waiting(&card.key, "cancel") {
            return Action::None;
        }
        self.backlog_focus = Some(card.key.clone());
        let reference = card.reference.clone();
        // Its agent runs in a terminal outside herdr: X stops it there, after the usual question,
        // and sets the work aside (intent 0591). An older backend refuses, saying so.
        if !card.cancellable()
            && !card.dismissed()
            && !card.done()
            && card.intake() != Some("pending")
            && let Some((stops, since)) = self.outside_said(&card)
        {
            let Some(machine) = self.home_machine(&card) else {
                return Action::None;
            };
            if !self.can_change(machine) {
                return Action::None;
            }
            // `outside_since` names the window this yes was for: its backend stops only that one.
            let request = json!({"action":"cancel","id":detail.id,"outside_since":since});
            self.mode = Mode::Confirm {
                machine,
                request,
                title: format!("Cancel {reference}?"),
                message: format!(
                    "Stops {stops} once its current turn ends, then sets {reference} aside in Won't do. Anything it hadn't committed stays as it left it. Its log says you cancelled it; u brings it back, and ⏎ then resumes its conversation in herdr."
                ),
                back: Box::new(self.mode.clone()),
                target: Some(Box::new(CardSnapshot::of(&card))),
                queue: false,
                ready_at: None,
                plain: false,
                choices: vec![],
                enter_queues: false,
            };
            return Action::None;
        }
        let finished = card.done()
            || card.state.as_deref() == Some("closed")
            || card.attention == Attention::Done;
        // Done work, or work in Won't do, with a workspace still open on it: to a backend that
        // does the step first, X stops what runs there and leaves the card as it is (intent
        // 0591, K1-29). Any other says why there's nothing to cancel.
        let stays = if card.dismissed() {
            Some("in Won't do")
        } else if finished {
            Some("done")
        } else {
            None
        }
        // Asked of the backend where its workspace runs, which is where X's request goes.
        .filter(|_| card.is_open_workspace() && self.steps_on(card.machine_index));
        if !card.cancellable() && stays.is_none() {
            let said = if card.dismissed() {
                format!("{reference} is in won't do already.")
            } else if finished {
                format!("{reference} is done already; there's nothing to cancel.")
            } else if card.closable() || waiting_work(&card) {
                // Nothing to stop: X's end is Won't do, so it sets it aside, after its question
                // (intent 0591, K1-30, K1-31).
                return self.ask_to_end(&card, "dismiss");
            } else {
                format!("X cancels work an agent is at; {reference} has none.")
            };
            self.say(said, Tone::Info);
            return Action::None;
        }
        // Its workspace is where the card came from: that machine's herdr closes it.
        let machine = card.machine_index;
        if !self.can_change(machine) {
            return Action::None;
        }
        // Work an agent is at, to a backend that does the step first: sent as C and x send theirs,
        // naming no place, so its backend asks its own question, naming where its agent is, with
        // `wrap-up` beside yes (the end-to-end check's fourth failure). Kerdr's question named the
        // place, which its backend took as the yes, so its wrap-up was never offered. An older
        // backend gets Kerdr's own question, as before.
        if stays.is_none() && self.steps_on(machine) {
            self.action_from = Some(CardSnapshot::of(&card));
            return Action::Mutate {
                machine,
                request: json!({"action":"cancel","id":detail.id}),
            };
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
        // What its feed showed running there when X was pressed, so its yes, held or late, never
        // stops a workspace remade under the same id before the board caught up (r4-a's point 13).
        // Its yes, and any resend, carry this list, never a fresh one.
        if let Some(conversations) =
            self.card_conversations(&card, machine, &session, &workspace, card.tab_id.as_deref())
        {
            request["conversations"] = json!(conversations);
        }
        let place = card
            .workspace_label
            .clone()
            .unwrap_or_else(|| workspace.clone());
        let what = self.stops_what(&card, machine);
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
        let mut message = match stays {
            Some(stays) => format!(
                "Stops {what} and closes {closes}. {reference} stays {stays}. Anything it hadn't committed stays as it left it."
            ),
            None => format!(
                "Stops {what} and closes {closes}, then sets {reference} aside in Won't do. Anything it hadn't committed stays as it left it. Its log says you cancelled it; u brings it back, and ⏎ then resumes its conversation."
            ),
        };
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
            title: match stays {
                Some(_) => format!("Stop {reference}'s agent?"),
                None => format!("Cancel {reference}?"),
            },
            message,
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(&card))),
            queue: false,
            ready_at: None,
            plain: false,
            choices: vec![],
            enter_queues: false,
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
        let mut session = match (self.scope(), tenant) {
            (Scope::Session(_, s), _) => s.clone(),
            (_, Some(t)) => t.to_string(),
            _ => "default".into(),
        };
        // Words a save gave back go where they were first sent, as the box then says.
        let (filed, unfiled) = match &self.draft_came_back {
            Some(back) => self.sendable(back.machine, &back.filed),
            None => self.filed_for(machine),
        };
        // On an owner, its own session; the backend homes it by its owner either way.
        if let Some(code) = self
            .scope()
            .owner()
            .and_then(|o| self.owner_info(o))
            .and_then(|o| o.code.clone())
        {
            session = code;
        }
        self.mode = Mode::Compose {
            draft: Draft {
                words: self.draft.clone(),
                machine,
                session,
                images: self.draft_images.clone(),
                came_back: self.draft_came_back.clone(),
                filed,
                unfiled,
                edit: None,
            },
            back: Box::new(self.mode.clone()),
        };
    }

    /// Who a new intent made in this view is filed under, sent to `machine`'s backend, and what
    /// the view wanted that the backend can't take, as the box says it (intent 0595).
    pub(crate) fn filed_for(&self, machine: usize) -> (Filed, Option<String>) {
        let Some(o) = self
            .scope()
            .owner()
            .filter(|o| *o != crate::owners::UNOWNED)
        else {
            return (Filed::default(), None);
        };
        let Some(info) = self.owner_info(o) else {
            return (Filed::default(), None);
        };
        let wanted = Filed {
            organization: Some(info.organization(&self.board.owners).id.clone()),
            tenant: info.parent.is_some().then(|| info.id.clone()),
            team: match self.scope() {
                Scope::Team(_, t) => t.clone(),
                _ => None,
            },
        };
        self.sendable(machine, &wanted)
    }

    /// What of `wanted` `machine`'s backend takes, by its feed's `create_fields`, and what the box
    /// says of the rest: a team it can't take goes under No team, an owner it can't take under
    /// whoever the words say (intent 0595).
    pub(crate) fn sendable(&self, machine: usize, wanted: &Filed) -> (Filed, Option<String>) {
        let Some(owner) = wanted.tenant.as_deref().or(wanted.organization.as_deref()) else {
            return (Filed::default(), None);
        };
        let name = self
            .owner_info(owner)
            .map(|o| o.label().to_string())
            .unwrap_or_else(|| owner.to_string());
        let feed = self.machines.get(machine).and_then(|m| m.feed.as_ref());
        let takes = |field: &str| feed.is_some_and(|f| f.creates_with(field));
        if !takes("organization") || (wanted.tenant.is_some() && !takes("tenant")) {
            return (
                Filed::default(),
                Some(format!(
                    "This machine's backend can't file new work under {name} yet: the agent that writes it up works out whose it is from your words."
                )),
            );
        }
        if wanted.team.is_some() && !takes("team") {
            return (
                Filed {
                    team: None,
                    ..wanted.clone()
                },
                Some(format!(
                    "This machine's backend can't file work under a team yet: it goes under {name}'s No team."
                )),
            );
        }
        (wanted.clone(), None)
    }

    /// Ctrl+Space: the same letters as the direct keys, listed, so the menu is a reminder and
    /// never a second set of keys to learn.
    fn palette_key(&mut self, key: KeyEvent, back: Box<Mode>, target: Option<String>) -> Action {
        self.mode = (*back).clone();
        let card_key = matches!(
            key.code,
            KeyCode::Char('d' | 'e' | 'y' | 'x' | 'u' | 't' | 'B' | 'J' | 'K' | 'C' | 'X' | 's')
        );
        // A refresh while the menu was open may have moved the selection off the card it opened
        // on: its keys act on that card, found by its key, never on whatever took its place, and
        // say so only when it has gone from the board (intent 0591, K1-32).
        if card_key && self.selected_for_control().map(|c| &c.key) != target.as_ref() {
            let Some(k) = target.clone() else {
                // Opened on no card: its card keys act on none, whatever is selected now.
                self.say(
                    "The menu was opened with no card selected, so nothing was changed.",
                    Tone::Warn,
                );
                return Action::None;
            };
            let reference = self
                .card(&k)
                .map(|c| c.reference.clone())
                .or_else(|| {
                    k.rsplit_once(':')
                        .and_then(|(_, id)| id.parse::<u32>().ok())
                        .map(|n| format!("#{n}"))
                })
                .unwrap_or_else(|| "That card".into());
            if self.card(&k).is_none() {
                self.say(
                    format!("{reference} is gone from the board, so nothing was changed."),
                    Tone::Warn,
                );
                return Action::None;
            }
            self.sel.key = Some(k.clone());
            self.backlog_focus = Some(k.clone());
            self.rebuild();
            // It left this view (a backlog card that started, say): nothing here acts on it.
            if self.selected_for_control().map(|c| &c.key) != Some(&k) {
                self.say(
                    format!(
                        "{reference} moved out of this view while the menu was open, so nothing was changed."
                    ),
                    Tone::Warn,
                );
                return Action::None;
            }
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
            KeyCode::Char('M') => self.open_machines(),
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
        // An item's text, edited here because no editor could start (K1-87): ⏎ saves it, Esc
        // keeps his version, and it holds words only.
        if draft.edit.is_some() {
            let newline = key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Esc => {
                    self.edit_box_left(draft, *back);
                    return Action::None;
                }
                KeyCode::Enter if !newline => return self.edit_box_saved(draft, *back),
                KeyCode::Enter => draft.words.push('\n'),
                KeyCode::Char('j') if ctrl => draft.words.push('\n'),
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
                KeyCode::Char(c) if !ctrl => draft.words.push(c),
                _ => {}
            }
            self.mode = Mode::Compose { draft, back };
            return Action::None;
        }
        if key.code == KeyCode::Esc {
            self.draft = draft.words;
            self.draft_images = draft.images;
            self.draft_came_back = draft.came_back;
            // Kept as you left them, emptied included.
            self.keep_words_now();
            // A read for words already sent carries on for them; the box's own stop.
            self.image_reads.retain(|r| r.for_send);
            self.mode = *back;
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let newline = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        let room = draft.words.len() < max_words(self.fixes_input(draft.machine));
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
                } else if self.send_after_load.is_some() && self.box_loading().is_some() {
                    // His last new intent still waits for its image, and this one's is read after
                    // it: the box keeps these words until theirs is in.
                    self.say(
                        "Your last new intent's image is still being read, and this one's after it: Enter sends these words once it's in.",
                        Tone::Info,
                    );
                } else if let Some(load) = self.box_loading() {
                    // The image still being read goes with them: the box closes now, and they're
                    // sent once it's in (intent 0445), with any read asked after it (intent 0591,
                    // K1-35). What the box kept for next time is these words, now on their way: it
                    // opens empty, and nothing else typed meanwhile is cleared when they go.
                    self.clear_kept_draft();
                    for r in &mut self.image_reads {
                        r.for_send = true;
                    }
                    self.send_troubles.clear();
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
    pub(super) fn send_draft(&mut self, mut draft: Draft) -> Action {
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
        // What the backend takes now: one that has stopped taking a team since gets none.
        let (filed, _) = self.sendable(draft.machine, &draft.filed);
        draft.filed = filed;
        if let Some(s) = draft.came_back.take().filter(|s| {
            s.words == words
                && s.images == images
                && self.sendable(s.machine, &s.filed).0 == s.filed
        }) {
            draft.machine = s.machine;
            draft.session = s.session.clone();
            draft.filed = s.filed.clone();
            if !self.submissions.iter().any(|k| k.id == s.id) {
                self.submissions.push(s);
            }
        } else if let Some(s) = self
            .submissions
            .iter()
            .find(|s| s.unknown && s.words == words && s.images == images && s.filed == draft.filed)
        {
            draft.machine = s.machine;
            draft.session = s.session.clone();
        }
        // The same request only with the same words, images, owner and team (intent 0595).
        let id = match self.submissions.iter().find(|s| {
            s.words == words
                && s.images == images
                && s.machine == draft.machine
                && s.session == draft.session
                && s.filed == draft.filed
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
                    filed: draft.filed.clone(),
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
        let request = create_request(&id, &words, &images, &draft.session, &draft.filed);
        // Past what its backend takes: nothing is sent, and the words come back (intent 0591,
        // K1-68): words kept for one machine and sent to another included (Codex's review 85, 6).
        if let Err(why) = self.fits_backend(draft.machine, &request) {
            self.give_back(draft.words, draft.images, draft.came_back);
            self.fail(format!(
                "Your new intent wasn't saved: {why}. Your words are kept: c opens them."
            ));
            return Action::None;
        }
        self.emit_mutation(draft.machine, request)
    }

    /// Empties what the box keeps for next time: Enter has taken those words.
    fn clear_kept_draft(&mut self) {
        self.draft.clear();
        self.draft_images.clear();
        self.draft_came_back = None;
    }

    /// The last image read for the box that's open, not one for words already sent.
    fn box_loading(&self) -> Option<u64> {
        self.image_reads
            .iter()
            .rev()
            .find(|r| !r.for_send)
            .map(|r| r.id)
    }

    /// Words a failed save gives back: into the box for the next c when it's free, else kept
    /// for a later c. Whether they went into the box.
    pub(super) fn give_back(
        &mut self,
        words: String,
        images: Vec<images::Image>,
        from: Option<crate::app::Submission>,
    ) -> bool {
        // Words that came back while q waited for them to be saved: Kerdr stays open with them
        // (intent 0591, K1-45).
        self.quitting = None;
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
        let fix = match &self.mode {
            Mode::Compose { draft, .. } => self.fixes_input(draft.machine),
            _ => false,
        };
        let kickoff_fix = match &self.mode {
            Mode::Kickoff { form, .. } => self.fixes_input(form.machine),
            _ => false,
        };
        let edit_box = matches!(&self.mode, Mode::Compose { draft, .. } if draft.edit.is_some());
        if matches!(self.mode, Mode::Compose { .. }) && !edit_box {
            // Spaces and blank lines are words; only a paste with nothing in it means an image.
            if text.is_empty() {
                return Action::LoadImages(images::Source::Clipboard);
            }
            if let Some(paths) = images::paths_in_paste_for(&text, fix) {
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
            Mode::Kickoff { form, .. } => App::kickoff_paste(form, &text, kickoff_fix),
            Mode::Compose { draft, .. } if draft.edit.is_some() => {
                draft.words.push_str(&text);
            }
            Mode::Compose { draft, .. } => {
                let room = max_words(fix).saturating_sub(draft.words.len());
                let mut end = text.len().min(room);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                draft.words.push_str(&text[..end]);
                // Never cut short unseen, nor lost: the rest is kept for c.
                if end < text.len() {
                    let said = format!(
                        "Only the first {} of that paste went in: the box holds {}. The rest is kept, and c brings it back once these words have gone.",
                        images::size(end),
                        images::size(max_words(fix))
                    );
                    let rest = text[end..].to_string();
                    self.keep_left_over(rest);
                    self.say(said, Tone::Warn);
                }
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

    /// Whether `machine`'s backend fixes input (images::FIX_INPUT): then the box takes long
    /// words, any kind and number of images within 20 MB, and a request up to 32 MiB.
    pub(crate) fn fixes_input(&self, machine: usize) -> bool {
        self.machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .is_some_and(|f| f.supports(images::FIX_INPUT))
    }

    /// Starts reading images for the new-intent box at once, so a clipboard read takes what the
    /// clipboard holds now, whatever reads are still going (Codex's review 85, 4): the read's
    /// number and the room its draft has left, or None when there's no box to read them into.
    /// What it finds goes in after what the reads before it found (intent 0591, K1-35).
    pub fn begin_image_load(&mut self, source: &images::Source) -> Option<(u64, images::Room)> {
        let Mode::Compose { draft, .. } = &self.mode else {
            return None;
        };
        let fix = self.fixes_input(draft.machine);
        let at = match source {
            images::Source::Smaller(_, at) => *at,
            _ => draft.words.len(),
        };
        let room = images::Room::for_backend(&draft.images, fix);
        let after = self.box_loading().is_some();
        self.image_loads += 1;
        let id = self.image_loads;
        self.image_reads.push_back(crate::app::ImageRead {
            id,
            at,
            for_send: false,
            started: Instant::now(),
            what: match source {
                images::Source::Clipboard => "Reading the clipboard",
                images::Source::Files(_) => "Reading the image",
                images::Source::Smaller(..) => "Making a smaller copy",
            },
            got: None,
        });
        if after {
            self.say(
                format!(
                    "{} too; it goes in after the last.",
                    reading(source).trim_end_matches('…')
                ),
                Tone::Info,
            );
        } else {
            self.say(reading(source), Tone::Info);
        }
        Some((id, room))
    }

    /// After words and tags went in at `at`, `inserted` long: the reads still to come for the same
    /// words, and smaller copies still asked about, keep their places after them.
    fn shift_queued(&mut self, for_send: bool, at: usize, inserted: usize) {
        for r in self
            .image_reads
            .iter_mut()
            .filter(|r| r.for_send == for_send)
        {
            if r.at >= at {
                r.at += inserted;
            }
        }
        if !for_send {
            for a in &mut self.smaller_asks {
                if a.at >= at {
                    a.at += inserted;
                }
            }
        }
    }

    /// A read started by begin_image_load has ended: what it found waits for the reads before
    /// it, then goes in (apply_reads). A read nobody waits for any more (its box closed, or it
    /// ran past its deadline) is let go.
    pub fn images_loaded(&mut self, id: u64, loading: images::Loading) -> Action {
        if let Some(r) = self
            .image_reads
            .iter_mut()
            .find(|r| r.id == id && r.got.is_none())
        {
            r.got = Some(loading);
        }
        self.apply_reads()
    }

    /// What reads found, put in in the order they were asked: the box's into the box, unless a
    /// question about an earlier one's images is up, which they wait for; the words Enter sent go
    /// once every read they waited for is in, the action that sends them (intent 0445).
    pub fn apply_reads(&mut self) -> Action {
        let mut said: Vec<(String, Tone)> = vec![];
        let action = self.apply_reads_saying(&mut said);
        if !said.is_empty() {
            let tone = if said.iter().any(|(_, t)| *t == Tone::Warn) {
                Tone::Warn
            } else {
                Tone::Good
            };
            let text = said
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
                .join(" ");
            // A question asked meanwhile (a smaller copy) is up already: said under it.
            self.say(text, tone);
        }
        action
    }

    fn apply_reads_saying(&mut self, said: &mut Vec<(String, Tone)>) -> Action {
        // A read past its deadline is let go, said as what it found, so the box, and words
        // waiting to be sent, never wait on it longer (Codex's review 85, 8).
        for read in self.image_reads.iter_mut().filter(|r| r.got.is_none()) {
            let most = images::read_most(read.what);
            if read.started.elapsed() >= most {
                read.got = Some(images::Loading::refused(format!(
                    "{} took longer than {} seconds, so Kerdr stopped waiting for it, and nothing from it went in.",
                    read.what,
                    most.as_secs()
                )));
            }
        }
        loop {
            let Some(front) = self.image_reads.front() else {
                return Action::None;
            };
            if front.got.is_none() {
                return Action::None;
            }
            let behind_question = matches!(&self.mode, Mode::Confirm { back, .. }
                if matches!(**back, Mode::Compose { .. }));
            if !front.for_send && behind_question {
                return Action::None;
            }
            let Some(read) = self.image_reads.pop_front() else {
                return Action::None;
            };
            let loading = read.got.unwrap_or_default();
            if !read.for_send {
                said.extend(self.attach_quietly(loading, Some(read.at)));
                continue;
            }
            let Some((load, mut draft)) = self.send_after_load.take() else {
                continue;
            };
            let fix = self.fixes_input(draft.machine);
            let mut got = attach_into(&mut draft, loading, Some(read.at), fix);
            if let Some(rest) = got.left_over.take() {
                self.keep_left_over(rest);
            }
            self.shift_queued(true, got.at, got.inserted);
            if got.refused || !got.oversized.is_empty() {
                self.send_troubles.extend(
                    got.said
                        .iter()
                        .filter(|s| !s.starts_with("Attached"))
                        .cloned(),
                );
                if !got.oversized.is_empty() {
                    self.send_troubles.push(too_large_to_send(&got.oversized));
                }
            }
            // More read for these words: they wait for them too (K1-35).
            if self.image_reads.iter().any(|r| r.for_send) {
                self.send_after_load = Some((load, draft));
                continue;
            }
            let troubles = std::mem::take(&mut self.send_troubles);
            if !troubles.is_empty() {
                // Not everything read went in (none found, the box full, too many or too big).
                // His words may be about it, so whether they go without it is his call (intent
                // 0591, K1-37).
                self.ask_send_without(draft, &troubles);
                return Action::None;
            }
            return self.send_draft(draft);
        }
    }

    /// Asks whether a new intent's words go without the image that didn't go in. They're given
    /// back at once, with what did go in, so nothing is lost whatever becomes of the question; a
    /// yes takes them out again and sends them.
    fn ask_send_without(&mut self, draft: Draft, said: &[String]) {
        let into_box = self.give_back(
            draft.words.clone(),
            draft.images.clone(),
            draft.came_back.clone(),
        );
        let back = if into_box {
            "c opens them again."
        } else {
            "c brings them back once the box is empty."
        };
        let message = format!(
            "{} Send your new intent's words as they are, without it? n keeps them unsent, and {back}",
            said.join(" ")
        );
        self.send_without_count += 1;
        let n = self.send_without_count;
        self.send_without.push(crate::app::SendWithout {
            n,
            machine: draft.machine,
            draft,
            message,
        });
        // He's typing other words, or answering something else (another of these, say): never
        // asked over that, where a y or ⏎ meant for it would send these. Asked once he's back on
        // the board (Codex's review).
        if !self.enter_can_land_here() {
            self.say(
                "Your new intent's image didn't go in, so its words weren't sent; Kerdr asks about them once you're back on the board.",
                Tone::Info,
            );
            return;
        }
        self.ask_send_without_now(n);
    }

    /// The question about these words, over the board: its answer is for them alone.
    pub(super) fn ask_send_without_now(&mut self, n: u64) {
        let Some(w) = self.send_without.iter().find(|w| w.n == n) else {
            return;
        };
        let (machine, message) = (w.machine, w.message.clone());
        self.mode = Mode::Confirm {
            machine,
            request: json!({ "action": crate::app::SEND_WITHOUT, "n": n }),
            title: "Send without the image?".into(),
            message,
            back: Box::new(self.mode.clone()),
            target: None,
            queue: false,
            ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
    }

    /// The yes to ask_send_without: the words it asked about go, taken back from where they were
    /// kept, as long as they're there unchanged.
    fn send_without(&mut self, n: u64) -> Action {
        let Some(at) = self.send_without.iter().position(|w| w.n == n) else {
            return Action::None;
        };
        let draft = self.send_without.remove(at).draft;
        let kept = if self.draft == draft.words && self.draft_images == draft.images {
            self.draft.clear();
            self.draft_images.clear();
            self.draft_came_back = None;
            true
        } else if let Some(i) = self
            .returned
            .iter()
            .position(|r| r.words == draft.words && r.images == draft.images)
        {
            self.returned.remove(i);
            true
        } else {
            false
        };
        if !kept {
            self.say(
                "Nothing was sent: your words have moved since, and c brings them back.",
                Tone::Info,
            );
            return Action::None;
        }
        self.send_draft(draft)
    }

    /// Puts images read from the clipboard or a pasted path into the new-intent box, each as
    /// the next `[Image #N]` tag at the end of the words, and says what went in and why
    /// anything didn't.
    pub fn attach_images(&mut self, got: images::Loading) {
        self.attach_images_at(got, None);
    }

    /// As attach_images, with the tags at `at` in the words (where they ended when the read
    /// began), or at the end when the words have since been cut shorter than that. Images past
    /// what this intent may carry, for a backend that fixes input, are asked about: a smaller copy
    /// of each goes in only if he says so.
    fn attach_images_at(&mut self, got: images::Loading, at: Option<usize>) {
        if let Some((said, tone)) = self.attach_quietly(got, at) {
            self.say(said, tone);
        }
    }

    /// As attach_images_at, giving back what to say rather than saying it, so what several reads
    /// found, put in at one turn, is said together (Codex's review 85, 8).
    fn attach_quietly(
        &mut self,
        got: images::Loading,
        at: Option<usize>,
    ) -> Option<(String, Tone)> {
        let fix = match &self.mode {
            Mode::Compose { draft, .. } => self.fixes_input(draft.machine),
            _ => return None,
        };
        let Mode::Compose { draft, .. } = &mut self.mode else {
            return None;
        };
        let mut attached = attach_into(draft, got, at, fix);
        let after = attached.at + attached.inserted;
        if let Some(rest) = attached.left_over.take() {
            self.keep_left_over(rest);
        }
        self.shift_queued(false, attached.at, attached.inserted);
        let said = (!attached.said.is_empty()).then(|| {
            let tone = if attached.refused {
                Tone::Warn
            } else {
                Tone::Good
            };
            (attached.said.join(" "), tone)
        });
        if !attached.oversized.is_empty() {
            self.ask_smaller(attached.oversized, after);
        }
        said
    }

    /// Yes to a backend's question about a new intent's words: the same send again, its words and
    /// images as kept with it, with the question's token and `override`.
    fn create_question_yes(&mut self, machine: usize, submission: &str, token: &str) -> Action {
        let Some(s) = self
            .submissions
            .iter()
            .find(|s| s.id == submission)
            .cloned()
        else {
            self.say(
                "Nothing was sent: those words have moved since, and c brings them back.",
                Tone::Info,
            );
            return Action::None;
        };
        let mut request = create_request(&s.id, &s.words, &s.images, &s.session, &s.filed);
        request["confirm"] = json!(token);
        request["override"] = json!(true);
        self.closed_on_send = true;
        self.emit_mutation(machine, request)
    }

    /// No to it: nothing is saved, and his words open in the box as they were, to change (or,
    /// when the box holds other words, wait for c).
    fn create_question_no(&mut self, submission: &str) {
        let Some(i) = self.submissions.iter().position(|s| s.id == submission) else {
            return;
        };
        let s = self.submissions.remove(i);
        let into_box = self.give_back(s.words, s.images, None);
        if into_box && matches!(self.mode, Mode::Board | Mode::Backlog { .. }) {
            self.new_intent();
            self.say(
                "Your new intent wasn't saved: its words are open here, as you left them.",
                Tone::Info,
            );
        } else {
            self.say(
                "Your new intent wasn't saved: its words are kept, and c brings them back.",
                Tone::Info,
            );
        }
    }

    /// Text that didn't fit the box, kept as words given back: c brings it back once the box is
    /// free, and the words file keeps it meanwhile (Codex's review 85, 5).
    fn keep_left_over(&mut self, rest: String) {
        self.returned.push_back(crate::app::Returned {
            words: rest,
            images: vec![],
            from: None,
        });
        self.keep_words_now();
    }

    /// Asks whether smaller copies of images too large to send go in their place, their tags at
    /// `at`. The images themselves are never changed; no leaves them out.
    fn ask_smaller(&mut self, images: Vec<images::Loaded>, at: usize) {
        let Mode::Compose { draft, .. } = &self.mode else {
            return;
        };
        let machine = draft.machine;
        let used: usize = draft.images.iter().map(|i| i.bytes.len()).sum();
        let left = images::MAX_IMAGES_BYTES.saturating_sub(used);
        self.send_without_count += 1;
        let n = self.send_without_count;
        let message = smaller_question(&images, left);
        let one = images.len() == 1;
        self.smaller_asks
            .push(crate::app::SmallerAsk { n, images, at });
        self.mode = Mode::Confirm {
            machine,
            request: json!({ "action": crate::app::SMALLER, "n": n }),
            title: if one {
                "Attach a smaller copy?".into()
            } else {
                "Attach smaller copies?".into()
            },
            message,
            back: Box::new(self.mode.clone()),
            target: None,
            queue: false,
            ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
    }

    /// The answer to ask_smaller: yes reads smaller copies, their tags where the images' would
    /// have gone; no leaves them out, saying so.
    fn smaller_answered(&mut self, n: u64, yes: bool) -> Action {
        let Some(i) = self.smaller_asks.iter().position(|a| a.n == n) else {
            return Action::None;
        };
        let ask = self.smaller_asks.remove(i);
        if yes {
            return Action::LoadImages(images::Source::Smaller(ask.images, ask.at));
        }
        let names: Vec<String> = ask.images.iter().map(|i| i.from.clone()).collect();
        let they = if names.len() == 1 {
            "It stays"
        } else {
            "They stay"
        };
        self.say(
            format!(
                "Left out {}. {they} as {} where you copied {}.",
                and_names(&names),
                if names.len() == 1 {
                    "it was"
                } else {
                    "they were"
                },
                if names.len() == 1 { "it" } else { "them" }
            ),
            Tone::Info,
        );
        Action::None
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
                choices,
                enter_queues,
                ..
            } => {
                // A yes is a new request: never the question's op (intent 0591). Nor the `held`
                // and `rev` of a request that waited in the hold file: the yes is pressed now, and
                // a held stop's yes marked held would only get its question back (release 4).
                if let Some(fields) = request.as_object_mut() {
                    for waited in ["op", "held", "rev"] {
                        fields.remove(waited);
                    }
                }
                // One of the answers its backend offers beyond yes and no, by its key (intent 0591).
                let keys = crate::actions::choice_keys(&choices);
                let chosen = match key.code {
                    KeyCode::Char(c) => choices
                        .iter()
                        .zip(&keys)
                        .find(|(_, k)| **k == Some(c))
                        .map(|(choice, _)| choice.clone()),
                    _ => None,
                };
                // At the most a session holds, ⏎ queues it: its queue choice when it offers one.
                let queue_choice = choices.iter().find(|c| c.id == "queue").cloned();
                // A question that just appeared, or a key held down, never answers it.
                let early = key.kind == KeyEventKind::Repeat
                    || ready_at.is_some_and(|at| Instant::now() < at);
                // Longer than the screen holds, it scrolls; and an answer while some of it is still
                // below shows the rest first, as the Kickoff box's ⏎ does. n and Esc still leave
                // (Codex's review).
                match key.code {
                    KeyCode::PageDown | KeyCode::Down => {
                        self.question_view.down(3);
                        return Some(Action::None);
                    }
                    KeyCode::PageUp | KeyCode::Up => {
                        self.question_view.up(3);
                        return Some(Action::None);
                    }
                    _ => {}
                }
                let answers = matches!(key.code, KeyCode::Enter | KeyCode::Char('y' | 'w'))
                    || chosen.is_some();
                if answers && !early && self.question_view.unseen() {
                    self.question_view.next_page();
                    if let Mode::Confirm { ready_at, .. } = &mut self.mode {
                        *ready_at = Some(Instant::now() + std::time::Duration::from_millis(500));
                    }
                    return Some(Action::None);
                }
                // A backend's question about a new intent's words (release 5): yes sends them
                // again, as the same send, with its token; no opens them in the box, unsent.
                if request["action"] == "create" && request.get("confirm").is_some() {
                    let submission = request["submission"].as_str().unwrap_or_default();
                    let token = request["confirm"].as_str().unwrap_or_default();
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                            self.mode = *back;
                            self.create_question_no(submission);
                        }
                        KeyCode::Enter | KeyCode::Char('y') if !early => {
                            self.mode = *back;
                            return Some(self.create_question_yes(machine, submission, token));
                        }
                        _ => {}
                    }
                    return Some(Action::None);
                }
                // Kerdr's own question whether to open both versions of a text (K1-100).
                if request["action"] == crate::app::OPEN_BOTH {
                    let card = request["key"].as_str().unwrap_or_default().to_string();
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                            self.mode = *back;
                            self.open_both_answered(&card, false);
                        }
                        KeyCode::Enter | KeyCode::Char('y') if !early => {
                            self.mode = *back;
                            self.open_both_answered(&card, true);
                        }
                        _ => {}
                    }
                    return Some(Action::None);
                }
                // Kerdr's own question about smaller copies of images too large to send.
                if request["action"] == crate::app::SMALLER {
                    let n = request["n"].as_u64().unwrap_or_default();
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                            self.mode = *back;
                            return Some(self.smaller_answered(n, false));
                        }
                        KeyCode::Enter | KeyCode::Char('y') if !early => {
                            self.mode = *back;
                            return Some(self.smaller_answered(n, true));
                        }
                        _ => {}
                    }
                    return Some(Action::None);
                }
                // Kerdr's own question about words whose image didn't go in (K1-37).
                if request["action"] == crate::app::SEND_WITHOUT {
                    let n = request["n"].as_u64().unwrap_or_default();
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                            self.mode = *back;
                            self.send_without.retain(|w| w.n != n);
                            self.say(
                                "Your new intent's words are kept, unsent: c brings them back.",
                                Tone::Info,
                            );
                        }
                        KeyCode::Enter | KeyCode::Char('y') if !early => {
                            self.mode = *back;
                            return Some(self.send_without(n));
                        }
                        _ => {}
                    }
                    return Some(Action::None);
                }
                // Kerdr's own question whether to stop a start Esc came too late for (K1-44).
                if request["action"] == crate::app::cancels::STOP_START {
                    let yes = match key.code {
                        KeyCode::Esc | KeyCode::Char('n' | 'q') => false,
                        KeyCode::Enter | KeyCode::Char('y') if !early => true,
                        _ => return Some(Action::None),
                    };
                    self.mode = *back;
                    return Some(self.stop_start_answered(&request, target.map(|t| *t), yes));
                }
                if early
                    && (matches!(key.code, KeyCode::Enter | KeyCode::Char('y' | 'w'))
                        || chosen.is_some())
                {
                    return Some(Action::None);
                }
                // Its yes is ⏎'s when the start it answers was (Codex's review): kept by its card's
                // key, never another source's with the same number (Codex's fifth look).
                let asked_by_enter = self
                    .confirm_by_enter
                    .take()
                    .filter(|k| target.as_ref().is_some_and(|t| t.key == *k));
                // The yes it sends is ⏎'s for that same card, by its key (Codex's sixth look).
                let by_enter = asked_by_enter.clone();
                let queues = key.code == KeyCode::Enter && enter_queues;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('n' | 'q') => {
                        self.mode = *back;
                    }
                    _ if chosen.is_some() || (queues && queue_choice.is_some()) => {
                        self.mode = *back;
                        let choice = chosen.or(queue_choice).unwrap_or_default();
                        return Some(
                            self.answer_choice(machine, request, target, &choice, by_enter),
                        );
                    }
                    // A full session's other answer, from a backend that offers no choices: the
                    // reopen waits in its queue.
                    KeyCode::Char('w') if queue => {
                        self.mode = *back;
                        return Some(self.answer_queue(machine, request, target, by_enter));
                    }
                    KeyCode::Enter if queues && queue => {
                        self.mode = *back;
                        return Some(self.answer_queue(machine, request, target, by_enter));
                    }
                    // At the most a session holds, a yes can't open it: only its queue is offered.
                    // When the queue is the question's own yes, y is that yes, as ⏎ is.
                    KeyCode::Char('y') if enter_queues && (queue || queue_choice.is_some()) => {
                        self.confirm_by_enter = asked_by_enter;
                    }
                    KeyCode::Enter | KeyCode::Char('y') => {
                        self.mode = *back;
                        if !self.confirm_still_applies(machine, &request, target.as_deref()) {
                            return Some(self.ask_again(machine, request, target.map(|t| *t)));
                        }
                        // A backend's own question carries its token: the yes goes back with
                        // `override`, as a start's does (intent 0591). Kerdr's own question has
                        // none, and its yes goes as it always has.
                        if !plain
                            && (matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
                                || (request.get("confirm").is_some()
                                    && matches!(
                                        request["action"].as_str(),
                                        Some("close" | "dismiss" | "cancel" | "edit" | "answer")
                                    )))
                        {
                            request["override"] = json!(true);
                        }
                        let action = self.emit_mutation(machine, request);
                        if matches!(action, Action::Mutate { .. }) {
                            self.enter_sent = by_enter;
                            self.action_from = target.map(|t| *t);
                        }
                        return Some(action);
                    }
                    _ => {
                        // Still asking: the yes to come is ⏎'s as before.
                        self.confirm_by_enter = asked_by_enter;
                    }
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
        let counted = !self.count.is_empty();
        let count: i64 = std::mem::take(&mut self.count).parse().unwrap_or(1);
        match key.code {
            KeyCode::Char(
                c @ ('d' | 'e' | 'y' | 'x' | 'u' | 't' | 'B' | 'J' | 'K' | 'C' | 'X'),
            ) => {
                return Some(self.card_action(c, count));
            }
            // In a peek, D drops what's held for its card: the first, or with a count the one of
            // that number, 2D the second (intent 0591, release 4).
            KeyCode::Char('D') if matches!(self.mode, Mode::Peek { .. }) => {
                let index = counted.then(|| usize::try_from(count - 1).unwrap_or(0));
                return Some(self.drop_held(index));
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
    /// A request as it goes to `machine`: to a backend that fixes input, every one says so
    /// (`fix_input`), so a line that looks like a secret is asked about and an edit's faults fixed,
    /// rather than refused (intent 0591, release 5). To any other, it goes as it is.
    pub(crate) fn with_fix_input(&self, machine: usize, mut request: Value) -> Value {
        if self.fixes_input_at(machine, &request) {
            request["fix_input"] = json!(true);
        } else if let Some(fields) = request.as_object_mut() {
            // What only a backend that fixes input reads goes nowhere else.
            fields.remove("fix_input");
            fields.remove("base");
        }
        request
    }

    /// Whether the request, sent to `machine`, ends at a backend that fixes input: that machine's,
    /// and the one it's sent on to (`forward_to`) when the board knows it (Codex's review 85, 6).
    pub(crate) fn fixes_input_at(&self, machine: usize, request: &Value) -> bool {
        let onward = request["forward_to"]
            .as_str()
            .and_then(|host| self.machine_index(host));
        self.fixes_input(machine) && onward.is_none_or(|m| self.fixes_input(m))
    }

    /// Whether a request, as it goes to `machine`, is one its backend takes (Codex's review 85, 6):
    /// checked at the last moment, whatever sent it (a key, kept words, the hold file, a resend
    /// on to another machine). A backend that fixes input takes up to 32 MiB and images of any kind
    /// and number within 20 MB; any other, 0.35.0's 64 KiB and 20 PNG, JPEG, GIF or WebP images
    /// of at most 10 MB, 20 MB together. Why not, in words that end "so nothing was sent".
    pub(crate) fn fits_backend(&self, machine: usize, request: &Value) -> Result<(), String> {
        let fix = self.fixes_input_at(machine, request);
        let name = self
            .machines
            .get(machine)
            .map(|m| m.name())
            .unwrap_or_else(|| "its machine".into());
        if let Some(sent) = request["images"].as_array() {
            use base64::Engine;
            let mut total = 0;
            if !fix && sent.len() > images::MAX_IMAGES {
                return Err(format!(
                    "{name}'s backend takes at most {} images in a new intent and this one has {}, so nothing was sent",
                    images::MAX_IMAGES,
                    sent.len()
                ));
            }
            for image in sent {
                let tag = image["tag"].as_u64().unwrap_or_default();
                let bytes = image["data"]
                    .as_str()
                    .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                    .unwrap_or_default();
                let kind = images::kind_of(&bytes);
                if !fix && !kind.is_some_and(images::shown) {
                    return Err(format!(
                        "{name}'s backend takes only PNG, JPEG, GIF and WebP images, and [Image #{tag}] is {}, so nothing was sent",
                        kind.map_or("no kind it knows".to_string(), |k| format!(
                            "a {} image",
                            images::kind_name(k)
                        ))
                    ));
                }
                let most = if fix {
                    images::FIX_IMAGE_BYTES
                } else {
                    images::MAX_IMAGE_BYTES
                };
                if bytes.len() > most {
                    return Err(format!(
                        "[Image #{tag}] is {}, over the {} {name}'s backend takes for one image, so nothing was sent",
                        images::size(bytes.len()),
                        images::size(most)
                    ));
                }
                total += bytes.len();
            }
            if total > images::MAX_IMAGES_BYTES {
                return Err(format!(
                    "the images are {} together, over the {} {name}'s backend takes, so nothing was sent",
                    images::size(total),
                    images::size(images::MAX_IMAGES_BYTES)
                ));
            }
        }
        let mut sized = request.clone();
        if fix {
            sized["fix_input"] = json!(true);
        }
        if crate::actions::too_large_for(&sized, fix) {
            return Err(if fix {
                "the request is too large to send (32 MiB at most), so nothing was sent".to_string()
            } else {
                crate::actions::too_large_said(request)
            });
        }
        Ok(())
    }

    pub fn queue_action(&mut self, machine: usize, request: Value) {
        let request = self.with_fix_input(machine, request);
        // Whether ⏎ asked for it: taken now, sent or not, so a request a check below stops can't
        // leave it for another card's (Codex's sixth look).
        let enter_sent = self.enter_sent.take();
        // The card it's for when it goes without its snapshot: taken now, likewise.
        let action_key = self.action_key.take();
        if request["action"] == "create" {
            return self.queue_create(machine, request);
        }
        let from = self.action_from.take();
        // Only what its backend takes, checked on the machine it goes to, whatever sent it: a
        // key, a question's yes, a resend (Codex's fifth look).
        if let Some(what) = self.missing_on(machine, &request, from.as_ref(), false) {
            self.not_sent_for(machine, &request, from.as_ref(), what, false);
            return;
        }
        if !self.starts_named_account_on(machine, &request) {
            let reference = from
                .as_ref()
                .and_then(|f| self.card(&f.key))
                .map(|c| c.reference.clone())
                .unwrap_or_else(|| self.reference_for(request["id"].as_str().unwrap_or_default()));
            let name = self
                .machines
                .get(machine)
                .map(|m| m.name())
                .unwrap_or_default();
            self.say(
                format!(
                    "{name}'s backend can't start work on a chosen account, so nothing was sent for {reference}: s on its card brings its ark-sdlc up to date first."
                ),
                Tone::Warn,
            );
            return;
        }
        let id = request["id"].as_str().unwrap_or_default().to_string();
        // Its card: the one it was asked from, else the one it's for though that's gone from the
        // board, and no other (Codex's seventh look). Only a change that never had a card goes
        // by number: an item's new text is for the card whose text Kerdr holds (intent 0414),
        // else the one its machine's feed names by its source and number, else the only card
        // with that number on its machine, else the only one anywhere. Never a guess between two
        // sources' cards with the same number (Codex's sixth look, swept).
        let held = || {
            (request["action"] == "edit")
                .then(|| self.edit_card(&id, machine))
                .flatten()
        };
        let key = from
            .as_ref()
            .map(|f| f.key.clone())
            .or(action_key)
            .or_else(held)
            .or_else(|| {
                let own = self.intent_key(machine, &id, None);
                if self.card(&own).is_some() {
                    return Some(own);
                }
                let mine = |c: &&Card| c.detail.as_ref().is_some_and(|d| d.id == id);
                let only = |found: Vec<&Card>| match found.as_slice() {
                    [one] => Some(one.key.clone()),
                    _ => None,
                };
                let cards = &self.board.cards;
                let here: Vec<&Card> = cards
                    .iter()
                    .filter(mine)
                    .filter(|c| c.machine_index == machine)
                    .collect();
                if here.is_empty() {
                    only(cards.iter().filter(mine).collect())
                } else {
                    only(here)
                }
            });
        // A key on a card whose start, save or write-up is on its way waits for that, and is
        // decided again then (intent 0591, release 4).
        if self.hold_behind(machine, &request, from.as_ref(), key.as_deref()) {
            return;
        }
        // The same change in the hold file, kept held because its backend doesn't check one, or
        // on its way from a Kerdr already: it goes as that record, with its op, never as a second
        // copy; or, on its way, or when Kerdr can't tell, not at all (Codex's review 64, 1).
        let again = match self.pressed_again(machine, &request, key.as_deref()) {
            super::holding::Again::Busy => return,
            super::holding::Again::Record(op) => Some(op),
            super::holding::Again::Fresh => None,
        };
        let effect = key.as_deref().and_then(|k| self.effect_for(k, &request));
        // A workspace with no intent shows nothing until it closes: said meanwhile (K1-27).
        if crate::app::cancels::is_bare_cancel(&request)
            && let Some(card) = key.as_deref().and_then(|k| self.card(k))
        {
            let said = format!("Closing {}…", card.workspace_name());
            self.say(said, Tone::Info);
        }
        let shows = effect.is_some();
        let starts = matches!(request["action"].as_str(), Some("dispatch" | "reopen"));
        let asking = (!shows).then(|| self.asking_line(&request)).flatten();
        let by_enter = enter_sent.is_some_and(|e| key.as_deref() == Some(e.as_str()));
        // Its machine isn't answering: sent through one that forwards, or held for it (intent
        // 0591, release 4). A held change shows on its card, not as done.
        let home = machine;
        // A record pressed again, held for its machine again, waits as itself (route's
        // `carrying`).
        if again.is_some() {
            self.carrying = again.clone();
        }
        let routed = self.route(machine, request, key.as_deref());
        if again.is_some() {
            self.carrying = None;
        }
        let Some((machine, request)) = routed else {
            return;
        };
        let rev = key
            .as_deref()
            .filter(|_| self.holds_on(home))
            .and_then(|k| self.card_rev(k));
        let n = self.resolver.add(machine, request, from, key, effect);
        self.resolver.set_rev(n, rev);
        if let Some(op) = again {
            // On its way now: why it stayed held no longer holds, so if its answer is lost and
            // its day runs out, its card never says it wasn't sent (Codex's review 64, 4).
            self.kept.stays(&op, None);
            self.resolver.keep_op(n, &op);
            self.held_unchecked.remove(&op);
            self.held_sending.insert(n, op);
        }
        if by_enter {
            self.resolver.mark_by_enter(n);
        }
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
                organization: request["organization"].as_str().map(str::to_string),
                tenant: request["tenant"].as_str().map(str::to_string),
                team: request["team"].as_str().map(str::to_string),
            })
        });
        let n = self
            .resolver
            .add(machine, request, None, Some(key.clone()), effect);
        // Into the words file as the send it is before it goes (Codex's review 62, 2), and not
        // sent until it's there: sent first, a crash could bring its words back as new ones, to be
        // saved twice (Codex's review 63, 2).
        if self.kept.on_disk() {
            self.resolver.wait_for_words(n);
        }
        if !self.keep_words_now() {
            let why = self
                .kept
                .words_trouble()
                .unwrap_or_else(|| "Kerdr couldn't write its words file".into());
            self.say(
                format!(
                    "Your new intent waits: {why}. It's sent once its words are kept there, so a crash can never save it twice."
                ),
                Tone::Warn,
            );
        }
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
        let outside = matches!(request["action"].as_str(), Some("close" | "dismiss"))
            && request.get("outside_since").is_some();
        let chose = request["choice"].as_str();
        if !matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
            && !outside
            && chose.is_none()
        {
            return None;
        }
        let reference = self.reference_for(request["id"].as_str()?);
        // One of a question's other answers: what it asks for, as far as Kerdr knows (intent 0591).
        if let Some(choice) = chose {
            return Some(match choice {
                "queue" => format!("Queuing {reference}…"),
                "shelve-oldest" => format!("Shelving the oldest, then queuing {reference}…"),
                "wrap-up" => format!("Asking {reference}'s agent to wrap up…"),
                "new-intent" => format!("Starting a new intent from {reference}…"),
                _ => format!("Answering {reference}'s question…"),
            });
        }
        if outside {
            return Some(format!(
                "Asking about stopping {reference}'s conversation outside herdr…"
            ));
        }
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
        // One of a question's other answers: nothing Kerdr can show for sure (intent 0591).
        if request.get("choice").is_some() {
            return None;
        }
        Some(match request["action"].as_str()? {
            "dispatch" | "reopen"
                if asking::answers_otherwise(request) || self.asks_first(key, request) =>
            {
                return None;
            }
            // C or x on a conversation outside herdr: its backend asks before it stops it, so
            // nothing shows until its yes (intent 0591).
            "close" | "dismiss"
                if request.get("outside_since").is_some() && request.get("override").is_none() =>
            {
                return None;
            }
            // C or x on work an agent may be at, or x on done work: its backend asks first, so
            // nothing shows until the yes (intent 0591, K1-21, K1-22).
            "close" | "dismiss"
                if request.get("override").is_none()
                    && self.card(key).is_some_and(|c| {
                        !c.dismissed()
                            && (c.is_open_workspace()
                                || (request["action"] == "dismiss"
                                    && (c.done() || c.state.as_deref() == Some("closed"))))
                    }) =>
            {
                return None;
            }
            "dispatch" => Effect::Starting,
            "reopen" => Effect::Reopening,
            "close" => Effect::Close,
            // A stop back to the backlog, or a workspace with no intent closed: nothing Kerdr shows
            // ahead of its answer (intent 0591, K1-44, K1-27).
            "cancel"
                if crate::app::cancels::is_stop_start(request)
                    || crate::app::cancels::is_bare_cancel(request) =>
            {
                return None;
            }
            // X on work an agent is at, naming no place: its backend asks first, so nothing shows
            // until the yes (the end-to-end check's fourth failure).
            "cancel" if names_no_place(request) && request.get("override").is_none() => {
                return None;
            }
            // X on done work stops its agent and leaves it done (intent 0591, K1-29): it never
            // shows in Won't do meanwhile. Work in Won't do is there already.
            "cancel"
                if self.card(key).is_some_and(|c| {
                    c.done()
                        || c.state.as_deref() == Some("closed")
                        || c.attention == Attention::Done
                }) =>
            {
                return None;
            }
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

    /// Whether a request whose answer was lost may be sent to this machine again by itself: its
    /// feed, the last one read, says its backend keeps ops, so the same request is never done twice
    /// there (intent 0591). Any other machine's is never resent: its result is said to be unknown.
    pub fn resends_to(&self, machine: usize) -> bool {
        self.machines
            .get(machine)
            .and_then(|m| m.feed.as_ref())
            .is_some_and(|f| f.supports("op"))
    }

    /// A request is being sent again by itself, with the same op (intent 0591): a change's card,
    /// or the busy line of an account request, says so quietly. Nothing red, and nothing in the
    /// error log: only its last answer counts, and that's said as any answer is.
    /// A part-done answer with `retry` is no plain refusal: what stood is said at once, in
    /// yellow, as any part-done answer is, and again with its last answer if that doesn't happen
    /// either, as at a follow's end (follow_again).
    pub fn resending(&mut self, job: Option<u64>, _machine: usize, why: &crate::actions::Resend) {
        match job {
            Some(n) => {
                if self.resolver.note_retrying(n, true) {
                    self.rebuild();
                }
                if let crate::actions::Resend::Retry {
                    message,
                    partial: true,
                    ..
                } = why
                    && let Some(request) = self.resolver.job_request(n).cloned()
                {
                    let line = short(self.partly_line(&request, message));
                    if self.resolver.keep_stood(n, line.clone()) {
                        self.say(line, Tone::Warn);
                    }
                }
            }
            None if self.busy => {
                if !self.busy_label.ends_with(RETRYING_TAIL) {
                    self.busy_label = format!("{}{RETRYING_TAIL}", self.busy_label);
                }
                // Its card, when it has one on the board, says so too, as a change's does.
                if self.account_retrying.is_none()
                    && let Some(key) = self.account_card_in_flight()
                {
                    self.account_retrying = Some(key);
                }
            }
            None => {}
        }
    }

    /// A start that waited for its intent's write-up is going ahead from his words as they are,
    /// since that write-up failed (intent 0591): said, so it's no surprise on the card.
    pub fn starts_from_words(&mut self, n: u64) {
        let Some(id) = self
            .resolver
            .job_request(n)
            .and_then(|r| r["id"].as_str().map(str::to_string))
        else {
            return;
        };
        let reference = self.reference_for(&id);
        let starts = self
            .resolver
            .job_request(n)
            .is_some_and(|r| r["action"] == "dispatch");
        let said = if starts {
            format!("{reference} couldn't be written up, so it starts from your words as they are.")
        } else {
            // Another key that waited for its write-up (intent 0591, K1-23): it goes now.
            let what = self
                .resolver
                .job_request(n)
                .map(super::holding::held_what)
                .unwrap_or_default();
            format!(
                "{reference} couldn't be written up; {what} goes now, with its words as they are."
            )
        };
        self.say(said, Tone::Info);
    }

    /// A change's answer, from the action resolution tool: one that went as shown keeps showing
    /// until the feed has it; any other takes its effect with it, so the card is back as it really
    /// is before anything is said or asked.
    pub fn finish_job(&mut self, n: u64, request: Value, result: Result<Response, String>) {
        // Sent from the hold file, through another machine, or held by its backend (intent 0591,
        // release 4).
        let unreached = self.answer_unreached.take();
        if self.held_answer(n, &result, unreached) {
            self.keep_words_now();
            return;
        }
        if let Ok(response) = &result
            && self.drop_answered(&request, response)
        {
            self.resolver.take(n);
            self.redirected();
            self.keep_words_now();
            return;
        }
        let to = request["forward_to"].as_str().map(str::to_string);
        let via = result.as_ref().ok().and_then(|r| r.via.clone());
        self.finish_job_answered(n, request, result);
        // A hold-file record refused because its card lives elsewhere stays one record until
        // what became of it is answered (holding::redirected).
        self.redirected();
        self.note_via(to, via);
        // A start or a save answered: what waited behind it is decided again.
        self.release_behind();
        // A new intent saved, or its words given back: the words file says so now.
        self.keep_words_now();
    }

    fn finish_job_answered(&mut self, n: u64, request: Value, result: Result<Response, String>) {
        let Some(job) = self.resolver.take(n) else {
            return;
        };
        let mut job = job;
        // Answered at last: whatever it says, it's no longer being sent again, and its card
        // stops saying so.
        let was_retrying = std::mem::replace(&mut job.retrying, false);
        let machine = job.machine;
        let by_enter = job.by_enter;
        // Its card, which its answer keeps whether or not that card is still on the board.
        let job_key = job.key.clone();
        self.action_from = job.from.clone();
        self.answer_went_in = job.entered;
        if job.asking
            && let Some(line) = self.asking_line(&job.request)
        {
            self.unsay(&line);
        }
        // A second key while it asked said it in other words (intent 0491).
        if let Some(said) = job.said.take() {
            self.unsay(&said);
        }
        let mut refused = None;
        if let Ok(response) = &result
            && response.ok
            && !response.confirmation
        {
            // It asked nothing after all (a slot came free since the feed was read): from here it
            // shows as any start or reopen does.
            let guessed_wrong = job.asking
                && job.key.is_some()
                && job.starts()
                && !asking::answers_otherwise(&job.request);
            if guessed_wrong {
                // It stays in the resolver until the feed has it, no longer asking anything: its
                // line says it's starting, and a ⏎ on it is remembered (intent 0491).
                job.asking = false;
                job.effect = Some(if job.request["action"] == "reopen" {
                    Effect::Reopening
                } else {
                    Effect::Starting
                });
            }
            match job.effect.as_mut() {
                // Its card stays until the feed has the record it made; nothing made, it goes.
                // Unless its backend is still saving it (`pending`, and no number yet): then it
                // stays, saying so, until the feed has a card with its submission (release 4).
                Some(Effect::Saving(p)) => {
                    p.made = response.id.clone();
                    let saving = response.pending.as_deref().is_some_and(|w| !w.is_empty())
                        && self.holds_on(machine);
                    if p.made.is_none() && !saving {
                        job.effect = None;
                        self.rebuild();
                    }
                }
                // Accepted but held by its backend (`pending`: queued until there's room, on this
                // machine or another): it shows as held on its card, in those words, never as
                // starting (intent 0591, release 4, r4-a's shape for B1-92).
                Some(Effect::Starting | Effect::Reopening)
                    if response
                        .pending
                        .as_deref()
                        .is_some_and(|w| !w.trim().is_empty()) =>
                {
                    job.effect = None;
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
            if let Some(follow) = response.follow.clone() {
                // It goes on after this answer, and Kerdr follows it (intent 0591): shown on its
                // card in the follow's words until it ends, with what's still to come said too.
                self.begin_follow(job, follow, response.note.as_deref());
                if was_retrying {
                    self.rebuild();
                }
                // Joined to a move already under way, which an account picked with s didn't
                // reach: that's said, as on any start that restarted nothing (intent 0591).
                if response.ok && response.account_unchanged && !response.message.is_empty() {
                    self.say(response.message.clone(), Tone::Info);
                }
            } else {
                // It stays as shown until the feed has it; a start answered yes moves on now.
                let changes = guessed_wrong
                    || matches!(job.effect, Some(Effect::Starting | Effect::StartingOn(_)));
                self.resolver.settle(job);
                if changes || was_retrying {
                    self.rebuild();
                }
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
            // It failed, or asks first: no workspace is coming for a ⏎ to wait for.
            if refused
                .as_ref()
                .is_some_and(|j| j.starts() && j.key.is_some() && j.key == self.enter_when_open)
            {
                self.forget_enter();
            }
            if moved_on {
                self.resolver.forget_moves_after(machine, n);
            }
            if shown {
                self.rebuild();
            }
        }
        // Whether ⏎ asked for it: a question it asks stays ⏎'s (Codex's review), and a start that
        // answers with its workspace goes in.
        self.answer_by_enter = by_enter;
        self.answer_job = Some(n);
        self.answer_key = job_key;
        // Sent again after a part-done answer, at once or at a follow's end: what stood then is
        // said again if this didn't happen either (intent 0591).
        let stood = refused
            .as_ref()
            .and_then(|j| j.stood.clone())
            .filter(|_| match &result {
                Ok(r) => !r.confirmation && r.questions.is_empty() && !redirected,
                Err(_) => true,
            });
        let since = Instant::now();
        self.finish_action(machine, request, result);
        if let Some(stood) = stood {
            self.say_what_stood(&stood, since);
        }
        self.answer_job = None;
        self.answer_key = None;
        self.answer_by_enter = false;
        // Sent on to the machine its refusal named: back in its place in the queue, still shown,
        // so a later change to the same record can't overtake it (intent 0416).
        if self.resend.len() > resends
            && let Some(job) = refused
            && let Some((to, request, from, key)) = self.resend.pop_back()
        {
            let mut job = job;
            job.key = job.key.or(key);
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
        // Work that goes on after its answer, still followed (intent 0591).
        if let Some(holds) = self.follow_holds(card, action) {
            return holds;
        }
        let Some(job) = self.resolver.starting(&card.key, &id) else {
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
        // Any other key waits behind it, and is decided again once it's answered (intent 0591,
        // release 4, K1-39 to K1-41): held by queue_action, not refused here.
        if !matches!(action, "dispatch" | "reopen" | "enter") {
            return false;
        }
        let verb = if job.request["action"] == "reopen" {
            "reopening"
        } else {
            "starting up"
        };
        let n = job.n;
        let secs = job.asked.elapsed().as_secs();
        let reference = &card.reference;
        // Nothing has started: a question or a place in a queue is all that can come (intent 0491).
        if job.asking {
            let what = if asking::answers_otherwise(&job.request) {
                "queuing"
            } else {
                "asking about"
            };
            let line = format!("Kerdr is still {what} {reference} ({secs} s so far).");
            self.resolver.note_said(n, line.clone());
            self.say(line, Tone::Info);
            return true;
        }
        let text = if open {
            format!(
                "{reference}'s workspace is open and its agent is still taking its brief ({secs} s so far): ⏎ goes in, and X stops it."
            )
        } else if action == "enter" {
            format!(
                "{reference} is {verb} ({secs} s so far). Kerdr goes in when its workspace opens; Esc stops waiting."
            )
        } else if matches!(action, "dispatch" | "reopen") {
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
        let asking = self.resolver.starting(&card.key, &id)?.asking;
        if let Some(action) = self.go_into_opened(card) {
            return Some(action);
        }
        self.still_changing(card, "enter");
        // Remembered: the workspace opens without another key from you (intent 0491). Not when
        // nothing has started, where only a question can come.
        if !asking {
            self.remember_enter(&card.key);
        }
        Some(Action::None)
    }

    /// Into the workspace a feed names for this card's record, opened since its start or reopen was
    /// asked for. None until one has.
    fn go_into_opened(&mut self, card: &Card) -> Option<Action> {
        let id = card.detail.as_ref()?.id.clone();
        let job = self.resolver.starting(&card.key, &id)?;
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
        })?;
        let action = Action::JumpWorkspace {
            machine: opened.machine.clone(),
            session: opened.session.clone(),
            id: opened.id.clone(),
        };
        // In by his own ⏎: its answer takes him nowhere later.
        self.resolver.went_in(&card.key, &id);
        self.forget_enter();
        Some(action)
    }

    /// Remembers a ⏎ on a card just started, with the words on screen that promise it, so they go
    /// when it does (intent 0491).
    pub(crate) fn remember_enter(&mut self, key: &str) {
        self.enter_when_open = Some(key.to_string());
        self.enter_said = self
            .message
            .as_ref()
            .filter(|(_, tone, _)| *tone != Tone::Fail)
            .map(|(text, _, _)| text.clone());
    }

    /// Forgets the remembered ⏎, and takes down any words still promising it.
    pub(crate) fn forget_enter(&mut self) {
        self.enter_when_open = None;
        if let Some(said) = self.enter_said.take() {
            self.unsay(&said);
        }
    }

    /// Whether going in can't take keys meant for something else here, so a remembered ⏎ counts.
    pub(crate) fn enter_can_land_here(&self) -> bool {
        matches!(
            self.mode,
            Mode::Board
                | Mode::Peek { .. }
                | Mode::Backlog { .. }
                | Mode::Misc { .. }
                | Mode::Done { .. }
                | Mode::Card { .. }
        ) && !self.typing_view_search()
    }

    /// The ⏎ remembered on a card just started, once its workspace is open: how to go in. Only where
    /// going in can't take keys meant for something else, and not while a failure is up. Forgotten
    /// when you've moved off the card, and when the start has gone with no workspace to show for
    /// it (intent 0491).
    pub fn enter_when_open_due(&mut self) -> Option<Action> {
        let key = self.enter_when_open.clone()?;
        if self.failing() || !self.enter_can_land_here() {
            return None;
        }
        let Some(card) = self
            .selected_for_control()
            .filter(|c| c.key == key)
            .cloned()
        else {
            self.forget_enter();
            return None;
        };
        if card.is_open_workspace() {
            self.forget_enter();
            return Some(self.jump_action(key));
        }
        let id = card.detail.as_ref()?.id.clone();
        if self.resolver.starting(&card.key, &id).is_some() {
            return self.go_into_opened(&card);
        }
        // Its start was queued, or is held by its backend: said, rather than waited for (intent
        // 0591, K1-43).
        let held = self
            .held_lines(&card)
            .into_iter()
            .find(|l| matches!(l.drop, Some(super::holding::DropOf::Backend(_))));
        if let Some(line) = held {
            self.forget_enter();
            self.say(
                format!(
                    "{} has no workspace yet: {}. ⏎ goes in once it's under way.",
                    card.reference, line.words
                ),
                Tone::Info,
            );
            return None;
        }
        // Followed work still going: it goes in once the card shows it open (intent 0591).
        if self.started_lately(&card.key, &id) || self.following_card(&card).is_some() {
            return None;
        }
        self.forget_enter();
        let says = Some(card.why.trim())
            .filter(|w| !w.is_empty())
            .map(|w| format!(": its card says \"{w}\""))
            .unwrap_or_default();
        self.say(
            format!(
                "{} still has no workspace ten minutes after its start, so Kerdr stopped waiting{says}. ⏎ tries again.",
                card.reference
            ),
            Tone::Info,
        );
        None
    }

    /// Whether this card's start was answered yes in the last ten minutes (START_WAIT): its
    /// workspace may yet come, and nothing starts it twice meanwhile (intent 0591, K1-43). Its own,
    /// by `Resolver::for_card`: another source's card with the same number isn't held up by it
    /// (Codex's seventh look).
    pub(super) fn started_lately(&self, key: &str, id: &str) -> bool {
        self.started.as_ref().is_some_and(|(k, s, at)| {
            self.resolver.for_card(Some(k), s, key, id) && at.elapsed() < START_WAIT
        })
    }

    /// The machine to read often for the start a ⏎ is waiting on: reading its feed shows the
    /// workspace, which opens well before the command answers. Only for a remembered ⏎, since each
    /// read costs that machine seconds of work and the timed feed covers every other start, and
    /// only one that's answering: a feed can't show anything from one that isn't, and these wakes
    /// would override its wait between tries (intent 0491).
    pub fn start_awaited_on(&self) -> Vec<usize> {
        // s's waits for a machine's accounts, or for its backend to update, are read for on their
        // own, ⏎ or no ⏎ (intent 0591).
        let mut machines = self.accounts_awaited_on();
        if let Some(key) = self.enter_when_open.as_deref() {
            machines.extend(
                self.resolver
                    .on_its_way()
                    .filter(|j| j.key.as_deref() == Some(key))
                    .filter(|j| j.starts() && !j.asking && !j.waits_for_write_up())
                    .filter(|j| !asking::answers_otherwise(&j.request))
                    .map(|j| j.machine)
                    .filter(|&m| self.machines.get(m).is_some_and(|m| m.conn.is_live())),
            );
        }
        machines.extend(self.follows_awaited_on());
        machines.sort_unstable();
        machines.dedup();
        machines
    }

    /// Going into a card's workspace yourself: a start or reopen still on its way for it neither
    /// asks nor takes you in when it answers, since you're already there (intent 0445).
    pub fn went_into(&mut self, key: &str) {
        self.forget_enter();
        self.entered_follow(key);
        if let Some(id) = self
            .card(key)
            .and_then(|c| c.detail.as_ref())
            .map(|d| d.id.clone())
        {
            self.resolver.went_in(key, &id);
        }
    }

    /// The same, going into a workspace by its id: the card it's for, if any.
    pub fn went_into_workspace(&mut self, machine: usize, session: &str, id: &str) {
        self.forget_enter();
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
        // To a backend that can put it back in the backlog, asked instead (intent 0591, K1-44).
        // m's D came too late, not Esc: the runner's answer to it says so (job_drop_answered).
        if started.too_late && !self.drops_asked.contains_key(&n) && !self.ask_stop_start(n) {
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
        self.resolver.start_waiting_for_write_up(key).is_some()
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
        if !card.is_open_workspace()
            && let Some(line) = self.follow_line(card)
        {
            return Some(line);
        }
        let Some(job) = self.resolver.starting(&card.key, id) else {
            // Any other change still on its way: shown as done, but not yet saved.
            let job = self.resolver.unanswered_on(&card.key)?;
            let secs = job.asked.elapsed().as_secs();
            let what = match job.request["action"].as_str() {
                Some("close") => "Marking it done",
                Some("dismiss") => "Setting it aside",
                Some("cancel") if crate::app::cancels::is_stop_start(&job.request) => {
                    "Putting it back in the backlog: stopping its agent"
                }
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
        // Nothing has started: its line says so, as the board does (intent 0491).
        if job.asking {
            let what = if asking::answers_otherwise(&job.request) {
                "Queuing it"
            } else {
                "Asking first"
            };
            return Some(format!("◐ {what}… {secs} s"));
        }
        let what = match &job.effect {
            Some(crate::resolve::Effect::StartingOn(m)) => format!("Starting up on {m}"),
            _ if job.request["action"] == "reopen" => "Reopening".to_string(),
            _ => "Starting up".to_string(),
        };
        // A ⏎ already remembered needs no second one: Kerdr will take the screen by itself.
        let then = if self.enter_when_open.as_deref() == Some(card.key.as_str()) {
            "Kerdr goes in when its workspace opens · Esc stops waiting"
        } else {
            "⏎ goes in as soon as its workspace opens"
        };
        Some(format!("◐ {what} · {secs} s so far · {then}"))
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
        let Some(first) = running.first() else {
            // Followed work carries on on its machine; only Kerdr's reads of it stop (intent 0591).
            return self.follows_running();
        };
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

    /// A part-done answer's line: its words, the card named in front when they don't name it.
    pub(super) fn partly_line(&self, request: &Value, message: &str) -> String {
        match request["id"].as_str() {
            Some(id) if !names(message, &self.reference_for(id)) => {
                format!("{}: {message}", self.reference_for(id))
            }
            _ => message.to_string(),
        }
    }

    /// What stood before a change was sent again by itself (Job::stood), said again with its
    /// last answer when that didn't happen either, so it's never lost (intent 0591): just above
    /// the line that answer put up since `since`, or after whatever is up when that line waits.
    pub(super) fn say_what_stood(&mut self, stood: &str, since: Instant) {
        match &self.message {
            Some((text, _, _)) if text.contains(stood) => {}
            Some((text, tone, at)) if *at >= since => {
                let text = match text.rsplit_once('\n') {
                    Some((older, newest)) => format!("{older}\n{stood}\n{newest}"),
                    None => format!("{stood}\n{text}"),
                };
                self.message = Some((text, *tone, Instant::now()));
            }
            _ => self.show(stood.to_string(), Tone::Warn),
        }
    }

    /// How the board names a request's card, "#41"; empty for one with no record.
    fn reference_of(&self, request: &Value) -> String {
        request["id"]
            .as_str()
            .map(|id| self.reference_for(id))
            .unwrap_or_default()
    }

    /// How a failure names the change that didn't happen: "#41 wasn't marked done".
    fn what_failed(&self, request: &Value) -> Option<String> {
        let id = request["id"].as_str()?;
        let reference = self.reference_for(id);
        Some(match request["action"].as_str()? {
            "dispatch" => format!("{reference} didn't start"),
            // A conversation outside herdr is moved in by its reopen (intent 0591).
            "reopen"
                if self.board.cards.iter().any(|c| {
                    c.detail.as_ref().is_some_and(|d| d.id == id) && c.outside_herdr().is_some()
                }) =>
            {
                format!("{reference} didn't move into herdr")
            }
            "reopen" => format!("{reference} didn't reopen"),
            "close" => format!("{reference} wasn't marked done"),
            "dismiss" => format!("{reference} wasn't set aside"),
            "cancel" if crate::app::cancels::is_stop_start(request) => {
                format!("{reference}'s start wasn't stopped")
            }
            "cancel" => format!("{reference} wasn't cancelled"),
            "restore" => format!("{reference} wasn't brought back"),
            "refresh" => format!("{reference} wasn't refreshed"),
            "move" => format!("{reference} didn't move"),
            _ => format!("{reference} didn't change"),
        })
    }

    /// How an answer that never came names the change it was for, without saying whether it
    /// happened, which nobody knows: "Marking #41 done" (intent 0591).
    fn what_was_asked(&self, request: &Value) -> Option<String> {
        let id = request["id"].as_str()?;
        let reference = self.reference_for(id);
        Some(match request["action"].as_str()? {
            "dispatch" => format!("Starting {reference}"),
            "reopen"
                if self.board.cards.iter().any(|c| {
                    c.detail.as_ref().is_some_and(|d| d.id == id) && c.outside_herdr().is_some()
                }) =>
            {
                format!("Moving {reference} into herdr")
            }
            "reopen" => format!("Reopening {reference}"),
            "close" => format!("Marking {reference} done"),
            "dismiss" => format!("Setting {reference} aside"),
            "cancel" if crate::app::cancels::is_stop_start(request) => {
                format!("Stopping {reference}'s start")
            }
            "cancel" => format!("Cancelling {reference}"),
            "restore" => format!("Bringing {reference} back"),
            "refresh" => format!("Refreshing {reference}"),
            "move" => format!("Moving {reference}"),
            "answer" => format!("Saving {reference}'s answers"),
            _ => format!("Changing {reference}"),
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
            // Matched to the request on its way before anything about it is touched: another
            // request's answer coming first leaves it on its way, its "trying again" and what it
            // was asked from as they were, and what waits its turn still waits (Codex's fourth
            // look).
            if self.account_reply_is_ours(machine, &request) {
                self.busy = false;
                self.busy_id = None;
                self.busy_since = None;
                self.busy_label.clear();
                // Answered at last: no longer being sent again (intent 0591).
                self.account_retrying = None;
            }
            // His answer to a question a held request waits on: taken, or its card asks again
            // (Codex's review 64, 3).
            self.asks_answer_came(machine, &request, &result);
            // A drop of an account request its backend held: said as any drop (r4-b's shape).
            if let Ok(response) = &result
                && self.drop_answered(&request, response)
            {
                return;
            }
            // A move for later, or one queued behind another: its card says so (intent 0591,
            // release 4, K2-30 and K2-31).
            self.account_pending(machine, &request, &result);
            self.finish_account_action(machine, request, result);
            return;
        }
        self.answered_at = Some(Instant::now());
        if request["action"] == "create" {
            return self.create_answered(machine, &request, result);
        }
        let from = self.action_from.take();
        // X on a workspace with no intent (intent 0591, K1-27).
        if crate::app::cancels::is_bare_cancel(&request) {
            return self.bare_cancel_answered(machine, request, result, from);
        }
        let id = request["id"].as_str().map(str::to_string);
        // Its card's key: what's kept for it (a resend, a move, his Kickoff answers) is its own,
        // never another source's with the same number (Codex's fifth and sixth looks), even when
        // its card has gone from the board meanwhile (Codex's seventh look).
        let held = id
            .as_deref()
            .map(|id| self.answer_card_key(machine, id, from.as_ref()));
        // Only this answer's own resend: another card's, still on its way, stays guarded.
        let was_resent = match self
            .resent
            .iter()
            .position(|(m, k)| *m == machine && Some(k) == held.as_ref())
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
            && let Some(key) = held.clone()
            && let Some(home) = response.home.as_deref()
        {
            let (host, session) = home.split_once('/').unwrap_or((home, ""));
            // A drop's `home` names the machine that holds what it drops, not where its record
            // lives (r4-a's round 4): the drop goes there, once, and its card's home never moves.
            let moves = request["action"] != "drop";
            let reference = self.reference_for(&id);
            let asked = self
                .machines
                .get(machine)
                .map(|m| m.name())
                .unwrap_or_default();
            // A save of an item's text that can't go on: his version stays (intent 0414).
            let kept = self.edit_kept(&request);
            // Sent on through a machine that forwards, when Kerdr can't reach that home itself,
            // once (intent 0591, release 4).
            let through = (!was_resent && request.get("forward_to").is_none())
                .then(|| self.forwarder_for(Some(host)))
                .flatten();
            match self.machine_index(host) {
                None if through.is_some() => {
                    let mut request = request;
                    request["forward_to"] = json!(host);
                    if request["action"] == "edit" {
                        self.edit_resend(&mut request, Some(&key));
                    }
                    let via = through.unwrap_or(machine);
                    self.resent.push((via, key.clone()));
                    self.resend.push_back((via, request, from, Some(key)));
                    return;
                }
                None if !moves => {
                    let said = self.drop_unreached(&key, &reference, &request, host);
                    self.say(said, Tone::Warn);
                    return;
                }
                None => {
                    self.fail(
                        format!("{reference} lives on {host} now, which isn't one of this board's machines, so Kerdr can't change it there.{kept}"),
                    );
                    return;
                }
                Some(to) if to == machine => {}
                Some(named) if was_resent => {
                    if moves {
                        self.moved_home.retain(|(k, ..)| *k != key);
                    }
                    // Once more by itself, after both copies are read and agree (K1-53).
                    if self.wait_for_agreement(machine, named, &id, &key, request, from) {
                        return;
                    }
                    self.fail(
                        format!("{reference}'s copies on {asked} and {host} disagree about where it lives; one is behind. Wait a minute for it to catch up, then try again.{kept}"),
                    );
                    return;
                }
                Some(to) => {
                    // The home its own card shows, never another source's with its number.
                    let old_home = self
                        .card(&key)
                        .and_then(|c| c.detail.as_ref())
                        .and_then(|d| d.home.as_ref())
                        .and_then(|h| h.machine.clone());
                    if moves {
                        self.moved_home.retain(|(k, ..)| *k != key);
                        self.moved_home.push((key.clone(), to, old_home));
                    }
                    let name = self.machines[to].name();
                    // Its home isn't answering: sent through a machine that forwards, else held
                    // for it, shown on its card (intent 0591, release 4, K1-54).
                    if !self.machines[to].conn.is_live() && (through.is_some() || self.holds_on(to))
                    {
                        let mut request = request;
                        if request["action"] == "edit" {
                            self.edit_resend(&mut request, Some(&key));
                        }
                        if let Some(via) = through {
                            request["forward_to"] = json!(name);
                            self.resent.push((via, key.clone()));
                            self.resend.push_back((via, request, from, Some(key)));
                        } else {
                            let rev = self.card_rev(&key);
                            self.hold_request(name, request, Some(key), rev, None);
                        }
                        return;
                    }
                    if !self.machines[to].conn.is_live() && !moves {
                        let said = self.drop_unreached(&key, &reference, &request, &name);
                        self.say(said, Tone::Warn);
                        return;
                    }
                    if !self.machines[to].conn.is_live() {
                        self.fail(
                            format!("{reference} lives on {name} now, and {name} isn't answering; nothing changed. Try again once it's back.{kept}"),
                        );
                        return;
                    }
                    // s's account goes on only to a backend that starts on one: an older one would
                    // start it on its owner's list without a word (intent 0580).
                    // That one is brought up to date first, then started there (intent 0591).
                    if request.get("account").is_some()
                        && !self
                            .account_feed(to)
                            .is_some_and(|f| f.starts_on(request["action"].as_str().unwrap_or("")))
                    {
                        let account = request["account"].as_str().unwrap_or("list").to_string();
                        if self.owe_after_update(&id, machine, from.as_ref(), to, &account) {
                            self.say(
                                format!(
                                    "{reference} lives on {name} now, whose backend can't start work on a chosen account yet: Kerdr is bringing its ark-sdlc up to date, then starts {reference} there. Esc on the board drops it."
                                ),
                                Tone::Info,
                            );
                        } else {
                            self.fail(format!(
                                "{reference} lives on {name} now, whose backend can't start work on a chosen account yet, and it's gone from the board, so nothing started."
                            ));
                        }
                        return;
                    }
                    // Only what that machine's backend takes, checked there: one it would read
                    // otherwise (a stop back to the backlog read as X's, say) isn't sent on
                    // (Codex's fifth look).
                    if let Some(what) = self.missing_on(to, &request, from.as_ref(), true) {
                        self.not_sent_for(to, &request, from.as_ref(), what, true);
                        return;
                    }
                    let mut request = request;
                    if request["action"] == "reopen" && !session.is_empty() {
                        request["session"] = json!(session);
                    }
                    // A save's text came back only counted: it goes again as he wrote it, his
                    // words for this card.
                    if request["action"] == "edit" {
                        self.edit_resend(&mut request, Some(&key));
                    }
                    // Sent on quietly: the card still shows the change while it goes there. Its
                    // answer comes from there, for the same card it was asked from.
                    self.resent.push((to, key.clone()));
                    self.resend.push_back((to, request, from, Some(key)));
                    return;
                }
            }
        }
        // A save's question (done work's words made a new intent, say) is asked as any other,
        // with his words in it again, since they came back only counted (intent 0591).
        let mut request = request;
        if request["action"] == "edit" {
            let asked = matches!(&result, Ok(r) if r.confirmation
                && r.confirm.is_some()
                && request.get("text").is_some()
                && (request["override"] != true
                    || r.confirm.as_deref() != request["confirm"].as_str()));
            if !asked {
                return self.finish_edit(machine, request, result, from);
            }
            self.edit_resend(&mut request, held.as_deref());
            if request.get("text").is_none() {
                return self.finish_edit(machine, request, result, from);
            }
        }
        if request["action"] == "kickoff" {
            return self.finish_kickoff_check(machine, &request, result);
        }
        // A start he asked to stop once it started (intent 0591, K1-44).
        if self.stop_if_started(machine, &request, &result, &from) {
            return;
        }
        // Answered with `follow`: its words are on its card and the message line already, and its
        // end is taken as this request's answer when it comes (intent 0591, follow.rs). Its
        // `message` is for a Kerdr that doesn't follow.
        if let Ok(response) = &result
            && response.ok
            && !response.confirmation
            && response.follow.is_some()
        {
            return;
        }
        // Its yes couldn't be matched to the question (a Kerdr and backend that disagree): nothing
        // started, so the question is asked afresh rather than dropped. Only a yes does this, so
        // it can't go round by itself (intent 0591). Only to a backend whose feed says it keeps
        // ops, as every request sent by itself: a released backend's refusal can't be shown to
        // come before anything changed (a followed worker can start a session, then refuse), so
        // there it's only said, and the next key press asks again.
        if let Ok(response) = &result
            && !response.ok
            && response.message.contains(YES_NOT_MATCHED)
            && matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
            && (request["override"] == true || request.get("confirm").is_some())
            && self.resends_to(machine)
        {
            let mut again = request;
            if let Some(fields) = again.as_object_mut() {
                for answer in ["op", "override", "confirm", "queue", "choice"] {
                    fields.remove(answer);
                }
            }
            self.say(
                "That yes couldn't be matched to the question it answered, so nothing started. Asking it again.",
                Tone::Info,
            );
            self.resend
                .push_back((machine, crate::actions::with_op(again), from, held));
            return;
        }
        // A question, or after a yes a new one: something changed, so it carries a new token.
        let asks = |response: &Response| {
            (response.confirmation || !response.questions.is_empty())
                && (matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
                    || (response.confirmation
                        && response.confirm.is_some()
                        && matches!(
                            request["action"].as_str(),
                            Some("close" | "dismiss" | "cancel" | "edit" | "answer")
                        )))
                && (request["override"] != true
                    || response
                        .confirm
                        .as_deref()
                        .is_some_and(|t| request["confirm"].as_str() != Some(t)))
        };
        match result {
            Ok(response) if asks(&response) => {
                // Only while you're still on that work: a question that pops up over typing would
                // take the next key for its answer. Otherwise it waits on its card, shown as asking
                // you rather than as a failure, and n or its key opens it (intent 0591).
                let by_enter = self.answer_by_enter;
                if !from.as_ref().is_some_and(|f| self.still_on(f, None)) {
                    self.wait_on_card(machine, request, response, from, by_enter);
                    return;
                }
                self.ask_now(machine, request, response, from, by_enter);
            }
            Ok(response) => {
                // Kickoff answers whose questions changed while he answered, with no box to fill
                // them into now (Esc's save, say): kept for the box d opens next (intent 0591,
                // B1-108).
                if matches!(request["action"].as_str(), Some("answer" | "dispatch"))
                    && !response.prefill.is_empty()
                    && response.questions.is_empty()
                    && let Some(held) = &held
                {
                    self.kickoff_prefill
                        .insert(held.clone(), response.prefill.clone());
                }
                let mut message = response.message.clone();
                // What a change that went as shown still says: only what you'd need to find it.
                let mut note = None;
                // Queued rather than started (`pending`): no workspace is coming yet.
                let queued = response
                    .pending
                    .as_deref()
                    .is_some_and(|w| !w.trim().is_empty());
                if request["action"] == "dispatch" && response.ok && !queued {
                    self.started = request["id"]
                        .as_str()
                        .zip(held.clone())
                        .map(|(id, key)| (key, id.to_string(), Instant::now()));
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
                // A reopen queued in a full session (w), or any other answer than yes to a
                // question, says what it did (intent 0591).
                if asking::answers_otherwise(&request) && response.ok {
                    note = Some(message.clone());
                }
                // The account s picked wasn't applied, since nothing restarted: that's said, and
                // Kerdr doesn't take you in as if it had been (intent 0580).
                let unchanged = response.ok && response.account_unchanged;
                if unchanged {
                    // What he picked still holds: the agent running it moves to that account once
                    // Kerdr sees it, as s on a running card does (intent 0591, K1-126).
                    let to = request["account"].as_str().filter(|a| *a != "list");
                    // Its own card, never another source's with the same number (Codex's fifth
                    // look).
                    let key = held.clone();
                    match (to, key) {
                        (Some(to), Some(key)) if self.owe_move(&key, to) => {
                            let reference =
                                self.reference_for(request["id"].as_str().unwrap_or_default());
                            message = format!(
                                "{reference}'s agent was running already, so it wasn't restarted: Kerdr moves it to {to} once it sees it, keeping its conversation. ⏎ on its card goes in."
                            );
                        }
                        _ => message.push_str(" ⏎ on its card goes in."),
                    }
                    note = Some(message.clone());
                }
                // A reopen takes you in only while you're still on its card; otherwise it says so.
                // So does a start ⏎ asked for, whether its answer names the workspace at once (a yes
                // to its question, say) or once its followed work is done (intent 0591). d's start
                // stays where you are.
                let enters = request["action"] == "reopen"
                    || self.follow_end.as_ref().is_some_and(|e| e.enter)
                    || (self.answer_by_enter && request["action"] == "dispatch");
                if enters
                    && response.ok
                    && !unchanged
                    && let (Some(session), Some(workspace)) =
                        (response.session.clone(), response.workspace.clone())
                {
                    // The board may already show the workspace it just made: that's still the card.
                    match from
                        .filter(|f| self.still_on(f, Some((&session, &workspace))))
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
                // A start stopped and put back in the backlog says so (intent 0591, K1-44).
                if crate::app::cancels::is_stop_start(&request) && response.ok {
                    note = Some(message.clone());
                }
                // Answers kept on leaving the Kickoff Questions box: how many are left.
                if request["action"] == "answer" && response.ok {
                    note = Some(message.clone());
                }
                // A change that stands with something still to come (its push, say): that's said
                // even when nothing else would be, and even as Kerdr goes in (intent 0591).
                let left = response
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| response.ok && !n.is_empty());
                if let Some(left) = left {
                    note = Some(match note.take() {
                        Some(n) if n.contains(left) => n,
                        Some(n) => format!("{n} {left}"),
                        None if message.contains(left) => message.clone(),
                        None => match request["id"].as_str() {
                            Some(id) => format!("{}: {left}", self.reference_for(id)),
                            None => left.to_string(),
                        },
                    });
                }
                // Followed work done, not gone into: what it did, and where it is.
                if self.follow_end.is_some()
                    && response.ok
                    && note.is_none()
                    && self.open_after.is_none()
                    && !self.answer_went_in
                    && !message.trim().is_empty()
                {
                    if response.workspace.is_some() && !message.contains('⏎') {
                        message.push_str(" ⏎ on its card goes in.");
                    }
                    note = Some(message.clone());
                }
                // Started: its answers are saved, and what was kept of them is done with.
                if request["action"] == "dispatch"
                    && response.ok
                    && let Some(held) = &held
                {
                    self.kickoff_kept.remove(held);
                    self.kickoff_prefill.remove(held);
                }
                if !response.ok && response.partial {
                    // Part of it happened, and the backend's words say which: not a change that
                    // didn't happen, so no "didn't" (intent 0591). The feed is read at once.
                    let kept = self.kickoff_kept_note(&request, held.as_deref());
                    let text = format!("{}{kept}", self.partly_line(&request, &message));
                    self.say(short(text), Tone::Warn);
                } else if !response.ok {
                    // The card is back as it really is; say what didn't happen, and why, naming
                    // the card once: words that already name it are said alone.
                    let what = self.what_failed(&request);
                    let reference = self.reference_of(&request);
                    let kept = self.kickoff_kept_note(&request, held.as_deref());
                    self.fail(short(match what {
                        Some(what) => {
                            format!("{}{kept}", what_and_why(&what, &reference, &message))
                        }
                        None => format!("{message}{kept}"),
                    }));
                } else if let Some(note) = note {
                    // Done as shown: only what you'd need to find it is said.
                    self.say(short(note), Tone::Info);
                }
            }
            Err(error) => {
                let kept = self.kickoff_kept_note(&request, held.as_deref());
                // Its result unknown (no answer came, the connection dropped): named by what was
                // asked, never as something that didn't happen. Otherwise nothing was sent, and
                // it says what didn't happen, as a refusal does (intent 0591).
                let what = if crate::actions::result_unknown(&error) {
                    self.what_was_asked(&request)
                } else {
                    self.what_failed(&request)
                };
                let reference = self.reference_of(&request);
                match what {
                    Some(what) if !error.starts_with(&what) => self.fail(short(format!(
                        "{}{kept}",
                        what_and_why(&what, &reference, &error)
                    ))),
                    _ => self.fail(short(format!("{error}{kept}"))),
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
                // Made, but only part of what follows happened: said plainly, not as a failure
                // (intent 0591). An older backend's refusal that names what it made is one.
                if !response.ok && response.partial {
                    return self.say(short(response.message), Tone::Warn);
                }
                if !response.ok {
                    return self.fail(short(response.message));
                }
                // Made, with something still to come by itself (its push): said even where the
                // card would have said it all (intent 0591).
                let left = response
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
                let mut message = response.message;
                if let Some(left) = &left
                    && !message.contains(left.as_str())
                {
                    message = format!("{message} {left}");
                }
                if self.filtering() {
                    message.push_str(
                        " A filter may hide it until it's written up: F shows everything.",
                    );
                } else if on_it && matches!(self.mode, Mode::Backlog { .. }) {
                    // Saved as shown: its card says so, and only what's still to come is said.
                    if let Some(left) = left {
                        self.say(short(left), Tone::Info);
                    }
                    return;
                } else {
                    message.push_str(" b shows it in the backlog.");
                }
                self.say(short(message), Tone::Info);
            }
            // A question about his words (a line that looks like a token or password, intent
            // 0591, release 5): asked, the words kept meanwhile with their send; nothing's made.
            Ok(response) if response.confirmation && response.confirm.is_some() => {
                let token = response.confirm.clone().unwrap_or_default();
                if at.is_none() || !self.enter_can_land_here() {
                    let into_box = self.give_back(words.0, words.1, None);
                    return self.say(
                        short(format!(
                            "Your new intent wasn't saved yet: {} {}; Enter there asks again.",
                            response.message,
                            back(into_box)
                        )),
                        Tone::Warn,
                    );
                }
                self.mode = Mode::Confirm {
                    machine,
                    request: json!({"action": "create", "submission": submission, "confirm": token}),
                    title: "Save your new intent?".into(),
                    message: format!(
                        "{} n keeps your words unsent, open in the box.",
                        response.message.trim()
                    ),
                    back: Box::new(self.mode.clone()),
                    target: None,
                    queue: false,
                    ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
                    plain: false,
                    choices: vec![],
                    enter_queues: false,
                };
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
                // Nothing sent: it wasn't saved. Otherwise nobody knows (intent 0591).
                let unknown = crate::actions::result_unknown(&error);
                if let Some(i) = at {
                    self.submissions[i].unknown |= unknown;
                }
                // As for a refusal: settled, unless this send or an earlier one went unheard.
                let unheard = at
                    .map(|i| self.submissions[i].clone())
                    .filter(|s| s.unknown);
                if let Some(i) = at.filter(|_| unheard.is_none()) {
                    self.submissions.remove(i);
                }
                let maybe = unheard.is_some();
                let into_box = self.give_back(words.0, words.1, unheard);
                let error = super::edit_controls::first_clause(&error);
                self.fail(if maybe {
                    format!(
                        "Your new intent may not have been saved: {error}. {}; sending them again never makes a second one.",
                        back(into_box)
                    )
                } else {
                    format!("Your new intent wasn't saved: {error}. {}.", back(into_box))
                });
            }
        }
    }

    /// Whether a request's own first step brought this card back from Won't do (intent 0591): a
    /// step-first d, ⏎, s or C on a card there restores it before anything else, so the question
    /// its backend asks after that is about the card as restored. That isn't a change made while
    /// he decided (the end-to-end check's second failure). Only for a backend's own question, by
    /// its token (a yes-or-no's `confirm`, or Kickoff questions' `asked`: Codex's look at the
    /// end-to-end fixes), from a backend that does steps. The card's workspace, conversation and
    /// home are still checked for real changes, by `same_work`.
    pub(super) fn restored_by_its_steps(
        &self,
        machine: usize,
        request: &Value,
        then: &CardSnapshot,
        now: &Card,
    ) -> bool {
        then.dismissed
            && !now.dismissed()
            && (request.get("confirm").is_some() || request.get("asked").is_some())
            && matches!(
                request["action"].as_str(),
                Some("dispatch" | "reopen" | "close")
            )
            && self.steps_on(machine)
    }

    /// The card a question was asked about, as its request's own first step left it: back from
    /// Won't do when that step brought it back, and in the backlog when that's where it came back
    /// to; else as it was.
    pub(super) fn as_its_steps_left_it(
        &self,
        machine: usize,
        request: &Value,
        then: &CardSnapshot,
        now: &Card,
    ) -> CardSnapshot {
        if self.restored_by_its_steps(machine, request, then, now) {
            CardSnapshot {
                dismissed: false,
                waiting: then.waiting || waiting_work(now),
                ..then.clone()
            }
        } else {
            then.clone()
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
        if crate::app::cancels::is_bare_cancel(request) {
            return self.bare_cancel_applies(machine, request, target);
        }
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
                    // Back from Won't do by its own first step: its token names the rest, which
                    // its backend checks, so here only that it's the same work, still back, and
                    // done or not as it was.
                    Some("dispatch" | "reopen" | "close")
                        if target.is_some_and(|t| {
                            self.restored_by_its_steps(machine, request, t, c)
                        }) =>
                    {
                        target.is_some_and(|t| t.same_work(c) && t.done == CardSnapshot::of(c).done)
                    }
                    Some("dispatch" | "reopen") if target.is_some_and(|t| t.dismissed) => {
                        c.dismissed() && target.is_some_and(|t| t.same_work(c))
                    }
                    Some("reopen") => {
                        (c.under_way() || c.done()) && target.is_some_and(|t| t.same_work(c))
                    }
                    // Started where it was asked: its home hasn't moved since.
                    Some("dispatch") => waiting_work(c) && target.is_none_or(|t| t.same_work(c)),
                    // Asked about its conversation outside herdr: only while it still runs there,
                    // the same work, so nothing in herdr is stopped by a yes meant for it (intent
                    // 0591).
                    Some("close" | "dismiss" | "cancel")
                        if request.get("outside_since").is_some() =>
                    {
                        c.outside_herdr().is_some()
                            && !c.dismissed()
                            && !c.done()
                            && target.is_some_and(|t| t.same_work(c))
                    }
                    // Its backend's own question, whose token names what it asked about: its
                    // backend checks that, so here only that it's the same work, still done or
                    // not and in Won't do or not, as it was (intent 0591).
                    Some("close" | "dismiss" | "cancel" | "edit")
                        if request.get("confirm").is_some() =>
                    {
                        target.is_some_and(|t| {
                            let now = CardSnapshot::of(c);
                            t.same_work(c) && t.done == now.done && t.dismissed == now.dismissed
                        })
                    }
                    // Marked done from Won't do only while it's still there (intent 0591).
                    Some("close") if target.is_some_and(|t| t.dismissed) => {
                        c.dismissed() && target.is_some_and(|t| t.same_work(c))
                    }
                    // Still open with no workspace, the same work (intent 0289), or that once
                    // it's written up (K1-23).
                    Some("close") => {
                        crate::app::behind::closable_once_written(c)
                            && target.is_some_and(|t| t.same_work(c))
                    }
                    // Its stop once it opens: only while its start is still on its way, as when
                    // it was asked (K1-41).
                    Some("cancel") if crate::app::behind::is_cancel_once_open(request) => {
                        !c.is_open_workspace()
                            && target.is_some_and(|t| t.same_work(c))
                            && self
                                .resolver
                                .starting(&c.key, id)
                                .is_some_and(|job| !job.asking)
                    }
                    // Still the same work, open where the yes was given (intent 0321); done work
                    // or work in Won't do only while it's still that (intent 0591).
                    Some("cancel") => {
                        let then = target.map(|t| (t.done, t.dismissed));
                        let now = CardSnapshot::of(c);
                        (c.cancellable()
                            || (c.is_open_workspace()
                                && (now.done || now.dismissed)
                                && then == Some((now.done, now.dismissed))))
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

/// Whether a backend's question already says how many workspaces its session has, or its limit:
/// "web has 2 workspaces", "at its usual limit of 3".
pub(super) fn says_the_count(message: &str, used: u32, limit: u32) -> bool {
    let m = message.to_lowercase();
    m.contains(&format!("{used} workspace"))
        || m.contains(&format!("{used} active"))
        || m.contains(&format!("limit of {limit}"))
        || m.contains(&format!("limit is {limit}"))
}
