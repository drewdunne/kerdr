//! Kerdr's state and its keys. The board owns the keyboard, so no key here needs a prefix;
//! the only keys that cross into herdr are herdr's own (see `herdr_keys`).

use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::board::{self, Board, MachineState, Scope};
use crate::config::Config;
use crate::model::{Attention, Card, CardSnapshot, Stage};
use crate::poll::{Conn, Event};

#[path = "account_controls.rs"]
pub mod account_controls;
#[path = "asking.rs"]
pub mod asking;
#[path = "controls.rs"]
mod controls;
#[path = "edit_controls.rs"]
pub mod edit_controls;
#[path = "errors.rs"]
pub(crate) mod errors;
#[path = "follow.rs"]
pub mod follow;
#[path = "inside.rs"]
pub mod inside;
#[path = "kickoff_controls.rs"]
pub mod kickoff_controls;
#[path = "resume.rs"]
pub mod resume;
#[path = "run_state.rs"]
pub mod run_state;
#[path = "view_controls.rs"]
pub mod view_controls;
use account_controls::AccountScreen;
pub(crate) use controls::waiting_work;
pub use kickoff_controls::Kickoff;

/// How long an action's answer stays on screen before a ⏎ that waited its turn says something
/// else over it (intent 0411).
const ANSWER_HOLD: Duration = Duration::from_secs(3);

/// A new intent in Andrew's own words. The agent that writes it up fills in everything else:
/// its title, who it's for, its project and its triage.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub words: String,
    pub machine: usize,
    pub session: String,
    /// Images pasted into it, each standing in the words as its `[Image #N]` tag.
    pub images: Vec<crate::images::Image>,
    /// The send these words came back from, when a save failed or went unheard: sent again
    /// unchanged, they go as that send, so the adapter finds what it may have made (intent 0445).
    pub came_back: Option<Submission>,
    /// Who it's filed under: the owner and team of the view it was made in (intent 0595).
    pub filed: Filed,
    /// What the view wanted to file it under that this machine's backend can't take, said in the
    /// box before it's sent.
    pub unfiled: Option<String>,
}

/// Who a new intent is filed under, from the view it was made in (intent 0595): on all of Kerdr
/// no one, and the agent that writes it up works it out from the words; on an owner (or its No
/// team), its organization and, for a tenant, the tenant; on a team, those and the team. Only
/// what the machine's backend says it takes (`create_fields`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filed {
    pub organization: Option<String>,
    pub tenant: Option<String>,
    pub team: Option<String>,
}

/// Words (and their images) a save gave back while the box held other words, with the send they
/// came back from (intent 0445).
#[derive(Clone, Debug)]
pub struct Returned {
    pub words: String,
    pub images: Vec<crate::images::Image>,
    pub from: Option<Submission>,
}

/// One new intent as sent: its id lets the adapter tell a resend of the same words from new
/// work. It's reused only for the same words to the same place, and only until an answer
/// names the record (or says plainly that nothing was made, when no earlier send went unheard).
/// Each send keeps its own, so several can be on their way at once (intent 0445).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub id: String,
    pub words: String,
    pub images: Vec<crate::images::Image>,
    pub machine: usize,
    pub session: String,
    /// The owner and team it was filed under: a resend counts as the same only with the same.
    pub filed: Filed,
    /// An earlier send of it got no answer, so it may have been made.
    pub unknown: bool,
}

/// One line of the filter list: an owner (organization or tenant), or one of its projects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterRow {
    pub label: String,
    pub kind: String,
    pub depth: u8,
    pub owner: Option<String>,
    pub project: Option<String>,
    pub open: usize,
    pub backlog: usize,
    /// What Enter on it picks.
    pub pick: Pick,
}

/// What a filter row picks: on the machine split, f's owner and project filter; on the owner
/// split (intent 0595), the top bar's owner or team (one choice, never a second filter), the
/// chosen owner's project, or a machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Filter,
    View(Scope),
    Project(Option<String>),
    Machine(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
    /// A change that didn't happen (intent 0416): red, over whatever screen is up, and kept until
    /// a key is pressed once it has been seen.
    Fail,
}

/// How long a failure stays up before a key may take it down: a key already on its way when it
/// appeared mustn't.
pub const FAIL_SEEN: Duration = Duration::from_millis(1500);

/// How long the board's turn waits for a key with nothing on its way. Answers from the background
/// threads are only taken when the turn comes round, and a key is what brings it round sooner.
pub const KEY_WAIT: Duration = Duration::from_millis(250);
/// The same while something is coming, so it shows when it comes, not up to a quarter of a second
/// later (intent 0491).
pub const KEY_WAIT_COMING: Duration = Duration::from_millis(50);

/// `R`, until every machine it asked has read its feed since: nothing on the board need change, so
/// the board says when it's done (intent 0491).
#[derive(Clone, Debug)]
pub struct Refresh {
    pub since: Instant,
    pub waiting: Vec<usize>,
    /// Those that stopped answering, or whose feed failed, meanwhile.
    pub missed: Vec<String>,
    /// What `R` said, which this replaces.
    pub said: String,
    /// `App::away` when it was pressed: its answer means nothing once he's been off the board.
    pub away: u64,
}

/// How long after `R` a machine it couldn't bring up to date coming back is said (intent 0591).
const REFRESH_BACK_WITHIN: Duration = Duration::from_secs(120);

/// How many failures one box shows: a key takes those down, and the rest come up next, so none
/// goes unseen.
pub const FAIL_PAGE: usize = 3;

/// The background filter (b in the filter screen): every card, only background work (the cards
/// marked bg), or none of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    #[default]
    All,
    Only,
    Hide,
}

impl Background {
    pub const ALL: [Background; 3] = [Background::All, Background::Only, Background::Hide];

    pub fn next(self) -> Background {
        match self {
            Background::All => Background::Only,
            Background::Only => Background::Hide,
            Background::Hide => Background::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Background::All => "all",
            Background::Only => "bg only",
            Background::Hide => "no bg",
        }
    }

    pub fn shows(self, background: bool) -> bool {
        match self {
            Background::All => true,
            Background::Only => background,
            Background::Hide => !background,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFilter {
    Everything,
    Cards,
    Workspaces,
    Sessions,
    DoneToo,
}

impl SearchFilter {
    pub const ALL: [SearchFilter; 5] = [
        SearchFilter::Everything,
        SearchFilter::Cards,
        SearchFilter::Workspaces,
        SearchFilter::Sessions,
        SearchFilter::DoneToo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchFilter::Everything => "everything",
            SearchFilter::Cards => "cards",
            SearchFilter::Workspaces => "workspaces",
            SearchFilter::Sessions => "sessions",
            SearchFilter::DoneToo => "done too",
        }
    }
}

/// A search inside the Backlog, Won't do or Misc view (`/` there, intent 0402): it narrows that
/// view's own list where it is, rather than opening the search of everything. While `typing`,
/// letters go into the query; after Enter, they're the view's keys again, on the cards it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewSearch {
    pub query: String,
    pub typing: bool,
}

#[derive(Clone, Debug)]
pub struct Search {
    pub query: String,
    pub filter: SearchFilter,
    pub sel: usize,
}

#[derive(Clone, Debug)]
pub enum Mode {
    Accounts {
        screen: AccountScreen,
        back: Box<Mode>,
    },
    Palette {
        back: Box<Mode>,
        /// The card the menu opened on: its card keys act on that card or on none.
        target: Option<String>,
    },
    Compose {
        draft: Draft,
        back: Box<Mode>,
    },
    Filter {
        query: String,
        sel: usize,
        /// Letters go into the search (after /) rather than acting as the screen's keys.
        typing: bool,
        back: Box<Mode>,
    },
    /// Background work's open questions, one at a time, taking your answers (intent 0409).
    Kickoff {
        form: Kickoff,
        back: Box<Mode>,
        target: Option<Box<CardSnapshot>>,
    },
    Confirm {
        machine: usize,
        request: serde_json::Value,
        title: String,
        message: String,
        back: Box<Mode>,
        /// The card it asks about, as it was when the action started: a yes applies to that
        /// work as it was, never to another card or a changed one.
        target: Option<Box<CardSnapshot>>,
        /// A full session's other answer, for a reopen: wait in its queue (w).
        queue: bool,
        /// For a question that appears on its own (an action's answer): keys before this are
        /// ignored, so one held or typed before it appeared can't answer it.
        ready_at: Option<Instant>,
        /// A question Kerdr asks for itself (reopening a card's own agent when its tab is gone):
        /// the yes sends the request as it is, so the backend still asks its own questions.
        plain: bool,
        /// The answers its backend offers beyond yes and no (intent 0591): each sends the request
        /// again with `choice` and the question's token. Empty for a backend that offers none.
        choices: Vec<crate::actions::Choice>,
        /// At the most its session holds, a yes can't open it there: ⏎ queues it (intent 0591).
        enter_queues: bool,
    },
    Board,
    /// A card's own screen: the card itself, then what's inside it (`o`). `focus` is the row
    /// selected: "" for the card itself, a piece's key, or `group:<id>` for a folded group.
    Card {
        card: String,
        focus: String,
        opened: Vec<crate::family::Group>,
        back: Box<Mode>,
    },
    Peek {
        scroll: u16,
        back: Box<Mode>,
    },
    Search(Search),
    Keys {
        filter: String,
        typing: bool,
        scroll: u16,
    },
    Machines {
        sel: usize,
    },
    Backlog {
        sel: usize,
    },
    Misc {
        sel: usize,
    },
    /// Finished work, on a screen of its own (`D`): the Maintain column's cards, for a board that
    /// hides that column or one too narrow to show it (intent 0444).
    Done {
        sel: usize,
    },
}

/// The selection in Misc or Done, the list views whose selection is the board's own.
fn misc_selection(mode: &Mode) -> Option<usize> {
    match mode {
        Mode::Misc { sel } | Mode::Done { sel } => Some(*sel),
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => misc_selection(back),
        _ => None,
    }
}

fn set_misc_selection(mode: &mut Mode, selected: usize) {
    match mode {
        Mode::Misc { sel } | Mode::Done { sel } => *sel = selected,
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => set_misc_selection(back, selected),
        _ => {}
    }
}

fn backlog_selection(mode: &Mode) -> Option<usize> {
    match mode {
        Mode::Backlog { sel } => Some(*sel),
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => backlog_selection(back),
        _ => None,
    }
}

fn set_backlog_selection(mode: &mut Mode, selected: usize) {
    match mode {
        Mode::Backlog { sel } => *sel = selected,
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => set_backlog_selection(back, selected),
        _ => {}
    }
}

fn holds_card(mode: &Mode) -> bool {
    match mode {
        Mode::Card { .. } => true,
        Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => holds_card(back),
        _ => false,
    }
}

/// The list view (Backlog, Won't do, Misc or Done) under this screen, if it was opened from one.
fn list_view(mode: &Mode) -> Option<&Mode> {
    match mode {
        Mode::Backlog { .. } | Mode::Misc { .. } | Mode::Done { .. } => Some(mode),
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. }
        | Mode::Card { back, .. } => list_view(back),
        _ => None,
    }
}

fn holds_peek(mode: &Mode) -> bool {
    match mode {
        Mode::Peek { .. } => true,
        Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Kickoff { back, .. }
        | Mode::Confirm { back, .. } => holds_peek(back),
        _ => false,
    }
}

/// The action of Kerdr's own question whether a new intent's words go without the image that
/// didn't go in: never sent to a backend.
pub(crate) const SEND_WITHOUT: &str = "kerdr:send-without-image";

/// Words whose image didn't go in, and the question about them: its number goes in the question's
/// request, so its answer finds these words and no others (Codex's review).
#[derive(Clone, Debug)]
pub(crate) struct SendWithout {
    pub n: u64,
    pub machine: usize,
    pub draft: Draft,
    pub message: String,
}
/// Kerdr's own question after a saved list leaves running agents on other accounts: its yes
/// moves them to the list, one at a time (intent 0591, B3-42).
pub(crate) const MOVE_OFF_LIST: &str = "kerdr:move-off-list";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Mutate {
        machine: usize,
        request: serde_json::Value,
    },
    /// Read images into the new-intent box, away from the board's own thread.
    LoadImages(crate::images::Source),
    None,
    CopyLink(String),
    /// A code to type on a sign-in page, to the terminal clipboard.
    CopyCode(String),
    OpenLink(String),
    Quit,
    /// Open herdr full screen on this card's workspace.
    Jump(String),
    /// Open herdr full screen on a workspace no card claims.
    JumpWorkspace {
        machine: String,
        session: String,
        id: String,
    },
    /// Read a machine again now: one, or every machine that isn't answering.
    Retry(Option<usize>),
    /// Run SSH in the foreground so you can answer its prompt.
    Answer(usize),
    /// Run herdr's own setup for a machine, which asks before changing anything.
    Setup(usize),
    ToggleHidden(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Card(usize),
    Workspace(usize),
    Session(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub col: usize,
    pub key: Option<String>,
}

/// How far a question's box is scrolled when the screen can't hold all of it (Codex's review): set
/// as it's drawn, as the Kickoff box's is, and a new question starts at its top.
#[derive(Clone, Debug, Default)]
pub struct QuestionView {
    /// Which question it's for: its title and words, hashed.
    of: std::cell::Cell<u64>,
    scroll: std::cell::Cell<u16>,
    /// Its lines below what the box shows when scrolled to its top: 0 when all of it shows.
    most: std::cell::Cell<u16>,
}

impl QuestionView {
    /// Whether some of the question is still below the box.
    pub fn unseen(&self) -> bool {
        self.scroll.get() < self.most.get()
    }
    pub fn down(&self, by: u16) {
        self.scroll
            .set(self.scroll.get().saturating_add(by).min(self.most.get()));
    }
    pub fn up(&self, by: u16) {
        self.scroll.set(self.scroll.get().saturating_sub(by));
    }
    pub fn to_end(&self) {
        self.scroll.set(self.most.get());
    }
    /// As it's drawn: this question, its lines and the rows the box gives them. Where its text
    /// starts.
    pub fn fit(&self, title: &str, message: &str, lines: u16, rows: u16) -> u16 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (title, message).hash(&mut h);
        let of = h.finish();
        if self.of.get() != of {
            self.of.set(of);
            self.scroll.set(0);
        }
        let most = lines.saturating_sub(rows);
        self.most.set(most);
        self.scroll.set(self.scroll.get().min(most));
        self.scroll.get()
    }
}

/// A workspace a reopen just made, to go into as soon as its answer arrives: on the machine the
/// action ran on, before any card claims it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAfter {
    pub machine: usize,
    pub session: String,
    pub workspace: String,
    pub label: String,
    /// The tab in it to land on, when the reopen opened a new tab in a workspace already open
    /// (beside its parent's agent, or the tab still open there): herdr leaves its active tab as
    /// it was.
    pub tab: Option<String>,
    /// The card it was reopened from, for going on to the next that needs you.
    pub from: String,
}

/// A card to jump into as soon as its machine answers, and what to call it if the card is gone
/// by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingJump {
    pub key: String,
    pub workspace: String,
    pub machine: String,
    /// Waiting for one herdr session there that didn't answer its last read, and since when: Kerdr
    /// goes in once it answers, and gives up after STUCK_WAIT (intent 0591).
    pub stuck: Option<(String, Instant)>,
    /// A workspace no card claims, or one a reopen's answer named, by machine, session and id, in
    /// place of a card, with the tab to land on when one was named (a child's new tab).
    pub bare: Option<(usize, String, String, Option<String>)>,
    /// A bare workspace's conversation, as the board saw it when ⏎ began to wait: offered to
    /// resume if the workspace has closed by the time its machine answers (intent 0591, K1-03).
    pub resume: Option<resume::BareResume>,
}

/// How long ⏎ waits for a herdr session that didn't answer its last read before saying it's still
/// stuck (intent 0591).
pub const STUCK_WAIT: Duration = Duration::from_secs(60);

pub struct App {
    pub cfg: Config,
    pub machines: Vec<MachineState>,
    pub board: Board,
    pub tabs: Vec<Scope>,
    pub tab: usize,
    /// None is the overview; session tabs belong only to this machine. Only on the machine split.
    pub machine: Option<String>,
    /// The owner the top bar is on, on the owner split (intent 0595); None is all of Kerdr.
    pub owner_pick: Option<String>,
    /// f's Machine row on the owner split: only this machine's work.
    pub machine_filter: Option<String>,
    /// The band moved onto last on a board that mixes stage lists: it stays open.
    pub open_band: Option<String>,
    /// The band h and l from Backlog or Done go into.
    pub last_band: Option<String>,
    /// f's owner filter, on the machine split.
    pub owner: Option<String>,
    pub project: Option<String>,
    pub provider: Option<String>,
    pub background: Background,
    pub wont_do: bool,
    /// An account action is running: they wait for their answer, one at a time. Changes to cards
    /// go through `resolver` instead and never hold the board (intent 0416).
    pub busy: bool,
    /// The action resolution tool: changes to cards on their way to the backend, shown at once.
    pub resolver: crate::resolve::Resolver,
    /// The record the running action is for, when it names one: ⏎ on its card says how long it
    /// has been running rather than sending it again (intent 0411).
    pub busy_id: Option<String>,
    /// When the running action started, and what the board says it's doing.
    pub busy_since: Option<Instant>,
    pub busy_label: String,
    pub count: String,
    /// Words typed into the new-intent box and not sent yet, kept for the next time it opens.
    pub draft: String,
    /// The images pasted with those words.
    pub draft_images: Vec<crate::images::Image>,
    /// The send those words came back from, if they did (Draft::came_back).
    pub draft_came_back: Option<Submission>,
    /// The images being read for the new-intent box, by the number of that read; a read whose
    /// box closed meanwhile is dropped when it ends.
    pub image_load: Option<u64>,
    image_loads: u64,
    /// Where in the words that read's tags go: where the words ended when it began, so words
    /// typed while it reads come after them.
    image_at: usize,
    /// New intents sent, each while its answer is still owed or unknown (intent 0445).
    pub submissions: Vec<Submission>,
    /// Words (and their images) whose save failed while the box held other words: c offers them,
    /// oldest first, once the box is empty (intent 0445).
    pub returned: std::collections::VecDeque<Returned>,
    /// A new intent Enter sent while its image was still being read: the read's number and its
    /// words, sent once the image is in (intent 0445).
    pub send_after_load: Option<(u64, Draft)>,
    /// New intents whose image didn't go in, while Kerdr asks whether their words go without it
    /// (intent 0591, K1-37), oldest first. Their words are kept meanwhile where give_back put
    /// them. Each question is bound to its own words, so answering one never sends another's;
    /// one that came while he typed or answered something else is asked once he's back on the
    /// board (Codex's review).
    pub(crate) send_without: Vec<SendWithout>,
    /// The number the last of those got.
    pub(crate) send_without_count: u64,
    /// How far the question box on screen is scrolled (Codex's review).
    pub question_view: QuestionView,
    /// The answer being handled is for a start or reopen whose workspace Andrew already went
    /// into himself (intent 0445).
    answer_went_in: bool,
    /// The intent just created here, selected in the backlog as soon as its card arrives.
    pub new_intent_id: Option<(usize, String)>,
    /// The new-intent box closed when Enter sent its words, and nothing has been pressed or pasted
    /// since: when the answer comes, the backlog opens on the new card. Once you've moved on,
    /// nothing pulls you away.
    pub(crate) closed_on_send: bool,
    backlog_focus: Option<String>,
    /// `/` in a list view: what narrows it, kept while you peek or act from it, and dropped
    /// once you leave the view.
    pub view_search: Option<ViewSearch>,
    pub sel: Selection,
    pub mode: Mode,
    pub message: Option<(String, Tone, Instant)>,
    /// What was said while a failure was up, shown once it's taken down.
    pub held: Vec<(String, Tone)>,
    /// A card to jump into as soon as its machine answers.
    pub pending_jump: Option<PendingJump>,
    /// The workspace a reopen just made, to go into now.
    pub open_after: Option<OpenAfter>,
    /// The record a start just went out for, and when, so a second ⏎ before the feed shows it
    /// under way doesn't start it again (intent 0404).
    pub started: Option<(String, Instant)>,
    /// The card the running action was asked from, as it was then: its answer asks or takes you
    /// in only while you're still on that work. Esc lets it go.
    pub action_from: Option<CardSnapshot>,
    /// A card whose workspace turned out to have closed when you went in: reopen it next, once
    /// nothing else is running.
    pub pending_reopen: Option<String>,
    /// Actions refused because their intent lives on another machine than the one asked: each
    /// request, to send there next, and the card it was asked from (intent 0411). Several can
    /// answer at once now that changes run side by side (intent 0416), so they wait in order.
    pub resend: std::collections::VecDeque<(usize, serde_json::Value, Option<CardSnapshot>)>,
    /// The machine and record of each such resend still unanswered: its answer is shown as it
    /// is, never sent on again, so one ⏎ asks two machines at most.
    pub resent: Vec<(usize, String)>,
    /// Each record a refusal said lives on another machine, that machine, and the home the
    /// board's feed still showed for it when the refusal came: its actions go there, and a
    /// question that machine asks can be answered yes, even before the board catches up. Each
    /// holds only while the feed still shows that old home; fresh news of its home ends it. One
    /// for each record, since several can be refused at once (intent 0416).
    pub moved_home: Vec<(String, usize, Option<String>)>,
    /// What's typed in a Kickoff Questions box and not saved yet, by its record's number.
    pub kickoff_kept: std::collections::HashMap<String, Vec<kickoff_controls::KeptAnswer>>,
    /// Kickoff answers its backend gave back because their questions changed while he answered,
    /// by its record's number: filled in on the next box d opens (intent 0591, B1-108).
    pub kickoff_prefill: std::collections::HashMap<String, Vec<crate::actions::Prefill>>,
    /// The tokens of questions answered from the box, whose copy in the feed is out of date until
    /// its next read: d doesn't open them again from there (intent 0491).
    pub kickoff_sent: std::collections::HashSet<String>,
    /// The last check sent behind a Kickoff Questions box, so a late answer can't touch a newer box.
    pub kickoff_checks: u64,
    /// When the last action answered: what it said stays on screen a moment before a waiting ⏎
    /// takes its turn and says something else.
    pub answered_at: Option<Instant>,
    pub width: u16,
    /// The screen's height, for how many bands fit; 0 for no limit.
    pub height: u16,
    pub demo: bool,
    /// Fixed in demo mode so screenshots don't change; the clock otherwise.
    pub fixed_now: Option<DateTime<Local>>,
    /// herdr's prefix, for the keys Kerdr names inside herdr.
    pub prefix: String,
    first_selection_done: bool,
    pub account_queue: std::collections::VecDeque<(usize, serde_json::Value)>,
    /// Sign-in cancellations for a machine not answering, sent once it answers again, so Esc
    /// during a disconnect still ends that machine's sign-in.
    pub account_cancels: Vec<(usize, serde_json::Value)>,
    pub signin_open: Option<String>,
    pub account_batch_messages: Vec<String>,
    /// Some machine's part of that batch didn't happen: the summary is a warning (intent 0591).
    pub account_batch_failed: bool,
    /// Machines `R` couldn't bring up to date, still being tried, and when (intent 0591).
    pub refresh_behind: Option<(Instant, Vec<usize>)>,
    pub account_pending: Option<account_controls::Pending>,
    /// Backlog items' texts held for editing, by card (intent 0414): what each was read as, and
    /// your version until a save of it succeeds.
    pub edits: std::collections::HashMap<String, edit_controls::Held>,
    /// The reads `e` sent that haven't answered, one per card: each opens the editor only while
    /// you're still where you pressed it.
    pub edit_flights: Vec<edit_controls::EditFlight>,
    /// The card whose text the editor opens next, and when and where you asked for it: not while a
    /// failure is up, never long after you pressed e, and only while you're still there.
    pub edit_open: Option<String>,
    pub edit_open_when: Option<edit_controls::OpenWhen>,
    /// Where every error shown is kept (`errors`), and what it needs to know.
    pub error_log: crate::errlog::ErrorLog,
    /// What the command behind the answer about to arrive printed on standard error.
    pub answer_stderr: String,
    pub(crate) answering: Option<errors::Answering>,
    pub(crate) signin_kept: Option<String>,
    pub(crate) machine_problems: std::collections::HashMap<(usize, String), String>,
    /// How many times the board has handed the terminal over (into herdr, to SSH, to an editor):
    /// an edit's answer opens the editor only while this is what it was at `e`.
    pub away: u64,
    /// `R`, until it's up to date.
    pub refresh: Option<Refresh>,
    /// A card ⏎ was pressed on while its start ran: Kerdr goes in when its workspace opens. Not
    /// `pending_jump`, which takes a card that isn't open to have closed (intent 0491).
    pub enter_when_open: Option<String>,
    /// The words on screen that promise it, which go when it does (intent 0491).
    pub enter_said: Option<String>,
    /// Work that goes on after its answer, which Kerdr follows (intent 0591, follow.rs).
    pub follows: Vec<follow::Following>,
    /// s waiting for a machine's accounts before it opens the picker (intent 0591).
    pub accounts_wait: Option<run_state::AccountsWait>,
    /// A waited jump into a workspace that's due: its name and the tab it lands on.
    pub pending_go: Option<(String, Option<String>)>,
    /// Accounts picked with s that their cards are still owed, one for each card (intent 0591).
    pub owed_accounts: Vec<run_state::OwedAccount>,
    /// A move waiting on its account's sign-in on that machine: sent again by itself, as a new
    /// request, once that sign-in is done; dropped if he leaves it (intent 0591, B3-10).
    pub after_signin: Option<account_controls::AfterSignin>,
    /// The agents a saved list left on other accounts, moved to the list one at a time once he
    /// said yes (intent 0591, B3-42).
    pub off_list_moves: Option<account_controls::OffListMoves>,
    /// What each account question needs beyond its words: the agent or card it came from, and
    /// what follows once it's settled (Codex's review).
    pub asked_about: Vec<account_controls::AskedAbout>,
    /// The account questions the answer being read keeps going (asked again, held, or waiting on
    /// a sign-in): any other it answers is settled once it's read (Codex's third look).
    pub asked_now: Vec<(usize, serde_json::Value)>,
    /// The card whose account request is being sent again by itself, with the same op: it says
    /// "Trying again…" until the answer comes (intent 0591).
    pub account_retrying: Option<String>,
    /// The number of the last request asking an old backend to update, so each answer finds the
    /// waits it was asked for.
    pub update_asks: u64,
    /// The end of followed work being taken as its request's answer.
    pub(crate) follow_end: Option<follow::FollowEnd>,
    /// The record a start or reopen ⏎ just asked for, for its change to note it was ⏎'s.
    pub(crate) enter_sent: Option<String>,
    /// The answer being taken is to a start ⏎ asked for; and the record whose question, up now,
    /// such a start asked (Codex's review).
    pub(crate) answer_by_enter: bool,
    pub(crate) confirm_by_enter: Option<String>,
    /// Questions that came while you were elsewhere, each waiting on its card (intent 0591).
    pub(crate) waiting: Vec<asking::Waiting>,
    /// When a peek was opened from the backlog, whose ⏎ peeks: a second ⏎ held or doubled with
    /// it never starts the work it shows (intent 0591, K1-07).
    pub(crate) peek_opened: Option<Instant>,
}

impl App {
    pub fn new(cfg: Config, machines: Vec<MachineState>) -> App {
        let mut app = App {
            cfg,
            machines,
            board: Board::default(),
            tabs: vec![Scope::All],
            tab: 0,
            machine: None,
            owner_pick: None,
            machine_filter: None,
            open_band: None,
            last_band: None,
            owner: None,
            project: None,
            provider: None,
            background: Background::All,
            wont_do: false,
            busy: false,
            resolver: Default::default(),
            busy_id: None,
            busy_since: None,
            busy_label: String::new(),
            count: String::new(),
            draft: String::new(),
            draft_images: vec![],
            draft_came_back: None,
            image_load: None,
            image_loads: 0,
            image_at: 0,
            new_intent_id: None,
            closed_on_send: false,
            submissions: vec![],
            returned: Default::default(),
            send_after_load: None,
            send_without: Vec::new(),
            send_without_count: 0,
            question_view: QuestionView::default(),
            answer_went_in: false,
            backlog_focus: None,
            view_search: None,
            sel: Selection::default(),
            mode: Mode::Board,
            message: None,
            held: vec![],
            pending_jump: None,
            open_after: None,
            started: None,
            action_from: None,
            pending_reopen: None,
            resend: Default::default(),
            resent: vec![],
            moved_home: vec![],
            kickoff_kept: Default::default(),
            kickoff_prefill: Default::default(),
            kickoff_sent: Default::default(),
            kickoff_checks: 0,
            answered_at: None,
            width: 200,
            height: 0,
            demo: false,
            fixed_now: None,
            prefix: "ctrl+b".to_string(),
            first_selection_done: false,
            account_queue: Default::default(),
            account_cancels: Vec::new(),
            signin_open: None,
            account_batch_messages: vec![],
            account_batch_failed: false,
            refresh_behind: None,
            account_pending: None,
            edits: Default::default(),
            edit_flights: vec![],
            edit_open: None,
            edit_open_when: None,
            error_log: crate::errlog::ErrorLog::on_this_machine(),
            answer_stderr: String::new(),
            answering: None,
            signin_kept: None,
            machine_problems: Default::default(),
            away: 0,
            refresh: None,
            enter_when_open: None,
            enter_said: None,
            follows: vec![],
            accounts_wait: None,
            pending_go: None,
            owed_accounts: Vec::new(),
            after_signin: None,
            off_list_moves: None,
            asked_about: Vec::new(),
            asked_now: Vec::new(),
            account_retrying: None,
            update_asks: 0,
            follow_end: None,
            enter_sent: None,
            answer_by_enter: false,
            confirm_by_enter: None,
            waiting: vec![],
            peek_opened: None,
        };
        app.rebuild();
        app
    }

    pub fn now(&self) -> DateTime<Local> {
        self.fixed_now.unwrap_or_else(Local::now)
    }

    pub fn this_machine(&self) -> String {
        self.machines.first().map(|m| m.name()).unwrap_or_default()
    }

    pub fn scope(&self) -> &Scope {
        self.tabs.get(self.tab).unwrap_or(&Scope::All)
    }

    pub fn say(&mut self, text: impl Into<String>, tone: Tone) {
        let text = text.into();
        match tone {
            Tone::Warn => self.keep_error(&text, "warn"),
            Tone::Fail => self.keep_error(&text, "fail"),
            _ => {}
        }
        self.show(text, tone);
    }

    /// Puts something on the message line, the log having had it.
    fn show(&mut self, text: String, tone: Tone) {
        // A failure stays up until it has been seen: the latest thing said meanwhile waits, and
        // shows once the failure is taken down.
        if tone != Tone::Fail && self.failing() {
            if !self.held.iter().any(|(t, _)| *t == text) {
                self.held.push((text, tone));
            }
            return;
        }
        self.message = Some((text, tone, Instant::now()));
    }

    /// Says that a change didn't happen, where it can't be missed (intent 0416). Failures that
    /// come together stay together, the newest last.
    pub fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.keep_error(&text, "fail");
        // Every failure not yet seen stays, the newest last; the box shows as many as fit.
        let text = match &self.message {
            Some((old, Tone::Fail, _)) => format!("{old}\n{text}"),
            _ => text,
        };
        self.message = Some((text, Tone::Fail, Instant::now()));
    }

    /// Whether a failure is up.
    pub fn failing(&self) -> bool {
        matches!(self.message, Some((_, Tone::Fail, _)))
    }

    /// How long this turn of the board waits for a key: less while an answer, an image or a jump is
    /// on its way, or a guard's hint is about to light up (intent 0491). Not for a start waiting
    /// for its write-up, which can take minutes; the runner hears of that itself.
    pub fn key_wait(&self) -> Duration {
        let answer_coming = self.busy
            || self.refresh.is_some()
            || self.enter_when_open.is_some()
            || self
                .resolver
                .on_its_way()
                .any(|job| !job.waits_for_write_up());
        let guard_up = match &self.mode {
            Mode::Confirm {
                ready_at: Some(at), ..
            } => Instant::now() < *at,
            Mode::Kickoff { form, .. } => Instant::now() < form.ready_at,
            _ => false,
        } || matches!(&self.message, Some((_, Tone::Fail, at)) if at.elapsed() < FAIL_SEEN);
        if answer_coming || guard_up || self.image_load.is_some() || self.pending_jump.is_some() {
            KEY_WAIT_COMING
        } else {
            KEY_WAIT
        }
    }

    /// A key was pressed while a failure is up: once it has been up long enough to be seen, the
    /// key takes it down and does nothing else, and what was said meanwhile shows. Before then a
    /// key already on its way does what it was for, except answer a question under the box.
    /// Whether the key is used up.
    fn seen_failure(&mut self, key: &KeyEvent) -> bool {
        let Some((_, Tone::Fail, at)) = &self.message else {
            return false;
        };
        if at.elapsed() >= FAIL_SEEN {
            // The failures shown go; any that didn't fit come up next.
            let rest: Vec<String> = self
                .message
                .take()
                .map(|(t, ..)| t.lines().skip(FAIL_PAGE).map(str::to_string).collect())
                .unwrap_or_default();
            if !rest.is_empty() {
                self.message = Some((rest.join("\n"), Tone::Fail, Instant::now()));
                return true;
            }
            // Then everything said meanwhile, together, in the strongest tone among it.
            let held = std::mem::take(&mut self.held);
            if !held.is_empty() {
                let tone = [Tone::Warn, Tone::Good]
                    .into_iter()
                    .find(|t| held.iter().any(|(_, h)| h == t))
                    .unwrap_or(Tone::Info);
                let text: Vec<String> = held.into_iter().map(|(t, _)| t).collect();
                self.show(text.join("  ·  "), tone);
            }
            return true;
        }
        // Too soon to have been read: nothing that answers a question under it, leaves the
        // screen or quits gets through.
        let quits = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
            || (key.code == KeyCode::Char('q')
                && matches!(
                    self.mode,
                    Mode::Board
                        | Mode::Backlog { .. }
                        | Mode::Misc { .. }
                        | Mode::Card { .. }
                        | Mode::Peek { .. }
                )
                && !self.typing_view_search());
        key.code == KeyCode::Esc
            || quits
            || matches!(self.mode, Mode::Confirm { .. })
            || matches!(&self.mode, Mode::Accounts { screen, .. }
                if matches!(screen, account_controls::AccountScreen::Confirm { .. }))
    }

    /// Takes back a message, but only if it's still the one on screen: something said since
    /// stays.
    pub fn unsay(&mut self, text: &str) {
        if self.message.as_ref().is_some_and(|(t, _, _)| t == text) {
            self.message = None;
        }
    }

    /// The message on screen, if it was said since the running action started: a refusal, a
    /// queued ⏎, the working line itself. Older ones give way to the action's own line.
    pub fn message_since_busy(&self) -> Option<(&str, Tone)> {
        let since = self.busy_since?;
        self.message
            .as_ref()
            .filter(|(_, _, at)| *at >= since && at.elapsed() < Duration::from_secs(8))
            .map(|(t, tone, _)| (t.as_str(), *tone))
    }

    /// What the running action is doing and for how long, as in "Reopening #39… 12 s"; None
    /// when it hasn't said.
    pub fn busy_line(&self) -> Option<String> {
        if !self.busy || self.busy_label.is_empty() {
            return None;
        }
        let secs = self.busy_since.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        Some(format!("{} {secs} s", self.busy_label))
    }

    pub fn message(&self) -> Option<(&str, Tone)> {
        self.message
            .as_ref()
            .filter(|(_, tone, at)| *tone == Tone::Fail || at.elapsed() < Duration::from_secs(8))
            .map(|(t, tone, _)| (t.as_str(), *tone))
    }

    /// `R`: says what it's about to read, and remembers what it waits on. A machine it can only try
    /// again goes down as missed from the start: its cards stay as they were, so `R` can't call it
    /// up to date (intent 0491). The machines to read, feeds and all, and those to try again.
    pub fn ask_refresh(&mut self, since: Instant) -> (Vec<usize>, Vec<usize>) {
        let shown = || (0..self.machines.len()).filter(|&i| self.machines[i].is_shown());
        let (reading, trying): (Vec<usize>, Vec<usize>) =
            shown().partition(|&i| self.machines[i].conn.is_live());
        let waiting: Vec<usize> = reading
            .iter()
            .copied()
            .filter(|&i| self.machines[i].machine.feed.is_some())
            .collect();
        let said = if trying.is_empty() {
            "Reading every machine now.".to_string()
        } else {
            format!(
                "Reading every machine now, and trying {} again.",
                trying.len()
            )
        };
        self.say(said.clone(), Tone::Info);
        self.refresh = (!waiting.is_empty()).then(|| Refresh {
            since,
            waiting,
            missed: trying.iter().map(|&i| self.machine_why(i)).collect(),
            said,
            away: self.away,
        });
        (reading, trying)
    }

    /// Which machines `R` waits on have read their feed since it. Asked before the reads waiting are
    /// taken, so what this finds has been applied by the time `settle_refresh` speaks.
    pub fn refreshed(&self, inbox: &crate::poll::Inbox) -> Vec<usize> {
        self.refresh.as_ref().map_or_else(Vec::new, |r| {
            r.waiting
                .iter()
                .copied()
                .filter(|&i| inbox.feed_read_since(i, r.since))
                .collect()
        })
    }

    /// Says `R` is done once every machine it asked has read its feed, or dropped out of the
    /// reckoning. Only in place of its own words: something else said since matters more.
    pub fn settle_refresh(&mut self, fed: &[usize]) {
        let Some(mut r) = self.refresh.take() else {
            return;
        };
        // He went into a card meanwhile: only a key on the board this moment wants this answer, and
        // the read asked for on his return hasn't landed yet (intent 0491).
        if r.away != self.away {
            return;
        }
        let whys: Vec<String> = (0..self.machines.len())
            .map(|i| self.machine_why(i))
            .collect();
        let machines = &self.machines;
        let missed = &mut r.missed;
        r.waiting.retain(|&i| {
            let Some(m) = machines.get(i) else {
                return false;
            };
            if matches!(m.conn, Conn::Hidden) {
                return false;
            }
            let failed = !m.conn.is_live() || (fed.contains(&i) && m.feed_error.is_some());
            if failed {
                missed.push(whys[i].clone());
            } else if fed.contains(&i) && m.sessions_failing.is_some() {
                // Its cards are new, but herdr's list of sessions, and so its workspaces, are as
                // they last were (intent 0591).
                missed.push(format!("{}'s herdr sessions", m.name()));
            } else if fed.contains(&i) && !m.snapshot_errors.is_empty() {
                // Its cards are new, but a stuck session's workspaces are as they last were.
                let mut sessions: Vec<&str> =
                    m.snapshot_errors.keys().map(String::as_str).collect();
                sessions.sort_unstable();
                missed.push(format!("{}'s {}", m.name(), sessions.join(", ")));
            }
            !failed && !fed.contains(&i)
        });
        if !r.waiting.is_empty() {
            self.refresh = Some(r);
            return;
        }
        if self.message().is_some_and(|(said, _)| said != r.said) {
            return;
        }
        if r.missed.is_empty() {
            self.say("Up to date.", Tone::Good);
        } else {
            let names = r.missed.join(", ");
            self.say(format!("Up to date, except {names}."), Tone::Info);
            // Those still reading again by themselves: said when they land.
            let behind: Vec<usize> = (0..self.machines.len())
                .filter(|&i| self.machines[i].is_shown() && !self.machines[i].conn.is_live())
                .collect();
            if !behind.is_empty() {
                self.refresh_behind = Some((Instant::now(), behind));
            }
        }
    }

    /// A machine by name, and why it's behind when that's known: "arknet-one (SSH timed out,
    /// still trying)" (intent 0591).
    /// Whether its machine's feed says herdr wasn't read for this card, so "no workspace" is
    /// only a guess (intent 0591).
    pub fn workspaces_stale(&self, c: &Card) -> bool {
        self.machines
            .get(c.machine_index)
            .is_some_and(|m| m.workspaces_stale(c.session.as_deref()))
    }

    fn machine_why(&self, i: usize) -> String {
        let m = &self.machines[i];
        let why = m.conn.briefly().or_else(|| {
            m.feed_error
                .as_ref()
                .map(|e| format!("its feed failed: {e}"))
        });
        match why {
            Some(why) => format!("{} ({why})", m.name()),
            None => m.name(),
        }
    }

    /// `R` couldn't bring these machines up to date and they've come back since: said once they
    /// all have, unless something else has been said meanwhile.
    fn refresh_landed(&mut self) {
        let Some((at, behind)) = self.refresh_behind.as_mut() else {
            return;
        };
        if at.elapsed() >= REFRESH_BACK_WITHIN {
            self.refresh_behind = None;
            return;
        }
        let machines = &self.machines;
        behind.retain(|&i| {
            machines
                .get(i)
                .is_some_and(|m| m.is_shown() && !m.conn.is_live())
        });
        if !behind.is_empty() {
            return;
        }
        self.refresh_behind = None;
        if self
            .message()
            .is_none_or(|(said, _)| said.starts_with("Up to date, except"))
        {
            self.say("Now up to date.", Tone::Good);
        }
    }

    pub fn apply(&mut self, ev: Event) {
        let Some(m) = self.machines.get_mut(ev.machine) else {
            return;
        };
        if matches!(m.conn, Conn::Hidden) {
            return;
        }
        m.conn = ev.conn;
        let mut read = false;
        if let Some(data) = ev.data {
            read = matches!(data.feed, Some(Ok(_)));
            m.take(data, ev.at);
        }
        self.keep_machine_problems(ev.machine);
        if read {
            self.refresh_landed();
        }
        // A change that succeeded shows from the feed itself once a read has it (intent 0416).
        if read {
            self.resolver
                .read(ev.machine, self.machines[ev.machine].feed.as_ref());
        }
        self.rebuild();
    }

    pub fn rebuild(&mut self) {
        self.refresh_accounts();
        let current = self.scope().clone();
        // Changes still on their way show as done (intent 0416).
        self.board = if self.resolver.shows() {
            board::build(&self.resolver.overlay(&self.machines), &self.cfg)
        } else {
            board::build(&self.machines, &self.cfg)
        };
        // A question that came while you were elsewhere shows its card as asking you (intent 0591).
        self.show_waiting();
        let tabs = self.view_tabs();
        self.tab = tabs
            .iter()
            .position(|t| *t == current)
            .or_else(|| match &current {
                Scope::Session(m, _) => tabs.iter().position(|t| *t == Scope::Machine(m.clone())),
                Scope::Team(o, _) => tabs.iter().position(|t| *t == Scope::Owner(o.clone())),
                _ => None,
            })
            .unwrap_or(0);
        self.tabs = tabs;
        if let Some(sel) = misc_selection(&self.mode) {
            let items = self.list_cards();
            let found = self
                .sel
                .key
                .as_ref()
                .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key));
            let selected = found.unwrap_or(sel.min(items.len().saturating_sub(1)));
            if !holds_peek(&self.mode) {
                self.sel.key = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
                self.sel.col = 0;
            }
            set_misc_selection(&mut self.mode, selected);
            return;
        }
        // The card shown while a new intent saved has given way to its record's, whichever
        // machine's copy shows it first and before its answer comes: whatever was on the one is on
        // the other (intent 0445).
        for shown in [self.sel.key.clone(), self.backlog_focus.clone()]
            .into_iter()
            .flatten()
        {
            let Some((_, submission)) = shown.split_once(":saving-") else {
                continue;
            };
            if self.card(&shown).is_some() {
                continue;
            }
            let Some(key) = self
                .board
                .cards
                .iter()
                .find(|c| {
                    c.detail
                        .as_ref()
                        .is_some_and(|d| d.submission.as_deref() == Some(submission))
                })
                .map(|c| c.key.clone())
            else {
                continue;
            };
            if self.sel.key.as_deref() == Some(shown.as_str()) {
                self.sel.key = Some(key.clone());
            }
            if self.backlog_focus.as_deref() == Some(shown.as_str()) {
                self.backlog_focus = Some(key);
            }
        }
        self.fix_selection();
        // Only once its card is in the backlog as shown: a filter may hide it for now. Any
        // machine's copy of the store it went to is its card.
        if let Some((machine, id)) = self.new_intent_id.clone()
            && let record = format!("{}:{id}", self.source_of(machine))
            && let Some(&i) = self.backlog_cards().iter().find(|&&i| {
                let c = &self.board.cards[i];
                c.key == record
                    || (c.machine_index == machine && c.detail.as_ref().is_some_and(|d| d.id == id))
            })
        {
            let key = self.board.cards[i].key.clone();
            // The board's selection, too, when it was on the card shown while it saved.
            if self
                .sel
                .key
                .as_ref()
                .is_some_and(|k| k.contains(":saving-") && self.card(k).is_none())
            {
                self.sel.key = Some(key.clone());
            }
            self.backlog_focus = Some(key);
            self.new_intent_id = None;
        }
        if let Some(sel) = backlog_selection(&self.mode) {
            let items = self.backlog_cards();
            let found = self
                .backlog_focus
                .as_ref()
                .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key));
            let selected = found.unwrap_or(sel.min(items.len().saturating_sub(1)));
            // A pending command retains its original target even if that record disappeared.
            // Direct list navigation can move to the next available row.
            if found.is_some()
                || matches!(self.mode, Mode::Backlog { .. })
                || self.backlog_focus.is_none()
            {
                self.backlog_focus = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
            }
            set_backlog_selection(&mut self.mode, selected);
        }
    }

    pub fn card(&self, key: &str) -> Option<&Card> {
        self.board.cards.iter().find(|c| c.key == key)
    }

    pub fn selected(&self) -> Option<&Card> {
        self.sel.key.as_deref().and_then(|k| self.card(k))
    }

    fn fix_selection(&mut self) {
        if let Some(sel) = misc_selection(&self.mode) {
            if !holds_peek(&self.mode) {
                let items = self.list_cards();
                let selected = self
                    .sel
                    .key
                    .as_ref()
                    .and_then(|key| items.iter().position(|&i| &self.board.cards[i].key == key))
                    .unwrap_or(sel.min(items.len().saturating_sub(1)));
                self.sel.key = items
                    .get(selected)
                    .map(|&i| self.board.cards[i].key.clone());
                set_misc_selection(&mut self.mode, selected);
            }
            return;
        }
        let on_card = matches!(self.mode, Mode::Card { .. });
        if self.fix_card_screen() {
            return;
        }
        // A card screen whose card has gone hands back to the list it was opened from: Misc and
        // Done keep their selection their own way, never the board's.
        if on_card && misc_selection(&self.mode).is_some() {
            self.fix_selection();
            return;
        }
        // A question over a card's screen keeps its row selected until it's answered, as a
        // peek keeps its record.
        if holds_card(&self.mode) {
            if let Some(card) = self.selected() {
                self.sel.col = self.col_of(card);
            }
            return;
        }
        if holds_peek(&self.mode) {
            // A peek is an operation on one stable record, including when its column is hidden.
            // Losing the record must never silently point an action at a neighbor.
            if let Some(card) = self.selected() {
                self.sel.col = self.col_of(card);
            }
            return;
        }
        let layout = self.layout();
        let cols = self.columns_in(&layout);
        let vis = self.visible_in(&layout);
        if let Some(key) = &self.sel.key {
            for (ci, col) in cols.iter().enumerate() {
                if vis[ci] && col.iter().any(|&i| &self.board.cards[i].key == key) {
                    self.sel.col = ci;
                    return;
                }
            }
        }
        // Start on the first card that needs you: that's what you came to the board for.
        let order = Self::start_order(&layout);
        if !self.first_selection_done || self.sel.key.is_some() {
            for ci in (1..layout.done()).filter(|&ci| vis[ci]) {
                if let Some(&i) = cols[ci].iter().find(|&&i| self.board.cards[i].needs_you()) {
                    self.sel = Selection {
                        col: ci,
                        key: Some(self.board.cards[i].key.clone()),
                    };
                    self.first_selection_done = true;
                    return;
                }
            }
        }
        let col =
            if self.sel.col < cols.len() && vis[self.sel.col] && !cols[self.sel.col].is_empty() {
                Some(self.sel.col)
            } else {
                order
                    .iter()
                    .copied()
                    .find(|&ci| vis[ci] && !cols[ci].is_empty())
            };
        match col {
            Some(ci) => {
                self.sel = Selection {
                    col: ci,
                    key: Some(self.board.cards[cols[ci][0]].key.clone()),
                };
                if !self.board.cards.is_empty() {
                    self.first_selection_done = true;
                }
            }
            None => {
                // An empty view: its columns may be fewer than the last view's.
                self.sel.key = None;
                self.sel.col = self.sel.col.min(cols.len().saturating_sub(1));
            }
        }
    }

    /// Cards that need you, in board order: column by column, top to bottom, each followed by
    /// the pieces inside it that need you.
    pub fn needs_you_order(&self) -> Vec<String> {
        self.needs_order()
    }

    /// Moves to the next (or previous) card or piece that needs you. Returns its key.
    pub fn step_needs_you(&mut self, dir: i32) -> Option<String> {
        self.needs_step(dir)
    }

    /// The next card that needs you after `key`, skipping it and any in `skip` (workspaces found
    /// closed on the way): for jumping straight on.
    pub fn next_needs_you_after(&self, key: &str, skip: &[String]) -> Option<String> {
        let order = self.needs_you_order();
        let pos = order.iter().position(|o| o == key);
        let candidates: Vec<&String> = match pos {
            Some(p) => order[p + 1..].iter().chain(order[..p].iter()).collect(),
            None => order.iter().collect(),
        };
        candidates
            .into_iter()
            .filter(|k| !skip.contains(k))
            .find(|k| {
                self.card(k)
                    .is_some_and(|c| c.is_open_workspace() && self.stuck_session(c).is_none())
            })
            .cloned()
    }

    /// How many others the next-card key could go on to from `key`.
    pub fn others_needing_you(&self, key: Option<&str>) -> usize {
        self.needs_you_order()
            .iter()
            .filter(|k| Some(k.as_str()) != key)
            .filter(|k| self.card(k).is_none_or(|c| self.stuck_session(c).is_none()))
            .count()
    }

    /// Why this card's herdr session is shown from its last good read, when it is. Kerdr can't go in
    /// till it answers, and trying would hold the board for the jump's whole limit (intent 0491).
    fn stuck_session(&self, card: &Card) -> Option<&str> {
        self.session_stuck(card.machine_index, card.session.as_deref()?)
    }

    /// The same, by machine and session.
    fn session_stuck(&self, machine: usize, session: &str) -> Option<&str> {
        let m = self.machines.get(machine)?;
        m.snapshot_errors.get(session).map(String::as_str)
    }

    /// What to say instead of going into this session, when it isn't answering. None when it is.
    pub fn stuck_refusal(&self, machine: usize, session: &str, what: &str) -> Option<String> {
        let why = self.session_stuck(machine, session)?;
        let name = self.machines[machine].name();
        Some(format!(
            "Kerdr can't go into {what} yet: {session} on {name} is stuck ({why}). R reads it again."
        ))
    }

    /// herdr has just said the workspace you went into, `tried`, has closed: work still under way,
    /// or done work, reopens next (done work after a yes), even before the board catches up; a card
    /// that has meanwhile moved to another workspace doesn't. Whether it will.
    pub fn closed_under_you(&mut self, key: &str, tried: Option<&str>) -> bool {
        let reopen = self.actions_on()
            && self.card(key).is_some_and(|c| {
                (c.under_way() || c.done())
                    && (c.workspace_id.is_none() || c.workspace_id.as_deref() == tried)
            });
        if reopen {
            self.owe_reopen(key, "workspace");
        }
        reopen
    }

    /// herdr has just said the workspace ⏎ went into has closed, or its session has stopped, on a
    /// card no reopen is owed (closed_under_you): what the board knows now decides, as plain ⏎
    /// would (intent 0591). The card names another workspace now: into that one, once. Work that
    /// never started, whose workspace has gone (a planner's): started, as d does, which starts its
    /// session first when that's what stopped. Otherwise None, and the caller says what happened.
    pub fn after_closed(&mut self, key: &str, tried: Option<&str>, retried: bool) -> Action {
        let Some(card) = self.card(key).cloned() else {
            return Action::None;
        };
        if !retried && card.is_open_workspace() && card.workspace_id.as_deref() != tried {
            return Action::Jump(key.to_string());
        }
        let never_started = card.detail.is_some()
            && !card.dismissed()
            && card.stage == Stage::Backlog
            && !matches!(
                card.state.as_deref(),
                Some("closed" | "active" | "background")
            );
        // As herdr says it is now, which the board will say at its next read.
        let mut gone = card.clone();
        gone.workspace_id = None;
        gone.session = None;
        gone.tab_id = None;
        gone.agents.clear();
        // A conversation on record: reopened, as for work under way, and its backend refuses
        // one still running elsewhere (intent 0591, K3-02).
        let conversation = card
            .detail
            .as_ref()
            .and_then(|d| d.resume.as_ref())
            .is_some_and(|r| r.conversation.is_some());
        if conversation && !card.dismissed() && self.actions_on() {
            let action = self.reopen_action(&gone);
            if matches!(action, Action::Mutate { .. }) {
                self.action_from = Some(CardSnapshot::of(&card));
            }
            return self.by_enter(action);
        }
        if never_started && self.actions_on() {
            let action = self.intent_action_on(gone, "dispatch", serde_json::json!({}));
            if matches!(action, Action::Mutate { .. }) {
                // Its answer asks or takes you in on the card as the board still shows it.
                self.action_from = Some(CardSnapshot::of(&card));
            }
            return self.by_enter(action);
        }
        Action::None
    }

    /// Kerdr owes this card a reopen, its workspace or tab found closed: it starts once no question
    /// is up, whatever account action runs meanwhile (intent 0445).
    pub(crate) fn owe_reopen(&mut self, key: &str, _what: &str) {
        self.pending_reopen = Some(key.to_string());
    }

    /// Whether a start just went out for this card's record and the board hasn't shown it under
    /// way yet, said when so: a second ⏎ mustn't start it twice (intent 0404).
    fn just_started(&mut self, card: &Card) -> bool {
        if self.still_changing(card, "enter") {
            return true;
        }
        let starting = card
            .detail
            .as_ref()
            .is_some_and(|d| self.started_lately(&d.id));
        if starting {
            self.say(
                format!(
                    "{} is starting. Kerdr goes in when its workspace opens; Esc stops waiting.",
                    card.reference
                ),
                Tone::Info,
            );
            self.remember_enter(&card.key);
        }
        starting
    }

    /// What Kerdr owes once no question is up: a resend to a record's real home, or a reopen of a
    /// card whose workspace closed as you went in. It waits while you're typing or anywhere but
    /// the board, a peek, a list or a card's own screen.
    pub fn next_owed(&mut self) -> Action {
        // A refusal that named the intent's home elsewhere: ask there, straight away.
        if let Some((machine, request, from)) = self.resend.pop_front() {
            self.action_from = from;
            return Action::Mutate { machine, request };
        }
        // Words whose image didn't go in, asked about once he's back where no typing is taken:
        // the oldest first, each on its own. Only while they're still kept unsent: sent or thrown
        // away meanwhile, there's no question left.
        if self.enter_can_land_here() && !self.send_without.is_empty() {
            let (draft, draft_images, returned) = (&self.draft, &self.draft_images, &self.returned);
            self.send_without.retain(|w| {
                (*draft == w.draft.words && *draft_images == w.draft.images)
                    || returned
                        .iter()
                        .any(|r| r.words == w.draft.words && r.images == w.draft.images)
            });
            if let Some(n) = self.send_without.first().map(|w| w.n) {
                self.ask_send_without_now(n);
            }
            return Action::None;
        }
        // An account action running holds none of this: card changes go their own way (intent
        // 0445).
        if self.open_after.is_some()
            || self.pending_reopen.is_none()
            || !matches!(
                self.mode,
                Mode::Board
                    | Mode::Peek { .. }
                    | Mode::Backlog { .. }
                    | Mode::Misc { .. }
                    | Mode::Done { .. }
                    | Mode::Card { .. }
            )
            || self.typing_view_search()
        {
            return Action::None;
        }
        // The last action's answer stays readable for a moment: what the waiting one says when it
        // starts would replace it before it was ever drawn.
        if let (Some(answered), Some((_, _, said))) = (self.answered_at, &self.message)
            && *said >= answered
            && said.elapsed() < ANSWER_HOLD
        {
            return Action::None;
        }
        match self
            .pending_reopen
            .take()
            .and_then(|k| self.card(&k).cloned())
        {
            Some(card) => self.reopen_action(&card),
            None => Action::None,
        }
    }

    /// Waits for this card's machine to answer, then jumps in.
    pub fn wait_to_jump(&mut self, card: &Card) {
        self.pending_jump = Some(PendingJump {
            key: card.key.clone(),
            workspace: card.workspace_name(),
            machine: card.machine.clone(),
            stuck: None,
            bare: None,
            resume: self.bare_resume_of(card),
        });
    }

    /// ⏎ into a card or workspace whose herdr session didn't answer its last read: rather than
    /// refuse, Kerdr waits for that session to answer (it's read again every few seconds), and goes
    /// in once it does and still has the workspace; after a minute it says it's still stuck, and
    /// why (intent 0591). Never while you're typing: pending_jump_ready's rules.
    pub fn wait_for_stuck(
        &mut self,
        machine: usize,
        session: &str,
        what: &str,
        key: Option<&str>,
        bare: Option<(String, String, Option<String>)>,
    ) {
        let name = self.machines[machine].name();
        let resume = key
            .and_then(|k| self.card(k))
            .and_then(|c| self.bare_resume_of(c));
        self.pending_jump = Some(PendingJump {
            key: key.unwrap_or_default().to_string(),
            workspace: what.to_string(),
            machine: name.clone(),
            stuck: Some((session.to_string(), Instant::now())),
            bare: bare.map(|(s, id, tab)| (machine, s, id, tab)),
            resume,
        });
        self.say(
            format!(
                "Waiting for {session} on {name} to answer; Kerdr goes into {what} when it does. Esc on the board stops waiting."
            ),
            Tone::Info,
        );
    }

    /// The waiting jump, once its machine (and its stuck session, when it waits for one) answers:
    /// where to go now, as ⏎ would. A card goes through jump_cards, which makes plain ⏎'s choice
    /// for one whose workspace closed meanwhile: reopen it, start it, or move it into herdr
    /// (intent 0591). None while it still waits, or once Kerdr has said why it can't.
    pub fn pending_jump_ready(&mut self) -> Option<Action> {
        let pending = self.pending_jump.clone()?;
        // Only where going in can't take keys meant for something else: never while typing or
        // answering a question (intent 0445).
        if !matches!(
            self.mode,
            Mode::Board
                | Mode::Peek { .. }
                | Mode::Backlog { .. }
                | Mode::Misc { .. }
                | Mode::Card { .. }
        ) || self.typing_view_search()
        {
            return None;
        }
        let machine = match (&pending.bare, self.card(&pending.key)) {
            (Some((mi, ..)), _) => Some(*mi),
            (None, Some(c)) => Some(c.machine_index),
            (None, None) => None,
        };
        if let Some(mi) = machine {
            if !self.machines[mi].conn.is_live() {
                return None;
            }
            if let Some((session, since)) = &pending.stuck
                && let Some(why) = self.session_stuck(mi, session).map(str::to_string)
            {
                if since.elapsed() < STUCK_WAIT {
                    return None;
                }
                self.pending_jump = None;
                self.say(
                    format!(
                        "Kerdr didn't go into {}: {session} on {} still isn't answering after a minute ({why}). R reads it again.",
                        pending.workspace, pending.machine
                    ),
                    Tone::Warn,
                );
                return None;
            }
        }
        self.pending_jump = None;
        if let Some((mi, session, id, tab)) = pending.bare {
            // Where it lands, as it was asked for: its name, and the tab (Codex's review).
            self.pending_go = Some((pending.workspace.clone(), tab));
            return Some(Action::JumpWorkspace {
                machine: self.machines[mi].name(),
                session,
                id,
            });
        }
        // An intent's card, wherever its record lives now and whichever machine's copy shows it:
        // its key is its feed's source and its number, which a move home doesn't change, and ⏎
        // does for it what it does now: goes in, reopens it, or asks the done-work question
        // (intent 0591, K1-04). Never another source's card with the same number: that's other
        // work (Codex's review).
        if self.card(&pending.key).is_some() {
            return Some(Action::Jump(pending.key));
        }
        if !pending.key.starts_with("ws:") {
            self.say(
                format!(
                    "{} closed while {} was away, and its card is no longer on this board, so Kerdr didn't go in.",
                    pending.workspace, pending.machine
                ),
                Tone::Warn,
            );
            return None;
        }
        // A bare workspace whose conversation the board knew: offered to resume, never resumed on
        // its own, since it may have closed because its agent wrapped (intent 0591, K1-03).
        let said = format!(
            "{} closed while {} was away.",
            pending.workspace, pending.machine
        );
        if let Some(resume) = &pending.resume
            && self.offer_resume(resume, None, &said)
        {
            return None;
        }
        self.say(
            format!(
                "{} closed while {} was away, and nothing of it is left to open.",
                pending.workspace, pending.machine
            ),
            Tone::Warn,
        );
        None
    }

    fn set_tab(&mut self, tab: usize) {
        if tab < self.tabs.len() {
            // On all of Kerdr the row lists owners: picking one goes a level down (intent 0595).
            if let (true, None, Scope::Owner(o)) =
                (self.owner_split(), &self.owner_pick, &self.tabs[tab])
            {
                let o = o.clone();
                return self.pick_owner(Some(o));
            }
            self.tab = tab;
            self.fix_selection();
        }
    }

    fn step_tab(&mut self, dir: i32) {
        let n = self.tabs.len() as i32;
        self.set_tab((self.tab as i32 + dir).rem_euclid(n) as usize);
    }

    fn step_machine(&mut self, dir: i32) {
        if self.owner_split() {
            return self.step_owner(dir);
        }
        let choices: Vec<Option<String>> = std::iter::once(None)
            .chain(
                self.machines
                    .iter()
                    .filter(|m| m.is_shown())
                    .map(|m| Some(m.name())),
            )
            .collect();
        let pos = choices.iter().position(|m| m == &self.machine).unwrap_or(0);
        let next = (pos as i32 + dir).rem_euclid(choices.len() as i32) as usize;
        self.machine = choices[next].clone();
        self.rebuild();
        self.set_tab(0);
    }

    fn choose_scope(&mut self, scope: Scope) {
        if self.owner_split() {
            // Machines left the top bar (intent 0595): a machine is f's Machine row, and a
            // session is its owner's work.
            return match scope {
                Scope::Machine(m) => {
                    self.machine_filter = Some(m);
                    self.fix_selection();
                }
                Scope::Session(m, s) => {
                    let owner = self
                        .board
                        .sessions
                        .iter()
                        .find(|r| r.machine == m && r.session == s)
                        .map(|r| r.owner.clone())
                        .unwrap_or_else(|| {
                            let listed = self
                                .machine_index(&m)
                                .and_then(|mi| self.machines[mi].feed.as_ref())
                                .map_or(&[][..], |f| &f.owners[..]);
                            crate::owners::session_owner(&self.board.owners, listed, &s)
                        });
                    self.choose_view(if owner.is_empty() {
                        Scope::All
                    } else {
                        Scope::Owner(owner)
                    })
                }
                other => self.choose_view(other),
            };
        }
        self.machine = match &scope {
            Scope::All | Scope::Owner(_) | Scope::Team(..) => None,
            Scope::Machine(m) | Scope::Session(m, _) => Some(m.clone()),
        };
        self.rebuild();
        if let Some(tab) = self.tabs.iter().position(|t| *t == scope) {
            self.set_tab(tab);
        }
    }

    fn session_tab(&mut self, n: usize) {
        let idx = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t, Scope::Session(..)))
            .nth(n.saturating_sub(1))
            .map(|(i, _)| i);
        match idx {
            Some(i) => self.set_tab(i),
            None => {
                let count = self
                    .tabs
                    .iter()
                    .filter(|t| matches!(t, Scope::Session(..)))
                    .count();
                self.say(
                    format!("There's no session tab {n}; this view has {count}."),
                    Tone::Info,
                )
            }
        }
    }

    pub fn machine_index(&self, name: &str) -> Option<usize> {
        self.machines.iter().position(|m| m.answers_to(name))
    }

    /// What ⏎ on this card does: go into its workspace, or, with none open, start it, reopen it or
    /// move its conversation into herdr. Every ⏎ that waited (for its machine, a start, the next
    /// card) makes this same choice when its turn comes (intent 0591).
    pub fn jump_action(&mut self, key: String) -> Action {
        let Some(card) = self.card(&key).cloned() else {
            return Action::None;
        };
        if self.still_saving(&card) {
            return Action::None;
        }
        // The question its start asked while you were elsewhere, waiting on it (intent 0591).
        if self.key_opens_waiting(&key, "enter") {
            return Action::None;
        }
        // A start or reopen on its way: into the workspace it opened, once one shows (intent 0445).
        if let Some(action) = self.enter_starting(&card) {
            return action;
        }
        if card.is_open_workspace() {
            if let Some(action) = self.own_tab_unknown(&card) {
                return action;
            }
            // Going into another card now: a reopen still waiting its turn would take you away
            // from it later, so it's dropped.
            if self.pending_reopen.as_ref().is_some_and(|k| *k != key) {
                self.pending_reopen = None;
            }
            return Action::Jump(key);
        }
        // Followed work for it still going (a session starting, a move into herdr): said, and ⏎
        // goes in once it's open (intent 0591).
        if self.follow_holds(&card, "enter").is_some() {
            return Action::None;
        }
        if let Some(action) = self.outside_first(&card) {
            return action;
        }
        // Work under way whose workspace closed, or done work: reopen it rather than leave a dead
        // end. Done work is opened again only after a yes to the question its backend asks.
        if card.reopenable() {
            let action = self.reopen_action(&card);
            return self.by_enter(action);
        }
        if card.dismissed() {
            // ⏎ means starting that card: to a backend that does the step first, asked once,
            // since Won't do was his own decision, and its yes brings it back and starts or
            // reopens it in one request, every usual check still asked (intent 0591, K1-06).
            if self.actions_on() && self.does_steps(&card) {
                return self.ask_to_bring_back(&card);
            }
            self.say(
                format!("{} is in won't do; u brings it back.", card.reference),
                Tone::Info,
            );
            return Action::None;
        }
        // Work nobody has started: ⏎ starts it, as d does, asking past the usual limit (intent
        // 0404), from a peek opened from the backlog screen too (intent 0591, K1-07): the
        // question's own wait keeps a doubled ⏎ from starting it by accident.
        if waiting_work(&card) && self.actions_on() {
            if matches!(&self.mode, Mode::Peek { back, .. } if matches!(**back, Mode::Backlog { .. }))
                && self
                    .peek_opened
                    .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(500))
            {
                return Action::None;
            }
            if self.just_started(&card) {
                return Action::None;
            }
            let action = self.intent_action_on(card, "dispatch", serde_json::json!({}));
            return self.by_enter(action);
        }
        // Backlog work that ran (its state says so) and has no workspace now: the reopen, whose
        // backend resumes its conversation, or starts it when it never ran (intent 0591).
        if self.actions_on()
            && card.detail.is_some()
            && card.stage == Stage::Backlog
            && matches!(card.state.as_deref(), Some("active" | "background"))
        {
            let action = self.reopen_action(&card);
            return self.by_enter(action);
        }
        // What's left can't be started or reopened from here: say why, plainly (intent 0591).
        // A bare workspace gone, its conversation known: offered to resume (intent 0591, K1-08).
        if card.detail.is_none() && self.offer_resume_closed(&card) {
            return Action::None;
        }
        let reference = &card.reference;
        let msg = if card.detail.is_none() {
            format!(
                "{} closed, and nothing recorded its conversation, so there's nothing to reopen.",
                card.workspace_name()
            )
        } else if !self.actions_on() {
            let what = match card.stage {
                Stage::Backlog => "hasn't started",
                Stage::Maintain => "is done",
                _ => "has no herdr workspace open",
            };
            format!(
                "{reference} {what}, and this board has no [actions] command to start or reopen it. m peeks at it."
            )
        } else if card.intake().is_some() {
            format!("{reference} is still being written up; ⏎ goes in once it's under way.")
        } else {
            format!(
                "{reference} has no herdr workspace open, and its record doesn't say it's waiting, under way or done, so Kerdr can't tell what to start. m peeks at it."
            )
        };
        self.say(msg, Tone::Info);
        Action::None
    }

    /// ⏎ on a card in Won't do, to a backend that brings it back first: "Bring #41 back and start
    /// it?", whose yes sends the start (or the reopen, for work that ran) as it is, so its backend
    /// still asks its own questions (intent 0591, K1-06).
    fn ask_to_bring_back(&mut self, card: &Card) -> Action {
        let Some(detail) = card.detail.clone() else {
            return Action::None;
        };
        let ran = card
            .detail
            .as_ref()
            .and_then(|d| d.resume.as_ref())
            .is_some_and(|r| r.conversation.is_some())
            || card.stage != Stage::Backlog
            || matches!(
                card.state.as_deref(),
                Some("active" | "background" | "closed")
            );
        let (machine, request) = if ran {
            match self.reopen_action(card) {
                Action::Mutate { machine, request } => (machine, request),
                other => return other,
            }
        } else {
            let Some(machine) = self.home_machine(card) else {
                return Action::None;
            };
            if !self.can_write(machine) {
                return Action::None;
            }
            (
                machine,
                serde_json::json!({"action": "dispatch", "id": detail.id}),
            )
        };
        // Nothing goes until the yes.
        self.action_from = None;
        let reference = &card.reference;
        let (title, does) = if ran {
            (
                format!("Bring {reference} back and reopen it?"),
                "brings it back from Won't do and reopens it, resuming its conversation",
            )
        } else {
            (
                format!("Bring {reference} back and start it?"),
                "brings it back from Won't do and starts it",
            )
        };
        self.mode = Mode::Confirm {
            machine,
            request,
            title,
            message: format!(
                "{reference} is in Won't do. Yes {does}, with every usual check: a full session or its own questions still ask first. u only brings it back."
            ),
            back: Box::new(self.mode.clone()),
            target: Some(Box::new(CardSnapshot::of(card))),
            queue: false,
            ready_at: Some(Instant::now() + std::time::Duration::from_millis(500)),
            plain: true,
            choices: vec![],
            enter_queues: false,
        };
        self.confirm_by_enter = Some(detail.id);
        Action::None
    }

    /// ⏎ on open work whose conversation runs in a terminal outside herdr: it goes on to the reopen,
    /// whose backend moves that conversation into herdr (stopping the outside copy once its turn
    /// ends, then resuming it in a workspace of its home session) and answers with `follow`, so
    /// Kerdr takes you in once it runs there (intent 0591, decision 2: without asking first). This
    /// says what's happening meanwhile. An older backend still refuses, or asks before a second
    /// copy, and that shows as it always has. Never anything to send by itself: None.
    fn outside_first(&mut self, card: &Card) -> Option<Action> {
        let outside = card.outside_herdr()?;
        if card.done() || !self.actions_on() {
            return None;
        }
        self.say(
            format!(
                "Moving {}'s conversation from {} into herdr…",
                card.reference,
                outside.place()
            ),
            Tone::Info,
        );
        None
    }

    /// Notes that a start or reopen was ⏎'s: when it's followed, its end takes you in (intent 0591).
    pub(crate) fn by_enter(&mut self, action: Action) -> Action {
        if let Action::Mutate { request, .. } = &action {
            self.enter_sent = request["id"].as_str().map(str::to_string);
        }
        action
    }

    /// A time for a sentence: "13:02" today, "yesterday 13:02", else "Mon 2 Nov 13:02".
    pub(super) fn when(&self, t: DateTime<Local>) -> String {
        let today = self.now().date_naive();
        let clock = t.format("%H:%M");
        if t.date_naive() == today {
            clock.to_string()
        } else if today.pred_opt() == Some(t.date_naive()) {
            format!("yesterday {clock}")
        } else {
            t.format("%a %-d %b %H:%M").to_string()
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        // Any key after the new-intent box closed is moving on (the Enter that closes it sets it).
        self.closed_on_send = false;
        if self.seen_failure(&key) {
            return Action::None;
        }
        // Esc on the board lets a running action's answer go: it still finishes, but neither asks
        // nor takes you anywhere, and a reopen or ⏎ waiting its turn is dropped. Anywhere else Esc
        // has its own job (no to a question, back from a peek), so waiting work keeps waiting,
        // except a remembered ⏎: Kerdr said Esc stops that wherever he pressed it, so it does,
        // and then the key does its usual job too (intent 0491).
        let mut stopped = vec![];
        let on_board = matches!(self.mode, Mode::Board);
        let stops_enter = self.enter_when_open.is_some() && self.enter_can_land_here();
        if key.code == KeyCode::Esc && (on_board || stops_enter) {
            let let_go = self.resolver.let_go() | self.let_go_follows();
            let was_waiting = self.enter_when_open.is_some();
            self.forget_enter();
            if (self.busy && self.action_from.take().is_some()) || let_go || was_waiting {
                stopped.push(
                    "Stopped waiting: it still finishes, and won't take you anywhere.".to_string(),
                );
            }
        }
        if key.code == KeyCode::Esc && on_board {
            let reopen = self.pending_reopen.take().map(|k| (k, "reopen"));
            let written: Vec<(String, &str)> = self
                .resolver
                .drop_awaited()
                .into_iter()
                .map(|k| (k, "start"))
                .collect();
            for (k, verb) in reopen.into_iter().chain(written) {
                let reference = self
                    .card(&k)
                    .map(|c| c.reference.clone())
                    .unwrap_or_else(|| "The waiting card".to_string());
                stopped.push(format!("{reference} won't {verb} after all."));
            }
        }
        let action = self.handle_key(key);
        // A list view's search belongs to it: gone once you leave the view.
        if list_view(&self.mode).is_none() {
            self.view_search = None;
        }
        if !stopped.is_empty() {
            self.say(stopped.join(" "), Tone::Info);
        }
        action
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(
                self.mode,
                Mode::Accounts {
                    screen: AccountScreen::Signin(_) | AccountScreen::PasteToken { .. },
                    ..
                }
            ) {
                return self
                    .account_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                    .unwrap_or(Action::None);
            }
            return Action::Quit;
        }
        if let Some(action) = self.view_search_key(key) {
            return action;
        }
        if let Some(action) = self.account_key(key) {
            return action;
        }
        if let Some(action) = self.control_key(key) {
            return action;
        }
        match self.mode.clone() {
            Mode::Board => self.board_key(key),
            Mode::Peek { scroll, back } => self.peek_key(key, scroll, back),
            Mode::Search(s) => self.search_key(key, s),
            Mode::Keys {
                filter,
                typing,
                scroll,
            } => self.keys_key(key, filter, typing, scroll),
            Mode::Machines { sel } => self.machines_key(key, sel),
            Mode::Backlog { sel } => self.backlog_key(key, sel),
            Mode::Misc { sel } | Mode::Done { sel } => self.misc_key(key, sel),
            Mode::Card {
                card,
                focus,
                opened,
                back,
            } => self.card_key(key, card, focus, opened, back),
            Mode::Accounts { .. }
            | Mode::Palette { .. }
            | Mode::Compose { .. }
            | Mode::Filter { .. }
            | Mode::Kickoff { .. }
            | Mode::Confirm { .. } => Action::None,
        }
    }

    fn board_key(&mut self, key: KeyEvent) -> Action {
        let ca = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('j' | 'J') if ca => {
                if shift || key.code == KeyCode::Char('J') {
                    self.move_row(1);
                } else {
                    self.step_needs_you(1);
                }
            }
            KeyCode::Char('k' | 'K') if ca => {
                if shift || key.code == KeyCode::Char('K') {
                    self.move_row(-1);
                } else {
                    self.step_needs_you(-1);
                }
            }
            KeyCode::Char('h' | 'H') if ca => {
                if shift || key.code == KeyCode::Char('H') {
                    self.step_machine(-1)
                } else {
                    self.step_row(-1, true)
                }
            }
            KeyCode::Char('l' | 'L') if ca => {
                if shift || key.code == KeyCode::Char('L') {
                    self.step_machine(1)
                } else {
                    self.step_row(1, true)
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_col(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_col(1),
            KeyCode::Char('j') | KeyCode::Down => self.move_row(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_row(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select_index(self.sel.col, 0),
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.columns().get(self.sel.col).map_or(0, Vec::len);
                if len > 0 {
                    self.select_index(self.sel.col, len - 1);
                }
            }
            KeyCode::Enter => {
                if let Some(k) = self.sel.key.clone() {
                    return self.jump_action(k);
                }
            }
            KeyCode::Char('m') | KeyCode::Char(' ') => {
                if self.sel.key.is_some() {
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Board),
                    };
                }
            }
            KeyCode::Char('n') => {
                self.step_needs_you(1);
            }
            KeyCode::Char('N') => {
                self.step_needs_you(-1);
            }
            KeyCode::Char('o') => {
                if let Some(k) = self.sel.key.clone() {
                    self.open_inside(&k);
                }
            }
            KeyCode::Char(']') => self.step_row(1, true),
            KeyCode::Char('[') => self.step_row(-1, true),
            KeyCode::Tab => self.step_row(1, false),
            KeyCode::BackTab => self.step_row(-1, false),
            KeyCode::Char('}') => self.step_machine(1),
            KeyCode::Char('{') => self.step_machine(-1),
            KeyCode::Char('0') => self.choose_scope(Scope::All),
            KeyCode::Char(c @ '1'..='9') => self.row_tab(c as usize - '0' as usize),
            KeyCode::Char('/') => {
                self.mode = Mode::Search(Search {
                    query: String::new(),
                    filter: SearchFilter::Everything,
                    sel: 0,
                })
            }
            KeyCode::Char('b') => self.enter_backlog(false),
            KeyCode::Char('D') => self.enter_done(),
            KeyCode::Char('M') => self.open_machines(),
            KeyCode::Char('r') => {
                let target = self
                    .selected()
                    .map(|c| c.machine_index)
                    .filter(|&i| !self.machines[i].conn.is_live());
                return Action::Retry(target);
            }
            KeyCode::Char('R') => return Action::Retry(None),
            KeyCode::Char('?') => {
                self.mode = Mode::Keys {
                    filter: String::new(),
                    typing: false,
                    scroll: 0,
                }
            }
            KeyCode::Esc => {
                // A failure goes only by being seen (seen_failure), never with the rest.
                if !self.failing() {
                    self.message = None;
                }
                if self.pending_jump.take().is_some() {
                    self.say("Stopped waiting.", Tone::Info);
                }
                // s waiting for accounts, or for a card to be brought back or its backend brought
                // up to date: what's under way finishes, and nothing starts after it (intent 0591).
                if self.accounts_wait.take().is_some() {
                    self.say("Stopped waiting for the accounts.", Tone::Info);
                }
                let dropped: Vec<String> = std::mem::take(&mut self.owed_accounts)
                    .into_iter()
                    .map(|owed| {
                        let reference = self
                            .card(&owed.key)
                            .map(|c| c.reference.clone())
                            .unwrap_or_else(|| "The card".into());
                        format!("{reference} won't start on {} after all.", owed.to)
                    })
                    .collect();
                if !dropped.is_empty() {
                    self.say(dropped.join(" "), Tone::Info);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn peek_key(&mut self, key: KeyEvent, scroll: u16, back: Box<Mode>) -> Action {
        let ca = key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') | KeyCode::Char(' ') => {
                self.mode = *back;
                if misc_selection(&self.mode).is_some() {
                    self.fix_selection();
                }
            }
            KeyCode::Enter => {
                if let Some(k) = self.sel.key.clone() {
                    let action = self.jump_action(k);
                    // A start stays in the peek, still on its card, so a question past the limit
                    // asks there (a search behind it would let the question go).
                    let starts = matches!(&action, Action::Mutate { request, .. } if request["action"] == "dispatch");
                    if action != Action::None && !starts {
                        self.mode = *back;
                    }
                    return action;
                }
            }
            KeyCode::Char('j' | 'k') if ca => {
                self.peek_step(
                    if key.code == KeyCode::Char('j') {
                        1
                    } else {
                        -1
                    },
                    back,
                );
            }
            KeyCode::Char('n' | 'N') => {
                self.peek_step(
                    if key.code == KeyCode::Char('n') {
                        1
                    } else {
                        -1
                    },
                    back,
                );
            }
            KeyCode::Char('o') => {
                if let Some(k) = self.sel.key.clone() {
                    self.mode = *back;
                    self.open_inside(&k);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Peek {
                    scroll: scroll.saturating_add(1),
                    back,
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Peek {
                    scroll: scroll.saturating_sub(1),
                    back,
                }
            }
            _ => {}
        }
        Action::None
    }

    /// n in a peek: the next that needs you, peeked at from where it shows, a piece from its
    /// card's screen.
    fn peek_step(&mut self, dir: i32, back: Box<Mode>) {
        self.mode = *back;
        self.step_needs_you(dir);
        self.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(self.mode.clone()),
        };
    }

    fn search_key(&mut self, key: KeyEvent, mut s: Search) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let hits_len = self.search_hits(&s).len();
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Board;
                return Action::None;
            }
            KeyCode::Enter => {
                let hits = self.search_hits(&s);
                if let Some((hit, _)) = hits.get(s.sel) {
                    return self.open_hit(*hit, s);
                }
                return Action::None;
            }
            KeyCode::Down => s.sel = (s.sel + 1).min(hits_len.saturating_sub(1)),
            KeyCode::Up => s.sel = s.sel.saturating_sub(1),
            KeyCode::Char('n' | 'j') if ctrl => s.sel = (s.sel + 1).min(hits_len.saturating_sub(1)),
            KeyCode::Char('p' | 'k') if ctrl => s.sel = s.sel.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                s.query.clear();
                s.sel = 0;
            }
            KeyCode::Char('o') if ctrl => {
                let hits = self.search_hits(&s);
                if let Some((Hit::Card(i), _)) = hits.get(s.sel) {
                    let card = &self.board.cards[*i];
                    self.sel = Selection {
                        col: self.col_of(card),
                        key: Some(card.key.clone()),
                    };
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Search(s)),
                    };
                    return Action::None;
                }
            }
            KeyCode::Tab => {
                let i = SearchFilter::ALL
                    .iter()
                    .position(|f| *f == s.filter)
                    .unwrap_or(0);
                s.filter = SearchFilter::ALL[(i + 1) % SearchFilter::ALL.len()];
                s.sel = 0;
            }
            KeyCode::BackTab => {
                let i = SearchFilter::ALL
                    .iter()
                    .position(|f| *f == s.filter)
                    .unwrap_or(0);
                s.filter =
                    SearchFilter::ALL[(i + SearchFilter::ALL.len() - 1) % SearchFilter::ALL.len()];
                s.sel = 0;
            }
            KeyCode::Backspace => {
                s.query.pop();
                s.sel = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                s.query.push(c);
                s.sel = 0;
            }
            _ => {}
        }
        self.mode = Mode::Search(s);
        Action::None
    }

    fn open_hit(&mut self, hit: Hit, s: Search) -> Action {
        match hit {
            Hit::Card(i) => {
                let card = self.board.cards[i].clone();
                self.sel = Selection {
                    col: self.col_of(&card),
                    key: Some(card.key.clone()),
                };
                if card.intake() == Some(crate::resolve::SAVING) {
                    self.mode = Mode::Board;
                    self.still_saving(&card);
                    return Action::None;
                }
                if let Some(action) = self.enter_starting(&card) {
                    self.mode = Mode::Board;
                    return action;
                }
                if card.is_open_workspace() {
                    self.mode = Mode::Board;
                    // As ⏎ on the board: never onto a child's tab when its own is unknown.
                    if let Some(action) = self.own_tab_unknown(&card) {
                        return action;
                    }
                    Action::Jump(card.key)
                } else if card.outside_herdr().is_some()
                    && !card.done()
                    && self.actions_on()
                    && let Some(action) = {
                        self.mode = Mode::Board;
                        self.outside_first(&card)
                    }
                {
                    action
                } else if card.can_reopen() || (card.can_reopen_done() && self.actions_on()) {
                    self.mode = Mode::Board;
                    let action = self.reopen_action(&card);
                    self.by_enter(action)
                } else if waiting_work(&card) && self.actions_on() {
                    // From a peek on it, which keeps it selected wherever its card sits (inside
                    // another card, a hidden column), so a question past the limit still asks.
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Board),
                    };
                    // As ⏎ on the board: once.
                    if self.just_started(&card) {
                        return Action::None;
                    }
                    let action = self.intent_action_on(card, "dispatch", serde_json::json!({}));
                    self.by_enter(action)
                } else {
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Search(s)),
                    };
                    Action::None
                }
            }
            Hit::Workspace(i) => {
                let w = self.board.workspaces[i].clone();
                self.mode = Mode::Board;
                if let Some(key) = w.card.clone()
                    && self.card(&key).is_some_and(|c| c.is_open_workspace())
                {
                    return Action::Jump(key);
                }
                Action::JumpWorkspace {
                    machine: w.machine,
                    session: w.session,
                    id: w.id,
                }
            }
            Hit::Session(i) => {
                let r = &self.board.sessions[i];
                let scope = Scope::Session(r.machine.clone(), r.session.clone());
                self.choose_scope(scope);
                self.mode = Mode::Board;
                Action::None
            }
        }
    }

    /// Search across cards, workspaces and sessions on every machine, best matches first.
    pub fn search_hits(&self, s: &Search) -> Vec<(Hit, i64)> {
        let q = s.query.trim().to_lowercase();
        let mut hits: Vec<(Hit, i64)> = Vec::new();
        let want = |f: SearchFilter| s.filter == SearchFilter::Everything || s.filter == f;
        if want(SearchFilter::Cards) || s.filter == SearchFilter::DoneToo {
            for (i, c) in self.board.cards.iter().enumerate() {
                if !self.card_visible(c) {
                    continue;
                }
                let done = c.attention == Attention::Done;
                if done && s.filter != SearchFilter::DoneToo && q.is_empty() {
                    continue;
                }
                if let Some(score) = card_score(&q, c) {
                    hits.push((Hit::Card(i), score));
                }
            }
        }
        if want(SearchFilter::Workspaces) {
            for (i, w) in self.board.workspaces.iter().enumerate() {
                if q.is_empty() && s.filter == SearchFilter::Everything {
                    continue;
                }
                let in_scope = match self.scope() {
                    Scope::All => true,
                    Scope::Machine(m) => &w.machine == m,
                    Scope::Session(m, s) => &w.machine == m && &w.session == s,
                    // A workspace is its card's owner's, else its session's (intent 0595).
                    Scope::Owner(_) | Scope::Team(..) => {
                        let mine = |c: &Card| {
                            c.machine == w.machine
                                && c.session.as_deref() == Some(w.session.as_str())
                                && c.workspace_id.as_deref() == Some(w.id.as_str())
                        };
                        match w.card.as_deref().and_then(|k| self.card(k)) {
                            Some(c) => self.scope().contains(c),
                            None => {
                                self.board
                                    .cards
                                    .iter()
                                    .any(|c| mine(c) && self.scope().contains(c))
                                    || (matches!(
                                        self.scope(),
                                        Scope::Owner(_) | Scope::Team(_, None)
                                    ) && self.scope().owner()
                                        == Some(
                                            self.board
                                                .sessions
                                                .iter()
                                                .find(|r| {
                                                    r.machine == w.machine && r.session == w.session
                                                })
                                                .map(|r| r.owner.clone())
                                                .unwrap_or_default()
                                                .as_str(),
                                        ))
                            }
                        }
                    }
                };
                if !in_scope {
                    continue;
                }
                // Shown when any card on it is: its own, or one for work in one of its tabs.
                if self.filtering()
                    && !self.board.cards.iter().any(|c| {
                        c.machine == w.machine
                            && c.session.as_deref() == Some(w.session.as_str())
                            && c.workspace_id.as_deref() == Some(w.id.as_str())
                            && self.card_visible(c)
                    })
                {
                    continue;
                }
                let hay = format!("{} {} {} {}", w.label, w.session, w.machine, w.id);
                if let Some(score) = score(&q, &hay) {
                    hits.push((Hit::Workspace(i), 1000 + score));
                }
            }
        }
        if want(SearchFilter::Sessions) {
            for (i, r) in self.board.sessions.iter().enumerate() {
                if q.is_empty() && s.filter == SearchFilter::Everything {
                    continue;
                }
                if !self.scope().contains_session(r) {
                    continue;
                }
                if self.filtering()
                    && !self.board.cards.iter().any(|c| {
                        c.machine == r.machine
                            && c.session.as_deref() == Some(r.session.as_str())
                            && self.card_visible(c)
                    })
                {
                    continue;
                }
                let hay = format!("{} {}", r.session, r.machine);
                if let Some(score) = score(&q, &hay) {
                    hits.push((Hit::Session(i), 2000 + score));
                }
            }
        }
        hits.sort_by_key(|(h, score)| {
            let group = match h {
                Hit::Card(_) => 0,
                Hit::Workspace(_) => 1,
                Hit::Session(_) => 2,
            };
            (group, *score)
        });
        hits
    }

    fn keys_key(&mut self, key: KeyEvent, mut filter: String, typing: bool, scroll: u16) -> Action {
        if typing {
            match key.code {
                KeyCode::Esc => filter.clear(),
                KeyCode::Enter => {}
                KeyCode::Backspace => {
                    filter.pop();
                }
                KeyCode::Char(c) => {
                    filter.push(c);
                    self.mode = Mode::Keys {
                        filter,
                        typing: true,
                        scroll: 0,
                    };
                    return Action::None;
                }
                _ => {}
            }
            let keep = key.code == KeyCode::Backspace;
            self.mode = Mode::Keys {
                filter,
                typing: keep,
                scroll: 0,
            };
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.mode = Mode::Board,
            KeyCode::Char('/') => {
                self.mode = Mode::Keys {
                    filter,
                    typing: true,
                    scroll: 0,
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Keys {
                    filter,
                    typing: false,
                    scroll: scroll.saturating_add(1),
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Keys {
                    filter,
                    typing: false,
                    scroll: scroll.saturating_sub(1),
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Every machine, in the order Kerdr reads them: this one first, hidden ones included.
    pub fn machine_rows(&self) -> Vec<usize> {
        (0..self.machines.len()).collect()
    }

    /// The machines the banner speaks of, the one in the most trouble first: one that needs you
    /// (SSH asks something), then one dropping or away, then one whose herdr needs setting up. A
    /// drop counts only once it lasts; its tab is marked meanwhile (intent 0591).
    pub fn troubled_machines(&self) -> Vec<usize> {
        let now = self.now();
        let rank = |c: &Conn| match c {
            Conn::HostKey { .. } | Conn::Auth { .. } => 0,
            Conn::Reconnecting { .. } => 1,
            Conn::Unreachable { .. } => 2,
            Conn::HerdrTooOld { .. } | Conn::HerdrMissing => 3,
            _ => 9,
        };
        let mut troubled: Vec<usize> = (0..self.machines.len())
            .filter(|&i| rank(&self.machines[i].conn) < 9 && self.machines[i].conn.lasting(now))
            .collect();
        troubled.sort_by_key(|&i| rank(&self.machines[i].conn));
        troubled
    }

    /// M: the machines screen, on the machine the banner names, so its ⏎ or u is the next key
    /// (intent 0591, K3-38 to K3-41).
    pub(crate) fn open_machines(&mut self) {
        let sel = self
            .troubled_machines()
            .first()
            .and_then(|&i| self.machine_rows().iter().position(|&r| r == i))
            .unwrap_or(0);
        self.mode = Mode::Machines { sel };
    }

    fn machines_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        let rows = self.machine_rows();
        let n = rows.len().max(1);
        let i = rows.get(sel.min(n - 1)).copied().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => self.mode = Mode::Board,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('}') => {
                self.mode = Mode::Machines {
                    sel: (sel + 1).min(n - 1),
                }
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('{') => {
                self.mode = Mode::Machines {
                    sel: sel.saturating_sub(1),
                }
            }
            KeyCode::Char('r') => return Action::Retry(Some(i)),
            KeyCode::Char('R') => return Action::Retry(None),
            // This machine too: only its cards leave the board, and it's still read for its
            // accounts and the actions it runs (intent 0591, K1-10).
            KeyCode::Char('x') => return Action::ToggleHidden(i),
            KeyCode::Char('u') => {
                if self.machines[i].machine.is_local() {
                    self.say("Update herdr here with: herdr update", Tone::Info);
                } else {
                    return Action::Setup(i);
                }
            }
            KeyCode::Enter => {
                let m = &self.machines[i];
                match &m.conn {
                    Conn::HostKey { .. } | Conn::Auth { .. } => return Action::Answer(i),
                    Conn::HerdrMissing | Conn::HerdrTooOld { .. } if !m.machine.is_local() => {
                        return Action::Setup(i);
                    }
                    Conn::Hidden => return Action::ToggleHidden(i),
                    _ if m.machine.hidden => return Action::ToggleHidden(i),
                    _ => {
                        let scope = Scope::Machine(m.name());
                        self.choose_scope(scope);
                        self.mode = Mode::Board;
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    /// The backlog in the order its view lists it, narrowed by the view's search.
    pub fn backlog_cards(&self) -> Vec<usize> {
        self.backlog_order()
            .into_iter()
            .filter(|&i| self.matches_view_search(&self.board.cards[i]))
            .collect()
    }

    /// Whether a card is in a list view's search: every card when there's none, or it's empty.
    /// Only a list view's search narrows, so another screen never loses cards to one left over.
    fn matches_view_search(&self, c: &Card) -> bool {
        let Some(s) = self
            .view_search
            .as_ref()
            .filter(|_| list_view(&self.mode).is_some())
        else {
            return true;
        };
        let q = s.query.trim().to_lowercase();
        q.is_empty() || card_score(&q, c).is_some()
    }

    /// The whole backlog in its order, whatever the view's search: a card's place in it is the
    /// number the view shows beside it.
    pub fn backlog_order(&self) -> Vec<usize> {
        let scope = self.scope();
        self.board
            .cards
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                (c.stage == Stage::Backlog || self.wont_do)
                    && scope.contains(c)
                    && self.card_visible(c)
                    && self.holder(*i).is_none()
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// `/` in the Backlog, Won't do or Misc view searches only that view (intent 0402); on the
    /// board it still searches everything. While typing, keys go into the query, except the
    /// ones that move, Enter, which does what Enter does in the view on the card selected, and
    /// Esc, which clears the search. With a search left in place, letters are the view's keys
    /// again, on the cards it shows, `/` goes back to typing and Esc clears it.
    fn view_search_key(&mut self, key: KeyEvent) -> Option<Action> {
        let sel = match self.mode {
            Mode::Backlog { sel } | Mode::Misc { sel } | Mode::Done { sel } => sel,
            _ => return None,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let slash = key.code == KeyCode::Char('/') && !ctrl && !alt;
        let Some(mut s) = self.view_search.clone() else {
            if !slash {
                return None;
            }
            self.count.clear();
            self.view_search = Some(ViewSearch {
                query: String::new(),
                typing: true,
            });
            return Some(Action::None);
        };
        if !s.typing {
            match key.code {
                _ if slash => {
                    self.count.clear();
                    s.typing = true;
                    self.view_search = Some(s);
                }
                KeyCode::Esc => {
                    self.view_search = None;
                    self.settle_view_selection(true);
                }
                _ => return None,
            }
            return Some(Action::None);
        }
        let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
        match key.code {
            KeyCode::Esc => {
                self.view_search = None;
                self.settle_view_selection(true);
            }
            KeyCode::Enter => {
                s.typing = false;
                self.view_search = (!s.query.trim().is_empty()).then_some(s);
                return Some(self.list_view_key(key, sel));
            }
            KeyCode::Down => return Some(self.list_view_key(key, sel)),
            KeyCode::Up => return Some(self.list_view_key(key, sel)),
            KeyCode::Char('n' | 'j') if ctrl => {
                return Some(self.list_view_key(plain(KeyCode::Down), sel));
            }
            KeyCode::Char('p' | 'k') if ctrl => {
                return Some(self.list_view_key(plain(KeyCode::Up), sel));
            }
            KeyCode::Char('u') if ctrl => {
                s.query.clear();
                self.view_search = Some(s);
                self.settle_view_selection(false);
            }
            KeyCode::Backspace if s.query.is_empty() => {
                self.view_search = None;
                self.settle_view_selection(true);
            }
            KeyCode::Backspace => {
                s.query.pop();
                self.view_search = Some(s);
                self.settle_view_selection(false);
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                if s.query.chars().count() < 64 {
                    s.query.push(c);
                }
                self.view_search = Some(s);
                self.settle_view_selection(false);
            }
            _ => {}
        }
        Some(Action::None)
    }

    fn list_view_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        match self.mode {
            Mode::Misc { .. } | Mode::Done { .. } => self.misc_key(key, sel),
            _ => self.backlog_key(key, sel),
        }
    }

    /// After a list view's search changes: the card selected before when `keep` and it's still
    /// listed, else the first card listed.
    fn settle_view_selection(&mut self, keep: bool) {
        match self.mode {
            Mode::Backlog { .. } => {
                let items = self.backlog_cards();
                let sel = keep
                    .then(|| {
                        let key = self.backlog_focus.as_ref()?;
                        items.iter().position(|&i| &self.board.cards[i].key == key)
                    })
                    .flatten()
                    .unwrap_or(0);
                self.backlog_focus = items.get(sel).map(|&i| self.board.cards[i].key.clone());
                self.mode = Mode::Backlog { sel };
            }
            Mode::Misc { .. } | Mode::Done { .. } => {
                let items = self.list_cards();
                let sel = keep
                    .then(|| {
                        let key = self.sel.key.as_ref()?;
                        items.iter().position(|&i| &self.board.cards[i].key == key)
                    })
                    .flatten()
                    .unwrap_or(0);
                self.sel.key = items.get(sel).map(|&i| self.board.cards[i].key.clone());
                set_misc_selection(&mut self.mode, sel);
            }
            _ => {}
        }
    }

    fn backlog_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        // Moving on your own ends the wait to select a just-made intent.
        self.new_intent_id = None;
        let items = self.backlog_cards();
        let n = items.len();
        match key.code {
            // The next that needs you, from the card selected here: it may be in this list (a
            // hidden Backlog column's work, intent 0444) or anywhere on the board.
            KeyCode::Char('n' | 'N') if !self.wont_do => {
                if let Some(&i) = items.get(sel) {
                    self.sel.key = Some(self.board.cards[i].key.clone());
                }
                self.step_needs_you(if key.code == KeyCode::Char('n') {
                    1
                } else {
                    -1
                });
                return Action::None;
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('b') => self.mode = Mode::Board,
            KeyCode::Char('j') | KeyCode::Down => {
                self.mode = Mode::Backlog {
                    sel: (sel + 1).min(n.saturating_sub(1)),
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.mode = Mode::Backlog {
                    sel: sel.saturating_sub(1),
                }
            }
            KeyCode::Char('o') => {
                if let Some(&i) = items.get(sel) {
                    let key = self.board.cards[i].key.clone();
                    self.backlog_focus = Some(key.clone());
                    self.open_inside(&key);
                    return Action::None;
                }
            }
            KeyCode::Enter | KeyCode::Char('m') | KeyCode::Char(' ') => {
                if let Some(&i) = items.get(sel) {
                    let key = self.board.cards[i].key.clone();
                    self.backlog_focus = Some(key.clone());
                    self.sel = Selection {
                        col: 0,
                        key: Some(key),
                    };
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(Mode::Backlog { sel }),
                    };
                    self.peek_opened = Some(Instant::now());
                }
            }
            _ => {}
        }
        if let Mode::Backlog { sel } = self.mode {
            self.backlog_focus = items.get(sel).map(|&i| self.board.cards[i].key.clone());
        }
        Action::None
    }

    pub fn misc_cards(&self) -> Vec<usize> {
        self.board
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.stage == Stage::Misc && self.card_visible(c) && self.matches_view_search(c)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn enter_misc(&mut self) {
        if !matches!(self.mode, Mode::Misc { .. }) {
            self.view_search = None;
        }
        self.mode = Mode::Misc { sel: 0 };
        self.sel.col = 0;
        self.sel.key = self
            .misc_cards()
            .first()
            .map(|&i| self.board.cards[i].key.clone());
    }

    /// Finished work, as the Maintain column lists it (a done card with work still open inside
    /// first), narrowed by the view's search: the Done screen's list (intent 0444).
    pub fn done_cards(&self) -> Vec<usize> {
        let mut cols = self.columns();
        cols.pop()
            .unwrap_or_default()
            .into_iter()
            .filter(|&i| self.matches_view_search(&self.board.cards[i]))
            .collect()
    }

    /// The cards of the list view this screen is on or was opened from: Done's or Misc's.
    pub fn list_cards(&self) -> Vec<usize> {
        match list_view(&self.mode) {
            Some(Mode::Done { .. }) => self.done_cards(),
            _ => self.misc_cards(),
        }
    }

    /// D: the Done screen, on its first card. Won't do is left, since done work isn't set aside.
    pub(super) fn enter_done(&mut self) {
        if !matches!(self.mode, Mode::Done { .. }) {
            self.view_search = None;
        }
        self.wont_do = false;
        self.mode = Mode::Done { sel: 0 };
        self.sel.col = self.layout().done();
        self.sel.key = self
            .done_cards()
            .first()
            .map(|&i| self.board.cards[i].key.clone());
    }

    /// Keys in Misc and Done, which work alike: ⏎ goes in (on done work, asks to reopen it),
    /// space peeks, and Done's o opens what's inside a card.
    fn misc_key(&mut self, key: KeyEvent, sel: usize) -> Action {
        let done = matches!(self.mode, Mode::Done { .. });
        let items = self.list_cards();
        let mut selected = sel.min(items.len().saturating_sub(1));
        let here = |sel: usize| {
            if done {
                Mode::Done { sel }
            } else {
                Mode::Misc { sel }
            }
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Board;
                self.fix_selection();
                return Action::None;
            }
            KeyCode::Char('i') if !done => {
                self.mode = Mode::Board;
                self.fix_selection();
                return Action::None;
            }
            KeyCode::Char('D') if done => {
                self.mode = Mode::Board;
                self.fix_selection();
                return Action::None;
            }
            KeyCode::Char('n' | 'N') if done => {
                if let Some(&i) = items.get(selected) {
                    self.sel.key = Some(self.board.cards[i].key.clone());
                }
                self.step_needs_you(if key.code == KeyCode::Char('n') {
                    1
                } else {
                    -1
                });
                return Action::None;
            }
            KeyCode::Char('o') if done => {
                if let Some(&i) = items.get(selected) {
                    let key = self.board.cards[i].key.clone();
                    self.sel.key = Some(key.clone());
                    self.open_inside(&key);
                }
                return Action::None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(items.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => selected = 0,
            KeyCode::Char('G') | KeyCode::End => selected = items.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(&i) = items.get(selected) {
                    return self.jump_action(self.board.cards[i].key.clone());
                }
            }
            KeyCode::Char('m' | ' ') => {
                if !items.is_empty() {
                    self.sel.key = Some(self.board.cards[items[selected]].key.clone());
                    self.mode = Mode::Peek {
                        scroll: 0,
                        back: Box::new(here(selected)),
                    };
                }
                return Action::None;
            }
            KeyCode::Char('[') | KeyCode::BackTab => {
                self.step_tab(-1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char(']') | KeyCode::Tab => {
                self.step_tab(1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char('{') => {
                self.step_machine(-1);
                self.rebuild();
                return Action::None;
            }
            KeyCode::Char('}') => {
                self.step_machine(1);
                self.rebuild();
                return Action::None;
            }
            _ => {}
        }
        self.mode = here(selected);
        self.sel.key = items
            .get(selected)
            .map(|&i| self.board.cards[i].key.clone());
        Action::None
    }
}

/// Ranks a card for a query: matches in its reference or title beat matches in its session or
/// machine, and more urgent cards come first among equals. Lower is better.
fn card_score(q: &str, c: &Card) -> Option<i64> {
    let urgency = c.attention as i64;
    let fields = [
        (0, c.reference.as_str()),
        (1, c.title.as_str()),
        (2, c.workspace_label.as_deref().unwrap_or("")),
        (3, c.why.as_str()),
        (4, c.session.as_deref().unwrap_or("")),
        (4, c.machine.as_str()),
    ];
    fields
        .iter()
        .filter_map(|(rank, text)| {
            score(q, text).map(|s| rank * 100_000 + urgency * 1000 + s.min(999))
        })
        .min()
}

/// Lower is better; None when it doesn't match. Plain substring first, then the letters in order.
pub fn score(query: &str, haystack: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let hay = haystack.to_lowercase();
    if let Some(pos) = hay.find(query) {
        return Some(pos as i64);
    }
    let mut it = hay.chars().enumerate();
    let mut first = None;
    let mut last = 0;
    for qc in query.chars().filter(|c| !c.is_whitespace()) {
        let (i, _) = it.find(|(_, hc)| *hc == qc)?;
        first.get_or_insert(i);
        last = i;
    }
    Some(500 + (last - first.unwrap_or(0)) as i64)
}

/// Where the query matches in a piece of text, for highlighting: (start, end) in chars.
pub fn match_range(query: &str, text: &str) -> Option<(usize, usize)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    let byte = lower.find(&q)?;
    let start = lower[..byte].chars().count();
    Some((start, start + q.chars().count()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_cycles_provider_directly_even_when_current_filter_leaves_board_empty() {
        let mut app = App::new(Config::default(), crate::demo::machines(false));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("claude"));
        app.project = Some("no-matching-project".into());
        assert!(app.columns().iter().all(Vec::is_empty));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("codex"));
        press(&mut app, 'a');
        assert_eq!(app.provider, None);
    }

    #[test]
    fn empty_board_still_switches_machines_and_sessions() {
        let mut app = App::new(Config::machine_split(), crate::demo::machines(false));
        app.project = Some("no-matching-project".into());
        press(&mut app, '}');
        let machine = app.machine.clone().unwrap();
        press(&mut app, ']');
        assert!(matches!(app.scope(), Scope::Session(m, _) if m == &machine));
    }

    /// The demo on the machine split, which these tests were written about; the owner split's
    /// are in `view_controls`.
    pub(crate) fn demo_app() -> App {
        App::new(Config::machine_split(), crate::demo::machines(false))
    }

    pub(crate) fn press(app: &mut App, c: char) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    pub(crate) fn press_code(app: &mut App, code: KeyCode) -> Action {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// A change's answer arriving, as the event loop takes it: through the action resolution tool
    /// when the change went there, else straight to the app.
    fn reply_to(
        app: &mut App,
        machine: usize,
        request: serde_json::Value,
        result: Result<crate::actions::Response, String>,
    ) {
        let action = request["action"].as_str().unwrap_or_default();
        let job = if action == "create" {
            let submission = request["submission"].clone();
            app.resolver
                .unanswered_where(|r| r["action"] == "create" && r["submission"] == submission)
        } else {
            app.resolver.unanswered_for(action, request["id"].as_str())
        };
        match job {
            Some(n) => app.finish_job(n, request, result),
            None => app.finish_action(machine, request, result),
        }
    }

    /// Sends a change as the event loop does: into the action resolution tool, its command
    /// started. What it shows meanwhile is tested on its own; these boards are set by hand.
    fn send(app: &mut App, machine: usize, request: serde_json::Value) {
        let from = app.action_from.take();
        let key = from.as_ref().map(|f| f.key.clone());
        app.resolver.add(machine, request, from, key, None);
        app.resolver.ready();
    }

    /// The last thing said: failures that come together stack, the newest last.
    fn last_said(app: &App) -> Option<(String, Tone)> {
        app.message()
            .map(|(t, tone)| (t.lines().last().unwrap_or_default().to_string(), tone))
    }

    /// A failure on screen has been read: the next key is no longer held back by it.
    fn seen(app: &mut App) {
        if app.failing() {
            app.message = None;
        }
    }

    /// As if the question on screen had been up long enough to be answered.
    fn settle(app: &mut App) {
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
    }

    fn asked_to_refresh(app: &mut App, waiting: Vec<usize>) {
        let said = "Reading every machine now.".to_string();
        app.say(said.clone(), Tone::Info);
        app.refresh = Some(Refresh {
            since: Instant::now(),
            waiting,
            missed: vec![],
            said,
            away: app.away,
        });
    }

    #[test]
    fn r_with_a_machine_down_never_calls_it_up_to_date() {
        // Down when he pressed it, not only one that drops after (intent 0491).
        let mut app = demo_app();
        app.machines.truncate(2);
        app.machines[0].conn = Conn::Live {
            latency: Duration::from_millis(1),
        };
        app.machines[0].machine.feed = Some("a-feed".into());
        let down = app.machines[1].name();
        app.machines[1].conn = reconnecting();
        let (reading, trying) = app.ask_refresh(Instant::now());
        assert_eq!((reading, trying), (vec![0], vec![1]));
        assert_eq!(
            app.message().unwrap().0,
            "Reading every machine now, and trying 1 again."
        );
        app.settle_refresh(&[0]);
        // Which machine, and why in a few words (intent 0591).
        let said = format!("Up to date, except {down} (no answer, still trying).");
        assert_eq!(app.message(), Some((said.as_str(), Tone::Info)));
        // Its retry lands by itself: that's said, once.
        app.machines[1].conn = Conn::Live {
            latency: Duration::from_millis(1),
        };
        app.apply(Event {
            machine: 1,
            conn: Conn::Live {
                latency: Duration::from_millis(1),
            },
            data: Some(crate::poll::MachineData {
                feed: Some(Ok(crate::feed::Feed::default())),
                ..Default::default()
            }),
            at: Local::now(),
        });
        assert_eq!(app.message(), Some(("Now up to date.", Tone::Good)));
        assert!(app.refresh_behind.is_none());
    }

    #[test]
    fn r_with_every_machine_down_says_only_that_it_is_trying() {
        let mut app = demo_app();
        for m in &mut app.machines {
            m.conn = reconnecting();
        }
        let n = app.machines.len();
        let (reading, trying) = app.ask_refresh(Instant::now());
        assert!(reading.is_empty());
        assert_eq!(trying.len(), n);
        assert!(app.refresh.is_none(), "nothing to wait for");
        let said = format!("Reading every machine now, and trying {n} again.");
        app.settle_refresh(&[]);
        assert_eq!(app.message().unwrap().0, said);
    }

    #[test]
    fn r_says_nothing_about_a_read_that_lands_while_he_is_in_a_card() {
        // The first turn back: nothing says up to date when the read asked on return hasn't come.
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0]);
        app.away += 1;
        let inside = "Opening it…";
        app.say(inside, Tone::Info);
        app.unsay(inside);
        app.settle_refresh(&[0]);
        assert!(app.refresh.is_none());
        assert_eq!(app.message(), None);
    }

    fn reconnecting() -> Conn {
        Conn::Reconnecting {
            since: Local::now(),
            attempt: 1,
            retry_at: Local::now(),
            detail: "no answer".into(),
        }
    }

    #[test]
    fn r_says_up_to_date_once_every_machine_it_asked_has_read_its_feed() {
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0, 1]);
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        app.settle_refresh(&[]);
        assert!(app.refresh.is_some(), "still reading both");
        app.settle_refresh(&[0]);
        assert_eq!(app.refresh.as_ref().unwrap().waiting, [1]);
        assert_eq!(app.message().unwrap().0, "Reading every machine now.");
        app.settle_refresh(&[1]);
        assert!(app.refresh.is_none());
        assert_eq!(app.message(), Some(("Up to date.", Tone::Good)));
    }

    #[test]
    fn r_reads_the_inbox_for_feeds_that_began_after_it() {
        let mut app = demo_app();
        let inbox = crate::poll::Inbox::default();
        let event = |feed: bool| Event {
            machine: 0,
            conn: Conn::Live {
                latency: Duration::from_millis(1),
            },
            data: Some(crate::poll::MachineData {
                feed: feed.then(|| Ok(crate::feed::Feed::default())),
                ..Default::default()
            }),
            at: Local::now(),
        };
        asked_to_refresh(&mut app, vec![0]);
        let since = app.refresh.as_ref().unwrap().since;
        inbox.put(event(true), since - Duration::from_millis(1), 0);
        assert!(app.refreshed(&inbox).is_empty(), "began before R");
        inbox.put(event(false), since, 0);
        assert!(app.refreshed(&inbox).is_empty(), "no feed in it");
        inbox.put(event(true), since, 0);
        assert_eq!(app.refreshed(&inbox), [0]);
    }

    #[test]
    fn r_says_which_machines_it_could_not_bring_up_to_date() {
        // Stopped answering while it waited, or read but its feed failed: not up to date, and a
        // hidden one isn't worth a word.
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0, 1]);
        let gone = app.machines[1].name();
        app.machines[1].conn = Conn::Connecting;
        app.settle_refresh(&[]);
        assert_eq!(app.refresh.as_ref().unwrap().waiting, [0]);
        app.settle_refresh(&[0]);
        let said = format!("Up to date, except {gone}.");
        assert_eq!(app.message(), Some((said.as_str(), Tone::Info)));

        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0, 1]);
        let failed = app.machines[0].name();
        app.machines[0].feed_error = Some("timed out".into());
        app.machines[1].conn = Conn::Hidden;
        app.settle_refresh(&[0]);
        let said = format!("Up to date, except {failed} (its feed failed: timed out).");
        assert_eq!(app.message(), Some((said.as_str(), Tone::Info)));
    }

    #[test]
    fn r_names_a_stuck_session_it_could_not_bring_up_to_date() {
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0]);
        let name = app.machines[0].name();
        app.machines[0]
            .snapshot_errors
            .insert("web".into(), crate::poll::SESSION_SILENT.into());
        app.settle_refresh(&[0]);
        let said = format!("Up to date, except {name}'s web.");
        assert_eq!(app.message(), Some((said.as_str(), Tone::Info)));
    }

    #[test]
    fn r_never_calls_a_list_of_sessions_kept_from_before_up_to_date() {
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0]);
        let name = app.machines[0].name();
        app.machines[0].sessions_failing = Some((
            "herdr printed nothing for its list of sessions".into(),
            chrono::Local::now(),
        ));
        app.settle_refresh(&[0]);
        let said = format!("Up to date, except {name}'s herdr sessions.");
        assert_eq!(app.message(), Some((said.as_str(), Tone::Info)));
    }

    #[test]
    fn r_finishing_never_talks_over_something_said_since() {
        let mut app = demo_app();
        asked_to_refresh(&mut app, vec![0]);
        app.say("Select an intent first.", Tone::Info);
        app.settle_refresh(&[0]);
        assert!(app.refresh.is_none());
        assert_eq!(app.message().unwrap().0, "Select an intent first.");
    }

    #[test]
    fn the_board_turns_faster_only_while_something_is_coming() {
        let mut app = demo_app();
        assert_eq!(app.key_wait(), KEY_WAIT);
        // An image being read.
        app.image_load = Some(1);
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        app.image_load = None;
        // An answer on its way, until it comes.
        let (key, id) = waiting_backlog_card(&app);
        let request = serde_json::json!({"action": "move", "id": id, "delta": 1});
        app.resolver.add(0, request, None, Some(key.clone()), None);
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        let n = app.resolver.on_its_way().next().unwrap().n;
        app.resolver.take(n);
        assert_eq!(app.key_wait(), KEY_WAIT);
        // A start that waits for its write-up, which can take minutes, isn't an answer coming.
        let request = serde_json::json!({"action": "dispatch", "id": id});
        app.resolver.add_gated(0, request, None, Some(key), None);
        assert_eq!(app.key_wait(), KEY_WAIT);
        let n = app.resolver.on_its_way().next().unwrap().n;
        app.resolver.take(n);
        // A failure until a key can take it down, then not.
        app.fail("Not this time");
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        if let Some((_, _, at)) = &mut app.message {
            *at -= FAIL_SEEN;
        }
        assert_eq!(app.key_wait(), KEY_WAIT);
        // A question's guard, until it has passed.
        app.mode = Mode::Confirm {
            machine: 0,
            request: serde_json::json!({}),
            title: "Sure?".into(),
            message: String::new(),
            back: Box::new(Mode::Board),
            target: None,
            queue: false,
            ready_at: Some(Instant::now() + Duration::from_secs(60)),
            plain: false,
            choices: vec![],
            enter_queues: false,
        };
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        settle(&mut app);
        assert_eq!(app.key_wait(), KEY_WAIT);
    }

    #[test]
    fn unlinked_workspace_goes_to_misc_and_enter_still_jumps() {
        let mut app = demo_app();
        let misc = app
            .board
            .cards
            .iter()
            .find(|c| c.source == crate::model::Source::Workspace)
            .unwrap()
            .clone();
        assert_eq!(misc.stage.id(), "misc");
        assert!(
            !app.columns()
                .iter()
                .flatten()
                .any(|&i| app.board.cards[i].key == misc.key)
        );
        press(&mut app, 'i');
        assert!(matches!(app.mode, Mode::Misc { .. }));
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Jump(misc.key)
        );
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert!(
            app.selected()
                .is_none_or(|c| c.source == crate::model::Source::Feed)
        );
    }

    #[test]
    fn misc_moves_to_its_intent_phase_once_and_survives_a_feed_error() {
        let mut app = demo_app();
        let misc = app
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Misc)
            .unwrap()
            .clone();
        let mi = misc.machine_index;
        let session = misc.session.clone().unwrap();
        let record = crate::feed::FeedCard {
            id: "1886".into(),
            title: "Real intent".into(),
            stage: Some("test".into()),
            workspace: Some(crate::feed::FeedWorkspace {
                own_tab: None,
                session: Some(session),
                id: misc.workspace_id.clone(),
                label: None,
                tab: None,
            }),
            ..Default::default()
        };
        app.machines[mi].feed = Some(crate::feed::Feed {
            kerdr_feed: 1,
            cards: vec![record],
            ..Default::default()
        });
        app.rebuild();
        assert!(!app.board.cards.iter().any(|c| c.key == misc.key));
        let claimed: Vec<_> = app
            .board
            .cards
            .iter()
            .filter(|c| {
                c.machine_index == mi
                    && c.session == misc.session
                    && c.workspace_id == misc.workspace_id
            })
            .collect();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].stage, Stage::Test);
        let mut data = app.machines[mi].data.clone().unwrap();
        data.feed = Some(Err("temporary feed failure".into()));
        app.apply(Event {
            machine: mi,
            conn: app.machines[mi].conn.clone(),
            data: Some(data),
            at: Local::now(),
        });
        assert!(
            app.board.cards.iter().any(
                |c| c.detail.as_ref().is_some_and(|d| d.id == "1886") && c.stage == Stage::Test
            )
        );
        assert!(!app.board.cards.iter().any(|c| c.key == misc.key));
    }

    #[test]
    fn misc_menu_and_peek_preserve_selection_across_poll_and_filters() {
        let mut app = demo_app();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'i');
        assert!(matches!(app.mode, Mode::Misc { .. }));
        let original = app.selected().unwrap().clone();
        press(&mut app, 'm');
        app.rebuild();
        assert_eq!(app.selected().unwrap().key, original.key);
        assert!(matches!(app.mode, Mode::Peek { .. }));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Misc { .. }));
        app.provider = Some("no-such-provider".into());
        app.rebuild();
        assert!(app.misc_cards().is_empty());
        assert!(app.selected().is_none());
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn misc_provider_menu_retargets_peek_before_the_next_poll() {
        let mut app = demo_app();
        press(&mut app, 'i');
        let mut other = app.selected().unwrap().clone();
        for a in &mut other.agents {
            a.agent = "claude".into();
        }
        other.key = "misc-other".into();
        other.title = "Other provider".into();
        for c in &mut app.board.cards {
            if c.stage == Stage::Misc {
                for a in &mut c.agents {
                    a.agent = "codex".into();
                }
            }
        }
        app.board.cards.push(other);
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'a');
        assert_eq!(app.provider.as_deref(), Some("claude"));
        assert_eq!(app.selected().unwrap().key, "misc-other");
        press(&mut app, 'm');
        assert_eq!(app.selected().unwrap().key, "misc-other");
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Jump("misc-other".into())
        );
    }

    #[test]
    fn misc_selection_survives_polling_while_composing_an_intent() {
        let mut app = demo_app();
        press(&mut app, 'i');
        let original = app.selected().unwrap().key.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        app.rebuild();
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Misc { .. }));
        assert_eq!(app.selected().unwrap().key, original);
    }

    #[test]
    fn overview_does_not_mix_all_machines_sessions_in_the_tab_bar() {
        let app = demo_app();
        assert_eq!(app.tabs, vec![Scope::All]);
    }

    #[test]
    fn braces_switch_machine_and_brackets_stay_in_its_sessions() {
        let mut app = demo_app();
        let first = app.machines[0].name();
        let second = app.machines[1].name();
        press(&mut app, '}');
        assert_eq!(app.scope(), &Scope::Machine(first.clone()));
        assert!(
            app.tabs
                .iter()
                .all(|t| matches!(t, Scope::Machine(m) | Scope::Session(m, _) if m == &first))
        );
        press(&mut app, ']');
        assert!(matches!(app.scope(), Scope::Session(m, _) if m == &first));
        press(&mut app, '}');
        assert_eq!(app.scope(), &Scope::Machine(second.clone()));
        assert!(
            app.tabs
                .iter()
                .all(|t| matches!(t, Scope::Machine(m) | Scope::Session(m, _) if m == &second))
        );
        press(&mut app, '0');
        assert_eq!(app.scope(), &Scope::All);
    }

    #[test]
    fn tenant_and_project_filters_compose_and_wont_do_has_its_own_view() {
        let mut app = demo_app();
        let key = app.board.cards[0].key.clone();
        let card = &mut app.board.cards[0];
        card.stage = Stage::Backlog;
        let d = card.detail.get_or_insert_with(Default::default);
        d.tenant = Some("shopfront".into());
        d.project = Some("website".into());
        app.owner = Some("tenant:shopfront".into());
        app.project = Some("website".into());
        assert!(
            app.columns()[0]
                .iter()
                .any(|&i| app.board.cards[i].key == key)
        );
        app.project = Some("another".into());
        assert!(app.columns().iter().all(Vec::is_empty));
        app.project = Some("website".into());
        app.board.cards[0].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(app.backlog_cards().is_empty());
        app.wont_do = true;
        assert_eq!(app.backlog_cards(), vec![0]);
    }

    #[test]
    fn dispatch_confirmation_cancels_without_action_and_only_yes_overrides() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        let home = app.card(&key).unwrap().machine_index;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        let request = serde_json::json!({"action":"dispatch","id":id});
        let response = crate::actions::Response {
            ok: false,
            message: "3 of 3 slots used".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            id: Some(id.clone()),
            ..Default::default()
        };
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        reply_to(&mut app, home, request.clone(), Ok(response.clone()));
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::None
        );
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        reply_to(&mut app, home, request, Ok(response));
        settle(&mut app);
        let Action::Mutate { machine, request } = press(&mut app, 'y') else {
            panic!("expected action")
        };
        assert_eq!(machine, home);
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["override"], true);
    }

    /// Intent 0409: a start refused for your open questions opens the box, not a yes-or-no, and
    /// the answers go back with the start. A question that comes while you've moved on waits for d.
    #[test]
    fn a_start_refused_for_questions_opens_the_box_and_the_answers_go_with_the_next_start() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        let home = app.card(&key).unwrap().machine_index;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        let request = serde_json::json!({"action":"dispatch","id":id});
        let response = crate::actions::Response {
            ok: false,
            message: "for an older Kerdr".into(),
            confirmation: true,
            confirm: Some("0123456789abcdef".into()),
            asked: Some("fedcba9876543210".into()),
            questions: vec![crate::actions::Question {
                num: "1".into(),
                question: "Whose is it?".into(),
                pick: "Kibbelz".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        reply_to(&mut app, home, request.clone(), Ok(response.clone()));
        let Mode::Kickoff { form, .. } = &mut app.mode else {
            panic!("expected the box: {:?}", app.mode)
        };
        assert_eq!(form.question().question, "Whose is it?");
        form.ready_at = Instant::now();
        let Action::Mutate {
            machine,
            request: sent,
        } = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("expected the start")
        };
        assert_eq!(machine, home);
        assert_eq!(sent["action"], "dispatch");
        assert_eq!(sent["asked"], "fedcba9876543210");
        assert_eq!(
            sent["answers"],
            serde_json::json!([{"num":"1","take":true}])
        );
        assert_eq!(sent.get("override"), None);

        // Moved on while it was asked: nothing pops up over what you're doing, and d asks again.
        app.mode = Mode::Board;
        let from = CardSnapshot::of(app.card(&key).unwrap());
        app.sel.key = None;
        app.action_from = Some(from);
        reply_to(&mut app, home, request, Ok(response));
        assert!(matches!(app.mode, Mode::Board), "{:?}", app.mode);
        assert!(
            last_said(&app).unwrap().0.contains("kickoff questions"),
            "{:?}",
            last_said(&app)
        );
    }

    /// Intent 0591, B1-106 and B1-108 (r3-a's `prefill`): his Kickoff answers whose questions
    /// changed while he answered come back, and the box fills them in again by number for him to
    /// check, rather than have him type them again.
    fn kickoff_reply(
        app: &mut App,
        home: usize,
        request: serde_json::Value,
        reply: serde_json::Value,
    ) {
        reply_to(
            app,
            home,
            request,
            Ok(crate::actions::parse_response(&reply.to_string()).unwrap()),
        );
    }

    fn kickoff_now() -> serde_json::Value {
        serde_json::json!({"ok":false,"message":"for an older Kerdr","asked":"newtoken00000000",
            "note":"Some of these changed while you were answering: your answers to question 3 were saved, and yours to the rest are filled in for you to check.",
            "questions":[
                {"num":"1","question":"Whose is it, now that it's two sites?","pick":"Kibbelz"},
                {"num":"2","question":"Where does it run?","pick":"The server"}]})
    }

    #[test]
    fn a_start_whose_questions_changed_while_he_answered_fills_his_answers_in_again() {
        for with_prefill in [true, false] {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            let (key, id) = waiting_backlog_card(&app);
            let home = app.card(&key).unwrap().machine_index;
            app.sel = Selection {
                col: Stage::Backlog.index(),
                key: Some(key.clone()),
            };
            let request = serde_json::json!({"action":"dispatch","id":id,"asked":"oldtoken00000000",
                "answers":[{"num":"1","text":"Kibbelz, the personal one"},{"num":"2","take":true},{"num":"3","text":"Soon"}]});
            let mut reply = kickoff_now();
            if with_prefill {
                reply["prefill"] = serde_json::json!([
                    {"num":"1","text":"Kibbelz, the personal one"},
                    {"num":"2","take":true}
                ]);
            }
            app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
            kickoff_reply(&mut app, home, request, reply);
            let Mode::Kickoff { form, .. } = &mut app.mode else {
                panic!("expected the box: {:?}", app.mode)
            };
            assert!(
                form.note
                    .as_deref()
                    .unwrap()
                    .contains("filled in for you to check")
            );
            if !with_prefill {
                // A backend without it: as before, nothing filled in on a reworded question.
                assert_eq!(form.drafts, ["", ""]);
                continue;
            }
            assert_eq!(form.drafts, ["Kibbelz, the personal one", ""]);
            // His to check: ⏎ takes each, his words and the recommendation he took.
            form.ready_at = Instant::now();
            press_code(&mut app, KeyCode::Enter);
            let Mode::Kickoff { form, .. } = &mut app.mode else {
                panic!("the box shut")
            };
            form.ready_at = Instant::now();
            let Action::Mutate { request: sent, .. } = press_code(&mut app, KeyCode::Enter) else {
                panic!("expected the start")
            };
            assert_eq!(sent["asked"], "newtoken00000000");
            assert_eq!(
                sent["answers"],
                serde_json::json!([{"num":"1","text":"Kibbelz, the personal one"},{"num":"2","take":true}])
            );
        }
    }

    #[test]
    fn answers_given_back_after_esc_are_filled_in_on_the_next_box() {
        // Esc's save found some questions changed: those answers come back with no box to fill,
        // and the next d's box fills them in, once. Refused whole or saved in part alike.
        for saved in [
            serde_json::json!({"ok":false,"message":"#41's questions changed while you were answering, so your answers weren't saved. d shows them as they are now, with yours filled in.","prefill":[{"num":"2","text":"on the laptop"}]}),
            serde_json::json!({"ok":true,"partial":true,"message":"Saved your answers to question 1 of #41; the rest changed while you were answering, so d shows them as they are now, with yours filled in.","prefill":[{"num":"2","text":"on the laptop"}]}),
        ] {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            let (key, id) = waiting_backlog_card(&app);
            let home = app.card(&key).unwrap().machine_index;
            app.sel = Selection {
                col: Stage::Backlog.index(),
                key: Some(key.clone()),
            };
            let mut saved = saved;
            saved["id"] = serde_json::json!(id);
            kickoff_reply(
                &mut app,
                home,
                serde_json::json!({"action":"answer","id":id,"asked":"oldtoken00000000",
                    "answers":[{"num":"1","take":true},{"num":"2","text":"on the laptop"}]}),
                saved,
            );
            assert!(app.kickoff_prefill.contains_key(&id));
            // The next d's answer: the questions as they are now, with nothing given back.
            app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
            let mut now = kickoff_now();
            now["note"] = serde_json::Value::Null;
            kickoff_reply(
                &mut app,
                home,
                serde_json::json!({"action":"dispatch","id":id}),
                now,
            );
            let Mode::Kickoff { form, .. } = &app.mode else {
                panic!("expected the box: {:?}", app.mode)
            };
            assert_eq!(form.drafts, ["", "on the laptop"]);
            assert!(!app.kickoff_prefill.contains_key(&id), "filled in twice");
        }
    }

    /// Intent 0404: ⏎ on work nobody started starts it, sending what d sends, and past the usual
    /// limit asks the same question, whose yes overrides it.
    #[test]
    fn enter_on_work_nobody_started_starts_it_as_d_does() {
        let mut app = demo_app();
        let (key, id) = waiting_backlog_card(&app);
        let home = app.card(&key).unwrap().machine_index;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        // With no intent actions there's nothing to start it with, and ⏎ says why.
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message().unwrap().0.contains("no [actions] command"),
            "{:?}",
            app.message()
        );
        app.cfg.actions.command = Some("test-adapter".into());
        let by_d = press(&mut app, 'd');
        app.busy = false;
        let by_enter = enter(&mut app);
        assert_eq!(by_enter, by_d);
        let Action::Mutate { machine, request } = by_enter else {
            panic!("{:?}", app.message())
        };
        assert_eq!(machine, home);
        assert_eq!(request, serde_json::json!({"action":"dispatch","id":id}));
        // Past the usual limit: the question, and its yes overrides.
        let response = crate::actions::Response {
            ok: false,
            message: "3 of 3 slots used".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            id: Some(id.clone()),
            ..Default::default()
        };
        reply_to(&mut app, home, request, Ok(response));
        settle(&mut app);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("{:?}", app.mode)
        };
        assert!(title.starts_with("Start "), "{title}");
        assert!(message.contains("the usual limit is 3"), "{message}");
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("expected the yes to go out")
        };
        assert_eq!(machine, home);
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["override"], true);
        // Started, but the feed still shows it waiting: ⏎ again (a held key, say) sends nothing.
        let started = crate::actions::Response {
            ok: true,
            message: "Started.".into(),
            ..Default::default()
        };
        reply_to(&mut app, home, request, Ok(started));
        assert!(!app.busy);
        assert!(crate::app::waiting_work(app.card(&key).unwrap()));
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("is starting"));
        assert!(matches!(press(&mut app, 'd'), Action::Mutate { .. }));
    }

    /// Intent 0404: the backlog screen's ⏎ still peeks, and ⏎ again in that peek leaves starting
    /// to d; a peek from the board starts it, as the board's ⏎ does.
    #[test]
    fn the_backlog_screens_enter_still_peeks() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        let sel = app
            .backlog_cards()
            .iter()
            .position(|&i| app.board.cards[i].key == key)
            .unwrap();
        app.mode = Mode::Backlog { sel };
        assert_eq!(enter(&mut app), Action::None);
        assert!(matches!(app.mode, Mode::Peek { .. }));
        assert_eq!(app.sel.key.as_deref(), Some(key.as_str()));
        // ⏎ again at once, doubled or held: it starts nothing (intent 0591, K1-07).
        assert_eq!(enter(&mut app), Action::None);
        assert!(matches!(app.mode, Mode::Peek { .. }));
        // Once the peek has been up a moment, ⏎ starts it, as it does everywhere else.
        app.peek_opened = Some(Instant::now() - std::time::Duration::from_secs(1));
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("⏎ in the peek started nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "dispatch");
        assert!(matches!(app.mode, Mode::Peek { .. }));
        app.resolver = Default::default();
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        assert!(matches!(enter(&mut app), Action::Mutate { .. }));
        // A start stays in the peek, still on its card, so a question past the limit asks there.
        assert!(matches!(app.mode, Mode::Peek { .. }));
    }

    /// Intent 0404: ⏎ on a search hit nobody started starts that card, not the one selected.
    #[test]
    fn enter_on_a_search_hit_nobody_started_starts_that_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .clone();
        app.sel = Selection {
            col: other.stage.index(),
            key: Some(other.key.clone()),
        };
        let title = app.card(&key).unwrap().title.clone();
        let mut s = Search {
            query: title,
            filter: SearchFilter::Cards,
            sel: 0,
        };
        s.sel = app
            .search_hits(&s)
            .iter()
            .position(|(h, _)| matches!(h, Hit::Card(i) if app.board.cards[*i].key == key))
            .expect("the card among the hits");
        app.mode = Mode::Search(s);
        let search = app.mode.clone();
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("{:?}", app.message())
        };
        assert_eq!(request, serde_json::json!({"action":"dispatch","id":id}));
        // It starts from a peek on the card, which keeps it selected through a feed rebuild,
        // wherever its card sits, so a question past the limit still asks.
        assert!(matches!(app.mode, Mode::Peek { .. }));
        app.rebuild();
        assert_eq!(app.sel.key.as_deref(), Some(key.as_str()));
        reply_to(&mut app, machine, request.clone(), Ok(over_limit(&id)));
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
        // Search, Ctrl+O peeks at the hit, then ⏎ there: the peek stays, so the question asks.
        app.mode = search;
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Peek { .. }));
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("{:?}", app.message())
        };
        assert_eq!(request["id"], id.as_str());
        assert!(matches!(app.mode, Mode::Peek { .. }));
        reply_to(&mut app, machine, request, Ok(over_limit(&id)));
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
    }

    fn over_limit(id: &str) -> crate::actions::Response {
        crate::actions::Response {
            ok: false,
            message: "3 of 3 slots used".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            id: Some(id.into()),
            ..Default::default()
        }
    }

    #[test]
    fn leader_affirm_and_counted_move_name_the_selected_backlog_record() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let i = app.backlog_cards()[0];
        let id = app.board.cards[i].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["action"], "refresh");
        assert_eq!(request["id"], id);
        press(&mut app, '1');
        press(&mut app, '0');
        let Action::Mutate { request, .. } = press(&mut app, 'K') else {
            panic!("expected reorder")
        };
        assert_eq!(request["action"], "move");
        assert_eq!(request["delta"], -10);
    }

    #[test]
    fn b_sends_the_selected_backlog_record_to_the_bottom_of_the_order() {
        // Intent 0415: one key, the mirror of t, from the backlog screen and from the menu.
        for leader in [false, true] {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            app.mode = Mode::Backlog { sel: 0 };
            let i = app.backlog_cards()[0];
            let id = app.board.cards[i].detail.as_ref().unwrap().id.clone();
            if leader {
                app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
            }
            let Action::Mutate { request, .. } = press(&mut app, 'B') else {
                panic!("B did nothing (menu: {leader})")
            };
            assert_eq!(
                request,
                serde_json::json!({"action": "move", "id": id, "bottom": true}),
                "menu: {leader}"
            );
        }
    }

    #[test]
    fn polling_preserves_peek_target_when_backlog_column_is_hidden() {
        let mut app = demo_app();
        app.width = 80;
        app.mode = Mode::Backlog { sel: 0 };
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let before = app.selected().unwrap().key.clone();
        app.rebuild();
        assert_eq!(app.selected().unwrap().key, before);
    }

    #[test]
    fn search_sessions_and_workspaces_respects_selected_machine() {
        let mut app = demo_app();
        press(&mut app, '}');
        for filter in [SearchFilter::Sessions, SearchFilter::Workspaces] {
            let search = Search {
                query: String::new(),
                filter,
                sel: 0,
            };
            for (hit, _) in app.search_hits(&search) {
                let machine = match hit {
                    Hit::Session(i) => &app.board.sessions[i].machine,
                    Hit::Workspace(i) => &app.board.workspaces[i].machine,
                    _ => continue,
                };
                assert_eq!(machine, app.machine.as_ref().unwrap());
            }
        }
    }

    #[test]
    fn owner_cycle_preserves_project_for_an_empty_intersection() {
        let mut app = demo_app();
        app.project = Some("website".into());
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 't');
        assert_eq!(app.project.as_deref(), Some("website"));
    }

    #[test]
    fn polling_reorder_never_retargets_a_palette_action_to_the_next_record() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let i = app.backlog_cards()[0];
        let id = app.board.cards[i].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        for machine in &mut app.machines {
            if let Some(feed) = &mut machine.feed {
                for card in &mut feed.cards {
                    if card.id == id {
                        card.backlog_rank = Some(99999);
                    }
                }
            }
        }
        app.rebuild();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
    }

    #[test]
    fn reopening_backlog_or_switching_machine_actions_the_visible_row() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'b');
        press(&mut app, 'j');
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, 'b');
        let id = app.board.cards[app.backlog_cards()[0]]
            .detail
            .as_ref()
            .unwrap()
            .id
            .clone();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
        press(&mut app, 'j');
        press(&mut app, '}');
        let id = app.board.cards[app.backlog_cards()[0]]
            .detail
            .as_ref()
            .unwrap()
            .id
            .clone();
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("expected affirmation")
        };
        assert_eq!(request["id"], id);
    }

    fn provider_app() -> App {
        let mut app = demo_app();
        let seed = app.board.cards[0].clone();
        app.board.cards = [
            vec!["claude"],
            vec!["codex"],
            vec!["claude", "codex"],
            vec!["future-agent"],
            vec![],
        ]
        .into_iter()
        .enumerate()
        .map(|(i, agents)| {
            let mut card = seed.clone();
            card.key = format!("provider-{i}");
            card.machine = "laptop".into();
            card.stage = if i == 4 { Stage::Backlog } else { Stage::Build };
            card.title = "Codex and Claude mentioned in title".into();
            card.agents = agents
                .into_iter()
                .map(|name| crate::model::AgentInfo {
                    agent: name.into(),
                    status: "working".into(),
                    pane_id: format!("p{i}"),
                    title: None,
                    conversation: None,
                })
                .collect();
            card
        })
        .collect();
        app
    }

    #[test]
    fn a_workspace_shows_under_a_filter_when_a_card_for_one_of_its_tabs_matches() {
        let mut app = demo_app();
        let i = app
            .board
            .cards
            .iter()
            .position(|c| c.workspace_id.is_some() && !c.agents.is_empty())
            .unwrap();
        for a in &mut app.board.cards[i].agents {
            a.agent = "claude".into();
        }
        let mut child = app.board.cards[i].clone();
        child.key = "child-in-a-tab".into();
        child.tab_id = Some("t2".into());
        child.agents[0].agent = "codex".into();
        child.agents.truncate(1);
        let ws = child.workspace_id.clone();
        app.board.cards.push(child);
        app.provider = Some("codex".into());
        let hits = app.search_hits(&Search {
            query: String::new(),
            filter: SearchFilter::Workspaces,
            sel: 0,
        });
        assert!(
            hits.iter().any(|(hit, _)| matches!(hit,
                Hit::Workspace(w) if Some(&app.board.workspaces[*w].id) == ws.as_ref())),
            "{hits:?}"
        );
    }

    fn provider_command(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(press(app, 'a'), Action::None);
    }

    fn visible_ids(app: &App) -> Vec<usize> {
        app.board
            .cards
            .iter()
            .enumerate()
            .filter(|(_, card)| app.card_visible(card))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn provider_cycle_uses_reported_agents_including_mixed_and_future_providers() {
        let mut app = provider_app();
        assert_eq!(visible_ids(&app), vec![0, 1, 2, 3, 4]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0, 2]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![1, 2]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![3]);
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn provider_filter_combines_with_machine_owner_project_and_dismissal() {
        let mut app = provider_app();
        for card in &mut app.board.cards {
            let detail = card.detail.get_or_insert_with(Default::default);
            detail.tenant = Some("shopfront".into());
            detail.project = Some("web".into());
        }
        app.board.cards[2].machine = "build-box".into();
        app.machine = Some("laptop".into());
        app.tabs = vec![Scope::Machine("laptop".into())];
        app.owner = Some("tenant:shopfront".into());
        app.project = Some("web".into());
        provider_command(&mut app);
        assert_eq!(visible_ids(&app), vec![0]);
        app.project = Some("other".into());
        assert!(visible_ids(&app).is_empty());
        app.project = Some("web".into());
        app.board.cards[0].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(visible_ids(&app).is_empty());
        app.wont_do = true;
        assert_eq!(visible_ids(&app), vec![0]);
    }

    #[test]
    fn provider_search_excludes_unmatched_workspaces_and_sessions() {
        let mut app = demo_app();
        for card in &mut app.board.cards {
            for agent in &mut card.agents {
                agent.agent = "claude".into();
            }
        }
        let selected = app
            .board
            .cards
            .iter_mut()
            .find(|c| c.workspace_id.is_some() && !c.agents.is_empty())
            .unwrap();
        selected.agents[0].agent = "codex".into();
        let machine = selected.machine.clone();
        let session = selected.session.clone().unwrap();
        provider_command(&mut app);
        provider_command(&mut app);
        for filter in [SearchFilter::Workspaces, SearchFilter::Sessions] {
            let hits = app.search_hits(&Search {
                query: String::new(),
                filter,
                sel: 0,
            });
            assert!(!hits.is_empty());
            for (hit, _) in hits {
                match hit {
                    Hit::Workspace(i) => {
                        let w = &app.board.workspaces[i];
                        assert!(
                            w.card
                                .as_deref()
                                .and_then(|key| app.card(key))
                                .is_some_and(|c| c.agents.iter().any(|a| a.agent == "codex"))
                        );
                    }
                    Hit::Session(i) => {
                        assert_eq!(app.board.sessions[i].machine, machine);
                        assert_eq!(app.board.sessions[i].session, session);
                    }
                    _ => panic!("wrong search group"),
                }
            }
        }
    }

    #[test]
    fn scores_substrings_before_scattered_letters() {
        assert_eq!(score("flyw", "#143 Find the next flywheel"), Some(19));
        assert!(score("ftnf", "#143 Find the next flywheel").unwrap() >= 500);
        assert_eq!(score("zzz", "#143 Find the next flywheel"), None);
        assert_eq!(
            match_range("shop", "On the shop's Spin screen"),
            Some((7, 11))
        );
    }

    #[test]
    fn a_capital_that_lowercases_to_two_letters_still_searches() {
        // "İ" lowercases to two characters, so a range can run past the original text; the
        // drawing code checks it (ui::tests::highlighting_never_slices_past_the_text).
        let (a, b) = match_range("move", "İstanbul office move").unwrap();
        assert!(a <= b);
        assert!(score("move", "İstanbul office move").is_some());
    }

    /// The build box answers again, without this card's workspace.
    fn build_box_back_without(app: &mut App, card: &Card) {
        let mi = card.machine_index;
        let ws = card.workspace_id.clone().unwrap();
        let session = card.session.clone().unwrap();
        let mut data = app.machines[mi].data.clone().unwrap();
        if let Some(Ok(snap)) = data.snapshots.get_mut(&session) {
            snap.workspaces.retain(|w| w.workspace_id != ws);
            snap.agents.retain(|a| a.workspace_id != ws);
        }
        app.apply(Event {
            machine: mi,
            conn: Conn::Live {
                latency: Duration::from_millis(9),
            },
            data: Some(data),
            at: Local::now(),
        });
    }

    fn away_card(app: &App, source: crate::model::Source) -> Card {
        let mi = app.machine_index("build-box").unwrap();
        app.board
            .cards
            .iter()
            .find(|c| c.machine_index == mi && c.source == source && c.is_open_workspace())
            .unwrap()
            .clone()
    }

    #[test]
    fn a_waiting_jump_says_so_when_its_workspace_closed_while_the_machine_was_away() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        let card = away_card(&app, crate::model::Source::Workspace);
        app.wait_to_jump(&card);
        assert_eq!(app.pending_jump_ready(), None);
        assert!(
            app.pending_jump.is_some(),
            "still waiting while build-box is away"
        );
        build_box_back_without(&mut app, &card);
        assert!(app.card(&card.key).is_none());
        assert_eq!(app.pending_jump_ready(), None);
        assert!(app.pending_jump.is_none());
        assert_eq!(
            app.message(),
            Some((
                format!(
                    "Workspace {} closed while build-box was away, and nothing of it is left to open.",
                    card.reference
                )
                .as_str(),
                Tone::Warn
            ))
        );
    }

    #[test]
    fn a_waiting_jump_into_an_intent_whose_workspace_closed_makes_plain_enters_choice() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        app.cfg.actions.command = Some("test-adapter".into());
        let card = away_card(&app, crate::model::Source::Feed);
        app.wait_to_jump(&card);
        build_box_back_without(&mut app, &card);
        assert!(app.card(&card.key).is_some_and(|c| !c.is_open_workspace()));
        // Its turn: jump_cards takes it, and with no workspace makes the choice ⏎ makes (intent
        // 0591), never "nothing to jump into".
        assert_eq!(
            app.pending_jump_ready(),
            Some(Action::Jump(card.key.clone()))
        );
        assert!(app.pending_jump.is_none());
        let now = app.card(&card.key).unwrap().clone();
        let Action::Mutate { request, .. } = app.jump_action(card.key.clone()) else {
            panic!("plain ⏎'s choice sent nothing: {:?}", app.message())
        };
        let expected = if now.reopenable() {
            "reopen"
        } else {
            "dispatch"
        };
        assert_eq!(request["action"], expected);
    }

    #[test]
    fn a_waiting_jump_does_for_its_intent_what_enter_does_now_and_never_takes_another_sources() {
        // Intent 0591, K1-04: its card under its own key, its workspace closed meanwhile: ⏎ does
        // what it does now, never "it's off the board".
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let pending = |key: String| PendingJump {
            key,
            workspace: "07-build".into(),
            machine: "gone-box".into(),
            stuck: None,
            bare: None,
            resume: None,
        };
        app.pending_jump = Some(pending(key.clone()));
        app.mode = Mode::Board;
        assert_eq!(app.pending_jump_ready(), Some(Action::Jump(key.clone())));
        // Codex's review: another source's card with the same number is other work. Nothing is
        // started or entered for it, and it's said.
        app.pending_jump = Some(pending(format!("another-source:{id}")));
        app.sel.key = None;
        assert_eq!(app.pending_jump_ready(), None);
        assert_eq!(app.sel.key, None);
        let (said, tone) = app.message().unwrap();
        assert!(said.contains("no longer on this board"), "{said}");
        assert_eq!(tone, Tone::Warn);
    }

    #[test]
    fn a_yes_to_a_question_whose_card_left_the_board_never_acts_on_another_sources_card() {
        // Codex's review: the question was about source-a's #N; it left the board while he
        // decided, and another source's card has the same number. Nothing is started, moved or
        // selected for it.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let card = app.card(&key).unwrap().clone();
        let mut asked_about = crate::model::CardSnapshot::of(&card);
        asked_about.key = format!("source-a:{id}");
        app.sel.key = None;
        for request in [
            serde_json::json!({"action":"dispatch","id":id,"account":"ARK_C","confirm":"t-1"}),
            serde_json::json!({"action":"reopen","id":id,"confirm":"t-1"}),
            serde_json::json!({"action":"close","id":id,"confirm":"t-1"}),
        ] {
            let action = app.ask_again(
                card.machine_index,
                request.clone(),
                Some(asked_about.clone()),
            );
            assert_eq!(action, Action::None, "{request}");
            assert!(matches!(app.mode, Mode::Board), "{request}: {:?}", app.mode);
            assert_eq!(app.sel.key, None, "{request}");
            let (said, tone) = app.message().unwrap();
            assert!(said.contains("is gone from the board"), "{said}");
            assert_eq!(tone, Tone::Warn);
        }
    }

    #[test]
    fn each_image_question_sends_its_own_words_never_a_later_one_s() {
        // Codex's review: A's question is held while he types B; B goes with its image still
        // being read, and A's question opens. B's image fails then: its question waits, the yes
        // to A sends A's words, and B is asked next, its yes sending B's.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        use crate::images::Source;
        let none = || crate::images::Loading {
            images: vec![],
            why: Some("The clipboard holds no image.".into()),
        };
        let type_in = |app: &mut App, words: &str| {
            press(app, 'c');
            for c in words.chars() {
                press(app, c);
            }
        };
        type_in(&mut app, "Words A");
        let (a, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        type_in(&mut app, "Words B");
        assert_eq!(app.images_loaded(a, none()), Action::None);
        assert!(
            matches!(app.mode, Mode::Compose { .. }),
            "asked over typing"
        );
        let (b, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.next_owed(), Action::None);
        let Mode::Confirm {
            request: asked_a, ..
        } = app.mode.clone()
        else {
            panic!("A not asked: {:?}", app.mode)
        };
        assert_eq!(app.images_loaded(b, none()), Action::None);
        assert!(
            matches!(&app.mode, Mode::Confirm { request, .. } if *request == asked_a),
            "B's question took A's place: {:?}",
            app.mode
        );
        answer_now(&mut app);
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        else {
            panic!("A's yes sent nothing: {:?}", app.message())
        };
        assert_eq!(request["words"], "Words A");
        assert_eq!(app.next_owed(), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }), "B not asked");
        answer_now(&mut app);
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        else {
            panic!("B's yes sent nothing: {:?}", app.message())
        };
        assert_eq!(request["words"], "Words B");
    }

    #[test]
    fn a_closed_workspace_on_work_with_a_conversation_on_record_reopens_it() {
        // Intent 0591, K3-02: neither under way nor done (shelved), but its conversation is on
        // record: reopened as for work under way, never "has closed since the board last read it".
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.stage = Stage::Backlog;
        card.state = Some("shelved".into());
        card.workspace_id = Some("w5".into());
        card.session = Some("web".into());
        assert!(!app.closed_under_you(&key, Some("w5")));
        let Action::Mutate { request, .. } = app.after_closed(&key, Some("w5"), false) else {
            panic!("nothing done: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
    }

    #[test]
    fn a_waiting_jump_goes_ahead_once_its_machine_answers_with_the_workspace() {
        let mut app = App::new(Config::default(), crate::demo::machines(true));
        let card = away_card(&app, crate::model::Source::Workspace);
        app.wait_to_jump(&card);
        let mi = card.machine_index;
        let data = app.machines[mi].data.clone();
        app.apply(Event {
            machine: mi,
            conn: Conn::Live {
                latency: Duration::from_millis(9),
            },
            data,
            at: Local::now(),
        });
        assert_eq!(app.pending_jump_ready(), Some(Action::Jump(card.key)));
        assert!(app.pending_jump.is_none());
    }

    #[test]
    fn the_next_card_skips_workspaces_found_closed() {
        let app = demo_app();
        let open: Vec<String> = app
            .needs_you_order()
            .into_iter()
            .filter(|k| app.card(k).is_some_and(|c| c.is_open_workspace()))
            .collect();
        assert_eq!(open.len(), 2, "{open:?}");
        assert_eq!(
            app.next_needs_you_after(&open[0], &[]),
            Some(open[1].clone())
        );
        assert_eq!(
            app.next_needs_you_after(&open[1], &[]),
            Some(open[0].clone())
        );
        assert_eq!(
            app.next_needs_you_after(&open[0], std::slice::from_ref(&open[1])),
            None
        );
    }

    fn waiting_backlog_card(app: &App) -> (String, String) {
        let card = app
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && !c.dismissed()
                    && !c.is_open_workspace()
                    && c.state.as_deref() != Some("shelved")
            })
            .expect("a waiting backlog card in the demo");
        (card.key.clone(), card.detail.as_ref().unwrap().id.clone())
    }

    #[test]
    fn card_keys_work_on_the_board_with_the_same_letters_as_the_backlog() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        assert_eq!(
            app.backlog_card_for_keys().map(|c| c.key.clone()),
            Some(key)
        );
        for (c, action) in [
            ('y', "refresh"),
            ('d', "dispatch"),
            ('t', "move"),
            ('B', "move"),
            ('J', "move"),
        ] {
            let Action::Mutate { request, .. } = press(&mut app, c) else {
                panic!("{c} did nothing on the board")
            };
            assert_eq!(request["action"], action, "{c}");
            assert_eq!(request["id"], id, "{c}");
            app.busy = false;
        }
        // Digits still pick sessions on the board; counted moves are for the backlog.
        press(&mut app, '0');
        assert!(app.count.is_empty());
        assert!(matches!(press(&mut app, 'x'), Action::None));
        assert!(matches!(app.mode, Mode::Confirm { .. }));
    }

    #[test]
    fn card_keys_on_work_under_way_say_what_they_are_for() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.stage == Stage::Build && c.detail.is_some())
            .unwrap()
            .clone();
        app.sel = Selection {
            col: app.col_of(&card),
            key: Some(card.key.clone()),
        };
        assert!(app.backlog_card_for_keys().is_none());
        // d does what ⏎ does: it goes in (intent 0591, K1-24).
        assert_eq!(press(&mut app, 'd'), Action::Jump(card.key.clone()));
        // The ordering keys say it has no place in the backlog order.
        for c in ['y', 't', 'B', 'J', 'K'] {
            assert_eq!(press(&mut app, c), Action::None, "{c}");
            let (text, _) = app.message().unwrap();
            assert!(
                text.contains(&card.reference) && text.contains("no place in the backlog order"),
                "{c}: {text}"
            );
        }
    }

    #[test]
    fn a_new_intent_is_prose_sent_with_enter_and_kept_on_escape() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        for c in "Fix the wifi".chars() {
            press(&mut app, c);
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        app.on_paste("after sleep.\r\nIt bites \"every\" morning.");
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(
            app.draft,
            "Fix the wifi\n\nafter sleep.\nIt bites \"every\" morning."
        );
        // The kept words come back, and Enter sends them with the session in view.
        press(&mut app, 'c');
        let Action::Mutate { request, machine } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "create");
        assert_eq!(
            request["words"],
            "Fix the wifi\n\nafter sleep.\nIt bites \"every\" morning."
        );
        assert_eq!(request["session"], "default");
        assert!(request.get("title").is_none() && request.get("tenant").is_none());
        // The box closed the moment Enter sent them, onto the backlog, where the new intent's
        // card shows at once, selected, while it's saved (intent 0445).
        app.queue_action(machine, request.clone());
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        let card = app
            .backlog_card_for_keys()
            .cloned()
            .expect("its card, selected");
        assert_eq!(card.intake(), Some(crate::resolve::SAVING));
        assert_eq!(
            card.title,
            "Fix the wifi after sleep. It bites \"every\" morning."
        );
        assert!(app.draft.is_empty(), "the box is free for the next one");
        // Nothing may be done to it until it has a record.
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(app.message().unwrap().0.contains("still being saved"));
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(matches!(app.mode, Mode::Backlog { .. }), "nothing asked");
        app.message = None;
        // It worked: the new intent is picked out in the backlog once its card arrives.
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: "Saved as #9001.".into(),
                confirmation: false,
                used: None,
                limit: None,
                session: Some("default".into()),
                id: Some("9001".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty());
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        assert_eq!(app.new_intent_id, Some((0, "9001".to_string())));
        assert!(app.message().is_none(), "saved as shown: nothing said");
        // Its card stays until the feed has the record, then the record's card takes its place,
        // still selected.
        assert_eq!(
            app.backlog_card_for_keys().map(|c| c.key.clone()),
            Some(card.key.clone())
        );
        let made = serde_json::from_value(serde_json::json!({
            "id": "9001", "title": "Fix the wifi", "stage": "backlog", "state": "queued",
            "intake": "pending"
        }))
        .unwrap();
        app.machines[0].feed.as_mut().unwrap().cards.push(made);
        let feed = app.machines[0].feed.clone();
        app.resolver.read(0, feed.as_ref());
        app.rebuild();
        let now = app
            .backlog_card_for_keys()
            .cloned()
            .expect("a card selected");
        assert_eq!(now.detail.as_ref().unwrap().id, "9001");
        assert!(app.board.cards.iter().all(|c| c.key != card.key));
    }

    #[test]
    fn images_go_in_as_tags_like_claude_code_and_travel_with_the_words() {
        use base64::Engine;
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("error shot.png");
        std::fs::write(&shot, &png).unwrap();
        let loaded = || crate::images::Loading {
            images: vec![crate::images::Loaded {
                media_type: "image/png",
                bytes: png.clone(),
            }],
            why: None,
        };
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        for c in "Moving failed:".chars() {
            press(&mut app, c);
        }
        let draft = |app: &App| match &app.mode {
            Mode::Compose { draft, .. } => draft.clone(),
            _ => panic!("the box closed"),
        };
        // Ctrl+V asks for the clipboard's image, read away from the board's thread; what it
        // finds goes in at the end as a tag.
        use crate::images::Source;
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_v), Action::LoadImages(Source::Clipboard));
        let (id, room) = app.begin_image_load(&Source::Clipboard).unwrap();
        assert_eq!(room, crate::images::Room::left(&[]));
        assert_eq!(app.message().unwrap().0, "Reading the clipboard…");
        // While it reads, a second read waits its turn.
        assert!(app.begin_image_load(&Source::Clipboard).is_none());
        // Words typed while it reads come after the tag, where it was pasted.
        for c in " again".chars() {
            press(&mut app, c);
        }
        app.images_loaded(id, loaded());
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] again");
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.truncate("Moving failed: [Image #1] ".len());
        }
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] ");
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("Attached [Image #1]: a PNG image")
        );
        // A pasted (or dropped) image file's path attaches that file; an empty paste looks at
        // the clipboard; other text, blank lines and spaces too, stays words.
        let source = app.on_paste(&format!("'{}'", shot.display()));
        assert_eq!(
            source,
            Action::LoadImages(Source::Files(vec![shot.clone()]))
        );
        let Action::LoadImages(source) = source else {
            unreachable!()
        };
        let (id, room) = app.begin_image_load(&source).unwrap();
        app.images_loaded(id, source.load(room));
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] [Image #2] ");
        assert_eq!(app.on_paste(""), Action::LoadImages(Source::Clipboard));
        assert_eq!(app.on_paste("  \n"), Action::None);
        assert_eq!(
            draft(&app).words,
            "Moving failed: [Image #1] [Image #2]   \n"
        );
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.truncate(draft.words.len() - 3);
        }
        app.on_paste("and");
        app.attach_images(crate::images::Loading::refused(
            "There's no image on the clipboard.",
        ));
        assert_eq!(app.message().unwrap().1, Tone::Warn);
        app.attach_images(loaded());
        assert_eq!(draft(&app).images.len(), 3);
        // Backspace right after a tag takes the whole tag and its image out.
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        app.on_key(backspace);
        app.on_key(backspace);
        assert_eq!(
            draft(&app).words,
            "Moving failed: [Image #1] [Image #2] and "
        );
        assert_eq!(draft(&app).images.len(), 2);
        // Ctrl+W takes a tag whole too, and a tag deleted any other way leaves its image unsent.
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] ");
        assert_eq!(draft(&app).images.len(), 1);
        app.attach_images(loaded());
        assert_eq!(draft(&app).words, "Moving failed: [Image #1] [Image #2] ");
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words = draft.words.replace("[Image #1]", "image one");
        }
        // A tag typed twice keeps its image until the last of them goes.
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.words.push_str("[Image #2]");
        }
        app.on_key(backspace);
        assert_eq!(draft(&app).images.len(), 2);
        // Esc keeps the images with the words for next time, and drops a read still going.
        let (late, room) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.draft_images.len(), 2);
        press(&mut app, 'c');
        app.images_loaded(late, Source::Files(vec![shot.clone()]).load(room));
        assert_eq!(
            draft(&app).images.len(),
            2,
            "a read for a closed box went in"
        );
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(request["words"], "Moving failed: image one [Image #2]");
        let sent = request["images"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["tag"], 2);
        assert_eq!(sent[0]["type"], "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(sent[0]["data"].as_str().unwrap())
                .unwrap(),
            png
        );
        // Enter closed the box, empty for the next; the send keeps its own words and images
        // until they're saved (intent 0445).
        assert!(!matches!(app.mode, Mode::Compose { .. }));
        assert!(app.draft.is_empty() && app.draft_images.is_empty());
        assert_eq!(app.submissions.len(), 1);
        assert_eq!(app.submissions[0].images.len(), 1);
        // Unanswered: they come back for c.
        reply_to(
            &mut app,
            0,
            crate::actions::request_metadata(&request),
            Err("timed out".into()),
        );
        assert_eq!(app.draft, "Moving failed: image one [Image #2]");
        assert_eq!(app.draft_images.len(), 1);
        app.message = None;
        // A different image is new work: it gets a submission of its own.
        let first = request["submission"].clone();
        press(&mut app, 'c');
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.images[0].bytes = std::sync::Arc::new([png.clone(), vec![1]].concat());
        }
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_ne!(request["submission"], first);
        reply_to(
            &mut app,
            0,
            crate::actions::request_metadata(&request),
            Ok(crate::actions::Response {
                ok: true,
                message: "Saved as #9003.".into(),
                id: Some("9003".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty() && app.draft_images.is_empty());
        assert_eq!(
            app.submissions.len(),
            1,
            "only the unanswered one stays, for a resend"
        );
    }

    #[test]
    fn enter_while_an_image_is_read_sends_once_it_is_in() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        for c in "The dock:".chars() {
            press(&mut app, c);
        }
        use crate::images::Source;
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        // Enter doesn't hold him in the box: it closes onto the backlog, and the words wait for
        // the image (intent 0445).
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        assert!(app.message().unwrap().0.contains("once it's in"));
        // The box opens empty for the next one meanwhile.
        press(&mut app, 'c');
        assert!(matches!(&app.mode, Mode::Compose { draft, .. } if draft.words.is_empty()));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let loaded = crate::images::Loading {
            images: vec![crate::images::Loaded {
                media_type: "image/png",
                bytes: png,
            }],
            why: None,
        };
        let Action::Mutate { request, .. } = app.images_loaded(id, loaded) else {
            panic!("the words weren't sent once the image was in")
        };
        assert_eq!(request["words"], "The dock: [Image #1]");
        assert_eq!(request["images"].as_array().map(Vec::len), Some(1));
        // A read that finds nothing sends nothing, and gives the words back.
        press(&mut app, 'c');
        for c in "Again".chars() {
            press(&mut app, c);
        }
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let none = crate::images::Loading {
            images: vec![],
            why: Some("The clipboard holds no image.".into()),
        };
        assert_eq!(app.images_loaded(id, none), Action::None);
        // Whether they go without it is his call (intent 0591, K1-37); they're kept meanwhile.
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("no question: {:?}", app.mode)
        };
        assert_eq!(title, "Send without the image?");
        assert!(
            message.contains("The clipboard holds no image."),
            "{message}"
        );
        assert!(message.contains("c opens them again"), "{message}");
        assert_eq!(app.draft, "Again");
        answer_now(&mut app);
        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Backlog { .. }), "{:?}", app.mode);
        assert!(app.message().unwrap().0.contains("kept, unsent"));
        assert_eq!(app.draft, "Again");
        assert!(app.send_without.is_empty());
    }

    /// Lets the question on screen take an answer now, as it does half a second after it appears.
    fn answer_now(app: &mut App) {
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
    }

    #[test]
    fn words_whose_image_didn_t_go_in_go_without_it_on_a_yes() {
        // Intent 0591, K1-37: the image couldn't be read; a yes sends the words as they are,
        // taken back from the box they were kept in, and not too soon.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        use crate::images::Source;
        app.draft = "The dock".into();
        press(&mut app, 'c');
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let none = crate::images::Loading {
            images: vec![],
            why: Some("The clipboard holds no image.".into()),
        };
        assert_eq!(app.images_loaded(id, none), Action::None);
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            Action::None,
            "a y typed before the question was seen answered it"
        );
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        answer_now(&mut app);
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        else {
            panic!("the yes didn't send the words: {:?}", app.message())
        };
        assert_eq!(request["action"], "create");
        assert_eq!(request["words"], "The dock");
        assert!(app.draft.is_empty(), "the words sent stayed in the box too");
        assert!(app.send_without.is_empty());
        // Kept in the queue behind words typed meanwhile, they're taken from there.
        app.mode = Mode::Board;
        press(&mut app, 'c');
        for c in "Fix the NAS".chars() {
            press(&mut app, c);
        }
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, 'c');
        for c in "Other".chars() {
            press(&mut app, c);
        }
        let none = crate::images::Loading {
            images: vec![],
            why: Some("The clipboard holds no image.".into()),
        };
        assert_eq!(app.images_loaded(id, none), Action::None);
        // Never asked over the words he's typing, where a y or ⏎ meant for them would send these
        // (Codex's review): held, and those keys go into his box.
        assert!(
            matches!(&app.mode, Mode::Compose { draft, .. } if draft.words == "Other"),
            "{:?}",
            app.mode
        );
        assert_eq!(app.next_owed(), Action::None);
        assert!(
            matches!(app.mode, Mode::Compose { .. }),
            "asked over typing"
        );
        press(&mut app, 'y');
        assert!(
            matches!(&app.mode, Mode::Compose { draft, .. } if draft.words == "Othery"),
            "{:?}",
            app.mode
        );
        assert_eq!(app.returned.len(), 1);
        // Back on the board or a list, it's asked, and its yes sends those words, not the box's.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.enter_can_land_here(), "{:?}", app.mode);
        assert_eq!(app.next_owed(), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("no question: {:?}", app.mode)
        };
        assert!(message.contains("once the box is empty"), "{message}");
        answer_now(&mut app);
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send the words")
        };
        assert_eq!(request["words"], "Fix the NAS");
        assert!(app.returned.is_empty());
        assert_eq!(app.draft, "Othery", "the words typed meanwhile stay");
    }

    fn a_png() -> crate::images::Loaded {
        crate::images::Loaded {
            media_type: "image/png",
            bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
        }
    }

    #[test]
    fn words_sent_while_their_image_is_read_never_clear_words_typed_since() {
        // Review of intent 0445: words Esc kept, sent with an image still being read; the box
        // opens empty meanwhile, and what's typed and kept there stays when the first go.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.draft = "Back up the NAS".into();
        press(&mut app, 'c');
        use crate::images::Source;
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, 'c');
        assert!(
            matches!(&app.mode, Mode::Compose { draft, .. } if draft.words.is_empty()),
            "the words on their way came back into the box"
        );
        for c in "Fix the dock".chars() {
            press(&mut app, c);
        }
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let loading = crate::images::Loading {
            images: vec![a_png()],
            why: None,
        };
        let Action::Mutate { request, .. } = app.images_loaded(id, loading) else {
            panic!("the words weren't sent once the image was in")
        };
        assert_eq!(request["words"], "Back up the NAS [Image #1]");
        assert_eq!(app.draft, "Fix the dock");
    }

    #[test]
    fn the_same_words_twice_go_once_and_words_given_back_keep_their_submission() {
        // Review of intent 0445: two sends of the same words would share one submission as two
        // jobs; one failing gives its words back while the other saves them, and a resend of
        // those words mustn't make a second intent.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        let submission = first["submission"].clone();
        app.queue_action(0, first.clone());
        // The same words again while the first is on its way: nothing more is sent.
        app.draft = "Back up the NAS".into();
        press(&mut app, 'c');
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert!(app.message().unwrap().0.contains("already being saved"));
        assert_eq!(
            app.resolver
                .unanswered_where(|r| r["action"] == "create")
                .into_iter()
                .count(),
            1
        );
        // Unheard: the words come back with the send they came from...
        reply_to(&mut app, 0, first, Err("timed out".into()));
        assert_eq!(app.draft, "Back up the NAS");
        assert_eq!(
            app.draft_came_back.as_ref().map(|s| s.id.as_str()),
            submission.as_str()
        );
        // ...and keep it even once nothing else remembers that send.
        app.submissions.clear();
        app.mode = Mode::Board;
        press(&mut app, 'c');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(request["submission"], submission);
        // Words given back by a send that's since been saved are never offered again.
        app.queue_action(0, request.clone());
        let from = app.submissions[0].clone();
        app.returned.push_back(Returned {
            words: from.words.clone(),
            images: vec![],
            from: Some(from),
        });
        reply_to(&mut app, 0, request, answer(true, Some("9003")));
        assert!(app.returned.is_empty());
    }

    #[test]
    fn an_image_that_doesn_t_go_in_keeps_its_words_unsent() {
        // Review of intent 0445: the image read while Enter waited didn't fit, or only some of
        // what was read went in: nothing is sent, and the words are kept with why.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        use crate::images::Source;
        let full = "x".repeat(47_995);
        app.draft = full.clone();
        press(&mut app, 'c');
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let loading = crate::images::Loading {
            images: vec![a_png()],
            why: None,
        };
        assert_eq!(app.images_loaded(id, loading), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("no question: {:?}", app.mode)
        };
        assert!(message.contains("The box is full"), "{message}");
        assert_eq!(app.draft, full);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.draft, full);
        // One image in and one that couldn't be read: not sent either.
        app.draft = "The dock".into();
        app.mode = Mode::Board;
        press(&mut app, 'c');
        let (id, _) = app.begin_image_load(&Source::Clipboard).unwrap();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let loading = crate::images::Loading {
            images: vec![a_png()],
            why: Some("photo.heic isn't an image Kerdr can send.".into()),
        };
        assert_eq!(app.images_loaded(id, loading), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("no question: {:?}", app.mode)
        };
        assert!(message.contains("photo.heic"), "{message}");
        assert_eq!(app.draft, "The dock [Image #1] ");
        assert_eq!(app.draft_images.len(), 1);
    }

    #[test]
    fn several_new_intents_save_at_once_and_a_failed_one_waits_its_turn_for_the_box() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        app.queue_action(0, first.clone());
        // c opens the box again at once, empty, while the first is still saving.
        press(&mut app, 'c');
        assert!(matches!(&app.mode, Mode::Compose { draft, .. } if draft.words.is_empty()));
        for c in "Fix the dock".chars() {
            press(&mut app, c);
        }
        let Action::Mutate {
            request: second, ..
        } = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        app.queue_action(0, second.clone());
        assert_ne!(first["submission"], second["submission"]);
        let saving = app
            .board
            .cards
            .iter()
            .filter(|c| c.intake() == Some(crate::resolve::SAVING))
            .count();
        assert_eq!(saving, 2, "a card for each");
        // He starts typing a third; the first fails meanwhile. Its words are kept for a later c,
        // and the failure says so.
        press(&mut app, 'c');
        press(&mut app, 'Z');
        reply_to(&mut app, 0, first, answer(false, None));
        let (said, _) = app.message().unwrap();
        assert!(
            said.contains("c brings them back once the box is empty"),
            "{said}"
        );
        assert!(matches!(&app.mode, Mode::Compose { draft, .. } if draft.words == "Z"));
        // Read, the failure goes; Esc never slips past one that hasn't been.
        app.message = None;
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, 'c');
        assert!(
            matches!(&app.mode, Mode::Compose { draft, .. } if draft.words == "Back up the NAS")
        );
        // The second saves; its card stays until the feed has it.
        reply_to(&mut app, 0, second, answer(true, Some("9005")));
        assert_eq!(app.resolver.unanswered(), 0);
    }

    #[test]
    fn images_past_the_limits_are_refused_by_name() {
        let mut app = demo_app();
        press(&mut app, 'c');
        let image = |n: usize| crate::images::Loaded {
            media_type: "image/png",
            bytes: [b"\x89PNG\r\n\x1a\n".to_vec(), vec![0; n]].concat(),
        };
        let some = |images| crate::images::Loading { images, why: None };
        app.attach_images(some(vec![
            image(9_000_000),
            image(9_000_000),
            image(3_000_000),
        ]));
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(text.contains("past 20 MB together"), "{text}");
        let Mode::Compose { draft, .. } = &app.mode else {
            panic!("the box closed")
        };
        assert_eq!(draft.images.len(), 2);
        app.attach_images(some((0..30).map(|_| image(10)).collect()));
        let Mode::Compose { draft, .. } = &app.mode else {
            panic!("the box closed")
        };
        assert_eq!(draft.images.len(), crate::images::MAX_IMAGES);
        assert!(app.message().unwrap().0.contains("at most 20 images"));
    }

    #[test]
    fn a_failed_save_after_the_box_closed_says_so_and_keeps_the_words() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let request = send_words(&mut app, "Back up the NAS");
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        app.queue_action(0, request.clone());
        reply_to(&mut app, 0, request, answer(false, None));
        settle(&mut app);
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(text.contains("wasn't saved: The store is locked"), "{text}");
        assert!(text.contains("c opens it again with your words"), "{text}");
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        assert!(
            app.board
                .cards
                .iter()
                .all(|c| c.intake() != Some(crate::resolve::SAVING)),
            "its card goes with the failure"
        );
        assert_eq!(app.draft, "Back up the NAS");
        press(&mut app, 'c');
        assert!(
            matches!(&app.mode, Mode::Compose { draft, .. } if draft.words == "Back up the NAS")
        );
        // No answer at all: it may have been saved, and he's told so, words kept.
        let again = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let Action::Mutate { request, .. } = again else {
            panic!("Enter didn't send")
        };
        reply_to(&mut app, 0, request, Err("timed out".into()));
        settle(&mut app);
        let (text, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(
            text.contains("may not have been saved: timed out."),
            "{text}"
        );
        assert_eq!(app.draft, "Back up the NAS");
    }

    #[test]
    fn a_save_that_lands_after_you_moved_on_leaves_you_where_you_are() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let request = send_words(&mut app, "Back up the NAS");
        app.queue_action(0, request.clone());
        app.mode = Mode::Board;
        press(&mut app, '?');
        assert!(matches!(app.mode, Mode::Keys { .. }));
        reply_to(&mut app, 0, request, answer(true, Some("9004")));
        settle(&mut app);
        assert!(
            matches!(app.mode, Mode::Keys { .. }),
            "the answer pulled him into the backlog"
        );
        assert!(app.draft.is_empty());
        // Moved on: told briefly where it is, and nothing pulls him there.
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Info);
        assert!(said.contains("b shows it in the backlog"), "{said}");
    }

    #[test]
    fn a_new_intent_saved_but_not_published_is_never_sent_twice() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.draft = "Make the dock wake the screens".into();
        press(&mut app, 'c');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: false,
                message: "Saved as #9002 on this machine only; it couldn't be pushed.".into(),
                confirmation: false,
                used: None,
                limit: None,
                session: None,
                id: Some("9002".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert!(app.draft.is_empty());
        assert!(!matches!(app.mode, Mode::Compose { .. }));
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Fail));
    }

    #[test]
    fn a_new_intent_saved_with_its_push_to_come_says_so_even_where_its_card_says_the_rest() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let request = send_words(&mut app, "Back up the NAS");
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        app.queue_action(0, request.clone());
        let left = "saved here; it reaches your other machines once the push goes through.";
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: format!("Saved as #9005: {left}"),
                note: Some(left.into()),
                id: Some("9005".into()),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert_eq!(app.message(), Some((left, Tone::Info)));
        // Only part of it: said plainly, not as a failure, and the words aren't sent again.
        let request = send_words(&mut app, "Wake the screens");
        app.queue_action(0, request.clone());
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: false,
                partial: true,
                message: "Saved as #9006, but its write-up couldn't start.".into(),
                id: Some("9006".into()),
                ..Default::default()
            }),
        );
        assert_eq!(app.message().map(|(_, t)| t), Some(Tone::Warn));
        assert!(app.submissions.is_empty());
    }

    fn answer(ok: bool, id: Option<&str>) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok,
            message: if ok { "Saved." } else { "The store is locked" }.into(),
            confirmation: false,
            used: None,
            limit: None,
            session: None,
            id: id.map(str::to_string),
            ..Default::default()
        })
    }

    fn send_words(app: &mut App, words: &str) -> serde_json::Value {
        app.busy = false;
        app.draft = words.into();
        app.mode = Mode::Board;
        press(app, 'c');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        request
    }

    #[test]
    fn a_resend_after_a_lost_answer_names_the_same_submission() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        let id = first["submission"].as_str().unwrap().to_string();
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && id.len() <= 64);
        // No answer (a timeout): it may have been made, so the same words keep the same id,
        // even past a later refusal, which can't say what the first send did.
        reply_to(
            &mut app,
            0,
            first,
            Err("timed out; the result is unknown".into()),
        );
        settle(&mut app);
        let second = send_words(&mut app, "Back up the NAS");
        assert_eq!(second["submission"], id.as_str());
        reply_to(&mut app, 0, second, answer(false, None));
        settle(&mut app);
        let third = send_words(&mut app, "Back up the NAS");
        assert_eq!(third["submission"], id.as_str());
        // The answer names the record: settled, and the next words are new work.
        reply_to(&mut app, 0, third, answer(true, Some("9003")));
        settle(&mut app);
        let fourth = send_words(&mut app, "Back up the NAS");
        assert_ne!(fourth["submission"], id.as_str());
    }

    #[test]
    fn a_retry_after_a_lost_answer_goes_where_the_first_send_went() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        reply_to(&mut app, 0, first.clone(), Err("timed out".into()));
        settle(&mut app);
        // He moves to another session's view before trying again: the retry still names the first
        // send's session and submission, so the adapter finds what it may have made.
        press(&mut app, 'c');
        if let Mode::Compose { draft, .. } = &mut app.mode {
            draft.session = "elsewhere".into();
        }
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("Enter didn't send")
        };
        assert_eq!(request["submission"], first["submission"]);
        assert_eq!(request["session"], first["session"]);
    }

    #[test]
    fn changed_words_after_a_lost_answer_are_new_work() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let first = send_words(&mut app, "Back up the NAS");
        let id = first["submission"].as_str().unwrap().to_string();
        reply_to(&mut app, 0, first, Err("timed out".into()));
        settle(&mut app);
        let edited = send_words(&mut app, "Back up the NAS and the laptop");
        assert_ne!(edited["submission"], id.as_str());
        // A refusal of a send nobody lost settles it.
        let edited_id = edited["submission"].as_str().unwrap().to_string();
        reply_to(&mut app, 0, edited, answer(false, None));
        settle(&mut app);
        let again = send_words(&mut app, "Back up the NAS and the laptop");
        assert_ne!(again["submission"], edited_id.as_str());
    }

    #[test]
    fn an_empty_new_intent_sends_nothing() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'c');
        press(&mut app, ' ');
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        assert!(matches!(app.mode, Mode::Compose { .. }));
    }

    #[test]
    fn the_menu_uses_the_direct_letters_and_n_is_not_new() {
        let mut app = demo_app();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Palette { .. }), "n left the menu");
        press(&mut app, 'c');
        assert!(matches!(app.mode, Mode::Compose { .. }));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 'f');
        assert!(matches!(app.mode, Mode::Filter { .. }));
    }

    #[test]
    fn the_filter_list_picks_an_owner_and_f_shows_everything_again() {
        let mut app = demo_app();
        press(&mut app, 'f');
        let rows = app.filter_rows();
        assert_eq!(rows[0].label, "All work");
        assert_eq!(rows[0].owner, None);
        let pick = rows
            .iter()
            .position(|r| r.owner.is_some() && r.project.is_none())
            .expect("an owner row");
        let owner = rows[pick].owner.clone();
        for _ in 0..pick {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(app.owner, owner);
        assert!(
            app.board
                .cards
                .iter()
                .filter(|c| app.card_visible(c))
                .all(|c| Some(c.owner_key()) == owner)
        );
        // After /, letters narrow the list rather than moving through it.
        press(&mut app, 'f');
        press(&mut app, '/');
        press(&mut app, 'j');
        let Mode::Filter { query, typing, .. } = &app.mode else {
            panic!("not in the filter list")
        };
        assert_eq!((query.as_str(), *typing), ("j", true));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let Mode::Filter { query, typing, .. } = &app.mode else {
            panic!("Esc while searching closed the filter list")
        };
        assert_eq!((query.as_str(), *typing), ("", false));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(app.owner, owner, "Esc changed the filter");
        app.provider = Some("codex".into());
        press(&mut app, 'F');
        assert_eq!(
            (app.owner.clone(), app.project.clone(), app.provider.clone()),
            (None, None, None)
        );
    }

    #[test]
    fn b_in_the_filter_list_cycles_background_work_and_f_clears_it() {
        let mut app = demo_app();
        let shown = |app: &App| -> Vec<bool> {
            app.board
                .cards
                .iter()
                .filter(|c| c.stage != Stage::Misc && app.card_visible(c))
                .map(|c| c.background)
                .collect()
        };
        let everything = shown(&app);
        assert!(everything.contains(&true) && everything.contains(&false));
        press(&mut app, 'f');
        press(&mut app, 'b');
        let Mode::Filter { query, .. } = &app.mode else {
            panic!("b left the filter list")
        };
        assert!(query.is_empty(), "b went into the search");
        assert_eq!(app.background, Background::Only);
        let only = shown(&app);
        assert!(!only.is_empty() && only.iter().all(|b| *b));
        // The list's counts follow it, so they say what picking a row would show.
        assert_eq!(app.filter_rows()[0].open + app.filter_rows()[0].backlog, {
            app.board
                .cards
                .iter()
                .filter(|c| {
                    c.background
                        && !c.dismissed()
                        && !matches!(c.stage, Stage::Maintain | Stage::Misc)
                })
                .count()
        });
        press(&mut app, 'b');
        assert_eq!(app.background, Background::Hide);
        let none = shown(&app);
        assert!(!none.is_empty() && none.iter().all(|b| !*b));
        assert!(
            app.needs_you_order()
                .iter()
                .all(|k| !app.card(k).unwrap().background)
        );
        press(&mut app, 'b');
        assert_eq!(app.background, Background::All);
        assert_eq!(shown(&app), everything);
        // While searching, b is a letter of the search.
        press(&mut app, '/');
        press(&mut app, 'b');
        let Mode::Filter { query, .. } = &app.mode else {
            panic!("left the filter list")
        };
        assert_eq!(query, "b");
        assert_eq!(app.background, Background::All);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        press(&mut app, 'b');
        press(&mut app, 'f');
        assert!(
            matches!(app.mode, Mode::Board),
            "f didn't close the filter list"
        );
        assert_eq!(app.background, Background::Only);
        press(&mut app, 'F');
        assert_eq!(app.background, Background::All);
    }

    #[test]
    fn closing_the_filters_after_b_leaves_the_backlog_on_a_card_it_shows() {
        let mut app = demo_app();
        app.enter_backlog(false);
        let len = app.backlog_cards().len();
        assert!(len > 2, "the demo's backlog is too short for this test");
        for _ in 0..len {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert!(matches!(app.mode, Mode::Backlog { sel } if sel == len - 1));
        for close in [KeyCode::Esc, KeyCode::Char('f')] {
            press(&mut app, 'f');
            press(&mut app, 'b');
            app.on_key(KeyEvent::new(close, KeyModifiers::NONE));
            let Mode::Backlog { sel } = app.mode else {
                panic!("closing the filters left the backlog")
            };
            assert!(
                sel < app.backlog_cards().len().max(1),
                "row {sel} of {}",
                app.backlog_cards().len()
            );
        }
    }

    #[test]
    fn search_leaves_out_workspaces_and_sessions_the_background_filter_hides() {
        let mut app = demo_app();
        let everything = |app: &App, filter| {
            app.search_hits(&Search {
                query: String::new(),
                filter,
                sel: 0,
            })
            .len()
        };
        for filter in [SearchFilter::Workspaces, SearchFilter::Sessions] {
            let all = everything(&app, filter);
            app.background = Background::Only;
            let only = everything(&app, filter);
            app.background = Background::Hide;
            let hide = everything(&app, filter);
            app.background = Background::All;
            assert!(only < all && hide < all, "{filter:?}: {all} {only} {hide}");
        }
    }

    #[test]
    fn a_new_intent_is_selected_once_its_card_arrives() {
        let mut app = demo_app();
        let (key, id) = waiting_backlog_card(&app);
        let machine = app.card(&key).unwrap().machine_index;
        app.enter_backlog(false);
        // Another store's record with the same id is someone else's.
        if let Some(f) = app.machines[machine + 1].feed.as_mut() {
            f.source = Some("elsewhere".into());
        }
        app.new_intent_id = Some((machine + 1, id.clone()));
        app.rebuild();
        assert!(app.new_intent_id.is_some());
        // Another machine's copy of the same store is the same record (intent 0445).
        let same = app.machines[machine].feed.as_ref().unwrap().source.clone();
        if let Some(f) = app.machines[machine + 1].feed.as_mut() {
            f.source = same;
        } else {
            app.new_intent_id = Some((machine, id.clone()));
        }
        app.rebuild();
        assert!(app.new_intent_id.is_none());
        app.new_intent_id = Some((machine, id));
        app.rebuild();
        assert!(app.new_intent_id.is_none());
        let Mode::Backlog { sel } = app.mode else {
            panic!("not in the backlog")
        };
        assert_eq!(app.board.cards[app.backlog_cards()[sel]].key, key);
    }

    #[test]
    fn a_menu_opened_on_a_card_never_acts_on_another_after_a_refresh() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.mode = Mode::Backlog { sel: 0 };
        let first = app.backlog_cards()[0];
        let id = app.board.cards[first].detail.as_ref().unwrap().id.clone();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        // The card it opened on starts while the menu is open, and leaves the backlog.
        for machine in &mut app.machines {
            if let Some(feed) = &mut machine.feed {
                for card in &mut feed.cards {
                    if card.id == id {
                        card.stage = Some("build".into());
                        card.state = Some("active".into());
                    }
                }
            }
        }
        app.rebuild();
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
    }

    #[test]
    fn card_keys_refuse_finished_work_whatever_its_state_says() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].state = None;
        app.sel = Selection {
            col: Stage::Maintain.index(),
            key: Some(key),
        };
        assert!(app.backlog_card_for_keys().is_none());
        for c in ['y', 'x', 't', 'B', 'J', 'u'] {
            assert_eq!(
                press(&mut app, c),
                Action::None,
                "{c} acted on finished work"
            );
        }
        // d does what ⏎ does: a reopen, whose backend always asks first for done work (intent
        // 0591, K1-25).
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert!(request.get("override").is_none());
    }

    #[test]
    fn deleting_a_word_never_splits_a_character() {
        let mut app = demo_app();
        for (words, left) in [
            ("one\u{2003}two", "one\u{2003}"),
            ("one\u{a0}two", "one\u{a0}"),
            ("ünïcödé wörds  ", "ünïcödé "),
            ("lone", ""),
        ] {
            app.draft = words.into();
            app.mode = Mode::Board;
            press(&mut app, 'c');
            app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
            let Mode::Compose { draft, .. } = &app.mode else {
                panic!("left the box")
            };
            assert_eq!(draft.words, left, "{words:?}");
        }
    }

    #[test]
    fn the_filter_list_applies_the_row_it_highlights() {
        let mut app = demo_app();
        press(&mut app, 'f');
        let rows = app.filter_rows();
        app.mode = Mode::Filter {
            query: String::new(),
            sel: rows.len() + 5,
            typing: false,
            back: Box::new(Mode::Board),
        };
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Mode::Filter { sel, .. } = app.mode else {
            panic!("left the list")
        };
        let shown = app.filter_matches("");
        assert_eq!(sel, shown.len() - 1);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.owner, shown[sel].owner);
        assert_eq!(app.project, shown[sel].project);
    }

    #[test]
    fn a_paste_reaches_the_keys_list_search() {
        let mut app = demo_app();
        app.mode = Mode::Keys {
            filter: String::new(),
            typing: true,
            scroll: 7,
        };
        app.on_paste("machine\nnow");
        let Mode::Keys { filter, scroll, .. } = &app.mode else {
            panic!("left the keys list")
        };
        assert_eq!((filter.as_str(), *scroll), ("machine now", 0));
    }

    #[test]
    fn a_menu_opened_on_no_card_acts_on_none_that_arrives_later() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.sel = Selection::default();
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Palette { target: None, .. }));
        // A card turns up selected while the menu is open.
        let (key, _) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key),
        };
        assert_eq!(press(&mut app, 'd'), Action::None);
    }

    #[test]
    fn a_confirmation_rechecks_its_intent_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        // It starts elsewhere while the question is open.
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].state = Some("active".into());
        // The yes isn't sent: x on the card as it is now asks again at once, saying what changed
        // (intent 0591, K1-38).
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        );
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("not asked again: {:?}", app.message())
        };
        assert!(message.contains("started meanwhile"), "{message}");
        press_code(&mut app, KeyCode::Esc);
        // Still waiting: yes sends it.
        app.board.cards[i].stage = Stage::Backlog;
        app.board.cards[i].state = Some("queued".into());
        press(&mut app, 'x');
        let Action::Mutate { request, .. } =
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (request["action"].as_str(), request["id"].as_str()),
            (Some("dismiss"), Some(id.as_str()))
        );
    }

    /// A demo card for work under way whose workspace has closed: in Build, with a record, no
    /// workspace, homed in session `web` on the first machine.
    fn closed_workspace_card(app: &mut App) -> (String, String) {
        let i = app
            .board
            .cards
            .iter()
            .position(|c| c.detail.is_some() && c.stage == Stage::Build)
            .expect("a Build card in the demo");
        let machine = app.machines[0].name();
        let card = &mut app.board.cards[i];
        card.workspace_id = None;
        card.session = None;
        card.attention = Attention::Idle;
        card.machine_index = 0;
        let d = card.detail.as_mut().unwrap();
        d.disposition = None;
        d.intake = None;
        d.home = Some(crate::feed::Home {
            machine: Some(machine),
            session: Some("web".into()),
        });
        d.resume = Some(crate::feed::Resume {
            agent: Some("codex".into()),
            conversation: Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41".into()),
        });
        let (key, id) = (card.key.clone(), d.id.clone());
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key.clone()),
        };
        (key, id)
    }

    fn enter(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn enter_reopens_work_whose_home_names_only_a_machine_there() {
        // A record with no home, its conversation on another machine: that machine's feed names
        // it, with no session, and Enter sends the reopen there, not to the card's own machine,
        // leaving the session to the backend.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let other = app.machines[1].name();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(other),
            session: None,
        });
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter didn't ask to reopen: {:?}", app.message())
        };
        assert_eq!(machine, 1);
        assert_eq!(request, serde_json::json!({"action": "reopen", "id": id}));
    }

    fn esc(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    /// Enter on the card, as you would: the request it sends, with Kerdr remembering the card.
    fn reopen_by_enter(app: &mut App) -> serde_json::Value {
        let Action::Mutate { machine, request } = enter(app) else {
            panic!("Enter didn't ask to reopen: {:?}", app.message())
        };
        send(app, machine, request.clone());
        request
    }

    fn reopen_reply(
        session: &str,
        ok: bool,
        confirmation: bool,
        token: &str,
    ) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok,
            message: if confirmation {
                format!("{session} has 3 workspaces, its usual limit. Reopen it anyway?")
            } else {
                "Reopened, resuming its Codex conversation.".into()
            },
            confirmation,
            used: confirmation.then_some(3),
            limit: confirmation.then_some(3),
            session: Some(session.into()),
            workspace: (ok && !confirmation).then(|| "w7".into()),
            label: (ok && !confirmation).then(|| "41-checkout-expired-cards".into()),
            confirm: confirmation.then(|| token.into()),
            ..Default::default()
        })
    }

    #[test]
    fn enter_on_work_whose_workspace_closed_asks_its_home_to_reopen_it() {
        let mut app = demo_app();
        let (key, id) = closed_workspace_card(&mut app);
        assert!(app.card(&key).unwrap().can_reopen());
        assert_eq!(app.card(&key).unwrap().resume_agent(), Some("codex"));
        // Without actions there's no one to ask: it says how to get there, and sends nothing.
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("[actions]"));
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter didn't ask to reopen")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], id.as_str());
        assert_eq!(request["session"], "web");
        assert!(request.get("override").is_none());
        assert_eq!(
            app.action_from.as_ref().map(|f| f.key.as_str()),
            Some(key.as_str())
        );
        assert_eq!(
            app.working_on(&request),
            format!("Reopening {}…", app.card(&key).unwrap().reference)
        );
        // From a peek too, which closes on the way.
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        assert!(matches!(enter(&mut app), Action::Mutate { .. }));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn enter_on_waiting_or_unrecorded_work_still_reopens_nothing() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        for (stage, attention) in [
            (Stage::Backlog, Attention::Idle),
            (Stage::Build, Attention::Done),
        ] {
            app.board.cards[i].stage = stage;
            app.board.cards[i].attention = attention;
            assert!(!app.board.cards[i].reopenable());
        }
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].attention = Attention::Idle;
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        assert!(!app.board.cards[i].can_reopen());
        app.board.cards[i].detail.as_mut().unwrap().intake = None;
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(!app.board.cards[i].can_reopen());
        app.board.cards[i].detail = None;
        assert!(!app.board.cards[i].can_reopen());
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key),
        };
        assert_eq!(enter(&mut app), Action::None);
        // A workspace that closed with no record of its conversation: said plainly (intent 0591).
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("nothing recorded its conversation"),
            "{:?}",
            app.message()
        );
    }

    /// Intent 0289: the Wi-Fi outage (0117)'s case, work its agent left open with no workspace.
    /// C asks, then marks it done; nothing reopens or starts.
    #[test]
    fn c_marks_open_work_with_no_workspace_done_after_asking() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'C'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("C didn't ask first")
        };
        assert_eq!(title, &format!("Mark {reference} done?"));
        assert!(message.contains("nothing started"), "{message}");
        // No is no.
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Board));
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(machine, 0);
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
        // Backlog work that turns out to be done already closes the same way.
        let (key, id) = waiting_backlog_card(&app);
        app.busy = false;
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key),
        };
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes on a backlog card sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
    }

    #[test]
    fn x_sets_aside_started_work_with_no_workspace_after_asking() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'x'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("x didn't ask first")
        };
        assert_eq!(title, &format!("Won't do {reference}?"));
        assert!(
            message.contains("Its record keeps where it was"),
            "{message}"
        );
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "dismiss", "id": id}));
    }

    #[test]
    fn c_and_x_refuse_work_with_a_workspace_done_work_and_won_t_do() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // Its workspace is open: an agent may be at it.
        app.board.cards[i].workspace_id = Some("w9".into());
        app.board.cards[i].session = Some("web".into());
        for c in ['C', 'x'] {
            assert_eq!(press(&mut app, c), Action::None);
            assert!(matches!(app.mode, Mode::Board));
            assert!(
                app.message().unwrap().0.contains("has a workspace open"),
                "{:?}",
                app.message()
            );
        }
        // Done.
        app.board.cards[i].workspace_id = None;
        app.board.cards[i].session = None;
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].attention = Attention::Done;
        app.sel.col = Stage::Maintain.index();
        press(&mut app, 'C');
        assert!(app.message().unwrap().0.ends_with("is done already."));
        press(&mut app, 'x');
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("won't do is for work that isn't")
        );
        // In won't do: brought back first.
        app.board.cards[i].stage = Stage::Build;
        app.board.cards[i].attention = Attention::Idle;
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        app.wont_do = true;
        app.mode = Mode::Backlog { sel: 0 };
        let sel = app.backlog_cards().iter().position(|&j| j == i).unwrap();
        app.mode = Mode::Backlog { sel };
        press(&mut app, 'C');
        assert!(app.message().unwrap().0.contains("u brings it back"));
        assert!(matches!(app.mode, Mode::Backlog { .. }));
    }

    #[test]
    fn d_on_started_work_with_no_workspace_reopens_it_as_enter_does() {
        // Intent 0591, K1-24: d on work under way does what ⏎ does, never "already under way".
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (_, id) = closed_workspace_card(&mut app);
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
    }

    #[test]
    fn a_close_or_won_t_do_rechecks_the_card_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // A workspace opens on it while the question is up: its agent is back.
        for c in ['C', 'x'] {
            press(&mut app, c);
            settle(&mut app);
            app.board.cards[i].workspace_id = Some("w9".into());
            app.board.cards[i].session = Some("web".into());
            assert_eq!(enter(&mut app), Action::None);
            assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Warn));
            app.board.cards[i].workspace_id = None;
            app.board.cards[i].session = None;
        }
        // Won't do asked about backlog work whose home moved meanwhile: not the work asked about.
        let (bkey, _) = waiting_backlog_card(&app);
        let b = app.board.cards.iter().position(|c| c.key == bkey).unwrap();
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(bkey),
        };
        press(&mut app, 'x');
        settle(&mut app);
        app.board.cards[b].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("elsewhere".into()),
        });
        // Its yes isn't sent: the question is asked again at once, for the card as it is now, led
        // by what changed (intent 0591, K1-38).
        assert_eq!(enter(&mut app), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("not asked again: {:?}", app.message())
        };
        assert!(message.contains("moved to"), "{message}");
        press_code(&mut app, KeyCode::Esc);
        app.sel = Selection {
            col: Stage::Build.index(),
            key: Some(key),
        };
        // Won't do asked about started work, which went back to the backlog meanwhile.
        press(&mut app, 'x');
        settle(&mut app);
        app.board.cards[i].stage = Stage::Backlog;
        app.board.cards[i].state = Some("queued".into());
        assert_eq!(enter(&mut app), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("not asked again: {:?}", app.message())
        };
        assert!(message.contains("went back to the backlog"), "{message}");
    }

    #[test]
    fn the_menu_offers_c_and_x_for_started_work_with_no_workspace() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        assert!(app.backlog_card_for_keys().is_none());
        assert_eq!(
            app.card_for_keys().map(|c| c.key.clone()),
            Some(key.clone())
        );
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(&app.mode, Mode::Palette { target: Some(t), .. } if *t == key));
        press(&mut app, 'C');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the menu's C sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
    }

    /// Work an agent is at (intent 0321): the demo's Build card, its workspace 07-build open in
    /// web on the first machine, a Claude agent mid-step in it.
    fn running_card(app: &mut App) -> (String, String) {
        let (key, id) = closed_workspace_card(app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        let card = &mut app.board.cards[i];
        card.workspace_id = Some("w7".into());
        card.workspace_label = Some("07-build".into());
        card.session = Some("web".into());
        card.attention = Attention::Running;
        card.agents = vec![crate::model::AgentInfo {
            agent: "claude".into(),
            status: "working".into(),
            pane_id: "w7:p1".into(),
            ..Default::default()
        }];
        (key, id)
    }

    #[test]
    fn x_upper_cancels_work_an_agent_is_at_after_saying_what_it_stops() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'X'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("X didn't ask first: {:?}", app.message())
        };
        assert_eq!(title, &format!("Cancel {reference}?"));
        assert!(
            message.starts_with("Stops its Claude Code agent (mid-step) and closes workspace 07-build (web), with every tab in it, then sets"),
            "{message}"
        );
        assert!(message.contains("aside in Won't do"), "{message}");
        assert!(message.contains("u brings it back"), "{message}");
        // No is no.
        press(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Board));
        press(&mut app, 'X');
        settle(&mut app);
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(machine, 0);
        assert_eq!(
            request,
            serde_json::json!({"action": "cancel", "id": id, "session": "web", "workspace": "w7"})
        );
        assert!(app.working_on(&request).contains("stopping its agent"));
    }

    #[test]
    fn x_upper_on_a_child_closes_only_its_tab_and_a_parent_names_its_children() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].tab_id = Some("w7:t2".into());
        press(&mut app, 'X');
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("X didn't ask first")
        };
        assert!(
            message.contains(
                "closes its tab in 07-build (web), leaving the rest of that workspace running"
            ),
            "{message}"
        );
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes sent nothing")
        };
        assert_eq!(request["tab"], "w7:t2");
        assert_eq!(request["id"], serde_json::json!(id));
        // The parent: another card, the whole workspace, its child named.
        app.busy = false;
        let child = app.board.cards[i].reference.clone();
        let p = app
            .board
            .cards
            .iter()
            .position(|c| {
                c.key != key
                    && c.detail.is_some()
                    && !matches!(c.stage, Stage::Maintain | Stage::Misc | Stage::Backlog)
            })
            .unwrap();
        let card = &mut app.board.cards[p];
        card.workspace_id = Some("w7".into());
        card.workspace_label = Some("07-build".into());
        card.session = Some("web".into());
        card.machine_index = 0;
        card.tab_id = None;
        card.attention = Attention::Idle;
        card.agents.clear();
        card.state = Some("active".into());
        let d = card.detail.as_mut().unwrap();
        d.disposition = None;
        d.intake = None;
        app.sel = Selection {
            col: app.board.cards[p].stage.index(),
            key: Some(app.board.cards[p].key.clone()),
        };
        press(&mut app, 'X');
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("X on the parent didn't ask: {:?}", app.message())
        };
        assert!(
            message.starts_with("Stops whatever runs there"),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "{child} in a tab there stops too and stays open with no agent"
            )),
            "{message}"
        );
    }

    #[test]
    fn x_upper_rechecks_where_the_work_runs_before_sending() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = running_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        press(&mut app, 'X');
        settle(&mut app);
        // Its workspace closed and another opened on it while the question was up: the yes stops
        // nothing, and the question comes back at once about where it runs now (intent 0591).
        app.board.cards[i].workspace_id = Some("w9".into());
        assert_eq!(enter(&mut app), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("not asked again: {:?}", app.message())
        };
        assert!(message.contains("now runs in"), "{message}");
        // Its yes is for the card as it is now.
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the new yes sent nothing")
        };
        assert_eq!(request["action"], "cancel");
    }

    #[test]
    fn x_upper_refuses_work_no_agent_is_at_and_done_work() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        // Nothing to stop: X's end is Won't do, so it asks to set it aside (intent 0591, K1-30).
        assert_eq!(press(&mut app, 'X'), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("X asked nothing: {:?}", app.message())
        };
        assert!(message.contains("Nothing is running for"), "{message}");
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the yes sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "dismiss", "id": id}));
        app.resolver = Default::default();
        assert!(matches!(app.mode, Mode::Board));
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w7".into());
        app.board.cards[i].session = Some("web".into());
        app.board.cards[i].stage = Stage::Maintain;
        app.board.cards[i].attention = Attention::Done;
        app.sel.col = Stage::Maintain.index();
        press(&mut app, 'X');
        assert!(app.message().unwrap().0.contains("done already"));
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn the_menu_and_the_bottom_line_offer_x_and_s_for_work_an_agent_is_at() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        assert_eq!(
            app.card_for_keys().map(|c| c.key.clone()),
            Some(key.clone())
        );
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(&app.mode, Mode::Palette { target: Some(t), .. } if *t == key));
        press(&mut app, 'X');
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the menu's X sent nothing")
        };
        assert_eq!(request["action"], "cancel");
        assert_eq!(request["id"], serde_json::json!(id));
        // The menu's s is the board's: it moves this card's own agent, by its pane (from review).
        app.busy = false;
        app.mode = Mode::Board;
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        press(&mut app, 's');
        let Mode::Accounts {
            screen: AccountScreen::Move { target, .. },
            ..
        } = &app.mode
        else {
            panic!("the menu's s didn't open the move: {:?}", app.message())
        };
        assert_eq!(target.key, key);
        assert_eq!(target.agent.pane, "w7:p1");
        assert_eq!(target.agent.session, "web");
    }

    /// A done card (intent 0278): the same work, in Maintain, its workspace gone with its wrap.
    fn done_card(app: &mut App) -> (String, String) {
        let (key, id) = closed_workspace_card(app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.stage = Stage::Maintain;
        card.attention = Attention::Done;
        app.sel = Selection {
            col: Stage::Maintain.index(),
            key: Some(key.clone()),
        };
        (key, id)
    }

    fn done_question(reference: &str, token: &str) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok: false,
            message: format!(
                "{reference} is done: a yes sets it open again and resumes its Codex conversation. Reopen it?"
            ),
            confirmation: true,
            session: Some("web".into()),
            confirm: Some(token.into()),
            ..Default::default()
        })
    }

    #[test]
    fn enter_on_a_done_card_says_it_is_asking_and_claims_no_reopening_until_the_yes() {
        // The backend always asks about done work, so "Reopening…" before that is false (intent
        // 0491).
        let mut app = demo_app();
        let (key, id) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter on a done card didn't ask to reopen it")
        };
        app.queue_action(machine, request.clone());
        let asking = format!("Asking about {reference}…");
        assert_eq!(app.message().unwrap().0, asking);
        let card = app.card(&key).unwrap();
        assert!(card.done(), "it still reads done");
        assert_ne!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("Reopening…")
        );
        let line = app.starting_line(&card.clone()).unwrap();
        assert!(line.contains("Asking first"), "{line}");
        // Enter again takes no second one, and doesn't claim anything either.
        assert_eq!(enter(&mut app), Action::None);
        let said = app.message().unwrap().0.to_string();
        assert!(said.contains("still asking about"), "{said}");
        assert_eq!(app.resolver.on_its_way().count(), 1);
        // The question comes: what it said goes, in either wording, and a yes shows as it always
        // has.
        reply_to(
            &mut app,
            machine,
            request,
            done_question(&reference, "0123456789ab0008"),
        );
        assert_ne!(app.message().map(|m| m.0), Some(said.as_str()));
        assert_ne!(app.message().map(|m| m.0), Some(asking.as_str()));
        settle(&mut app);
        let Action::Mutate { machine, request } = press(&mut app, 'y') else {
            panic!("the yes sent nothing")
        };
        assert_eq!(request["override"], true);
        assert_eq!(request["id"], id.as_str());
        app.queue_action(machine, request);
        let card = app.card(&key).unwrap();
        assert_eq!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("Reopening…")
        );
    }

    #[test]
    fn a_start_on_a_full_session_says_it_is_asking_and_one_with_room_says_starting() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let session = app
            .card(&key)
            .unwrap()
            .detail
            .as_ref()
            .unwrap()
            .home
            .clone()
            .unwrap()
            .session;
        assert_eq!(
            session.as_deref(),
            Some("web"),
            "a full session in the demo"
        );
        let reference = app.card(&key).unwrap().reference.clone();
        let start = serde_json::json!({"action": "dispatch", "id": id, "session": "web"});
        // Full: a question is all that can come.
        assert_eq!(app.effect_for(&key, &start), None);
        // The yes, or a session with room, or the background lane, starts it for real.
        let mut yes = start.clone();
        yes["override"] = serde_json::json!(true);
        assert_eq!(
            app.effect_for(&key, &yes),
            Some(crate::resolve::Effect::Starting)
        );
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().lane = Some("background".into());
        assert_eq!(
            app.effect_for(&key, &start),
            Some(crate::resolve::Effect::Starting)
        );
        app.board.cards[i].detail.as_mut().unwrap().lane = None;
        assert_eq!(app.effect_for(&key, &start), None);
        let mut demo = demo_app();
        room_in_every_session(&mut demo);
        assert_eq!(
            demo.effect_for(&key, &start),
            Some(crate::resolve::Effect::Starting)
        );
        // As it goes: words on the board, and nothing on the card.
        app.queue_action(machine, start.clone());
        assert_eq!(
            app.message().unwrap().0,
            format!("Asking about {reference}…")
        );
        assert_ne!(
            app.card(&key)
                .unwrap()
                .detail
                .as_ref()
                .unwrap()
                .status
                .as_deref(),
            Some("Starting…")
        );
        // A slot came free since the feed was read: it started after all, and shows it.
        let started = crate::actions::Response {
            ok: true,
            message: "Started.".into(),
            ..Default::default()
        };
        reply_to(&mut app, machine, start, Ok(started));
        assert_eq!(app.message().map(|m| m.0), None);
        assert_eq!(
            app.card(&key)
                .unwrap()
                .detail
                .as_ref()
                .unwrap()
                .status
                .as_deref(),
            Some("Starting…")
        );
        assert_eq!(app.card(&key).unwrap().state.as_deref(), Some("active"));
        // Its line and ⏎ agree with that, not with what it was guessed it would do.
        let card = app.card(&key).cloned().unwrap();
        let line = app.starting_line(&card).unwrap();
        assert!(line.contains("Starting up"), "{line}");
        select_key(&mut app, &key);
        assert_eq!(enter(&mut app), Action::None);
        let said = app.message().unwrap().0;
        assert!(!said.contains("asking"), "{said}");
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn w_says_it_is_queuing_until_the_answer_comes() {
        let mut app = demo_app();
        let (key, id) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let request = serde_json::json!({"action": "reopen", "id": id, "queue": true});
        app.queue_action(0, request.clone());
        assert_eq!(app.message().unwrap().0, format!("Queuing {reference}…"));
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: "Queued in web.".into(),
                ..Default::default()
            }),
        );
        assert_eq!(app.message().unwrap().0, "Queued in web.");
    }

    #[test]
    fn enter_on_a_done_card_asks_first_then_reopens_it_on_a_yes() {
        let mut app = demo_app();
        let (key, id) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let card = app.card(&key).unwrap();
        assert!(card.can_reopen_done() && card.reopenable() && !card.can_reopen());
        // Without actions there's no one to ask: it says how to get there, and sends nothing.
        assert_eq!(enter(&mut app), Action::None);
        let said = app.message().unwrap().0.to_string();
        assert!(
            said.contains("is done") && said.contains("[actions]"),
            "{said}"
        );
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter on a done card didn't ask to reopen it")
        };
        assert_eq!(machine, 0);
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"], id.as_str());
        assert!(
            request.get("override").is_none(),
            "the first ask is never a yes"
        );
        // The backend asks first, saying it's done: that's a question on screen, not a failure.
        send(&mut app, 0, request.clone());
        reply_to(
            &mut app,
            0,
            request,
            done_question(&reference, "0123456789ab0008"),
        );
        let Mode::Confirm {
            title,
            message,
            queue,
            ..
        } = &app.mode
        else {
            panic!("a done card's reopen didn't ask")
        };
        assert_eq!(title, &format!("Reopen {reference} in web?"));
        assert!(message.starts_with(&format!("{reference} is done: a yes sets it open again")));
        assert!(!*queue);
        // No: nothing is sent, and the board is as it was.
        settle(&mut app);
        assert_eq!(press(&mut app, 'n'), Action::None);
        assert!(matches!(app.mode, Mode::Board));
        assert!(app.card(&key).unwrap().can_reopen_done());
        // Yes, to the question asked again: the yes carries its token.
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            done_question(&reference, "0123456789ab0008"),
        );
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (
                sent["override"].as_bool(),
                sent["confirm"].as_str(),
                sent["session"].as_str()
            ),
            (Some(true), Some("0123456789ab0008"), Some("web"))
        );
        // Reopened: Kerdr goes in, while you're still on its card.
        send(&mut app, 0, sent.clone());
        reply_to(&mut app, 0, sent, reopen_reply("web", true, false, ""));
        assert_eq!(
            app.open_after.as_ref().map(|o| o.workspace.as_str()),
            Some("w7")
        );
    }

    #[test]
    fn enter_on_a_done_card_is_refused_plainly_where_it_cant_reopen() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // Won't do: said by Kerdr itself, and nothing sent.
        app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        assert!(!app.board.cards[i].reopenable());
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(
            app.message().unwrap().0,
            format!("{reference} is in won't do; u brings it back.")
        );
        app.board.cards[i].detail.as_mut().unwrap().disposition = None;
        // Still being written up: nothing to reopen.
        app.board.cards[i].detail.as_mut().unwrap().intake = Some("pending".into());
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message().unwrap().0.contains("still being written up"),
            "{:?}",
            app.message()
        );
        app.board.cards[i].detail.as_mut().unwrap().intake = None;
        // Homed on a machine this Kerdr doesn't know: said, and nothing sent.
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some("elsewhere".into()),
            session: Some("web".into()),
        });
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("home machine elsewhere is not configured")
        );
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("web".into()),
        });
        // What only its home can tell (another account's, another machine's, one that never ran
        // there): its refusal is said as it comes, with no question.
        for refusal in [
            format!("{reference} belongs to another account; reopen it there."),
            format!("{reference} belongs on lab; reopen it there."),
            format!(
                "{reference} is done, with no record of running on this machine (no home session, no conversation found), so there's nothing to reopen. m peeks at it."
            ),
        ] {
            let request = reopen_by_enter(&mut app);
            reply_to(
                &mut app,
                0,
                request,
                Ok(crate::actions::Response {
                    ok: false,
                    message: refusal.clone(),
                    ..Default::default()
                }),
            );
            assert!(matches!(app.mode, Mode::Board), "a refusal asked something");
            let (said, tone) = last_said(&app).unwrap();
            assert_eq!(said, format!("{reference} didn't reopen: {refusal}"));
            assert_eq!(tone, Tone::Fail);
            assert_eq!(app.open_after, None);
        }
    }

    #[test]
    fn a_yes_to_reopen_a_done_card_goes_in_once_it_has_a_workspace_again() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            done_question(&reference, "0123456789ab0008"),
        );
        settle(&mut app);
        // Opened meanwhile, from somewhere else: the yes reopens nothing, and Enter on the card
        // as it is now goes in, saying why (intent 0591).
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.workspace_id = Some("w9".into());
        card.session = Some("web".into());
        assert!(matches!(press(&mut app, 'y'), Action::Jump(k) if k == key));
        let said = app.message().unwrap().0;
        assert!(
            said.contains("now runs in") && said.contains("going in"),
            "{said}"
        );
    }

    #[test]
    fn a_done_cards_workspace_closing_as_you_go_in_asks_to_reopen_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.workspace_id = Some("w9".into());
        card.session = Some("web".into());
        // Its wrap closed the pane just as you went in.
        assert!(app.closed_under_you(&key, Some("w9")));
        assert_eq!(app.pending_reopen.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn a_reopen_question_and_its_yes_are_for_what_was_asked() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        // The backend asks about another session than the request named: the yes is for that one.
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("api", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let Mode::Confirm { title, queue, .. } = &app.mode else {
            panic!("no question")
        };
        assert_eq!(
            title,
            &format!("Reopen {} in api?", app.card(&key).unwrap().reference)
        );
        assert!(*queue, "a full session's question offers the queue");
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (
                sent["override"].as_bool(),
                sent["session"].as_str(),
                sent["confirm"].as_str()
            ),
            (Some(true), Some("api"), Some("0123456789ab0004"))
        );
        // Something changed since: the backend asks again, with a new token, and that's a new
        // question for a new yes, not a failure.
        app.busy = true;
        reply_to(
            &mut app,
            0,
            sent,
            reopen_reply("api", false, true, "0123456789ab0005"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        let Action::Mutate { request: again, .. } = press(&mut app, 'y') else {
            panic!("the second yes sent nothing")
        };
        assert_eq!(again["confirm"], "0123456789ab0005");
        // The same answer to the yes, the same token: no loop of questions.
        app.busy = true;
        reply_to(
            &mut app,
            0,
            again,
            reopen_reply("api", false, true, "0123456789ab0005"),
        );
        settle(&mut app);
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
    }

    #[test]
    fn a_full_session_can_queue_the_reopen_instead() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'w') else {
            panic!("w sent nothing")
        };
        assert_eq!(
            (sent["queue"].as_bool(), sent.get("override")),
            (Some(true), None)
        );
        // The token goes with it: queueing done work is a yes to what was asked, too.
        assert_eq!(sent["confirm"], "0123456789ab0004");
        reply_to(
            &mut app,
            0,
            sent,
            Ok(crate::actions::Response {
                ok: true,
                message: "Queued in web.".into(),
                ..Default::default()
            }),
        );
        settle(&mut app);
        assert_eq!(app.open_after, None);
    }

    #[test]
    fn a_yes_asks_again_when_the_work_changed_since_it_was_asked_for() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // The feed names another conversation while the request runs: no question at all.
        let request = reopen_by_enter(&mut app);
        app.board.cards[i]
            .detail
            .as_mut()
            .unwrap()
            .resume
            .as_mut()
            .unwrap()
            .conversation = Some("9a8b7c6d-0000-4000-8000-000000000002".into());
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(!matches!(app.mode, Mode::Confirm { .. }));
        assert!(app.waiting_on(&key).is_none());
        let said = app.message().unwrap();
        assert_eq!(said.1, Tone::Warn);
        assert!(said.0.contains("conversation changed"), "{said:?}");
        seen(&mut app);
        // Its agent changes while the question is up: the yes overrides nothing, and Enter on
        // the card as it is now sends its reopen afresh, for the backend to ask again (0591).
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        app.board.cards[i]
            .detail
            .as_mut()
            .unwrap()
            .resume
            .as_mut()
            .unwrap()
            .agent = Some("claude".into());
        let Action::Mutate { request: again, .. } = press(&mut app, 'y') else {
            panic!("not asked again: {:?}", app.message())
        };
        assert_eq!(again["action"], "reopen");
        assert!(again.get("override").is_none() && again.get("confirm").is_none());
        let said = app.message().unwrap().0;
        assert!(
            said.contains("conversation changed") && said.contains("asking again"),
            "{said}"
        );
        // It gets a workspace of its own while the question is up: the yes reopens nothing, and
        // goes in instead, as Enter on it would now.
        let (key, _) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w9".into());
        app.board.cards[i].session = Some("web".into());
        assert!(matches!(press(&mut app, 'y'), Action::Jump(k) if k == key));
    }

    #[test]
    fn a_late_answer_never_catches_keys_meant_for_another_view_or_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        // Searching when it arrives: no question, and the search keeps its keys.
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Search(Search {
            query: "chec".into(),
            filter: SearchFilter::Everything,
            sel: 0,
        });
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Search(_)));
        // It waits on its card, asking, not failed (intent 0591).
        assert!(app.message().unwrap().0.contains("n goes to it"));
        assert_eq!(app.message().unwrap().1, Tone::Info);
        press(&mut app, 'y');
        assert!(matches!(&app.mode, Mode::Search(s) if s.query == "checy"));
        // Enter on its card asks that question now; Esc leaves it, and it stops waiting.
        app.mode = Mode::Board;
        app.sel.key = Some(key.clone());
        assert_eq!(enter(&mut app), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        esc(&mut app);
        assert!(app.waiting_on(&key).is_none());
        // Reopened from the board, then into the backlog: the board's selection still names it,
        // but you're looking at the backlog, so it doesn't pull you out.
        app.mode = Mode::Board;
        app.sel.key = Some(key.clone());
        let request = reopen_by_enter(&mut app);
        press(&mut app, 'b');
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        // A dispatch from the backlog, then moving to another backlog card: no question for it.
        app.mode = Mode::Backlog { sel: 0 };
        app.rebuild();
        let first = app.backlog_card_for_keys().map(|c| c.key.clone());
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        app.busy = true;
        press(&mut app, 'j');
        assert_ne!(app.backlog_card_for_keys().map(|c| c.key.clone()), first);
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Backlog { .. }));
    }

    #[test]
    fn a_reopen_takes_you_in_only_while_you_are_still_on_its_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .key
            .clone();
        // Moved to another card while it ran: it says so instead of taking you away.
        let request = reopen_by_enter(&mut app);
        app.sel.key = Some(other);
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        assert!(app.message().unwrap().0.contains("on its card goes in"));
        // Esc from its peek while it runs only goes back to the board, still waiting; Esc on the
        // board lets it go: it finishes, but takes you nowhere.
        app.sel.key = Some(key.clone());
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Board));
        assert!(app.resolver.holds_you(), "leaving a peek isn't stopping");
        esc(&mut app);
        assert!(!app.resolver.holds_you());
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        // Still on it, in its peek: in it goes, remembering where it came from.
        let request = reopen_by_enter(&mut app);
        app.mode = Mode::Peek {
            scroll: 0,
            back: Box::new(Mode::Board),
        };
        reply_to(
            &mut app,
            0,
            request.clone(),
            reopen_reply("web", true, false, ""),
        );
        settle(&mut app);
        assert_eq!(
            app.open_after.take(),
            Some(OpenAfter {
                machine: 0,
                session: "web".into(),
                workspace: "w7".into(),
                label: "41-checkout-expired-cards".into(),
                tab: None,
                from: key.clone(),
            })
        );
        // Taken in: that's the answer, with nothing more said (intent 0416).
        assert_ne!(app.message().map(|(_, tone)| tone), Some(Tone::Good));
        // The board already shows the workspace it made by the time the answer comes: still in.
        app.mode = Mode::Board;
        let request = reopen_by_enter(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w7".into());
        app.board.cards[i].session = Some("web".into());
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert!(app.open_after.take().is_some());
        // But a workspace other than the one it made is another matter.
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        app.board.cards[i].workspace_id = Some("w8".into());
        app.board.cards[i].session = Some("web".into());
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        closed_workspace_card(&mut app);
        // A refusal goes nowhere.
        app.mode = Mode::Board;
        let request = reopen_by_enter(&mut app);
        reply_to(&mut app, 0, request, reopen_reply("web", false, false, ""));
        settle(&mut app);
        assert_eq!(app.open_after, None);
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Fail));
    }

    #[test]
    fn a_workspace_found_closed_on_the_way_in_reopens_work_under_way() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        // herdr says w5 closed, and the board hasn't caught up: it still shows w5. Reopen.
        app.board.cards[i].workspace_id = Some("w5".into());
        app.board.cards[i].session = Some("web".into());
        assert!(app.closed_under_you(&key, Some("w5")));
        assert_eq!(app.pending_reopen.take().as_deref(), Some(key.as_str()));
        // It has moved to another workspace meanwhile: that one's not closed, so no reopen.
        assert!(!app.closed_under_you(&key, Some("w4")));
        assert_eq!(app.pending_reopen, None);
        // Esc before its turn drops the reopen too.
        assert!(app.closed_under_you(&key, Some("w5")));
        esc(&mut app);
        assert_eq!(app.pending_reopen, None);
        // Its machine was away and its workspace closed meanwhile: when it answers, the jump goes
        // on as ⏎, which reopens it (intent 0591).
        app.board.cards[i].workspace_id = None;
        app.board.cards[i].session = None;
        let pending = |app: &App| PendingJump {
            key: key.clone(),
            workspace: "#41's workspace".into(),
            machine: app.card(&key).unwrap().machine.clone(),
            stuck: None,
            bare: None,
            resume: None,
        };
        app.pending_jump = Some(pending(&app));
        assert_eq!(app.pending_jump_ready(), Some(Action::Jump(key.clone())));
        let Action::Mutate { request, .. } = app.jump_action(key.clone()) else {
            panic!("no reopen")
        };
        assert_eq!(request["action"], "reopen");
        // Without actions, it says why it can't, plainly.
        app.cfg.actions.command = None;
        app.pending_jump = Some(pending(&app));
        assert_eq!(app.pending_jump_ready(), Some(Action::Jump(key.clone())));
        assert_eq!(app.jump_action(key.clone()), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("with [actions] configured"),
            "{:?}",
            app.message()
        );
        assert!(!app.closed_under_you(&key, None));
    }

    #[test]
    fn a_question_that_appears_on_its_own_ignores_keys_held_or_typed_before_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            reopen_reply("web", false, true, "0123456789ab0004"),
        );
        // Enter still held from opening it, a repeat of y, w: none answers it.
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert_eq!(press(&mut app, 'w'), Action::None);
        let mut held = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        held.kind = KeyEventKind::Repeat;
        settle(&mut app);
        assert_eq!(app.on_key(held), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        // A fresh y once it has been up a moment does.
        assert!(matches!(press(&mut app, 'y'), Action::Mutate { .. }));
    }

    #[test]
    fn a_start_work_yes_goes_where_it_was_asked_and_not_after_the_home_moved() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        let Action::Mutate { request, machine } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        app.busy = true;
        let asks = crate::actions::Response {
            ok: false,
            message: "web has 3 workspaces. Start anyway?".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            ..Default::default()
        };
        reply_to(&mut app, machine, request.clone(), Ok(asks.clone()));
        settle(&mut app);
        let Action::Mutate { request: sent, .. } = press(&mut app, 'y') else {
            panic!("yes sent nothing")
        };
        assert_eq!(
            (sent["override"].as_bool(), sent["session"].as_str()),
            (Some(true), Some("web"))
        );
        // Its home moves while the question is up: the yes never overrides the limit where it
        // was asked. It sends d again, as the card is now, for its new home to ask (intent 0591).
        app.action_from = Some(CardSnapshot::of(app.card(&key).unwrap()));
        reply_to(&mut app, machine, request, Ok(asks));
        settle(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some(app.machines[0].name()),
            session: Some("api".into()),
        });
        let Action::Mutate { request: again, .. } = press(&mut app, 'y') else {
            panic!("the yes didn't ask again: {:?}", app.message())
        };
        assert_eq!(again["action"], "dispatch");
        assert!(again.get("override").is_none() && again.get("confirm").is_none());
        assert_ne!(again["session"].as_str(), Some("web"));
        assert!(app.message().unwrap().0.contains("asking again"));
    }

    fn type_in(app: &mut App, words: &str) {
        for c in words.chars() {
            press(app, c);
        }
    }

    fn query(app: &App) -> Option<(String, bool)> {
        app.view_search
            .as_ref()
            .map(|s| (s.query.clone(), s.typing))
    }

    /// The card listed last in the backlog, and a word only it has: the search should leave it
    /// alone, so its reference with nothing else.
    fn last_backlog_card(app: &App) -> (String, String) {
        let order = app.backlog_order();
        let c = &app.board.cards[*order.last().unwrap()];
        (c.key.clone(), c.reference.clone())
    }

    #[test]
    fn slash_in_the_backlog_searches_only_the_backlog_and_stays_in_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'b');
        let all = app.backlog_order();
        assert!(all.len() > 2, "the demo has a backlog to narrow");
        let (key, reference) = last_backlog_card(&app);
        assert_eq!(press(&mut app, '/'), Action::None);
        assert!(
            matches!(app.mode, Mode::Backlog { .. }),
            "no search of everything"
        );
        assert_eq!(query(&app), Some((String::new(), true)));
        // Letters, the card keys among them, go into the search while typing.
        type_in(&mut app, &reference);
        assert!(matches!(app.mode, Mode::Backlog { sel: 0 }));
        assert_eq!(query(&app), Some((reference.clone(), true)));
        let shown = app.backlog_cards();
        assert_eq!(shown.len(), 1);
        assert_eq!(app.board.cards[shown[0]].key, key);
        assert!(shown.iter().all(|i| all.contains(i)));
        assert_eq!(
            press(&mut app, 'd'),
            Action::None,
            "d is typed, not started"
        );
        assert!(app.backlog_cards().is_empty());
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.backlog_cards(), shown);
        // Esc clears the search and stays in the backlog, on the card it showed; a second leaves.
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Backlog { sel } if sel == all.len() - 1));
        assert_eq!(query(&app), None);
        assert_eq!(app.backlog_cards(), all);
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn enter_on_a_backlog_search_peeks_and_coming_back_keeps_the_search() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        press(&mut app, 'b');
        let all = app.backlog_order();
        let (key, reference) = last_backlog_card(&app);
        press(&mut app, '/');
        type_in(&mut app, &reference);
        // Enter does what Enter does in the backlog: peek, on the card the search left.
        enter(&mut app);
        assert!(
            matches!(&app.mode, Mode::Peek { back, .. } if matches!(**back, Mode::Backlog { .. }))
        );
        assert_eq!(app.sel.key.as_deref(), Some(key.as_str()));
        // Back in the backlog, the search is still there, and letters are its keys again.
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Backlog { sel: 0 }));
        assert_eq!(query(&app), Some((reference.clone(), false)));
        assert_eq!(app.backlog_cards().len(), 1);
        assert_eq!(
            app.backlog_card_for_keys().map(|c| c.key.clone()),
            app.card(&key)
                .filter(|c| c.dismissed() || waiting_work(c))
                .map(|c| c.key.clone())
        );
        // / goes back to typing the same search; Esc clears it, and the next leaves.
        press(&mut app, '/');
        assert_eq!(query(&app), Some((reference, true)));
        esc(&mut app);
        assert_eq!(app.backlog_cards(), all);
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(query(&app), None);
    }

    #[test]
    fn a_backlog_search_ends_when_you_leave_and_not_in_a_peek() {
        let mut app = demo_app();
        press(&mut app, 'b');
        let all = app.backlog_order();
        let (_, reference) = last_backlog_card(&app);
        press(&mut app, '/');
        type_in(&mut app, &reference);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Peek { .. }));
        assert!(
            app.view_search.is_some(),
            "kept while you peek from the view"
        );
        esc(&mut app);
        // q leaves the backlog (it isn't typing now): the search goes with it.
        press(&mut app, 'q');
        assert!(matches!(app.mode, Mode::Board));
        assert_eq!(query(&app), None);
        press(&mut app, 'b');
        assert_eq!(app.backlog_cards(), all);
        // Into Won't do from a searched backlog: Won't do starts whole.
        press(&mut app, '/');
        type_in(&mut app, "zzz");
        esc(&mut app);
        press(&mut app, '/');
        type_in(&mut app, "zzz");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        press(&mut app, 'w');
        assert!(app.wont_do);
        assert_eq!(query(&app), None);
    }

    #[test]
    fn slash_in_wont_do_searches_only_what_was_set_aside() {
        let mut app = demo_app();
        // Set two backlog cards aside; the rest stay in the backlog.
        let backlog: Vec<usize> = app.backlog_order();
        for &i in &backlog[..2] {
            app.board.cards[i].detail.as_mut().unwrap().disposition = Some("wont_do".into());
        }
        let backlog_only = app.board.cards[backlog[2]].reference.clone();
        press(&mut app, 'w');
        assert!(matches!(app.mode, Mode::Backlog { .. }) && app.wont_do);
        let all = app.backlog_order();
        assert_eq!(all, backlog[..2]);
        press(&mut app, '/');
        type_in(&mut app, "zzzz-nothing");
        assert!(matches!(app.mode, Mode::Backlog { .. }) && app.wont_do);
        assert!(app.backlog_cards().is_empty());
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.backlog_cards(), all);
        let first = app.board.cards[all[0]].reference.clone();
        type_in(&mut app, &first);
        assert_eq!(app.backlog_cards(), vec![all[0]]);
        // A card still in the backlog isn't found from Won't do.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_in(&mut app, &backlog_only);
        assert!(app.backlog_cards().is_empty());
    }

    #[test]
    fn slash_in_misc_searches_only_misc_and_enter_jumps_in() {
        let mut app = demo_app();
        press(&mut app, 'i');
        let all = app.misc_cards();
        assert!(!all.is_empty());
        let card = app.board.cards[*all.last().unwrap()].clone();
        assert_eq!(press(&mut app, '/'), Action::None);
        assert!(
            matches!(app.mode, Mode::Misc { .. }),
            "no search of everything"
        );
        type_in(&mut app, "zzzz-nothing");
        assert!(app.misc_cards().is_empty());
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_in(&mut app, &card.title);
        let shown = app.misc_cards();
        assert!(shown.contains(all.last().unwrap()));
        assert!(shown.iter().all(|i| all.contains(i)));
        // q is typed while searching, not a way out.
        assert!(matches!(app.mode, Mode::Misc { .. }));
        // Enter does what Enter does in Misc: jump into the card selected.
        let first = app.board.cards[shown[0]].key.clone();
        assert_eq!(enter(&mut app), Action::Jump(first));
        // Back in Misc with the search left in place, Esc clears it and the next leaves.
        app.mode = Mode::Misc { sel: 0 };
        assert_eq!(query(&app), Some((card.title.clone(), false)));
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Misc { .. }));
        assert_eq!(app.misc_cards(), all);
        esc(&mut app);
        assert!(matches!(app.mode, Mode::Board));
    }

    #[test]
    fn slash_on_the_board_still_searches_everything() {
        let mut app = demo_app();
        assert!(matches!(app.mode, Mode::Board));
        press(&mut app, '/');
        assert!(matches!(
            &app.mode,
            Mode::Search(s) if s.filter == SearchFilter::Everything && s.query.is_empty()
        ));
        assert_eq!(query(&app), None);
    }

    #[test]
    fn a_searched_backlog_shows_the_search_and_each_card_keeps_its_place() {
        let mut app = demo_app();
        press(&mut app, 'b');
        let n = app.backlog_order().len();
        let (_, reference) = last_backlog_card(&app);
        press(&mut app, '/');
        type_in(&mut app, &reference);
        let screen = crate::ui::tests_render(&app, 160, 40);
        assert!(screen.contains(&format!("/ {reference}")), "{screen}");
        assert!(screen.contains(&format!("1 of {n}")), "{screen}");
        assert!(
            screen.contains(&format!("{n:>3} ")),
            "keeps its place {n}: {screen}"
        );
        assert!(screen.contains("clear the search"), "{screen}");
        press(&mut app, 'q');
        let screen = crate::ui::tests_render(&app, 160, 40);
        assert!(
            screen.contains("Nothing here matches this search."),
            "{screen}"
        );
        esc(&mut app);
        let screen = crate::ui::tests_render(&app, 300, 40);
        assert!(screen.contains("/ search this list"), "{screen}");
    }

    #[test]
    fn the_keys_say_what_slash_does_on_the_board_and_in_a_list() {
        let mut app = demo_app();
        press(&mut app, '?');
        let screen = crate::ui::tests_render(&app, 200, 80);
        assert!(
            screen.contains("search cards, workspaces and sessions"),
            "{screen}"
        );
        assert!(
            screen.contains("search only the list you're in"),
            "{screen}"
        );
    }

    #[test]
    fn a_question_that_arrives_while_a_list_search_is_typed_never_takes_its_letters() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        press(&mut app, 'b');
        let sel = app
            .backlog_cards()
            .iter()
            .position(|&i| app.board.cards[i].key == key)
            .unwrap();
        app.mode = Mode::Backlog { sel };
        app.backlog_focus = Some(key.clone());
        let Action::Mutate { request, machine } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        app.busy = true;
        // Typing a search before its answer comes: the question doesn't take over.
        press(&mut app, '/');
        let reference = app.card(&key).unwrap().reference.clone();
        type_in(&mut app, &reference);
        let asks = crate::actions::Response {
            ok: false,
            message: "web has 3 workspaces. Start anyway?".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            ..Default::default()
        };
        reply_to(&mut app, machine, request, Ok(asks));
        settle(&mut app);
        assert!(matches!(app.mode, Mode::Backlog { .. }));
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert!(query(&app).is_some_and(|(q, typing)| q.ends_with('y') && typing));
        // It waits on its card instead, asking, never a failure (intent 0591).
        assert_eq!(app.message().map(|(_, tone)| tone), Some(Tone::Info));
        assert!(app.waiting_on(&key).is_some());
    }

    #[test]
    fn a_paste_goes_into_a_list_search() {
        let mut app = demo_app();
        press(&mut app, 'b');
        let (key, reference) = last_backlog_card(&app);
        press(&mut app, '/');
        app.on_paste(&format!("{reference}\r\n"));
        assert_eq!(query(&app), Some((format!("{reference} "), true)));
        let shown = app.backlog_cards();
        assert_eq!(shown.len(), 1);
        assert_eq!(app.board.cards[shown[0]].key, key);
        assert!(matches!(app.mode, Mode::Backlog { sel: 0 }));
        app.on_paste(&"x".repeat(100));
        assert_eq!(query(&app).unwrap().0.chars().count(), 64);
        esc(&mut app);
        esc(&mut app);
        press(&mut app, 'i');
        let title = app.board.cards[app.misc_cards()[0]].title.clone();
        press(&mut app, '/');
        app.on_paste(&title);
        assert_eq!(query(&app), Some((title, true)));
        assert!(!app.misc_cards().is_empty());
    }

    /// A second card for work under way whose workspace has closed, beside `other`, on the first
    /// machine and homed in `web`.
    fn second_closed_card(app: &mut App, other: &str) -> (String, String) {
        let machine = app.machines[0].name();
        let card = app
            .board
            .cards
            .iter_mut()
            .find(|c| c.detail.is_some() && c.key != other)
            .expect("a second card with a record in the demo");
        card.stage = Stage::Test;
        card.workspace_id = None;
        card.session = None;
        card.attention = Attention::Idle;
        card.machine_index = 0;
        let d = card.detail.as_mut().unwrap();
        d.disposition = None;
        d.intake = None;
        d.state = None;
        d.home = Some(crate::feed::Home {
            machine: Some(machine),
            session: Some("web".into()),
        });
        let (key, id) = (card.key.clone(), d.id.clone());
        assert!(app.card(&key).unwrap().reopenable());
        (key, id)
    }

    fn select_key(app: &mut App, key: &str) {
        let col = app.card(key).unwrap().stage.index();
        app.sel = Selection {
            col,
            key: Some(key.to_string()),
        };
    }

    #[test]
    fn enter_on_another_card_while_a_change_runs_goes_at_once() {
        // Intent 0416: a card's change never holds the board, so ⏎ on another card sends its own
        // reopen straight away, while ⏎ on the card still reopening says so and sends nothing.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (a, _) = closed_workspace_card(&mut app);
        let Action::Mutate {
            machine,
            request: first,
        } = enter(&mut app)
        else {
            panic!("Enter didn't ask to reopen")
        };
        app.queue_action(machine, first.clone());
        app.resolver.ready();
        assert!(!app.busy);
        // The rebuild from the feed undoes the hand-made board: make both cards again.
        let (a2, _) = closed_workspace_card(&mut app);
        assert_eq!(a2, a);
        let (b, b_id) = second_closed_card(&mut app, &a);
        select_key(&mut app, &b);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("⏎ on another card waited: {:?}", app.message())
        };
        assert_eq!(request["id"].as_str(), Some(b_id.as_str()));
        select_key(&mut app, &a);
        assert_eq!(enter(&mut app), Action::None);
        assert!(
            app.message().unwrap().0.contains("is reopening"),
            "{:?}",
            app.message()
        );
    }

    #[test]
    fn a_reopen_owed_goes_whatever_account_action_runs() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].workspace_id = Some("w5".into());
        app.board.cards[i].session = Some("web".into());
        // An account action running doesn't hold it (intent 0445).
        app.busy = true;
        app.busy_since = Some(Instant::now());
        assert!(app.closed_under_you(&key, Some("w5")));
        assert_eq!(app.pending_reopen.as_deref(), Some(key.as_str()));
        // The owed reopen goes out, even though the board still shows the workspace herdr said
        // has closed.
        let Action::Mutate { request, .. } = app.next_owed() else {
            panic!("the owed reopen sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
        assert_eq!(app.pending_reopen, None);
        // Esc before its turn drops it.
        assert!(app.closed_under_you(&key, Some("w5")));
        esc(&mut app);
        assert_eq!(app.pending_reopen, None);
        assert_eq!(app.next_owed(), Action::None);
    }

    #[test]
    fn enter_on_cards_goes_at_once_while_an_account_action_runs() {
        // Intent 0445: busy means only an account action now, and ⏎ on a card never waits for it.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.busy = true;
        app.busy_id = Some("KBZ_C".into());
        app.busy_since = Some(Instant::now());
        let (key, id) = closed_workspace_card(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("⏎ waited: {:?}", app.message())
        };
        assert_eq!(
            (request["action"].as_str(), request["id"].as_str()),
            (Some("reopen"), Some(id.as_str()))
        );
        let (wkey, wid, _) = waiting_card(&app);
        select_key(&mut app, &wkey);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("⏎ waited: {:?}", app.message())
        };
        assert_eq!(
            (request["action"].as_str(), request["id"].as_str()),
            (Some("dispatch"), Some(wid.as_str()))
        );
        assert_ne!(key, wkey);
    }

    #[test]
    fn unsay_takes_back_only_its_own_message() {
        let mut app = demo_app();
        app.say("Opening 41-checkout on LAB-Server…", Tone::Info);
        app.unsay("Opening 41-checkout on LAB-Server…");
        assert!(app.message().is_none());
        app.say("Couldn't reach LAB-Server: timed out", Tone::Warn);
        app.unsay("Opening 41-checkout on LAB-Server…");
        assert_eq!(
            app.message().unwrap().0,
            "Couldn't reach LAB-Server: timed out"
        );
    }

    #[test]
    fn a_reopen_into_a_new_tab_goes_onto_that_tab() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        let mut reply = reopen_reply("web", true, false, "").unwrap();
        reply.tab = Some("w7:t2".into());
        reply_to(&mut app, 0, request, Ok(reply));
        assert_eq!(app.open_after.take().unwrap().tab.as_deref(), Some("w7:t2"));
        // One into a new workspace names none.
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(&mut app, 0, request, reopen_reply("web", true, false, ""));
        assert_eq!(app.open_after.take().unwrap().tab, None);
    }

    #[test]
    fn a_yes_that_cant_be_matched_to_its_question_asks_the_question_afresh() {
        let refused = || {
            Ok(crate::actions::Response {
                ok: false,
                message: "Kerdr's yes couldn't be matched to the question it answered, so nothing started; ask again.".into(),
                ..Default::default()
            })
        };
        let yes_refused = |keeps_ops: bool| {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            if keeps_ops {
                app.machines[0].feed.as_mut().unwrap().supports = vec!["op".into()];
            }
            let (_, id) = closed_workspace_card(&mut app);
            reopen_by_enter(&mut app);
            let n = app
                .resolver
                .unanswered_for("reopen", Some(id.as_str()))
                .unwrap();
            let mut yes = app.resolver.job_request(n).unwrap().clone();
            let op = yes["op"].clone();
            yes["override"] = serde_json::json!(true);
            yes["confirm"] = serde_json::json!("not-a-token");
            yes["queue"] = serde_json::json!(true);
            app.finish_job(n, yes, refused());
            (app, id, op)
        };
        // A backend whose feed doesn't say it keeps ops is sent nothing by itself: the refusal is
        // said, and the next key press asks again.
        let (app, id, _) = yes_refused(false);
        assert!(app.resend.is_empty());
        assert!(
            app.resolver
                .unanswered_for("reopen", Some(id.as_str()))
                .is_none()
        );
        assert!(last_said(&app).unwrap().0.contains("couldn't be matched"));

        let (app, id, op) = yes_refused(true);
        // The same job goes again, as a new ask: no answer of the old question, and its own op.
        let n = app
            .resolver
            .unanswered_for("reopen", Some(id.as_str()))
            .unwrap();
        let again = app.resolver.job_request(n).unwrap().clone();
        assert_eq!(again["action"], "reopen");
        assert_eq!(again["id"], serde_json::json!(id));
        for answer in ["override", "confirm", "queue"] {
            assert!(again.get(answer).is_none(), "{again}");
        }
        assert!(again["op"].is_string() && again["op"] != op, "{again}");
        assert!(last_said(&app).unwrap().0.contains("Asking it again"));

        // Not a yes: only said, so it can't go round by itself.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(&mut app, 0, request, refused());
        assert!(app.resend.is_empty());
        assert!(last_said(&app).unwrap().0.contains("couldn't be matched"));
    }

    #[test]
    fn a_refusal_naming_another_home_is_sent_there_once() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        reopen_by_enter(&mut app);
        // As the runner has it: with the op Kerdr gave it.
        let n = app
            .resolver
            .unanswered_for("reopen", Some(id.as_str()))
            .unwrap();
        let request = app.resolver.job_request(n).unwrap().clone();
        let moved = || {
            Ok(crate::actions::Response {
                ok: false,
                message: "It lives on build-box now (ARK); ⏎ again opens it there.".into(),
                home: Some("build-box/ARK".into()),
                ..Default::default()
            })
        };
        reply_to(&mut app, 0, request.clone(), moved());
        assert!(!app.busy);
        // Sent on quietly: nothing is said unless it fails there too.
        assert_eq!(app.message(), None);
        // Sent on in its own place in the queue, to the machine the refusal named.
        let Some((_, machine, again, _)) = app.resolver.ready().into_iter().next() else {
            panic!("nothing was sent on")
        };
        assert_eq!(machine, 1);
        assert_eq!(again["action"], "reopen");
        assert_eq!(again["id"].as_str(), Some(id.as_str()));
        assert_eq!(again["session"], "ARK");
        // The same request, sent on: never under a new op (intent 0591).
        assert!(
            request["op"]
                .as_str()
                .is_some_and(|op| op.starts_with("k-"))
        );
        assert_eq!(again["op"], request["op"]);
        assert!(app.resolver.holds_you(), "its answer may still take you in");
        let _ = &key;
        // Until the board catches up, its next actions go there too.
        assert_eq!(
            app.moved_home
                .iter()
                .map(|(i, t, _)| (i.clone(), *t))
                .collect::<Vec<_>>(),
            vec![(id.clone(), 1)]
        );
        let card = app.card(&key).cloned().unwrap();
        assert_eq!(app.home_machine(&card), Some(1));
        // Once the feed says something new about its home, the feed wins over the refusal.
        let mut later = card.clone();
        later.detail.as_mut().unwrap().home = Some(crate::feed::Home {
            machine: Some("somewhere-new".into()),
            session: Some("ARK".into()),
        });
        assert_eq!(app.moved_target(&later), None);
        assert_eq!(app.moved_target(&card), Some(1));
        // That machine's answer naming yet another home is never sent on again: Kerdr says the
        // copies disagree, and stops guessing.
        app.busy = true;
        let back = || {
            Ok(crate::actions::Response {
                ok: false,
                message: "It lives on laptop now; ⏎ again opens it there.".into(),
                home: Some("laptop/web".into()),
                ..Default::default()
            })
        };
        reply_to(&mut app, 1, again, back());
        let said = last_said(&app).unwrap().0;
        assert_eq!(
            said,
            format!(
                "{reference}'s copies on build-box and laptop disagree about where it lives; one is behind. Wait a minute for it to catch up, then try again."
            )
        );
        assert_eq!(app.next_owed(), Action::None);
        assert!(app.moved_home.is_empty());
        // A home that's the machine asked is only said; one Kerdr doesn't know, or one that isn't
        // answering, is said in Kerdr's words, never with a "⏎ again" that would ask the same copy.
        app.busy = true;
        let reply = Ok(crate::actions::Response {
            ok: false,
            message: "refused: laptop/web".into(),
            home: Some("laptop/web".into()),
            ..Default::default()
        });
        reply_to(&mut app, 0, request.clone(), reply);
        assert_eq!(
            last_said(&app).unwrap().0,
            format!("{reference} didn't reopen: refused: laptop/web")
        );
        assert!(app.resend.is_empty());
        let refused = |home: &str| {
            Ok(crate::actions::Response {
                ok: false,
                message: format!("It lives on {home} now; ⏎ again opens it there."),
                home: Some(home.into()),
                ..Default::default()
            })
        };
        reply_to(&mut app, 0, request.clone(), refused("elsewhere/ARK"));
        assert_eq!(
            last_said(&app).unwrap().0,
            format!(
                "{reference} lives on elsewhere now, which isn't one of this board's machines, so Kerdr can't change it there."
            )
        );
        assert!(app.resend.is_empty());
        app.machines[1].conn = Conn::Unreachable {
            since: Local::now(),
            retry_at: Local::now(),
            detail: "timed out".into(),
        };
        reply_to(&mut app, 0, request.clone(), refused("build-box/ARK"));
        assert_eq!(
            last_said(&app).unwrap().0,
            format!(
                "{reference} lives on build-box now, and build-box isn't answering; nothing changed. Try again once it's back."
            )
        );
        assert!(app.resend.is_empty());
        assert_eq!(
            app.moved_home
                .iter()
                .map(|(i, t, _)| (i.clone(), *t))
                .collect::<Vec<_>>(),
            vec![(id.clone(), 1)],
            "⏎ goes there once it's back"
        );
    }

    #[test]
    fn a_long_answer_is_cut_to_what_the_board_shows() {
        let mut app = demo_app();
        let long = "x".repeat(3000);
        reply_to(
            &mut app,
            0,
            serde_json::json!({"action": "create"}),
            Ok(crate::actions::Response {
                ok: false,
                message: long.clone(),
                ..Default::default()
            }),
        );
        let said = app.message().unwrap().0.to_string();
        assert!(said.chars().count() <= 301, "{}", said.chars().count());
        assert!(said.ends_with('…'));
        reply_to(
            &mut app,
            0,
            serde_json::json!({"action": "close", "id": "1"}),
            Err(long),
        );
        assert!(last_said(&app).unwrap().0.chars().count() <= 301);
        reply_to(
            &mut app,
            0,
            serde_json::json!({"action": "close", "id": "1"}),
            Err("Couldn't close it.".into()),
        );
        // Nothing says it wasn't sent, so nobody knows whether it happened: named by what was
        // asked, never as a change that didn't happen (intent 0591).
        assert_eq!(
            last_said(&app).unwrap().0,
            "Marking 1 done: Couldn't close it."
        );
        reply_to(
            &mut app,
            0,
            serde_json::json!({"action": "close", "id": "1"}),
            Err(crate::actions::NO_BACKEND.into()),
        );
        assert_eq!(
            last_said(&app).unwrap().0,
            "1 wasn't marked done: Kerdr has no action backend set ([actions] command in its config), so nothing was sent"
        );
    }

    #[test]
    fn a_question_from_the_machine_a_refusal_named_can_be_answered_yes() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        reply_to(
            &mut app,
            0,
            request,
            Ok(crate::actions::Response {
                ok: false,
                message: "It lives on build-box now; ⏎ again opens it there.".into(),
                home: Some("build-box/ARK".into()),
                ..Default::default()
            }),
        );
        // Sent on in its own place in the queue, to the machine the refusal named.
        let Some((_, machine, again, _)) = app.resolver.ready().into_iter().next() else {
            panic!("nothing was sent on")
        };
        assert_eq!(machine, 1);
        app.busy = true;
        // build-box asks first (done work, a full session): the board still shows the old home.
        reply_to(
            &mut app,
            1,
            again,
            reopen_reply("ARK", false, true, "tok-1"),
        );
        assert!(
            matches!(app.mode, Mode::Confirm { machine: 1, .. }),
            "{:?}",
            app.message()
        );
        if let Mode::Confirm { ready_at, .. } = &mut app.mode {
            *ready_at = None;
        }
        let yes = app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        let Action::Mutate { machine, request } = yes else {
            panic!("the yes sent nothing: {:?}", app.message())
        };
        assert_eq!(machine, 1);
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
        assert_eq!(request["confirm"], "tok-1");
        assert_eq!(request["override"], true);
        assert_eq!(app.card(&key).unwrap().machine_index, 0);
    }

    /// The card's conversation runs in a foot window outside herdr, since 13:02 on 1 November.
    fn runs_outside(app: &mut App, key: &str) -> String {
        let since = chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 11, 1, 13, 2, 0)
            .single()
            .unwrap()
            .to_rfc3339();
        for c in app.board.cards.iter_mut().filter(|c| c.key == key) {
            c.detail.as_mut().unwrap().outside_herdr = Some(crate::feed::OutsideHerdr {
                terminal: Some("foot".into()),
                since: Some(since.clone()),
            });
        }
        since
    }

    /// Intent 0591: C and x on work whose conversation runs outside herdr go to its backend, which
    /// stops it there first and asks before it does; its question is the one asked, and the yes
    /// carries its token.
    #[test]
    fn c_and_x_on_work_running_outside_herdr_ask_its_backend_first() {
        for (key_char, action, title) in [('C', "close", "Mark"), ('x', "dismiss", "Won't do")] {
            let mut app = demo_app();
            app.cfg.actions.command = Some("test-adapter".into());
            let (key, id) = closed_workspace_card(&mut app);
            let reference = app.card(&key).unwrap().reference.clone();
            let since = runs_outside(&mut app, &key);
            let Action::Mutate { machine, request } = press(&mut app, key_char) else {
                panic!("{key_char} sent nothing: {:?}", app.message())
            };
            assert_eq!(
                request,
                serde_json::json!({"action": action, "id": id, "outside_since": since})
            );
            // It can only come back as a question, so the card doesn't show it done meanwhile.
            assert_eq!(app.effect_for(&key, &request), None);
            let asked = format!(
                "{reference}'s Codex conversation is running in a foot window since 13:02, outside herdr. A yes stops that agent once its current turn ends, with nothing lost. Stop it?"
            );
            app.finish_action(
                machine,
                request,
                Ok(crate::actions::parse_response(
                    &serde_json::json!({"ok":false,"confirmation":true,"confirm":"t-1","id":id,"message":asked}).to_string(),
                )
                .unwrap()),
            );
            let Mode::Confirm {
                title: t, message, ..
            } = &app.mode
            else {
                panic!("its question wasn't asked: {:?}", app.message())
            };
            assert!(t.starts_with(title), "{t}");
            assert_eq!(message, &asked);
            settle(&mut app);
            let Action::Mutate { request, .. } = enter(&mut app) else {
                panic!("yes sent nothing: {:?}", app.message())
            };
            assert_eq!(request["action"], action);
            assert_eq!(request["override"], true);
            assert_eq!(request["confirm"], "t-1");
            assert_eq!(request["outside_since"], since.as_str());
        }
    }

    /// Intent 0591: X on work whose conversation runs outside herdr asks, naming that window, and
    /// its yes names it too, so the backend stops only that one. Moved into herdr meanwhile, the yes
    /// sends nothing.
    #[test]
    fn x_upper_on_work_running_outside_herdr_names_the_window_it_stops() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.fixed_now = Some(
            chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 11, 2, 9, 0, 0)
                .single()
                .unwrap(),
        );
        let (key, id) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let since = runs_outside(&mut app, &key);
        assert_eq!(press(&mut app, 'X'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("X didn't ask: {:?}", app.message())
        };
        assert_eq!(title, &format!("Cancel {reference}?"));
        assert!(
            message.starts_with("Stops its Codex conversation running in a foot window since"),
            "{message}"
        );
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("yes sent nothing: {:?}", app.message())
        };
        assert_eq!(
            request,
            serde_json::json!({"action": "cancel", "id": id, "outside_since": since})
        );
        // Asked again, and moved into herdr before the yes: nothing is stopped.
        app.busy = false;
        press(&mut app, 'X');
        settle(&mut app);
        for c in app.board.cards.iter_mut().filter(|c| c.key == key) {
            c.workspace_id = Some("w9".into());
            c.session = Some("web".into());
        }
        assert!(!matches!(enter(&mut app), Action::Mutate { .. }));
    }

    /// Intent 0591: ⏎ on a card whose herdr session didn't answer its last read waits for it, then
    /// goes in; after a minute still stuck, it says so instead.
    #[test]
    fn a_jump_waits_for_a_stuck_session_then_goes_in() {
        let mut app = demo_app();
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.is_open_workspace() && app.machines[c.machine_index].conn.is_live())
            .unwrap()
            .clone();
        let (mi, session) = (card.machine_index, card.session.clone().unwrap());
        app.machines[mi]
            .snapshot_errors
            .insert(session.clone(), "timed out".into());
        app.wait_for_stuck(mi, &session, &card.workspace_name(), Some(&card.key), None);
        assert!(app.message().unwrap().0.starts_with("Waiting for"));
        assert_eq!(app.pending_jump_ready(), None);
        app.machines[mi].snapshot_errors.remove(&session);
        assert_eq!(
            app.pending_jump_ready(),
            Some(Action::Jump(card.key.clone()))
        );
        assert!(app.pending_jump.is_none());
        // Still stuck after a minute.
        app.machines[mi]
            .snapshot_errors
            .insert(session.clone(), "timed out".into());
        app.wait_for_stuck(mi, &session, &card.workspace_name(), Some(&card.key), None);
        app.pending_jump.as_mut().unwrap().stuck.as_mut().unwrap().1 -=
            STUCK_WAIT + Duration::from_secs(1);
        assert_eq!(app.pending_jump_ready(), None);
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert!(
            said.contains("still isn't answering after a minute"),
            "{said}"
        );
        assert!(app.pending_jump.is_none());
    }

    /// A workspace a reopen's answer named, with the tab to land on, whose session didn't answer:
    /// once it does, Kerdr goes to that tab, not just the workspace (Codex's review).
    #[test]
    fn a_wait_for_a_stuck_session_keeps_the_tab_it_was_going_to() {
        let mut app = demo_app();
        let mi = app.machine_index("build-box").unwrap();
        let machine = app.machines[mi].name();
        app.machines[mi]
            .snapshot_errors
            .insert("web".into(), "timed out".into());
        app.wait_for_stuck(
            mi,
            "web",
            "41-checkout",
            None,
            Some(("web".into(), "w7".into(), Some("w7:t3".into()))),
        );
        assert_eq!(app.pending_jump_ready(), None, "still waiting");
        assert_eq!(app.pending_go, None);
        app.machines[mi].snapshot_errors.remove("web");
        assert_eq!(
            app.pending_jump_ready(),
            Some(Action::JumpWorkspace {
                machine,
                session: "web".into(),
                id: "w7".into(),
            })
        );
        assert_eq!(
            app.pending_go,
            Some(("41-checkout".into(), Some("w7:t3".into()))),
            "in to the tab it named"
        );
        assert!(app.pending_jump.is_none());
    }

    /// Intent 0591: herdr says the workspace ⏎ went for has closed. A card that has another one
    /// open goes there, once; work nobody started is started, as plain ⏎ would.
    #[test]
    fn a_workspace_closed_under_you_makes_plain_enters_choice() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        for c in app.board.cards.iter_mut().filter(|c| c.key == key) {
            c.workspace_id = Some("w5".into());
            c.session = Some("web".into());
            if let Some(d) = c.detail.as_mut() {
                d.kickoff = None;
            }
        }
        // Another workspace than the one that closed: go there, once.
        assert_eq!(
            app.after_closed(&key, Some("w4"), false),
            Action::Jump(key.clone())
        );
        // The one that closed, on work nobody started: it starts.
        let Action::Mutate { request, .. } = app.after_closed(&key, Some("w5"), false) else {
            panic!("nothing started: {:?}", app.message())
        };
        assert_eq!(request["action"], "dispatch");
        assert!(request.get("workspace").is_none());
        assert!(app.enter_sent.is_some(), "its answer goes in as ⏎'s would");
        assert!(app.action_from.is_some());
    }

    #[test]
    fn enter_on_work_running_outside_herdr_moves_it_into_herdr_at_once() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        app.fixed_now = Some(
            chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 11, 2, 9, 0, 0)
                .single()
                .unwrap(),
        );
        let (key, _) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        let since = chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 11, 1, 13, 2, 0)
            .single()
            .unwrap()
            .to_rfc3339();
        app.board.cards[i].detail.as_mut().unwrap().outside_herdr =
            Some(crate::feed::OutsideHerdr {
                terminal: Some("foot".into()),
                since: Some(since),
            });
        // One ⏎: the reopen goes at once, whose backend moves the conversation into herdr
        // (intent 0591, decision 2), and Kerdr says what's happening meanwhile.
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("⏎ sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        let said = app.message().unwrap().0.to_string();
        assert_eq!(
            said,
            format!("Moving {reference}'s conversation from a foot window into herdr…")
        );
        assert!(!said.contains("pid") && !said.contains("--"), "{said}");
        // Search's ⏎ does the same.
        app.mode = Mode::Board;
        app.sel.key = Some(key.clone());
        let search = || Search {
            query: reference.clone(),
            filter: SearchFilter::Everything,
            sel: 0,
        };
        let hit = app
            .search_hits(&search())
            .into_iter()
            .find(|(h, _)| matches!(h, Hit::Card(c) if *c == i))
            .map(|(h, _)| h)
            .unwrap();
        let Action::Mutate { request, .. } = app.open_hit(hit, search()) else {
            panic!("search's ⏎ sent nothing")
        };
        assert_eq!(request["action"], "reopen");
        // Done work asks its own question as ever.
        app.board.cards[i].stage = Stage::Maintain;
        assert!(matches!(enter(&mut app), Action::Mutate { .. }));
    }

    #[test]
    fn d_on_unstarted_work_in_a_working_column_does_what_enter_does() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = closed_workspace_card(&mut app);
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        app.board.cards[i].state = None;
        app.board.cards[i].agents.clear();
        app.board.cards[i].detail.as_mut().unwrap().no_agent = true;
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
        let Action::Mutate {
            request: by_enter, ..
        } = enter(&mut app)
        else {
            panic!("⏎ sent nothing")
        };
        assert_eq!(request, by_enter);
        // Work with a state of its own too: d reopens it, as ⏎ does (intent 0591, K1-24).
        app.board.cards[i].state = Some("active".into());
        app.resolver = Default::default();
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "reopen");
    }

    #[test]
    fn a_reopen_at_a_sessions_most_asks_to_queue_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        app.machines[0].feed.as_mut().unwrap().sessions.insert(
            "web".into(),
            crate::feed::FeedSlots {
                used: 5,
                limit: Some(3),
                max: Some(5),
            },
        );
        let request = reopen_by_enter(&mut app);
        let reply = Ok(crate::actions::Response {
            ok: false,
            message: "web holds 5 workspaces, its most. w queues it.".into(),
            confirmation: true,
            confirm: Some("t".into()),
            used: Some(5),
            limit: Some(5),
            session: Some("web".into()),
            ..Default::default()
        });
        reply_to(&mut app, 0, request, reply);
        settle(&mut app);
        let Mode::Confirm { title, queue, .. } = &app.mode else {
            panic!("no question")
        };
        assert_eq!(title, &format!("Queue {reference} in web?"));
        assert!(queue);
        let Action::Mutate { request, .. } = press(&mut app, 'w') else {
            panic!("w sent nothing")
        };
        assert_eq!(request["queue"], true);
        assert_eq!(request["confirm"], "t");
        assert!(request.get("override").is_none());
        // Past only the usual limit, it's still a reopen's question.
        let request = reopen_by_enter(&mut app);
        reply_to(&mut app, 0, request, reopen_reply("web", false, true, "t2"));
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("no question")
        };
        assert_eq!(title, &format!("Reopen {reference} in web?"));
    }

    /// A backlog card with an intent record in the demo, and its machine.
    /// The demo's sessions are at their limits, where a start only asks (intent 0491).
    pub(crate) fn room_in_every_session(app: &mut App) {
        for m in &mut app.machines {
            for slots in m.feed.iter_mut().flat_map(|f| f.sessions.values_mut()) {
                slots.limit = None;
            }
        }
        app.rebuild();
    }

    fn waiting_card(app: &App) -> (String, String, usize) {
        let c = app
            .board
            .cards
            .iter()
            .find(|c| {
                c.stage == Stage::Backlog
                    && c.detail.is_some()
                    && !c.dismissed()
                    && c.intake().is_none()
            })
            .expect("a backlog card in the demo");
        (
            c.key.clone(),
            c.detail.as_ref().unwrap().id.clone(),
            c.machine_index,
        )
    }

    fn status_of(app: &App, key: &str) -> Option<String> {
        app.card(key)?.detail.as_ref()?.status.clone()
    }

    #[test]
    fn a_change_sent_again_by_itself_says_so_quietly_on_its_card_and_nowhere_red() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        app.queue_action(machine, serde_json::json!({"action": "refresh", "id": id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        assert!(
            request["op"]
                .as_str()
                .is_some_and(|op| op.starts_with("k-"))
        );
        app.message = None;
        app.resending(
            Some(n),
            machine,
            &crate::actions::Resend::Retry {
                wait: Duration::from_secs(5),
                message: "#1's record is locked by a pull under way.".into(),
            },
        );
        assert_eq!(
            status_of(&app, &key).as_deref(),
            Some(crate::resolve::RETRYING)
        );
        assert_eq!(app.message(), None, "nothing said while it tries again");
        // Its answer at last takes the words down, and a change that went says nothing.
        app.finish_job(n, request, done("Refreshed."));
        assert_ne!(
            status_of(&app, &key).as_deref(),
            Some(crate::resolve::RETRYING)
        );
        assert_eq!(app.message(), None);
    }

    #[test]
    fn a_change_that_stands_with_its_push_still_to_come_says_what_is_left() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (_, id, machine) = waiting_card(&app);
        let reference = app.reference_for(&id);
        app.queue_action(machine, serde_json::json!({"action": "refresh", "id": id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.message = None;
        let left = "saved here; it reaches your other machines once the push goes through, which is retried by itself.";
        app.finish_job(
            n,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: format!("Refreshed {reference}: {left}"),
                note: Some(left.into()),
                ..Default::default()
            }),
        );
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Info, "it stands: not a failure");
        assert!(said.contains(left), "{said}");
    }

    #[test]
    fn a_change_done_only_in_part_is_a_warning_that_says_what_happened_without_didnt() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (_, id, machine) = waiting_card(&app);
        let reference = app.reference_for(&id);
        app.queue_action(machine, serde_json::json!({"action": "close", "id": id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(
            n,
            request,
            Ok(crate::actions::Response {
                ok: false,
                partial: true,
                message: "marked done, but its workspace couldn't be closed: herdr didn't answer."
                    .into(),
                ..Default::default()
            }),
        );
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Warn);
        assert_eq!(
            said,
            format!(
                "{reference}: marked done, but its workspace couldn't be closed: herdr didn't answer."
            )
        );
        assert!(!said.contains("wasn't marked done"), "{said}");
    }

    #[test]
    fn an_answer_that_never_came_names_what_was_asked_and_never_says_it_didnt_happen() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (_, id, machine) = waiting_card(&app);
        let reference = app.reference_for(&id);
        app.queue_action(machine, serde_json::json!({"action": "close", "id": id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        let lost = format!(
            "the connection to arknet-one dropped before its answer came; {}, and its card shows how it stands.",
            crate::actions::UNKNOWN
        );
        app.finish_job(n, request, Err(lost.clone()));
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert_eq!(said, format!("Marking {reference} done: {lost}"));
        // A request never sent is known not to have happened, and says so.
        app.message = None;
        app.queue_action(machine, serde_json::json!({"action": "close", "id": id}));
        let (n, _, request, _) = app.resolver.ready()[0].clone();
        app.finish_job(n, request, Err(crate::actions::NO_BACKEND.into()));
        assert!(
            app.message().unwrap().0.starts_with(&format!(
                "{reference} wasn't marked done: Kerdr has no action backend"
            )),
            "{:?}",
            app.message()
        );
    }

    #[test]
    fn only_a_machine_whose_feed_says_it_keeps_ops_is_sent_a_lost_request_again() {
        let mut app = demo_app();
        assert!(!app.resends_to(0), "the demo's feed says nothing of ops");
        app.machines[0].feed.as_mut().unwrap().supports = vec!["op".into(), "retry".into()];
        assert!(app.resends_to(0));
        app.machines[0].feed.as_mut().unwrap().supports = vec!["retry".into()];
        assert!(!app.resends_to(0));
        assert!(!app.resends_to(99));
    }

    #[test]
    fn what_a_key_cant_do_says_why_in_plain_words() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        // A tab number past the last says how many there are (intent 0591).
        let count = app
            .tabs
            .iter()
            .filter(|t| matches!(t, Scope::Session(..)))
            .count();
        app.session_tab(count + 5);
        assert_eq!(
            app.message().unwrap().0,
            format!(
                "There's no session tab {}; this view has {count}.",
                count + 5
            )
        );
        // u on work that isn't set aside: a fact, not a refusal.
        let (key, _, _) = waiting_card(&app);
        select_key(&mut app, &key);
        let reference = app.card(&key).unwrap().reference.clone();
        assert_eq!(press(&mut app, 'u'), Action::None);
        assert_eq!(
            app.message(),
            Some((
                format!("{reference} isn't set aside, so there's nothing to bring back.").as_str(),
                Tone::Info
            ))
        );
        // A full board advises; it doesn't refuse.
        assert_eq!(
            crate::model::Verdict::Full.text(),
            "Sessions full: d asks before starting another"
        );
    }

    fn refused(message: &str) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok: false,
            message: message.into(),
            ..Default::default()
        })
    }

    fn done(message: &str) -> Result<crate::actions::Response, String> {
        Ok(crate::actions::Response {
            ok: true,
            message: message.into(),
            ..Default::default()
        })
    }

    #[test]
    fn a_change_shows_at_once_and_a_failure_puts_the_card_back() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let request = serde_json::json!({"action": "close", "id": id});
        app.queue_action(machine, request.clone());
        assert!(
            app.card(&key).unwrap().done(),
            "shown done before any answer"
        );
        assert!(!app.busy, "the board isn't held");
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1);
        app.finish_job(ready[0].0, request, refused("The store is locked."));
        let card = app.card(&key).unwrap();
        assert_eq!(card.stage, Stage::Backlog, "back as it really is");
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Fail);
        assert!(
            said.contains("wasn't marked done: The store is locked."),
            "{said}"
        );
        assert_eq!(app.resolver.unanswered(), 0);
    }

    #[test]
    fn a_change_that_succeeds_says_nothing_and_stays_until_the_feed_has_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let request = serde_json::json!({"action": "dismiss", "id": id});
        app.queue_action(machine, request.clone());
        assert!(app.card(&key).unwrap().dismissed());
        let n = app.resolver.ready()[0].0;
        app.finish_job(n, request, done("Intent moved to Won't do."));
        assert_eq!(
            app.message(),
            None,
            "nothing more than the change already shown"
        );
        assert!(app.card(&key).unwrap().dismissed());
        // The demo's feed never catches up: after two reads the feed alone speaks.
        for _ in 0..2 {
            let feed = app.machines[machine].feed.clone();
            app.resolver.read(machine, feed.as_ref());
            app.rebuild();
        }
        assert!(!app.card(&key).unwrap().dismissed());
    }

    #[test]
    fn a_start_shows_starting_and_a_question_still_asks() {
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        select_key(&mut app, &key);
        let Action::Mutate {
            machine: m,
            request,
        } = press(&mut app, 'd')
        else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(m, machine);
        app.queue_action(m, request.clone());
        let card = app.card(&key).unwrap();
        // It stays where it is until the backend answers: a question still finds it.
        assert_eq!(card.stage, Stage::Backlog);
        assert_eq!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("Starting…")
        );
        // A second d or ⏎ on it sends nothing while the start is on its way.
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(app.message().unwrap().0.contains("is starting"));
        let n = app.resolver.ready()[0].0;
        let asks = crate::actions::Response {
            ok: false,
            message: "web has 3 workspaces. Start anyway?".into(),
            confirmation: true,
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            ..Default::default()
        };
        app.finish_job(n, request, Ok(asks));
        assert_eq!(app.card(&key).unwrap().stage, Stage::Backlog);
        // It names the card as the board does, #21, not its bare id (intent 0591, K1-60).
        let reference = format!("#{}", id.trim_start_matches('0'));
        assert!(
            matches!(&app.mode, Mode::Confirm { title, .. } if title.contains(&format!("Start {reference}"))),
            "{:?}",
            app.mode
        );
    }

    #[test]
    fn moves_show_at_once_in_the_order_the_backend_will_save() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        // Two neighbours in one group of the backlog.
        let order = app.backlog_order();
        let (a, b) = order
            .windows(2)
            .map(|w| (&app.board.cards[w[0]], &app.board.cards[w[1]]))
            .find(|(a, b)| a.owner_key() == b.owner_key() && a.machine_index == b.machine_index)
            .map(|(a, b)| (a.key.clone(), b.key.clone()))
            .expect("two neighbours in the demo's backlog");
        let id = app.card(&a).unwrap().detail.as_ref().unwrap().id.clone();
        let machine = app.card(&a).unwrap().machine_index;
        let place = |app: &App, k: &str| {
            app.backlog_order()
                .iter()
                .position(|&i| app.board.cards[i].key == k)
                .unwrap()
        };
        assert!(place(&app, &a) < place(&app, &b));
        app.queue_action(
            machine,
            serde_json::json!({"action": "move", "id": id, "delta": 1}),
        );
        assert!(place(&app, &a) > place(&app, &b), "moved down at once");
        // Moved back before the first answers: shown at once, sent after it.
        app.queue_action(
            machine,
            serde_json::json!({"action": "move", "id": id, "delta": -1}),
        );
        assert!(place(&app, &a) < place(&app, &b));
        // Both go to the runner, which starts one record's changes one at a time.
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 2);
        let mut queue = crate::resolve::Queue::default();
        for (n, m, request, _) in &ready {
            queue.push(*n, *m, app.resolver.key_of(*n), request);
        }
        assert_eq!(
            queue.startable(),
            vec![ready[0].0],
            "one record's changes go one at a time"
        );
        app.finish_job(ready[0].0, ready[0].2.clone(), done("Backlog order saved."));
        queue.done(ready[0].0);
        assert_eq!(queue.startable(), vec![ready[1].0]);
        assert_eq!(ready[1].2["delta"], -1);
        // B, to the bottom of its group, shows at once too.
        app.queue_action(
            machine,
            serde_json::json!({"action": "move", "id": id, "bottom": true}),
        );
        let last = app
            .backlog_order()
            .into_iter()
            .rfind(|&i| {
                let c = &app.board.cards[i];
                c.owner_key() == app.card(&a).unwrap().owner_key() && c.machine_index == machine
            })
            .map(|i| app.board.cards[i].key.clone());
        assert_eq!(last.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn a_failure_stays_up_until_a_key_comes_after_it_was_seen() {
        let mut app = demo_app();
        app.fail("#41 didn't start: the store is locked.");
        app.say("Select an intent first.", Tone::Info);
        assert!(app.failing(), "nothing said meanwhile replaces it");
        press(&mut app, 'j');
        assert!(
            app.failing(),
            "a key already on its way doesn't take it down"
        );
        app.fail("#42 wasn't marked done: refused.");
        assert_eq!(app.message().unwrap().0.lines().count(), 2, "both stay");
        if let Some(m) = app.message.as_mut() {
            m.2 = m.2.checked_sub(FAIL_SEEN).unwrap();
        }
        assert!(app.message().is_some(), "it never fades on its own");
        press(&mut app, 'j');
        assert!(!app.failing());
    }

    #[test]
    fn a_cancel_sets_the_card_aside_at_once_leaving_no_stray_workspace_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.cancellable())
            .cloned()
            .expect("work an agent is at in the demo");
        let (ws, before) = (card.workspace_id.clone(), app.board.cards.len());
        let id = card.detail.as_ref().unwrap().id.clone();
        app.queue_action(
            card.machine_index,
            serde_json::json!({"action": "cancel", "id": id, "session": card.session, "workspace": ws}),
        );
        assert!(app.card(&card.key).unwrap().dismissed());
        assert_eq!(
            app.board.cards.len(),
            before,
            "{:?}",
            app.board.cards.iter().map(|c| &c.key).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_started_card_moves_on_and_takes_no_second_start_until_the_feed_has_it() {
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let request = serde_json::json!({"action": "dispatch", "id": id});
        app.queue_action(machine, request.clone());
        let n = app.resolver.ready()[0].0;
        app.finish_job(n, request, done("Started."));
        app.rebuild();
        let card = app.card(&key).cloned().unwrap();
        assert_eq!(card.stage, Stage::Build, "started: out of the backlog");
        select_key(&mut app, &key);
        assert_eq!(
            enter(&mut app),
            Action::None,
            "no reopen of work just started"
        );
        assert!(app.message().unwrap().0.contains("is starting"));
    }

    #[test]
    fn a_failure_over_a_question_never_answers_it() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        select_key(&mut app, &key);
        press(&mut app, 'C');
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        settle(&mut app);
        app.fail("#9 didn't start: refused.");
        // A key on its way as the box appeared: it doesn't answer the question under it.
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        if let Some(m) = app.message.as_mut() {
            m.2 = m.2.checked_sub(FAIL_SEEN).unwrap();
        }
        // Seen: the next key only takes the box down.
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert!(!app.failing());
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        let Action::Mutate {
            machine: m,
            request,
        } = press(&mut app, 'y')
        else {
            panic!("the yes sent nothing: {:?}", app.message())
        };
        assert_eq!((m, request["id"].as_str()), (machine, Some(id.as_str())));
    }

    #[test]
    fn what_is_said_while_a_failure_is_up_shows_once_it_goes() {
        let mut app = demo_app();
        app.fail("#9 didn't start: refused.");
        app.say("That image is too big.", Tone::Warn);
        assert!(app.failing());
        if let Some(m) = app.message.as_mut() {
            m.2 = m.2.checked_sub(FAIL_SEEN).unwrap();
        }
        press(&mut app, 'j');
        assert_eq!(app.message(), Some(("That image is too big.", Tone::Warn)));
    }

    #[test]
    fn esc_and_q_never_slip_past_a_failure_not_yet_read() {
        let mut app = demo_app();
        app.fail("#9 didn't start: refused.");
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::None
        );
        assert!(app.failing());
        assert_eq!(press(&mut app, 'q'), Action::None);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.on_key(ctrl_c), Action::None);
        assert!(app.failing());
    }

    #[test]
    fn failures_come_up_a_few_at_a_time_and_none_goes_unseen() {
        let mut app = demo_app();
        for i in 1..=4 {
            app.fail(format!("#{i} didn't start."));
        }
        app.say("That image is too big.", Tone::Warn);
        app.say("Showing laptop again.", Tone::Info);
        let read = |app: &mut App| {
            if let Some(m) = app.message.as_mut() {
                m.2 = m.2.checked_sub(FAIL_SEEN).unwrap();
            }
            press(app, 'j');
        };
        read(&mut app);
        assert_eq!(
            app.message(),
            Some(("#4 didn't start.", Tone::Fail)),
            "the one that didn't fit"
        );
        read(&mut app);
        assert_eq!(
            app.message(),
            Some((
                "That image is too big.  ·  Showing laptop again.",
                Tone::Warn
            ))
        );
    }

    /// A card just started with d, ⏎ pressed on it: the board, and its id and machine.
    fn entered_while_starting() -> (App, String, String, usize) {
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        select_key(&mut app, &key);
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        app.queue_action(machine, request);
        select_key(&mut app, &key);
        assert_eq!(enter(&mut app), Action::None);
        (app, key, id, machine)
    }

    /// The feed names a workspace for this record, opened since the start, on the board.
    fn its_workspace_opens(app: &mut App, id: &str, machine: usize, ws: &str) {
        let name = app.machines[machine].name();
        let two = &id[id.len() - 2..];
        for c in app.machines[machine]
            .feed
            .as_mut()
            .unwrap()
            .cards
            .iter_mut()
            .filter(|c| c.id == id)
        {
            c.workspace = Some(crate::feed::FeedWorkspace {
                session: Some("web".into()),
                id: Some(ws.into()),
                ..Default::default()
            });
        }
        app.board.workspaces.push(crate::board::WorkspaceRow {
            machine: name,
            session: "web".into(),
            id: ws.into(),
            label: format!("{two}-the-work"),
            status: "working".into(),
            card: None,
            note: None,
            cached: false,
        });
    }

    #[test]
    fn enter_on_a_starting_card_goes_in_by_itself_when_its_workspace_opens() {
        let (mut app, key, id, machine) = entered_while_starting();
        let (said, _) = app.message().unwrap();
        assert!(
            said.contains("Kerdr goes in when its workspace opens"),
            "{said}"
        );
        assert!(said.contains("Esc stops waiting"), "{said}");
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        // Waits, with no key from you, as long as nothing has opened.
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        assert_eq!(app.key_wait(), KEY_WAIT_COMING);
        its_workspace_opens(&mut app, &id, machine, "w-new");
        let name = app.machines[machine].name();
        assert_eq!(
            app.enter_when_open_due(),
            Some(Action::JumpWorkspace {
                machine: name,
                session: "web".into(),
                id: "w-new".into()
            })
        );
        assert_eq!(app.enter_when_open, None, "once");
        assert_eq!(app.enter_when_open_due(), None);
    }

    #[test]
    fn enter_remembered_goes_in_once_the_card_itself_shows_open() {
        let (mut app, key, _, _) = entered_while_starting();
        let i = app.board.cards.iter().position(|c| c.key == key).unwrap();
        let card = &mut app.board.cards[i];
        card.session = Some("web".into());
        card.workspace_id = Some("w-new".into());
        assert!(app.card(&key).unwrap().is_open_workspace());
        assert_eq!(app.enter_when_open_due(), Some(Action::Jump(key)));
        assert_eq!(app.enter_when_open, None);
    }

    #[test]
    fn enter_remembered_is_dropped_by_esc_and_by_moving_off_the_card() {
        let (mut app, _, _, _) = entered_while_starting();
        esc(&mut app);
        assert_eq!(app.enter_when_open, None);
        assert!(app.message().unwrap().0.contains("Stopped waiting"));
        let (mut app, key, _, _) = entered_while_starting();
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .key
            .clone();
        select_key(&mut app, &other);
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open, None, "gone, even if you come back");
        select_key(&mut app, &key);
        assert_eq!(app.enter_when_open_due(), None);
    }

    #[test]
    fn enter_remembered_is_dropped_by_esc_from_a_peek_and_a_cards_screen() {
        // "Esc stops waiting" holds wherever ⏎ was pressed: the key also does its usual job, and
        // herdr doesn't take the screen later (intent 0491).
        for card_screen in [false, true] {
            let (mut app, key, id, machine) = entered_while_starting();
            app.mode = if card_screen {
                Mode::Card {
                    card: key.clone(),
                    focus: key.clone(),
                    opened: vec![],
                    back: Box::new(Mode::Board),
                }
            } else {
                Mode::Peek {
                    scroll: 0,
                    back: Box::new(Mode::Board),
                }
            };
            app.sel.key = Some(key);
            esc(&mut app);
            assert_eq!(app.enter_when_open, None, "{card_screen}");
            assert!(matches!(app.mode, Mode::Board), "{card_screen}");
            assert!(app.message().unwrap().0.contains("Stopped waiting"));
            its_workspace_opens(&mut app, &id, machine, "w-new");
            assert_eq!(app.enter_when_open_due(), None, "{card_screen}");
        }
    }

    #[test]
    fn enter_remembered_stays_through_esc_where_going_in_could_not_land_anyway() {
        // A help screen's Esc only closes it.
        let (mut app, key, _, _) = entered_while_starting();
        app.mode = Mode::Keys {
            filter: String::new(),
            typing: false,
            scroll: 0,
        };
        esc(&mut app);
        assert_eq!(app.enter_when_open, Some(key));
    }

    #[test]
    fn enter_remembered_takes_its_words_with_it_and_keeps_saying_so_after_they_expire() {
        let (mut app, key, _, _) = entered_while_starting();
        let (said, _) = app.message().unwrap();
        assert!(said.contains("Esc stops waiting"), "{said}");
        // The card's own line says Kerdr will go in, not that he has to press it.
        let card = app.card(&key).cloned().unwrap();
        let line = app.starting_line(&card).unwrap();
        assert!(line.contains("Kerdr goes in when"), "{line}");
        assert!(line.contains("Esc stops waiting"), "{line}");
        assert!(!line.contains("⏎"), "{line}");
        // Moved off the card, forgotten: nothing still promises it.
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .key
            .clone();
        select_key(&mut app, &other);
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.message(), None);
        // Not remembered: back to the card's plain line.
        let line = app.starting_line(&card).unwrap();
        assert!(line.contains("⏎ goes in"), "{line}");
    }

    #[test]
    fn enter_remembered_doesnt_take_down_something_said_since() {
        let (mut app, key, _, _) = entered_while_starting();
        app.say("Something else", Tone::Info);
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key)
            .unwrap()
            .key
            .clone();
        select_key(&mut app, &other);
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.message().unwrap().0, "Something else");
    }

    #[test]
    fn enter_remembered_is_dropped_when_the_start_fails_or_asks_first() {
        let (mut app, key, id, machine) = entered_while_starting();
        let start = serde_json::json!({"action": "dispatch", "id": id});
        reply_to(&mut app, machine, start, Err("no answer".into()));
        assert_eq!(app.enter_when_open, None);
        assert!(app.failing());
        let (mut app, _, id, machine) = entered_while_starting();
        let start = serde_json::json!({"action": "dispatch", "id": id});
        reply_to(&mut app, machine, start, Ok(over_limit(&id)));
        assert_eq!(app.enter_when_open, None);
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{key}");
    }

    #[test]
    fn enter_remembered_waits_out_a_failure_and_a_box_and_ends_when_no_workspace_comes() {
        let (mut app, key, id, machine) = entered_while_starting();
        // Never over a failure, which going in would hide, nor while typing.
        its_workspace_opens(&mut app, &id, machine, "w-new");
        app.fail("Something else");
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        app.message = None;
        let board = std::mem::replace(
            &mut app.mode,
            Mode::Keys {
                filter: String::new(),
                typing: false,
                scroll: 0,
            },
        );
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()));
        app.mode = board;
        assert!(matches!(
            app.enter_when_open_due(),
            Some(Action::JumpWorkspace { .. })
        ));
        // Answered yes, gone from the resolver, and still no workspace: says so and stops.
        let (mut app, key, id, machine) = entered_while_starting();
        let n = app
            .resolver
            .unanswered_where(|r| r["action"] == "dispatch")
            .unwrap();
        app.resolver.take(n);
        app.started = Some((id, Instant::now()));
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open.as_deref(), Some(key.as_str()), "lately");
        app.started = None;
        assert_eq!(app.enter_when_open_due(), None);
        assert_eq!(app.enter_when_open, None);
        assert!(app.message().unwrap().0.contains("stopped waiting"));
        assert!(machine < app.machines.len());
    }

    #[test]
    fn enter_on_work_it_is_still_asking_about_remembers_nothing() {
        // A question is all that can come, so nothing would ever go in.
        let mut app = demo_app();
        let (key, _) = done_card(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("Enter on a done card didn't ask to reopen it")
        };
        app.queue_action(machine, request);
        assert_eq!(enter(&mut app), Action::None);
        assert_eq!(app.enter_when_open, None, "{key}");
        assert!(app.start_awaited_on().is_empty());
    }

    #[test]
    fn only_a_start_actually_running_has_its_machine_read_again() {
        let (mut app, key, _, machine) = entered_while_starting();
        assert_eq!(app.start_awaited_on(), [machine]);
        // Forgotten: the timed feed will do, without the cost of reading it every few seconds.
        app.forget_enter();
        assert!(app.start_awaited_on().is_empty());
        // A ⏎ waiting on another card's start doesn't pay for this one.
        app.enter_when_open = Some("someone-else".into());
        assert!(app.start_awaited_on().is_empty());
        // A machine that isn't answering keeps its wait between tries: a feed can show nothing
        // from it, and one waiting on a person must not be tried every 5 s.
        app.enter_when_open = Some(key);
        assert_eq!(app.start_awaited_on(), [machine]);
        app.machines[machine].conn = reconnecting();
        assert!(app.start_awaited_on().is_empty());
    }

    #[test]
    fn a_stuck_sessions_cards_are_passed_over_and_refused_at_once() {
        // Its last good read keeps them on the board, but going in would hold the board for the
        // jump's whole limit (intent 0491).
        let mut app = demo_app();
        let open: Vec<String> = app
            .needs_you_order()
            .into_iter()
            .filter(|k| app.card(k).is_some_and(|c| c.is_open_workspace()))
            .collect();
        assert_eq!(open.len(), 2, "{open:?}");
        assert_eq!(
            app.others_needing_you(Some(&open[0])),
            app.needs_you_order().len() - 1
        );
        let card = app.card(&open[1]).cloned().unwrap();
        let session = card.session.clone().unwrap();
        let why = crate::poll::SESSION_SILENT.to_string();
        assert_eq!(app.stuck_refusal(card.machine_index, &session, "it"), None);
        assert!(app.next_needs_you_after(&open[0], &[]).is_some());
        app.machines[card.machine_index]
            .snapshot_errors
            .insert(session.clone(), why.clone());
        // Everything in that session stuck, the next one past it comes up, or none.
        let next = app.next_needs_you_after(&open[0], &[]);
        assert_ne!(next, Some(open[1].clone()));
        let said = app
            .stuck_refusal(card.machine_index, &session, &card.reference)
            .unwrap();
        assert!(said.contains(&card.reference), "{said}");
        assert!(said.contains(&why), "{said}");
        assert!(said.contains("R reads it again"), "{said}");
    }

    #[test]
    fn enter_on_a_starting_card_goes_into_the_workspace_it_opened() {
        // Intent 0445: ark-dispatch opens the workspace before its agent takes its brief, and
        // answers only then. ⏎ goes in as soon as one shows, and says it's starting up till then.
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        select_key(&mut app, &key);
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        app.queue_action(machine, request);
        select_key(&mut app, &key);
        assert_eq!(enter(&mut app), Action::None);
        let (said, _) = app.message().unwrap();
        assert!(said.contains("is starting up"), "{said}");
        assert!(said.contains("when its workspace opens"), "{said}");
        // Its card says so too, wherever it's looked at.
        let card = app.card(&key).cloned().unwrap();
        assert!(app.starting_line(&card).unwrap().contains("Starting up"));
        // x (or C, X) on it says what will work, and when.
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(app.message().unwrap().0.contains("X stops it"));
        // A workspace that was already there isn't its; one named for it that opened since is.
        let name = app.machines[machine].name();
        let two = &id[id.len() - 2..];
        let row = |id: &str| crate::board::WorkspaceRow {
            machine: name.clone(),
            session: "web".into(),
            id: id.into(),
            label: format!("{two}-the-work"),
            status: "working".into(),
            card: None,
            note: None,
            cached: false,
        };
        let i = app
            .resolver
            .unanswered_where(|r| r["action"] == "dispatch")
            .unwrap();
        app.resolver
            .note_workspaces(i, vec![("web".into(), "w-old".into())]);
        app.board.workspaces.push(row("w-old"));
        assert_eq!(enter(&mut app), Action::None);
        // One whose name carries its last two digits, as 0045's and 0145's both do, isn't
        // enough: Kerdr waits for a feed to name it for this record (review of intent 0445).
        app.board.workspaces.push(row("w-new"));
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("is starting up"));
        let named = |app: &mut App, ws: &str| {
            for c in app.machines[machine]
                .feed
                .as_mut()
                .unwrap()
                .cards
                .iter_mut()
                .filter(|c| c.id == id)
            {
                c.workspace = Some(crate::feed::FeedWorkspace {
                    session: Some("web".into()),
                    id: Some(ws.into()),
                    ..Default::default()
                });
            }
        };
        // Named for it, but already there when it was asked: an old one.
        named(&mut app, "w-old");
        assert_eq!(enter(&mut app), Action::None);
        named(&mut app, "w-new");
        assert_eq!(
            enter(&mut app),
            Action::JumpWorkspace {
                machine: name,
                session: "web".into(),
                id: "w-new".into()
            }
        );
        // In by his own ⏎: its answer takes him nowhere later.
        assert!(!app.resolver.holds_you());
    }

    #[test]
    fn a_reopen_s_answer_never_takes_you_back_into_the_workspace_you_left() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.done() && c.detail.is_some() && !c.dismissed())
            .cloned()
            .expect("a done card in the demo");
        let id = card.detail.as_ref().unwrap().id.clone();
        select_key(&mut app, &card.key);
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("⏎ didn't reopen it: {:?}", app.message())
        };
        app.queue_action(machine, request.clone());
        let n = app.resolver.ready()[0].0;
        app.resolver.started(n);
        // Its workspace opens while ark-dispatch waits for the agent to take its brief, and he
        // goes in (item 3), then comes back to the board with nothing else pressed.
        app.went_into(&card.key);
        select_key(&mut app, &card.key);
        app.finish_job(
            n,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: format!("Reopened {id}."),
                session: Some("web".into()),
                workspace: Some("w-re".into()),
                ..Default::default()
            }),
        );
        assert!(app.open_after.is_none(), "he's been there already");
        assert!(
            app.message().is_none_or(|(m, _)| !m.contains("goes in")),
            "{:?}",
            app.message()
        );
    }

    #[test]
    fn a_start_with_its_workspace_open_can_be_stopped_and_says_so_truthfully() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.is_open_workspace() && c.detail.is_some())
            .cloned()
            .expect("a card with its workspace open in the demo");
        let id = card.detail.as_ref().unwrap().id.clone();
        let n = app.resolver.add(
            card.machine_index,
            serde_json::json!({"action":"dispatch","id":id}),
            None,
            Some(card.key.clone()),
            None,
        );
        // Unanswered: X goes ahead (its cancel waits its turn behind the start); anything else
        // says what's really happening.
        assert!(!app.still_changing(&card, "cancel"));
        assert!(app.still_changing(&card, "close"));
        let said = app.message().unwrap().0.to_string();
        assert!(said.contains("workspace is open"), "{said}");
        assert!(said.contains("X stops it"), "{said}");
        assert!(!said.contains("Once its workspace opens"), "{said}");
        // Answered (here, or by a start that ran elsewhere), the copy with its workspace is
        // under way, and nothing is refused for it.
        app.resolver.ready();
        let mut job = app.resolver.take(n).unwrap();
        job.effect = Some(crate::resolve::Effect::StartingOn("far-box".into()));
        app.resolver.settle(job);
        assert!(app.resolver.starting(&id).is_some());
        assert!(!app.still_changing(&card, "close"));
    }

    #[test]
    fn d_on_work_being_written_up_starts_it_once_that_s_done() {
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let set_intake = |app: &mut App, intake: Option<&str>| {
            for c in app.machines[machine]
                .feed
                .as_mut()
                .unwrap()
                .cards
                .iter_mut()
            {
                if c.id == id {
                    c.intake = intake.map(str::to_string);
                }
            }
            app.rebuild();
        };
        set_intake(&mut app, Some("pending"));
        select_key(&mut app, &key);
        // Remembered, not refused: handed to the runner, which holds it until the write-up's
        // done, wherever Andrew is meanwhile (intent 0445).
        assert_eq!(press(&mut app, 'd'), Action::None);
        let (said, _) = app.message().unwrap();
        assert!(said.contains("starts as soon as that's done"), "{said}");
        assert!(app.starts_when_written(&key));
        let ready = app.resolver.ready();
        assert_eq!(ready.len(), 1);
        let (n, m, request, gated) = ready[0].clone();
        assert!(gated, "it waits for the write-up");
        assert_eq!(
            (request["action"].as_str(), request["id"].as_str()),
            (Some("dispatch"), Some(id.as_str()))
        );
        // Shown as waiting, not as starting, and nothing else counts it as a start yet.
        let card = app.card(&key).cloned().unwrap();
        assert!(
            app.starting_line(&card)
                .unwrap()
                .contains("Being written up")
        );
        assert!(app.resolver.starting(&id).is_none());
        assert_eq!(app.quit_held(), None);
        assert!(
            app.quit_loses()
                .unwrap()
                .contains("starts once it's written up")
        );
        // A second d asks for no second start.
        press(&mut app, 'd');
        assert!(app.resolver.ready().is_empty());
        // The runner holds it until its machine's feed shows it written up.
        let mut queue = crate::resolve::Queue::default();
        queue.push_gated(n, m, app.resolver.key_of(n), &request);
        let intake = |v: Option<&str>| {
            std::collections::HashMap::from([(id.clone(), v.map(str::to_string))])
        };
        assert!(queue.written(m, &intake(Some("pending"))).is_empty());
        assert!(queue.startable().is_empty());
        assert!(queue.written(m, &intake(None)).is_empty());
        assert_eq!(queue.startable(), vec![n]);
        // Started: it shows as starting from now.
        assert!(app.resolver.started(n).shows_now);
        app.rebuild();
        assert!(!app.starts_when_written(&key));
        assert!(app.resolver.starting(&id).is_some());
        app.finish_job(n, request, done("Started."));
        // Esc on the board drops one still waiting, and the runner is told.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        set_intake(&mut app, Some("pending"));
        select_key(&mut app, &key);
        press(&mut app, 'd');
        let n = app.resolver.ready()[0].0;
        esc(&mut app);
        assert!(!app.starts_when_written(&key));
        assert_eq!(app.resolver.take_dropped(), vec![n]);
        assert!(app.message().unwrap().0.contains("won't start after all"));
    }

    #[test]
    fn a_start_waiting_for_a_write_up_that_fails_never_starts_and_says_so() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        for c in app.machines[machine]
            .feed
            .as_mut()
            .unwrap()
            .cards
            .iter_mut()
        {
            if c.id == id {
                c.intake = Some("pending".into());
            }
        }
        app.rebuild();
        select_key(&mut app, &key);
        press(&mut app, 'd');
        let (n, m, request, _) = app.resolver.ready()[0].clone();
        let mut queue = crate::resolve::Queue::default();
        queue.push_gated(n, m, app.resolver.key_of(n), &request);
        let failed = std::collections::HashMap::from([(id.clone(), Some("failed".to_string()))]);
        assert_eq!(queue.written(m, &failed), vec![n]);
        // It starts from his words as they are, and the board says so (intent 0591).
        assert_eq!(queue.startable(), vec![n]);
        app.starts_from_words(n);
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Info);
        assert!(
            said.contains("couldn't be written up, so it starts from your words as they are"),
            "{said}"
        );
    }

    #[test]
    fn q_waits_only_for_what_would_be_lost() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (_, id, machine) = waiting_card(&app);
        app.queue_action(machine, serde_json::json!({"action": "dispatch", "id": id}));
        let n = app.resolver.ready()[0].0;
        // Handed to the runner but not yet started: it would never be sent.
        assert!(app.quit_held().is_some());
        app.resolver.started(n);
        // Started, but its command doesn't have the whole request yet: quitting now would cut it
        // off (review of intent 0445).
        let held = app.quit_held().unwrap();
        assert!(held.contains("still being handed to its command"), "{held}");
        app.resolver.handed(n);
        // Its command has it all: it carries on without Kerdr, and q says so before it quits.
        assert_eq!(app.quit_held(), None);
        let said = app.still_running().unwrap();
        assert!(
            said.contains("is still starting; its command has the whole request and carries on after Kerdr quits"),
            "{said}"
        );
        // A new intent holds it until it's answered: its words may still be on their way.
        let request = send_words(&mut app, "Back up the NAS");
        app.queue_action(0, request);
        let n = app.resolver.ready()[0].0;
        app.resolver.started(n);
        assert!(app.quit_held().unwrap().contains("new intent"));
    }

    #[test]
    fn q_says_first_what_only_kerdr_keeps() {
        let mut app = demo_app();
        assert_eq!(app.quit_loses(), None);
        // Words waiting for their image.
        app.send_after_load = Some((
            1,
            Draft {
                words: "Back up the NAS".into(),
                ..Default::default()
            },
        ));
        assert!(app.quit_loses().unwrap().contains("once its image is in"));
        app.send_after_load = None;
        // Words a failed save gave back, kept for c.
        app.returned.push_back(Returned {
            words: "Back up the NAS".into(),
            images: Vec::new(),
            from: None,
        });
        assert!(app.quit_loses().unwrap().contains("c brings them back"));
        app.returned.clear();
        app.draft = "half a thought".into();
        assert!(app.quit_loses().is_some());
    }

    #[test]
    fn a_start_asked_for_on_another_machine_shows_as_starting_there() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        let request = serde_json::json!({"action": "dispatch", "id": id});
        app.queue_action(machine, request.clone());
        let n = app.resolver.ready()[0].0;
        app.finish_job(
            n,
            request,
            Ok(crate::actions::Response {
                ok: true,
                message: "Asked for on far-box: it's starting in web there.".into(),
                machine: Some("far-box".into()),
                ..Default::default()
            }),
        );
        let card = app.card(&key).cloned().unwrap();
        assert_eq!(
            card.detail.as_ref().unwrap().status.as_deref(),
            Some("Starting on far-box…")
        );
        assert_eq!(card.stage, Stage::Backlog, "it hasn't started here");
        assert!(app.message().unwrap().0.contains("Asked for on far-box"));
        // Reads go by without the start reaching this copy: it still shows.
        for _ in 0..3 {
            let feed = app.machines[machine].feed.clone();
            app.resolver.read(machine, feed.as_ref());
        }
        app.rebuild();
        assert!(
            app.card(&key)
                .unwrap()
                .detail
                .as_ref()
                .unwrap()
                .status
                .as_deref()
                == Some("Starting on far-box…")
        );
    }

    #[test]
    fn a_reopen_of_done_work_reads_as_reopening_where_it_is() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let card = app
            .board
            .cards
            .iter()
            .find(|c| c.can_reopen_done())
            .cloned()
            .expect("done work in the demo");
        let id = card.detail.as_ref().unwrap().id.clone();
        app.queue_action(
            card.machine_index,
            serde_json::json!({"action": "reopen", "id": id, "override": true}),
        );
        let now = app.card(&card.key).unwrap();
        assert_eq!(now.stage, Stage::Maintain, "it stays where it is");
        assert_eq!(now.why, "Reopening…");
        assert_ne!(now.attention, Attention::Done);
    }

    #[test]
    fn d_in_the_backlog_view_keeps_the_starting_card_selected_and_takes_no_second_start() {
        let mut app = demo_app();
        room_in_every_session(&mut app);
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _, _) = waiting_card(&app);
        app.enter_backlog(false);
        let at = app
            .backlog_cards()
            .iter()
            .position(|&i| app.board.cards[i].key == key)
            .unwrap();
        app.mode = Mode::Backlog { sel: at };
        app.backlog_focus = Some(key.clone());
        let Action::Mutate { machine, request } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        app.queue_action(machine, request);
        assert_eq!(
            app.backlog_card_for_keys().map(|c| c.key.clone()),
            Some(key.clone()),
            "the starting card stays, selected"
        );
        // d again, or held: the same card, which refuses; never its neighbour.
        assert_eq!(press(&mut app, 'd'), Action::None);
        assert!(app.message().unwrap().0.contains("is starting up"));
        // Its peek says what's under way, not that it's stalled.
        let card = app.card(&key).cloned().unwrap();
        assert!(
            app.starting_line(&card)
                .unwrap()
                .starts_with("◐ Starting up")
        );
    }

    #[test]
    fn a_change_on_its_way_says_so_in_its_peek() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id, machine) = waiting_card(&app);
        app.queue_action(machine, serde_json::json!({"action": "close", "id": id}));
        let card = app.card(&key).cloned().unwrap();
        assert!(
            app.starting_line(&card)
                .unwrap()
                .contains("Marking it done")
        );
    }

    /// The demo session a backlog card's start asks about, on the machine its d goes to.
    fn start_question(
        app: &mut App,
        key: &str,
        used: u32,
        choices: Vec<crate::actions::Choice>,
    ) -> (usize, serde_json::Value) {
        select_key(app, key);
        let Action::Mutate { machine, request } = press(app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        let session = app.machines[machine]
            .feed
            .as_ref()
            .and_then(|f| f.sessions.iter().find(|(_, s)| s.max.is_some()))
            .map(|(name, _)| name.clone())
            .expect("a session with a most in the demo");
        let asks = crate::actions::Response {
            ok: false,
            message: format!(
                "{session} has {used} workspaces, past its usual limit. Start it anyway?"
            ),
            confirmation: true,
            confirm: Some("0123456789ab0011".into()),
            used: Some(used),
            limit: Some(3),
            session: Some(session),
            choices,
            ..Default::default()
        };
        reply_to(app, machine, request.clone(), Ok(asks));
        settle(app);
        (machine, request)
    }

    fn a_choice(id: &str, key: &str, label: &str) -> crate::actions::Choice {
        crate::actions::Choice {
            id: id.into(),
            key: key.into(),
            label: label.into(),
        }
    }

    #[test]
    fn a_choice_sends_its_question_again_with_the_same_token_and_no_override() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = waiting_backlog_card(&app);
        start_question(
            &mut app,
            &key,
            3,
            vec![
                a_choice("queue", "w", "queue it"),
                // y is the yes: it takes the first free digit.
                a_choice("shelve-oldest", "y", "shelve the oldest, then start it"),
            ],
        );
        let Mode::Confirm { choices, .. } = &app.mode else {
            panic!("no question: {:?}", app.message())
        };
        assert_eq!(choices.len(), 2);
        // Shown beside yes and no, each with its key, one to a line.
        let screen = crate::ui::tests_render(&app, 120, 40);
        for shown in [
            "Enter / y start anyway",
            "w queue it",
            "1 shelve the oldest, then start it",
            "Esc / n leave it",
        ] {
            assert!(screen.contains(shown), "{shown} isn't on screen");
        }
        let Action::Mutate { request, .. } = press(&mut app, '1') else {
            panic!("the choice sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "dispatch");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
        assert_eq!(request["choice"], "shelve-oldest");
        assert_eq!(request["confirm"], "0123456789ab0011");
        assert!(request.get("override").is_none() && request.get("queue").is_none());
    }

    #[test]
    fn at_the_most_a_session_holds_enter_queues_and_there_is_no_yes() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        start_question(&mut app, &key, 5, vec![a_choice("queue", "w", "queue it")]);
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("no question: {:?}", app.message())
        };
        assert!(title.starts_with("Queue #"), "{title}");
        let screen = crate::ui::tests_render(&app, 120, 40);
        assert!(!screen.contains("start anyway"), "{screen}");
        assert!(screen.contains("Enter / w queue it"));
        // y does nothing here; Enter queues it.
        assert_eq!(press(&mut app, 'y'), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("Enter queued nothing: {:?}", app.message())
        };
        assert_eq!(request["choice"], "queue");
        assert!(request.get("override").is_none());
    }

    #[test]
    fn a_question_that_arrives_while_you_are_elsewhere_waits_on_its_card() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = waiting_backlog_card(&app);
        select_key(&mut app, &key);
        let Action::Mutate { machine, request } = press(&mut app, 'd') else {
            panic!("d sent nothing")
        };
        // On another card when it comes.
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && c.stage == Stage::Backlog)
            .unwrap()
            .key
            .clone();
        select_key(&mut app, &other);
        let asks = crate::actions::Response {
            ok: false,
            message: "web has 3 workspaces, its usual limit. Start it anyway?".into(),
            confirmation: true,
            confirm: Some("0123456789ab0012".into()),
            used: Some(3),
            limit: Some(3),
            session: Some("web".into()),
            ..Default::default()
        };
        reply_to(&mut app, machine, request, Ok(asks));
        assert!(matches!(app.mode, Mode::Board));
        let (said, tone) = app.message().unwrap();
        assert_eq!(tone, Tone::Info, "{said}");
        assert!(
            said.contains("asks you first") && said.contains("n goes to it"),
            "{said}"
        );
        // Its card shows it as asking, never as failed, and a read keeps it so.
        app.rebuild();
        let card = app.card(&key).unwrap();
        assert_eq!(card.attention, Attention::NeedsYou);
        assert!(card.why.starts_with("asks you:"), "{}", card.why);
        assert!(app.needs_you_order().contains(&key));
        // n finds it and asks.
        for _ in 0..app.needs_you_order().len() {
            press(&mut app, 'n');
            if matches!(app.mode, Mode::Confirm { .. }) {
                break;
            }
        }
        let Mode::Confirm { request, .. } = &app.mode else {
            panic!("n never asked: {:?}", app.message())
        };
        assert_eq!(request["confirm"], "0123456789ab0012");
        assert_eq!(app.sel.key.as_deref(), Some(key.as_str()));
        assert!(app.waiting_on(&key).is_none());
    }

    #[test]
    fn a_question_is_shown_whole() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = closed_workspace_card(&mut app);
        let request = reopen_by_enter(&mut app);
        let long = format!(
            "{} The end of it shows.",
            "It was closed by its agent after the work was merged and its tab was shut. ".repeat(6)
        );
        let mut asks = reopen_reply("web", false, true, "0123456789ab0004").unwrap();
        asks.message = long.clone();
        reply_to(&mut app, 0, request, Ok(asks));
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("no question")
        };
        assert_eq!(message, &long);
        let screen = crate::ui::tests_render(&app, 120, 50);
        // Its last words, wrapped, still in the box.
        assert!(screen.contains("it shows."), "{screen}");
        assert!(app.card(&key).is_some());
    }

    /// Every machine's backend does each key's step first (intent 0591): its feed says "steps".
    fn steps_everywhere(app: &mut App) {
        for machine in &mut app.machines {
            if let Some(feed) = machine.feed.as_mut() {
                feed.supports = vec!["op".into(), "retry".into(), "follow".into(), "steps".into()];
            }
        }
    }

    /// A backlog card set aside in Won't do, selected on the board.
    fn wont_do_card(app: &mut App) -> (String, String) {
        let (key, id) = waiting_backlog_card(app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.detail.as_mut().unwrap().disposition = Some("wont_do".into());
        app.sel = Selection {
            col: Stage::Backlog.index(),
            key: Some(key.clone()),
        };
        (key, id)
    }

    #[test]
    fn c_and_x_on_work_an_agent_may_be_at_go_to_a_backend_that_asks_itself() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        // A backend without it: refused, as 0.33.0 did.
        assert_eq!(press(&mut app, 'C'), Action::None);
        assert!(app.message().unwrap().0.contains("a workspace open"));
        steps_everywhere(&mut app);
        for (c, action) in [('C', "close"), ('x', "dismiss")] {
            app.resolver = Default::default();
            let Action::Mutate { machine, request } = press(&mut app, c) else {
                panic!("{c} sent nothing: {:?}", app.message())
            };
            assert_eq!(request, serde_json::json!({"action": action, "id": id}));
            // Nothing shows on the card until its yes: the backend asks first.
            assert!(app.effect_for(&key, &request).is_none());
            // Its question, with wrap-up beside yes: w asks the agent to wrap up instead.
            let asks = crate::actions::Response {
                ok: false,
                message: format!(
                    "An agent is at {} (workspace 07-build in web). Stop it?",
                    app.card(&key).unwrap().reference
                ),
                confirmation: true,
                confirm: Some("00000000000000aa".into()),
                session: Some("web".into()),
                choices: vec![a_choice("wrap-up", "w", "ask it to wrap up")],
                ..Default::default()
            };
            reply_to(&mut app, machine, request, Ok(asks));
            settle(&mut app);
            let Mode::Confirm { title, .. } = &app.mode else {
                panic!("no question: {:?}", app.message())
            };
            assert!(
                title.starts_with(if c == 'C' { "Mark" } else { "Won't do" }),
                "{title}"
            );
            let Action::Mutate { request, .. } = press(&mut app, 'w') else {
                panic!("w sent nothing: {:?}", app.message())
            };
            assert_eq!(request["choice"], "wrap-up");
            assert_eq!(request["confirm"], "00000000000000aa");
            assert!(request.get("override").is_none());
        }
    }

    #[test]
    fn a_late_question_about_stopping_running_work_waits_on_its_card_and_stays_answerable() {
        // Codex's review: C's question about work an agent is at, come after you'd moved on,
        // waits on its card as the backend's question it is, never dropped because the card has
        // a workspace open.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = running_card(&mut app);
        steps_everywhere(&mut app);
        let Action::Mutate { machine, request } = press(&mut app, 'C') else {
            panic!("C sent nothing: {:?}", app.message())
        };
        let other = app
            .board
            .cards
            .iter()
            .find(|c| c.key != key && c.detail.is_some())
            .unwrap()
            .key
            .clone();
        app.sel.key = Some(other);
        let asks = crate::actions::Response {
            ok: false,
            message: "An agent is at it (workspace 07-build in web). Stop it and mark it done?"
                .into(),
            confirmation: true,
            confirm: Some("00000000000000bb".into()),
            session: Some("web".into()),
            choices: vec![a_choice("wrap-up", "w", "ask it to wrap up")],
            ..Default::default()
        };
        reply_to(&mut app, machine, request, Ok(asks));
        assert!(
            app.waiting_on(&key).is_some(),
            "dropped: {:?}",
            app.message()
        );
        assert!(app.card(&key).unwrap().why.starts_with("asks you"));
        // Back on it, C opens it, and its yes goes with its token.
        app.sel.key = Some(key.clone());
        assert_eq!(press(&mut app, 'C'), Action::None);
        assert!(matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
        settle(&mut app);
        let Action::Mutate { request, .. } = press(&mut app, 'y') else {
            panic!("the yes sent nothing: {:?}", app.message())
        };
        assert_eq!(request["action"], "close");
        assert_eq!(request["id"].as_str(), Some(id.as_str()));
        assert_eq!(request["confirm"], "00000000000000bb");
        assert_eq!(request["override"], true);
    }

    #[test]
    fn keys_on_work_in_wont_do_start_or_finish_it_where_the_backend_brings_it_back() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = wont_do_card(&mut app);
        let reference = app.card(&key).unwrap().reference.clone();
        // A backend without it: each says u brings it back, as before.
        for c in ['d', 'C'] {
            assert_eq!(press(&mut app, c), Action::None);
            assert!(app.message().unwrap().0.contains("u brings it back"), "{c}");
        }
        assert_eq!(enter(&mut app), Action::None);
        assert!(app.message().unwrap().0.contains("u brings it back"));
        steps_everywhere(&mut app);
        // d means starting it: sent at once, for its backend to bring it back and start it.
        let Action::Mutate { request, .. } = press(&mut app, 'd') else {
            panic!("d sent nothing: {:?}", app.message())
        };
        assert_eq!(request, serde_json::json!({"action": "dispatch", "id": id}));
        app.resolver = Default::default();
        // ⏎ asks once, since Won't do was his own choice; its yes sends the start as it is.
        assert_eq!(enter(&mut app), Action::None);
        let Mode::Confirm { title, .. } = &app.mode else {
            panic!("⏎ asked nothing: {:?}", app.message())
        };
        assert_eq!(title, &format!("Bring {reference} back and start it?"));
        settle(&mut app);
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the yes sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "dispatch", "id": id}));
        app.resolver = Default::default();
        // C asks as any C does, and sends the close for its backend to bring it back first.
        assert_eq!(press(&mut app, 'C'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("C asked nothing: {:?}", app.message())
        };
        assert_eq!(title, &format!("Mark {reference} done?"));
        assert!(message.contains("brings it back"));
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the yes sent nothing")
        };
        assert_eq!(request, serde_json::json!({"action": "close", "id": id}));
        app.resolver = Default::default();
        // Its yes waits for nothing that changed: brought back meanwhile, it asks again.
        assert_eq!(press(&mut app, 'C'), Action::None);
        app.board
            .cards
            .iter_mut()
            .find(|c| c.key == key)
            .unwrap()
            .detail
            .as_mut()
            .unwrap()
            .disposition = None;
        assert_eq!(enter(&mut app), Action::None);
        let Mode::Confirm { message, .. } = &app.mode else {
            panic!("not asked again: {:?}", app.message())
        };
        assert!(message.contains("brought back meanwhile"), "{message}");
        app.mode = Mode::Board;
        // The ordering keys have no backlog place to change for it.
        app.board
            .cards
            .iter_mut()
            .find(|c| c.key == key)
            .unwrap()
            .detail
            .as_mut()
            .unwrap()
            .disposition = Some("wont_do".into());
        app.mode = Mode::Board;
        assert_eq!(press(&mut app, 't'), Action::None);
        assert!(
            app.message()
                .unwrap()
                .0
                .contains("no place in the backlog order")
        );
    }

    #[test]
    fn x_on_done_work_goes_to_a_backend_that_asks_to_move_it_to_wont_do() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = done_card(&mut app);
        assert_eq!(press(&mut app, 'x'), Action::None);
        assert!(app.message().unwrap().0.contains("is done"));
        steps_everywhere(&mut app);
        let Action::Mutate { request, .. } = press(&mut app, 'x') else {
            panic!("x sent nothing: {:?}", app.message())
        };
        assert_eq!(request, serde_json::json!({"action": "dismiss", "id": id}));
        assert!(app.effect_for(&key, &request).is_none());
    }

    #[test]
    fn x_upper_on_done_work_still_open_stops_its_agent_and_leaves_it_done() {
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, id) = done_card(&mut app);
        let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
        card.workspace_id = Some("w9".into());
        card.session = Some("web".into());
        let reference = card.reference.clone();
        // A backend without it: done already, as before.
        press(&mut app, 'X');
        assert!(app.message().unwrap().0.contains("done already"));
        steps_everywhere(&mut app);
        assert_eq!(press(&mut app, 'X'), Action::None);
        let Mode::Confirm { title, message, .. } = &app.mode else {
            panic!("X asked nothing: {:?}", app.message())
        };
        assert_eq!(title, &format!("Stop {reference}'s agent?"));
        assert!(message.contains("stays done"), "{message}");
        let Action::Mutate { request, .. } = enter(&mut app) else {
            panic!("the yes sent nothing: {:?}", app.message())
        };
        assert_eq!(
            request,
            serde_json::json!({"action": "cancel", "id": id, "session": "web", "workspace": "w9"})
        );
        // While it goes, the card stays done: never shown in Won't do (Codex's review).
        assert!(app.effect_for(&key, &request).is_none());
        app.queue_action(app.card(&key).unwrap().machine_index, request);
        app.rebuild();
        assert!(
            !app.card(&key).unwrap().dismissed(),
            "a done card showed in Won't do"
        );
    }

    #[test]
    fn x_upper_on_done_work_asks_the_backend_where_its_workspace_runs_about_steps() {
        // Codex's review: X's request goes to the machine its workspace runs on, so that
        // machine's backend, not its home's, decides whether it does the step.
        let mut app = demo_app();
        app.cfg.actions.command = Some("test-adapter".into());
        let (key, _) = done_card(&mut app);
        let runs = app.card(&key).unwrap().machine_index;
        let home = (0..app.machines.len())
            .find(|&m| m != runs && app.machines[m].feed.is_some())
            .expect("a second machine with a feed");
        let home_name = app.machines[home].name();
        let set = |app: &mut App| {
            let card = app.board.cards.iter_mut().find(|c| c.key == key).unwrap();
            card.workspace_id = Some("w9".into());
            card.session = Some("web".into());
            card.detail.as_mut().unwrap().home = Some(crate::feed::Home {
                machine: Some(home_name.clone()),
                session: Some("web".into()),
            });
        };
        set(&mut app);
        let steps = |app: &mut App, on: usize| {
            for (m, machine) in app.machines.iter_mut().enumerate() {
                if let Some(feed) = machine.feed.as_mut() {
                    feed.supports = vec!["op".into(), "retry".into(), "follow".into()];
                    if m == on {
                        feed.supports.push("steps".into());
                    }
                }
            }
        };
        // Its home does the step, but the machine its workspace runs on doesn't: as before.
        steps(&mut app, home);
        assert_eq!(press(&mut app, 'X'), Action::None);
        assert!(!matches!(app.mode, Mode::Confirm { .. }), "{:?}", app.mode);
        assert!(app.message().unwrap().0.contains("done already"));
        // The machine its workspace runs on does: X asks, and its yes goes there.
        steps(&mut app, runs);
        set(&mut app);
        assert_eq!(press(&mut app, 'X'), Action::None);
        assert!(
            matches!(app.mode, Mode::Confirm { .. }),
            "{:?}",
            app.message()
        );
        let Action::Mutate { machine, request } = enter(&mut app) else {
            panic!("the yes sent nothing: {:?}", app.message())
        };
        assert_eq!(machine, runs);
        assert_eq!(request["action"], "cancel");
    }

    #[test]
    fn m_opens_the_machines_screen_on_the_machine_the_banner_names() {
        // Intent 0591, K3-38 to K3-41: its ⏎ or u is the next key, with no hunting for it.
        let mut app = demo_app();
        let last = app.machines.len() - 1;
        for m in &mut app.machines {
            if m.conn.needs_you() {
                m.conn = crate::poll::Conn::Live {
                    latency: Duration::from_millis(9),
                };
            }
        }
        app.machines[last].conn = crate::poll::Conn::HostKey {
            detail: "the host key changed".into(),
        };
        assert_eq!(app.troubled_machines().first(), Some(&last));
        press(&mut app, 'M');
        let Mode::Machines { sel } = app.mode else {
            panic!("not the machines screen: {:?}", app.mode)
        };
        assert_eq!(app.machine_rows()[sel], last);
        assert_eq!(enter(&mut app), Action::Answer(last));
    }

    #[test]
    fn this_machine_hidden_leaves_the_board_and_is_still_read() {
        // Intent 0591, K1-10: x hides this machine's cards too, and Kerdr keeps reading it for
        // its accounts and the actions it runs.
        let mut app = demo_app();
        let local = app
            .machines
            .iter()
            .position(|m| m.machine.is_local())
            .expect("this machine in the demo");
        app.mode = Mode::Machines { sel: local };
        assert_eq!(press(&mut app, 'x'), Action::ToggleHidden(local));
        let had = app
            .board
            .cards
            .iter()
            .filter(|c| c.machine_index == local)
            .count();
        assert!(had > 0, "the demo shows this machine's cards");
        app.machines[local].machine.hidden = true;
        app.rebuild();
        assert!(app.machines[local].is_shown(), "still read");
        assert!(!app.machines[local].on_board());
        assert_eq!(
            app.board
                .cards
                .iter()
                .filter(|c| c.machine_index == local)
                .count(),
            0
        );
        // ⏎ on its row shows its cards again.
        app.mode = Mode::Machines { sel: local };
        assert_eq!(enter(&mut app), Action::ToggleHidden(local));
    }
}
