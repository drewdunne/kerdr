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
