//! Families of cards: what an agent made for a card (its children, the undertaker seeing its
//! agents through, the follow-ups that came out of its work) sits inside that card rather than on
//! the board. Each record names at most one card it sits in: its `parent`, else `undertaker_for`,
//! else `from`, matched by id within the same feed, across every machine's feed.

use std::collections::HashMap;

use crate::model::{Attention, Card, Link, Source, Stage};

/// An id as records write it, for matching: `#256`, `0256` and `256` are the same record.
pub fn norm_id(id: &str) -> String {
    let id = id.trim().trim_start_matches('#');
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        let trimmed = id.trim_start_matches('0');
        if trimmed.is_empty() { "0" } else { trimmed }.to_string()
    } else {
        id.to_string()
    }
}

/// The handle a link shows by, `#256` for a number, as the card itself would show it.
fn link_ref(id: &str) -> String {
    let n = norm_id(id);
    if n.chars().all(|c| c.is_ascii_digit()) {
        format!("#{n}")
    } else {
        n
    }
}

/// The feed a feed card came from: its key without the id at the end.
fn source_of(card: &Card) -> Option<&str> {
    let id = &card.detail.as_ref()?.id;
    card.key.strip_suffix(id.as_str())?.strip_suffix(':')
}

/// Sets each feed card's link and the card it sits inside. A link to a record no feed has keeps
/// its `link` (so the card can say "in #256") with no `inside`; a loop of links is broken where it
/// closes, so every card stays reachable from the top of its family.
pub fn link(cards: &mut [Card]) {
    let mut by_id: HashMap<(String, String), usize> = HashMap::new();
    for (i, c) in cards.iter().enumerate() {
        if c.source != Source::Feed {
            continue;
        }
        if let (Some(source), Some(d)) = (source_of(c), c.detail.as_ref()) {
            by_id.insert((source.to_string(), norm_id(&d.id)), i);
        }
    }
    fn named(v: &Option<String>) -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
    let mut inside: Vec<Option<usize>> = vec![None; cards.len()];
    for (i, c) in cards.iter_mut().enumerate() {
        let Some(d) = c.detail.as_ref() else { continue };
        let found = named(&d.parent)
            .map(|id| (id, Link::Part))
            .or_else(|| named(&d.undertaker_for).map(|id| (id, Link::Undertaker)))
            .or_else(|| named(&d.from).map(|id| (id, Link::FollowUp)));
        let Some((id, kind)) = found else { continue };
        let target = source_of(c)
            .and_then(|s| by_id.get(&(s.to_string(), norm_id(&id))))
            .copied();
        // A record naming itself sits in nothing.
        if target == Some(i) {
            continue;
        }
        c.link = Some((link_ref(&id), kind));
        inside[i] = target;
    }
    // Follow each chain once; a link back into the chain being followed closes a loop, and is
    // dropped there.
    let mut state = vec![0u8; cards.len()];
    let mut order: Vec<usize> = (0..cards.len()).collect();
    order.sort_by(|&a, &b| cards[a].key.cmp(&cards[b].key));
    for start in order {
        if state[start] != 0 {
            continue;
        }
        let mut path = vec![start];
        state[start] = 1;
        let mut cur = start;
        while let Some(next) = inside[cur] {
            match state[next] {
                2 => break,
                1 => {
                    inside[cur] = None;
                    cards[cur].link = None;
                    break;
                }
                _ => {
                    state[next] = 1;
                    path.push(next);
                    cur = next;
                }
            }
        }
        for p in path {
            state[p] = 2;
        }
    }
    for i in 0..cards.len() {
        cards[i].inside = inside[i].map(|t| cards[t].key.clone());
    }
}

/// By card, the index of the card it sits inside, and of the cards directly inside it.
pub fn tree(cards: &[Card]) -> (Vec<Option<usize>>, Vec<Vec<usize>>) {
    let index: HashMap<&str, usize> = cards
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key.as_str(), i))
        .collect();
    let up: Vec<Option<usize>> = cards
        .iter()
        .map(|c| c.inside.as_deref().and_then(|k| index.get(k).copied()))
        .collect();
    let mut down = vec![Vec::new(); cards.len()];
    for (i, u) in up.iter().enumerate() {
        if let Some(u) = u {
            down[*u].push(i);
        }
    }
    (up, down)
}

/// Where a piece of work stands, as its card's screen groups it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    NeedsYou,
    UnderWay,
    NotStarted,
    Done,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::NeedsYou,
        Group::UnderWay,
        Group::NotStarted,
        Group::Done,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::NeedsYou => "NEEDS YOU",
            Group::UnderWay => "UNDER WAY",
            Group::NotStarted => "NOT STARTED",
            Group::Done => "DONE",
        }
    }

    pub fn words(self, n: usize) -> String {
        match self {
            Group::NeedsYou if n == 1 => "1 needs you".into(),
            Group::NeedsYou => format!("{n} need you"),
            Group::UnderWay => format!("{n} under way"),
            Group::NotStarted => format!("{n} not started"),
            Group::Done => format!("{n} done"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Group::NeedsYou => "needs",
            Group::UnderWay => "under-way",
            Group::NotStarted => "not-started",
            Group::Done => "done",
        }
    }
}

/// Finished: closed, done, or set aside as won't do.
pub fn finished(c: &Card) -> bool {
    c.stage == Stage::Maintain
        || c.attention == Attention::Done
        || c.dismissed()
        || c.state.as_deref() == Some("closed")
}

/// A piece that needs you: asking, waiting on you, blocked, or open with no agent at it.
pub fn needs(c: &Card) -> bool {
    !finished(c)
        && (c.needs_you() || c.attention == Attention::Blocked || c.no_agent())
        && c.cached_at.is_none()
}

/// Open work waiting on another intent that isn't done.
pub fn waits(c: &Card) -> bool {
    c.detail
        .as_ref()
        .is_some_and(|d| d.waits_on.iter().any(|w| w.open != Some(false)))
}

pub fn group(c: &Card) -> Group {
    if finished(c) {
        Group::Done
    } else if needs(c) {
        Group::NeedsYou
    } else if c.stage == Stage::Backlog {
        Group::NotStarted
    } else {
        Group::UnderWay
    }
}

/// The mark a piece shows by: ▲ needs you, ○ no agent at it, ● under way, ■ waiting, ◇ waits on
/// another intent, · not started, ✓ done, × won't do. Its kind names its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    NeedsYou,
    NoAgent,
    Running,
    Waiting,
    WaitsOn,
    NotStarted,
    Done,
    WontDo,
}

impl Mark {
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::NeedsYou => "▲",
            Mark::NoAgent => "○",
            Mark::Running => "●",
            Mark::Waiting => "■",
            Mark::WaitsOn => "◇",
            Mark::NotStarted => "·",
            Mark::Done => "✓",
            Mark::WontDo => "×",
        }
    }
}

pub fn mark(c: &Card) -> Mark {
    if c.dismissed() {
        Mark::WontDo
    } else if finished(c) {
        Mark::Done
    } else if c.no_agent() && !c.needs_you() {
        Mark::NoAgent
    } else if needs(c) {
        Mark::NeedsYou
    } else if c.stage == Stage::Backlog {
        if waits(c) {
            Mark::WaitsOn
        } else {
            Mark::NotStarted
        }
    } else if c.attention == Attention::Running {
        Mark::Running
    } else {
        Mark::Waiting
    }
}

/// What a card holds, for its line on the board and its row in a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// How many pieces, nested ones included.
    pub count: usize,
    /// How many still open.
    pub open: usize,
    /// How many need you, and the first of them.
    pub needs: usize,
    pub first_needing: Option<String>,
    /// One mark per piece, the most urgent first.
    pub marks: Vec<Mark>,
}

impl Holds {
    pub fn of<'a>(pieces: impl IntoIterator<Item = &'a Card>) -> Holds {
        let mut h = Holds::default();
        let mut first: Option<(Group, &str)> = None;
        for p in pieces {
            h.count += 1;
            if !finished(p) {
                h.open += 1;
            }
            if needs(p) {
                h.needs += 1;
                if first.is_none() {
                    first = Some((Group::NeedsYou, p.reference.as_str()));
                }
            }
            h.marks.push(mark(p));
        }
        h.marks.sort();
        h.first_needing = first.map(|(_, r)| r.to_string());
        h
    }

    /// The card's line: its count, a few marks (the rest as +N), and the piece that needs you
    /// named, the whole fitted to `width` columns. Each part is a (text, mark) pair, the mark
    /// None for the count and the words.
    pub fn line(&self, width: usize) -> Vec<(String, Option<Mark>)> {
        const MOST: usize = 5;
        let count = self.count.to_string();
        let first = self.first_needing.clone().unwrap_or_default();
        let named = match self.needs {
            0 => vec![],
            1 => vec![format!("{first} needs you"), first.clone()],
            n => vec![
                format!("{first} +{} need you", n - 1),
                format!("{first} +{}", n - 1),
            ],
        };
        let build = |shown: usize, words: Option<&String>| {
            let mut parts: Vec<(String, Option<Mark>)> = vec![(count.clone(), None)];
            if shown > 0 {
                parts.push((" ".into(), None));
                for m in self.marks.iter().take(shown) {
                    parts.push((m.glyph().into(), Some(*m)));
                }
                if self.marks.len() > shown {
                    parts.push((format!("+{}", self.marks.len() - shown), None));
                }
            }
            if let Some(w) = words {
                parts.push((format!(" {w}"), Some(Mark::NeedsYou)));
            }
            parts
        };
        let fits = |parts: &[(String, Option<Mark>)]| {
            parts
                .iter()
                .map(|(t, _)| unicode_width::UnicodeWidthStr::width(t.as_str()))
                .sum::<usize>()
                <= width
        };
        let most = self.marks.len().min(MOST);
        let words: Vec<Option<&String>> = if named.is_empty() {
            vec![None]
        } else {
            named.iter().map(Some).collect()
        };
        for w in &words {
            for shown in (1..=most).rev() {
                let parts = build(shown, *w);
                if fits(&parts) {
                    return parts;
                }
            }
            let parts = build(0, *w);
            if fits(&parts) {
                return parts;
            }
        }
        build(0, words.last().copied().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::FeedCard;
    use crate::model::{Card, Priority};

    pub(crate) fn card(id: &str, f: impl FnOnce(&mut FeedCard)) -> Card {
        card_in("feed", id, f)
    }

    fn card_in(source: &str, id: &str, f: impl FnOnce(&mut FeedCard)) -> Card {
        let mut fc = FeedCard {
            id: id.into(),
            title: format!("card {id}"),
            ..Default::default()
        };
        f(&mut fc);
        Card {
            key: format!("{source}:{id}"),
            reference: fc.reference(),
            title: fc.title.clone(),
            stage: Stage::Build,
            attention: Attention::Running,
            why: String::new(),
            machine: "m".into(),
            machine_index: 0,
            session: None,
            workspace_id: None,
            workspace_label: None,
            tab_id: None,
            priority: Priority::Normal,
            background: false,
            cached_at: None,
            updated: None,
            stale_days: None,
            state: None,
            detail: Some(fc),
            agents: vec![],
            source: Source::Feed,
            progress: None,
            own_tab: None,
            inside: None,
            link: None,
        }
    }

    fn inside(cards: &[Card]) -> Vec<(String, Option<String>)> {
        cards
            .iter()
            .map(|c| (c.key.clone(), c.inside.clone()))
            .collect()
    }

    #[test]
    fn a_parent_wins_over_an_undertaker_line_and_that_over_from() {
        let mut cards = vec![
            card("0256", |_| {}),
            card("0139", |_| {}),
            card("0262", |f| {
                f.parent = Some("0256".into());
                f.from = Some("0139".into());
            }),
            card("0351", |f| {
                f.undertaker_for = Some("#139".into());
                f.from = Some("0256".into());
            }),
            card("0348", |f| f.from = Some("139".into())),
        ];
        link(&mut cards);
        assert_eq!(cards[2].inside.as_deref(), Some("feed:0256"));
        assert_eq!(cards[2].link, Some(("#256".into(), Link::Part)));
        assert_eq!(cards[3].inside.as_deref(), Some("feed:0139"));
        assert_eq!(cards[3].link, Some(("#139".into(), Link::Undertaker)));
        assert_eq!(cards[4].inside.as_deref(), Some("feed:0139"));
        assert_eq!(cards[4].link, Some(("#139".into(), Link::FollowUp)));
        assert_eq!(cards[0].inside, None);
    }

    #[test]
    fn links_match_only_within_the_same_feed_and_a_missing_record_keeps_its_name() {
        let mut cards = vec![
            card_in("other", "0256", |_| {}),
            card("0331", |f| f.from = Some("0256".into())),
        ];
        link(&mut cards);
        assert_eq!(
            cards[1].inside, None,
            "another feed's #256 is another record"
        );
        assert_eq!(cards[1].link, Some(("#256".into(), Link::FollowUp)));
    }

    #[test]
    fn a_loop_is_broken_where_it_closes_and_a_card_naming_itself_sits_in_nothing() {
        let mut cards = vec![
            card("1", |f| f.parent = Some("3".into())),
            card("2", |f| f.parent = Some("1".into())),
            card("3", |f| f.parent = Some("2".into())),
            card("4", |f| f.from = Some("4".into())),
        ];
        link(&mut cards);
        // Followed from 1: 1 → 3 → 2 → back to 1, so 2's link closes the loop.
        assert_eq!(
            inside(&cards),
            vec![
                ("feed:1".into(), Some("feed:3".into())),
                ("feed:2".into(), None),
                ("feed:3".into(), Some("feed:2".into())),
                ("feed:4".into(), None),
            ]
        );
        assert_eq!(cards[1].link, None);
        assert_eq!(cards[3].link, None);
    }

    #[test]
    fn a_line_fits_its_width_dropping_marks_before_the_one_that_needs_you() {
        let mut a = card("262", |_| {});
        a.attention = Attention::NeedsYou;
        let b = card("331", |_| {});
        let mut c = card("277", |f| {
            f.waits_on = vec![crate::feed::WaitsOn {
                open: Some(true),
                ..Default::default()
            }]
        });
        c.stage = Stage::Backlog;
        let mut d = card("332", |_| {});
        d.stage = Stage::Backlog;
        let holds = Holds::of([&a, &b, &c, &d]);
        assert_eq!((holds.count, holds.open, holds.needs), (4, 4, 1));
        let text = |w: usize| {
            holds
                .line(w)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<String>()
        };
        assert_eq!(text(21), "4 ▲●◇· #262 needs you");
        assert_eq!(text(19), "4 #262 needs you");
        assert_eq!(text(14), "4 ▲●◇· #262");
        let quiet = Holds::of([&b, &c]);
        assert_eq!(text_of(&quiet, 21), "2 ●◇");
    }

    fn text_of(h: &Holds, w: usize) -> String {
        h.line(w).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn many_marks_are_capped_with_a_count_of_the_rest() {
        let cards: Vec<Card> = (0..14).map(|i| card(&i.to_string(), |_| {})).collect();
        let holds = Holds::of(cards.iter());
        assert_eq!(text_of(&holds, 40), "14 ●●●●●+9");
    }
}
