//! The board's vocabulary: stages, attention, cards and the summary across the top.

use chrono::{DateTime, Local};

use crate::feed::FeedCard;

/// Where a piece of work is in its life. The six working stages are Anthropic's AI-native SDLC
/// stages (Plan, Design, Build, Test, Deploy, Maintain); Backlog sits in front of them for work
/// that hasn't started, and Maintain is where finished work goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Backlog,
    Plan,
    Design,
    Build,
    Test,
    Deploy,
    Maintain,
    /// Conversations without a feed record; kept outside the working stages.
    Misc,
}

impl Stage {
    pub const ALL: [Stage; 7] = [
        Stage::Backlog,
        Stage::Plan,
        Stage::Design,
        Stage::Build,
        Stage::Test,
        Stage::Deploy,
        Stage::Maintain,
    ];

    /// Reads a stage name, accepting the common words people and agents use for each one.
    pub fn parse(s: &str) -> Option<Stage> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "backlog" | "queued" | "todo" | "to do" | "shelved" => Stage::Backlog,
            "plan" | "planning" | "triage" | "intake" => Stage::Plan,
            "design" | "spec" | "designing" => Stage::Design,
            "build" | "building" | "implement" | "implementation" | "code" | "doing" => {
                Stage::Build
            }
            "test" | "testing" | "check" | "verify" | "review" | "reviewing" => Stage::Test,
            "deploy" | "deploying" | "ship" | "release" | "merge" => Stage::Deploy,
            "maintain" | "maintenance" | "done" | "closed" | "shipped" => Stage::Maintain,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Stage::Backlog => "backlog",
            Stage::Plan => "plan",
            Stage::Design => "design",
            Stage::Build => "build",
            Stage::Test => "test",
            Stage::Deploy => "deploy",
            Stage::Maintain => "maintain",
            Stage::Misc => "misc",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Backlog => "Backlog",
            Stage::Plan => "Plan",
            Stage::Design => "Design",
            Stage::Build => "Build",
            Stage::Test => "Test",
            Stage::Deploy => "Deploy",
            Stage::Maintain => "Maintain",
            Stage::Misc => "Misc",
        }
    }

    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Backlog and Maintain are drawn as compact lists; the stages between them as cards.
    pub fn is_list(self) -> bool {
        matches!(self, Stage::Backlog | Stage::Maintain)
    }
}

/// What a card needs from you right now. Ordered so the most urgent sorts first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attention {
    NeedsYou,
    Blocked,
    Running,
    Waiting,
    Idle,
    Unreachable,
    Done,
}

impl Attention {
    pub fn parse(s: &str) -> Option<Attention> {
        Some(
            match s
                .trim()
                .to_ascii_lowercase()
                .replace(['-', ' '], "_")
                .as_str()
            {
                "needs_you" | "needs_input" | "asking" | "question" | "waiting_on_you" => {
                    Attention::NeedsYou
                }
                "blocked" | "stuck" => Attention::Blocked,
                "running" | "working" | "under_way" | "underway" => Attention::Running,
                "waiting" => Attention::Waiting,
                "idle" => Attention::Idle,
                "done" | "closed" | "finished" => Attention::Done,
                _ => return None,
            },
        )
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Attention::NeedsYou => "●",
            Attention::Blocked => "■",
            Attention::Running | Attention::Waiting => "◐",
            Attention::Idle => "·",
            Attention::Unreachable => "○",
            Attention::Done => "✓",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Attention::NeedsYou => "needs you",
            Attention::Blocked => "blocked",
            Attention::Running => "running",
            Attention::Waiting => "waiting",
            Attention::Idle => "idle",
            Attention::Unreachable => "can't reach",
            Attention::Done => "done",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

impl Priority {
    pub fn parse(s: &str) -> Priority {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" | "urgent" => Priority::High,
            "low" => Priority::Low,
            _ => Priority::Normal,
        }
    }
}

/// One herdr agent in a card's workspace.
#[derive(Clone, Debug, Default)]
pub struct AgentInfo {
    pub agent: String,
    pub status: String,
    pub pane_id: String,
    pub title: Option<String>,
    /// herdr's conversation id for the pane, so a move of an agent no feed records still names
    /// the conversation it was asked for.
    pub conversation: Option<String>,
}

/// Where a card came from: a feed (a record of work, such as a ticket), or a herdr
/// workspace that no feed claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Feed,
    Workspace,
}

#[derive(Clone, Debug)]
pub struct Card {
    /// Unique across the whole board: `<source>:<id>` for feed cards, `ws:<machine>:<session>:<id>`
    /// for bare workspaces.
    pub key: String,
    /// The short handle shown first on the card: `#143`, or a workspace's name.
    pub reference: String,
    pub title: String,
    pub stage: Stage,
    pub attention: Attention,
    /// One short line saying why it has that attention, or what it's doing.
    pub why: String,
    pub machine: String,
    /// Which of Kerdr's machines it's on: names can repeat, positions can't.
    pub machine_index: usize,
    pub session: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
    /// The tab in that workspace the work runs in, when its feed names one: Enter focuses it.
    pub tab_id: Option<String>,
    pub priority: Priority,
    pub background: bool,
    /// Set when the card's machine isn't answering: the moment its data was last fresh.
    pub cached_at: Option<DateTime<Local>>,
    pub updated: Option<DateTime<Local>>,
    pub stale_days: Option<i64>,
    pub state: Option<String>,
    pub detail: Option<FeedCard>,
    pub agents: Vec<AgentInfo>,
    pub source: Source,
    /// Done-when progress: met and total, from the feed's criteria or a reported `n/m`.
    pub progress: Option<(u32, u32)>,
}

impl Card {
    pub fn dismissed(&self) -> bool {
        self.detail.as_ref().and_then(|d| d.disposition.as_deref()) == Some("wont_do")
    }

    /// `pending` or `failed` while an agent hasn't written the intent up from its words.
    pub fn intake(&self) -> Option<&str> {
        self.detail.as_ref().and_then(|d| d.intake.as_deref())
    }

    pub fn owner_key(&self) -> String {
        let d = self.detail.as_ref();
        if let Some(t) = d
            .and_then(|d| d.tenant.as_deref())
            .filter(|s| !s.is_empty())
        {
            format!("tenant:{t}")
        } else if let Some(o) = d
            .and_then(|d| d.organization.as_deref())
            .filter(|s| !s.is_empty())
        {
            format!("org:{o}")
        } else {
            "unassigned".into()
        }
    }

    pub fn owner_label(&self) -> String {
        let key = self.owner_key();
        match key.split_once(':') {
            Some(("org", name)) => format!("{name} (organization)"),
            Some((_, name)) => name.to_string(),
            None => "Unassigned".into(),
        }
    }

    pub fn project(&self) -> &str {
        self.detail
            .as_ref()
            .and_then(|d| d.project.as_deref())
            .unwrap_or("")
    }

    pub fn backlog_rank(&self) -> i64 {
        self.detail
            .as_ref()
            .and_then(|d| d.backlog_rank)
            .or_else(|| self.detail.as_ref().and_then(|d| d.id.parse().ok()))
            .unwrap_or(i64::MAX)
    }

    pub fn is_open_workspace(&self) -> bool {
        self.workspace_id.is_some() && self.session.is_some()
    }

    /// Work under way whose workspace is gone: a record in a working stage, not done, dismissed or
    /// still being written up, with no workspace open. Enter asks the action backend to reopen it.
    pub fn can_reopen(&self) -> bool {
        self.under_way() && !self.is_open_workspace()
    }

    /// Done work with no workspace open. Enter asks the action backend to open it again, and the
    /// backend always asks first (intent 0278).
    pub fn can_reopen_done(&self) -> bool {
        self.done() && !self.is_open_workspace()
    }

    /// Done work, whatever its workspace: a record in Maintain, not dismissed or still being
    /// written up. What reopening it needs, once its workspace is known to have gone.
    pub fn done(&self) -> bool {
        self.detail.is_some()
            && self.stage == Stage::Maintain
            && !self.dismissed()
            && self.intake().is_none()
    }

    /// Whether Enter reopens this card: work under way whose workspace closed, or done work.
    pub fn reopenable(&self) -> bool {
        self.can_reopen() || self.can_reopen_done()
    }

    /// Open work no agent is at, as its feed says (no workspace on it, and nothing running its
    /// conversation), and no workspace or agent here says otherwise. The card says "no agent".
    pub fn no_agent(&self) -> bool {
        self.detail.as_ref().is_some_and(|d| d.no_agent)
            && !self.is_open_workspace()
            && self.agents.is_empty()
    }

    /// Open work that `C` can mark done, and `x` set aside, without starting anything: a record
    /// that isn't done, dismissed or still being written up, with no workspace open. The action
    /// backend still refuses one it finds an agent at (intent 0289).
    pub fn closable(&self) -> bool {
        self.detail.is_some()
            && !matches!(self.stage, Stage::Maintain | Stage::Misc)
            && self.state.as_deref() != Some("closed")
            && self.attention != Attention::Done
            && !self.dismissed()
            && self.intake() != Some("pending")
            && !self.is_open_workspace()
    }

    /// Work an agent may be at, its workspace (or, for a child, its tab) open on its card's
    /// machine: `X` stops that agent, closes the workspace or tab, and sets the work aside in Won't
    /// do, after a yes (intent 0321). A record that isn't done, dismissed or still being written up.
    pub fn cancellable(&self) -> bool {
        self.detail.is_some()
            && self.is_open_workspace()
            && !matches!(self.stage, Stage::Maintain | Stage::Misc)
            && self.state.as_deref() != Some("closed")
            && self.attention != Attention::Done
            && !self.dismissed()
            && self.intake() != Some("pending")
    }

    /// Work under way, whatever its workspace: a record in a working stage, not done, dismissed or
    /// still being written up. What a reopen needs, once its workspace is known to have gone.
    pub fn under_way(&self) -> bool {
        self.detail.is_some()
            && matches!(
                self.stage,
                Stage::Plan | Stage::Design | Stage::Build | Stage::Test | Stage::Deploy
            )
            && self.attention != Attention::Done
            && !self.dismissed()
            && self.intake().is_none()
    }

    /// The agent whose conversation a reopen would resume, named for people: "Claude", "Codex".
    pub fn resume_agent(&self) -> Option<String> {
        let agent = self
            .detail
            .as_ref()?
            .resume
            .as_ref()?
            .agent
            .as_deref()?
            .trim();
        let mut chars = agent.chars();
        let first = chars.next()?;
        Some(first.to_uppercase().chain(chars).collect())
    }

    /// How a message names this card's workspace: "#41's workspace", or "Workspace 41-dark-mode"
    /// for a card that is only a workspace.
    pub fn workspace_name(&self) -> String {
        match self.source {
            _ if self.reference.is_empty() => "That workspace".into(),
            Source::Feed => format!("{}'s workspace", self.reference),
            Source::Workspace => format!("Workspace {}", self.reference),
        }
    }

    pub fn needs_you(&self) -> bool {
        self.attention == Attention::NeedsYou
    }

    /// The text search matches against.
    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.reference,
            self.title,
            self.workspace_label.as_deref().unwrap_or(""),
            self.session.as_deref().unwrap_or(""),
            self.machine,
            self.why
        )
    }
}

/// A card as it was when an action about it started: which card, on which machine, its home, the
/// conversation a reopen would resume and its workspace. A late question, its yes and a jump apply
/// only to the same work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardSnapshot {
    pub key: String,
    pub machine: usize,
    pub home_machine: Option<String>,
    pub home_session: Option<String>,
    pub resume_agent: Option<String>,
    pub resume_conversation: Option<String>,
    pub workspace: Option<String>,
    /// Whether it was work waiting in the backlog: a won't do asked about it then applies only
    /// while it still is, and one asked about started work only while it isn't. Not part of
    /// same_work.
    pub waiting: bool,
}

impl CardSnapshot {
    pub fn of(card: &Card) -> CardSnapshot {
        let d = card.detail.as_ref();
        let home = d.and_then(|d| d.home.as_ref());
        let resume = d.and_then(|d| d.resume.as_ref());
        CardSnapshot {
            key: card.key.clone(),
            machine: card.machine_index,
            home_machine: home.and_then(|h| h.machine.clone()),
            home_session: home.and_then(|h| h.session.clone()),
            resume_agent: resume.and_then(|r| r.agent.clone()),
            resume_conversation: resume.and_then(|r| r.conversation.clone()),
            workspace: card.workspace_id.clone(),
            waiting: crate::app::waiting_work(card),
        }
    }

    /// Whether this card is still the same work, allowing only for its workspace to have gone.
    pub fn same_work(&self, card: &Card) -> bool {
        self.same_work_or(card, None)
    }

    /// The same, also allowing for it to have the workspace `made` (the one a reopen just made).
    pub fn same_work_or(&self, card: &Card, made: Option<&str>) -> bool {
        let now = CardSnapshot::of(card);
        now.workspace
            .as_deref()
            .is_none_or(|w| self.workspace.as_deref() == Some(w) || made == Some(w))
            && CardSnapshot {
                workspace: None,
                waiting: false,
                ..now
            } == CardSnapshot {
                workspace: None,
                waiting: false,
                ..self.clone()
            }
    }
}

/// Slot use for one herdr session: how many workspaces with an agent you talk to it holds, and
/// how many it may hold. `limit: None` means no limit (a background lane).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slots {
    pub used: u32,
    pub limit: Option<u32>,
}

impl Slots {
    pub fn is_full(&self) -> bool {
        matches!(self.limit, Some(limit) if self.used >= limit)
    }

    pub fn free(&self) -> Option<u32> {
        self.limit.map(|limit| limit.saturating_sub(self.used))
    }

    pub fn label(&self) -> String {
        match self.limit {
            Some(limit) => format!("{}/{}", self.used, limit),
            None => format!("{}", self.used),
        }
    }
}

/// The one-line answer to "should I take on more?".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    FinishFirst(usize),
    Full,
    Room { session: String, free: u32 },
    Empty,
}

impl Verdict {
    pub fn text(&self) -> String {
        match self {
            Verdict::FinishFirst(1) => "Finish first: 1 waits on you".to_string(),
            Verdict::FinishFirst(n) => format!("Finish first: {n} wait on you"),
            Verdict::Full => "Full: finish or shelve one first".to_string(),
            Verdict::Room { session, free: 1 } => format!("Room for one more in {session}"),
            Verdict::Room { session, free } => format!("Room for {free} more in {session}"),
            Verdict::Empty => "Nothing under way".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub needs_you: usize,
    pub running: usize,
    pub blocked: usize,
    pub unreachable: usize,
    pub cached: usize,
    pub cached_since: Option<DateTime<Local>>,
    pub slots_used: u32,
    pub slots_limit: u32,
    pub full_sessions: Vec<String>,
    pub backlog: usize,
    pub queued: usize,
    pub shelved: usize,
    pub verdict: Option<Verdict>,
}
