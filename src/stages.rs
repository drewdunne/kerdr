//! Stage lists and the board's columns (intent 0595). Backlog and Done are Kerdr's own, at either
//! end of every board; between them, each team's own stages, read from the feeds' `stage_lists`.
//! A view whose cards all share one list is laid out as the board always was. A view that mixes
//! lists stacks a band of columns for each list between Backlog and Done, ordered by how much in
//! it needs you; a band with nothing under way folds to one line.

use std::collections::BTreeMap;

use crate::feed::{Feed, StageEntry, StageList};
use crate::model::{STANDARD, Stage};

/// Rows a band needs to show a few cards: its title, its columns' heading and rule, and cards.
pub const BAND_ROWS: u16 = 8;

/// The standard list as Kerdr knows it when no feed gives one: Plan, Design, Build, Test, Deploy.
pub fn builtin_standard() -> StageList {
    StageList {
        id: STANDARD.into(),
        name: "Standard".into(),
        owner: None,
        rev: 0,
        stages: Stage::STANDARD
            .iter()
            .map(|s| StageEntry {
                id: s.id().into(),
                name: s.name().into(),
                retired: false,
            })
            .collect(),
    }
}

/// Every machine's stage lists as one: a list per id, the copy with the higher `rev` winning (a
/// machine that hasn't caught up yet has the lower), and `standard` always there.
pub fn merge<'a>(feeds: impl Iterator<Item = &'a Feed>) -> Vec<StageList> {
    let mut by_id: BTreeMap<String, StageList> = BTreeMap::new();
    for list in feeds.flat_map(|f| f.stage_lists.iter()) {
        let id = list.id.trim();
        if id.is_empty() || list.stages.iter().all(|s| s.id.trim().is_empty()) {
            continue;
        }
        if by_id.get(id).is_none_or(|held| list.rev > held.rev) {
            by_id.insert(id.to_string(), list.clone());
        }
    }
    let standard = by_id.remove(STANDARD).unwrap_or_else(builtin_standard);
    let mut out = vec![standard];
    out.extend(by_id.into_values());
    // The names lists give their stages are what the board calls them.
    for list in &out {
        for s in &list.stages {
            Stage::work(&list.id, &s.id, &s.name);
        }
    }
    out
}

/// The list with this id, if a feed gave it (or it's the standard one).
pub fn find<'a>(lists: &'a [StageList], id: &str) -> Option<&'a StageList> {
    lists.iter().find(|l| l.id == id)
}

/// What a card's list is called on its band's title: `Standard stages`, `Tinyhost Marketing's
/// stages`. `owner_name` is the list owner's name as the owners list gives it.
pub fn list_title(lists: &[StageList], id: &str, owner_name: Option<&str>) -> String {
    match find(lists, id) {
        Some(l) if l.id == STANDARD => format!("{} stages", nonempty(&l.name, "Standard")),
        Some(l) => match owner_name.filter(|n| !n.is_empty()) {
            Some(owner) => format!("{owner} {}'s stages", nonempty(&l.name, &l.id)),
            None => format!("{}'s stages", nonempty(&l.name, &l.id)),
        },
        None => format!("{id} (a list Kerdr wasn't sent)"),
    }
}

fn nonempty<'a>(s: &'a str, or: &'a str) -> &'a str {
    if s.trim().is_empty() { or } else { s }
}

/// The column a feed card goes in. `list` is the whole id of its stage list (empty for none:
/// standard), `said` the stage it gave, `state` its state (`closed`, `queued`, …).
///
/// Backlog and Maintain come from the stage it gave (`backlog`, `maintain`) or its state, the same
/// for every list. On the standard list a stage is read as it always was, the common words for
/// each accepted. On any other list it must be one of the list's ids. A stage a list doesn't have,
/// or one on a list no feed sent, is still that stage, never another: the layout gives it a column
/// of its own, marked unknown. A card that names no stage is in Build on the standard list (the
/// feed's guess comes with its next read) and in the first stage of any other.
pub fn card_stage(lists: &[StageList], list: &str, said: Option<&str>, state: &str) -> Stage {
    let list = if list.trim().is_empty() {
        STANDARD
    } else {
        list.trim()
    };
    let said = said.map(str::trim).filter(|s| !s.is_empty());
    let found = find(lists, list);
    if list == STANDARD {
        if let Some(s) = said {
            if let Some(entry) = found.and_then(|l| l.stages.iter().find(|e| e.id == s)) {
                return Stage::work(STANDARD, &entry.id, &entry.name);
            }
            if let Some(stage) = Stage::parse(s) {
                // A word for one of the standard stages counts only while the list still has it.
                if !stage.is_working()
                    || found.is_none_or(|l| l.stages.iter().any(|e| e.id == stage.id()))
                {
                    return stage;
                }
            }
            return Stage::work(STANDARD, &s.to_ascii_lowercase(), s);
        }
        return match state {
            "closed" => Stage::Maintain,
            "queued" | "shelved" => Stage::Backlog,
            _ => found
                .filter(|l| !l.stages.iter().any(|e| e.id == Stage::Build.id()))
                .and_then(|l| l.stages.iter().find(|e| !e.retired))
                .map(|e| Stage::work(STANDARD, &e.id, &e.name))
                .unwrap_or(Stage::Build),
        };
    }
    match said.map(str::to_ascii_lowercase).as_deref() {
        Some("backlog") => return Stage::Backlog,
        Some("maintain" | "done") => return Stage::Maintain,
        Some(s) => {
            let name = found
                .and_then(|l| l.stages.iter().find(|e| e.id == s))
                .map(|e| e.name.as_str())
                .unwrap_or(s);
            return Stage::work(list, s, name);
        }
        None => {}
    }
    match state {
        "closed" => Stage::Maintain,
        "queued" | "shelved" => Stage::Backlog,
        _ => match found.and_then(|l| l.stages.iter().find(|e| !e.retired)) {
            Some(e) => Stage::work(list, &e.id, &e.name),
            None => Stage::work(list, "unknown", "unknown"),
        },
    }
}

/// One column of the board: Backlog, a stage of a band's list, or Done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub stage: Stage,
    /// The band it's in; None for Backlog and Done, which span every band.
    pub band: Option<usize>,
    /// A stage its list doesn't have, or of a list no feed sent: the stage its cards gave.
    pub unknown: bool,
    /// A stage taken out of its list, shown only while it still holds a card.
    pub retired: bool,
}

/// A band: one stage list's columns on a view that mixes lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    /// The whole id of its list.
    pub list: String,
    /// What its title says: whose list it is and who's on it.
    pub title: String,
    /// Its columns: `columns[first..first + len]`.
    pub first: usize,
    pub len: usize,
    /// Cards under way in it, in this view.
    pub open: usize,
    /// Of those, the ones that need you.
    pub needs: usize,
    /// Drawn as one line with its counts; moving onto it opens it.
    pub folded: bool,
}

/// The board's columns for one view, in order: Backlog, each band's stages, Done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub columns: Vec<Column>,
    pub bands: Vec<Band>,
}

impl Default for Layout {
    fn default() -> Self {
        layout(&[builtin_standard()], &[], &[], &Fold::default())
    }
}

impl Layout {
    /// The column a card in this stage is in.
    pub fn col_of(&self, stage: Stage) -> Option<usize> {
        self.columns.iter().position(|c| c.stage == stage)
    }

    pub fn done(&self) -> usize {
        self.columns.len() - 1
    }

    /// Whether the board shows band titles: only when the view mixes lists.
    pub fn banded(&self) -> bool {
        self.bands.len() > 1
    }
}

/// A list in view, and who's on it there (the teams, and "No team"), for its band's title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub list: String,
    pub title: String,
    pub who: Vec<String>,
}

/// What may stay open when the bands don't all fit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fold {
    /// Rows the bands have between them; 0 for no limit.
    pub rows: u16,
    /// The bands moved onto last and holding the selection: they never fold while they have cards.
    pub keep: Vec<String>,
}

/// The columns for a view. `wants` are the lists the view's teams use, with their titles; `cards`
/// the stage of each card under way in view and whether it needs you. Every list a card is on
/// gets a band even when no team in view uses it.
pub fn layout(lists: &[StageList], wants: &[Want], cards: &[(Stage, bool)], fold: &Fold) -> Layout {
    // The lists in view, in the order they come, and each one's stages.
    let mut order: Vec<String> = wants.iter().map(|w| w.list.clone()).collect();
    for (stage, _) in cards {
        if let Some(list) = stage.list()
            && !order.iter().any(|l| l == list)
        {
            order.push(list.to_string());
        }
    }
    if order.is_empty() {
        order.push(STANDARD.into());
    }
    order.dedup();
    struct Draft {
        list: String,
        title: String,
        cols: Vec<Column>,
        open: usize,
        needs: usize,
    }
    let mut drafts: Vec<Draft> = Vec::new();
    for id in &order {
        let mut cols: Vec<Column> = Vec::new();
        let in_list = |s: Stage| s.list() == Some(id.as_str());
        let holds = |s: Stage| cards.iter().any(|(c, _)| *c == s);
        if let Some(list) = find(lists, id) {
            for e in &list.stages {
                let stage = Stage::work(id, &e.id, &e.name);
                if !e.retired || holds(stage) {
                    cols.push(Column {
                        stage,
                        band: None,
                        unknown: false,
                        retired: e.retired,
                    });
                }
            }
        }
        // Stages its cards gave that the list doesn't have: a column each, at the end, marked.
        for (stage, _) in cards {
            if in_list(*stage) && !cols.iter().any(|c| c.stage == *stage) {
                cols.push(Column {
                    stage: *stage,
                    band: None,
                    unknown: true,
                    retired: false,
                });
            }
        }
        let mine: Vec<&(Stage, bool)> = cards.iter().filter(|(s, _)| in_list(*s)).collect();
        let want = wants.iter().find(|w| &w.list == id);
        let mut title = want
            .map(|w| w.title.clone())
            .unwrap_or_else(|| list_title(lists, id, None));
        let who: Vec<&String> = want.map(|w| w.who.iter().collect()).unwrap_or_default();
        if !who.is_empty() {
            title = format!(
                "{title} · {}",
                who.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        drafts.push(Draft {
            list: id.clone(),
            title,
            cols,
            open: mine.len(),
            needs: mine.iter().filter(|(_, n)| *n).count(),
        });
    }
    // What needs you first, then what's busiest, the standard list before others alike.
    drafts.sort_by(|a, b| {
        b.needs
            .cmp(&a.needs)
            .then(b.open.cmp(&a.open))
            .then((a.list != STANDARD).cmp(&(b.list != STANDARD)))
            .then(a.list.cmp(&b.list))
    });
    let mut columns = vec![Column {
        stage: Stage::Backlog,
        band: None,
        unknown: false,
        retired: false,
    }];
    let mut bands = Vec::new();
    let several = drafts.len() > 1;
    for (bi, d) in drafts.into_iter().enumerate() {
        let first = columns.len();
        let len = d.cols.len();
        columns.extend(d.cols.into_iter().map(|c| Column {
            band: Some(bi),
            ..c
        }));
        bands.push(Band {
            list: d.list,
            title: d.title,
            first,
            len,
            open: d.open,
            needs: d.needs,
            // One band alone is the whole board, as it always was, and never folds.
            folded: several && d.open == 0,
        });
    }
    columns.push(Column {
        stage: Stage::Maintain,
        band: None,
        unknown: false,
        retired: false,
    });
    // When the open bands don't all fit, those with nothing needing you fold first, from the
    // bottom, never the one kept open.
    if several && fold.rows > 0 {
        let need = |bands: &[Band]| -> u16 {
            bands
                .iter()
                .map(|b| if b.folded { 1 } else { BAND_ROWS })
                .sum()
        };
        for bi in (0..bands.len()).rev() {
            if need(&bands) <= fold.rows {
                break;
            }
            let b = &bands[bi];
            if !b.folded && b.needs == 0 && !fold.keep.contains(&b.list) {
                bands[bi].folded = true;
            }
        }
    }
    Layout { columns, bands }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, owner: Option<&str>, rev: u64, stages: &[&str]) -> StageList {
        StageList {
            id: id.into(),
            name: id.rsplit('/').next().unwrap_or(id).to_string(),
            owner: owner.map(str::to_string),
            rev,
            stages: stages
                .iter()
                .map(|s| StageEntry {
                    id: s.to_string(),
                    name: s[..1].to_uppercase() + &s[1..],
                    retired: false,
                })
                .collect(),
        }
    }

    fn feed(lists: Vec<StageList>) -> Feed {
        Feed {
            kerdr_feed: 1,
            stage_lists: lists,
            ..Default::default()
        }
    }

    #[test]
    fn the_higher_rev_of_a_list_wins_and_standard_is_always_there() {
        let a = feed(vec![list(
            "tinyhost/marketing",
            Some("tinyhost"),
            2,
            &["plan", "learn"],
        )]);
        let b = feed(vec![list(
            "tinyhost/marketing",
            Some("tinyhost"),
            1,
            &["plan"],
        )]);
        let merged = merge([&b, &a].into_iter());
        assert_eq!(merged[0].id, STANDARD);
        assert_eq!(merged[0].stages.len(), 5);
        let m = find(&merged, "tinyhost/marketing").unwrap();
        assert_eq!(m.rev, 2);
        assert_eq!(m.stages.len(), 2);
        let merged = merge([&a, &b].into_iter());
        assert_eq!(find(&merged, "tinyhost/marketing").unwrap().rev, 2);
    }

    #[test]
    fn two_owners_lists_of_one_name_never_mix() {
        let merged = merge(
            [&feed(vec![
                list(
                    "tinyhost/marketing",
                    Some("tinyhost"),
                    1,
                    &["plan", "learn"],
                ),
                list("arkship/marketing", Some("arkship"), 1, &["brief", "ship"]),
            ])]
            .into_iter(),
        );
        let a = card_stage(&merged, "tinyhost/marketing", Some("plan"), "active");
        let b = card_stage(&merged, "arkship/marketing", Some("brief"), "active");
        assert_eq!(a.list(), Some("tinyhost/marketing"));
        assert_eq!(b.list(), Some("arkship/marketing"));
        assert_ne!(
            a,
            card_stage(&merged, "arkship/marketing", Some("plan"), "active")
        );
    }

    #[test]
    fn a_card_with_no_list_is_on_the_standard_one_as_it_always_was() {
        let lists = vec![builtin_standard()];
        assert_eq!(
            card_stage(&lists, "", Some("review"), "active"),
            Stage::Test
        );
        assert_eq!(
            card_stage(&lists, "", Some("build"), "active"),
            Stage::Build
        );
        assert_eq!(card_stage(&lists, "", None, "active"), Stage::Build);
        assert_eq!(card_stage(&lists, "", None, "queued"), Stage::Backlog);
        assert_eq!(card_stage(&lists, "", None, "closed"), Stage::Maintain);
        assert_eq!(
            card_stage(&lists, "", Some("maintain"), "closed"),
            Stage::Maintain
        );
    }

    #[test]
    fn backlog_and_done_are_the_same_on_every_list() {
        let lists = merge([&feed(vec![list("t/m", Some("t"), 1, &["plan", "learn"])])].into_iter());
        assert_eq!(
            card_stage(&lists, "t/m", Some("backlog"), "queued"),
            Stage::Backlog
        );
        assert_eq!(
            card_stage(&lists, "t/m", Some("maintain"), "closed"),
            Stage::Maintain
        );
        assert_eq!(card_stage(&lists, "t/m", None, "queued"), Stage::Backlog);
        assert_eq!(card_stage(&lists, "t/m", None, "closed"), Stage::Maintain);
    }

    #[test]
    fn a_card_naming_no_stage_starts_in_its_lists_first() {
        let lists = merge([&feed(vec![list("t/m", Some("t"), 1, &["plan", "learn"])])].into_iter());
        let s = card_stage(&lists, "t/m", None, "active");
        assert_eq!((s.list(), s.id()), (Some("t/m"), "plan"));
    }

    #[test]
    fn words_for_a_standard_stage_mean_nothing_on_another_list() {
        let lists = merge([&feed(vec![list("t/m", Some("t"), 1, &["plan", "learn"])])].into_iter());
        // "review" is Test on the standard list; on this list it's a stage it doesn't have.
        let s = card_stage(&lists, "t/m", Some("review"), "active");
        assert_eq!((s.list(), s.id()), (Some("t/m"), "review"));
        assert_ne!(s, Stage::Test);
    }

    #[test]
    fn one_list_in_view_is_the_board_as_it_always_was() {
        let lists = vec![builtin_standard()];
        let l = layout(&lists, &[], &[(Stage::Build, true)], &Fold::default());
        let ids: Vec<&str> = l.columns.iter().map(|c| c.stage.id()).collect();
        assert_eq!(
            ids,
            [
                "backlog", "plan", "design", "build", "test", "deploy", "maintain"
            ]
        );
        assert_eq!(l.bands.len(), 1);
        assert!(!l.banded() && !l.bands[0].folded);
        // An empty board too.
        assert_eq!(Layout::default().columns.len(), 7);
    }

    #[test]
    fn a_mixed_view_stacks_a_band_per_list_needing_you_first() {
        let lists = merge([&feed(vec![list("t/m", Some("t"), 1, &["plan", "learn"])])].into_iter());
        let learn = card_stage(&lists, "t/m", Some("learn"), "active");
        let wants = vec![
            Want {
                list: STANDARD.into(),
                title: "Standard stages".into(),
                who: vec!["Dev".into(), "No team".into()],
            },
            Want {
                list: "t/m".into(),
                title: "Tinyhost Marketing's stages".into(),
                who: vec![],
            },
        ];
        let l = layout(
            &lists,
            &wants,
            &[(Stage::Build, false), (learn, true)],
            &Fold::default(),
        );
        assert!(l.banded());
        assert_eq!(
            l.bands[0].list, "t/m",
            "the band with work needing you first"
        );
        assert_eq!(l.bands[0].title, "Tinyhost Marketing's stages");
        assert_eq!(l.bands[1].title, "Standard stages · Dev, No team");
        assert_eq!(l.columns[0].stage, Stage::Backlog);
        assert_eq!(l.columns[l.done()].stage, Stage::Maintain);
        assert_eq!(l.columns.len(), 2 + 2 + 5);
        assert_eq!(l.col_of(learn), Some(2));
        assert_eq!(l.columns[2].band, Some(0));
        assert_eq!(l.col_of(Stage::Build), Some(3 + 2));
    }

    #[test]
    fn a_band_with_nothing_under_way_folds_and_crowded_quiet_bands_fold_first() {
        let lists = merge(
            [&feed(vec![
                list("t/m", Some("t"), 1, &["plan", "learn"]),
                list("t/s", Some("t"), 1, &["call", "close"]),
            ])]
            .into_iter(),
        );
        let wants: Vec<Want> = [STANDARD, "t/m", "t/s"]
            .iter()
            .map(|l| Want {
                list: l.to_string(),
                ..Default::default()
            })
            .collect();
        let learn = card_stage(&lists, "t/m", Some("learn"), "active");
        let l = layout(&lists, &wants, &[(learn, false)], &Fold::default());
        assert_eq!(l.bands.iter().filter(|b| b.folded).count(), 2);
        assert!(!l.bands[0].folded && l.bands[0].list == "t/m");
        // Three busy bands in room for two: the quiet one folds, never one needing you.
        let call = card_stage(&lists, "t/s", Some("call"), "active");
        let cards = [(learn, false), (call, true), (Stage::Build, false)];
        let fold = Fold {
            rows: BAND_ROWS * 2 + 1,
            keep: vec![],
        };
        let l = layout(&lists, &wants, &cards, &fold);
        let folded: Vec<&str> = l
            .bands
            .iter()
            .filter(|b| b.folded)
            .map(|b| b.list.as_str())
            .collect();
        assert_eq!(folded.len(), 1);
        assert_ne!(folded[0], "t/s");
        // The band moved onto stays open.
        let keep = folded[0].to_string();
        let l = layout(
            &lists,
            &wants,
            &cards,
            &Fold {
                keep: vec![keep.clone()],
                ..fold
            },
        );
        assert!(l.bands.iter().any(|b| b.list == keep && !b.folded));
    }

    #[test]
    fn an_unknown_stage_or_list_gets_its_own_column_never_another() {
        let lists = merge([&feed(vec![list("t/m", Some("t"), 1, &["plan", "learn"])])].into_iter());
        let odd = card_stage(&lists, "t/m", Some("execute"), "active");
        let lost = card_stage(&lists, "t/gone", Some("plan"), "active");
        let l = layout(
            &lists,
            &[],
            &[(odd, false), (lost, false)],
            &Fold::default(),
        );
        let band = l.bands.iter().find(|b| b.list == "t/m").unwrap();
        let last = &l.columns[band.first + band.len - 1];
        assert!(last.unknown && last.stage == odd);
        let gone = l.bands.iter().find(|b| b.list == "t/gone").unwrap();
        assert_eq!(gone.len, 1);
        assert!(l.columns[gone.first].unknown);
        assert!(gone.title.contains("wasn't sent"));
    }

    #[test]
    fn a_retired_stage_shows_only_while_it_holds_a_card() {
        let mut m = list("t/m", Some("t"), 3, &["plan", "learn"]);
        m.stages[1].retired = true;
        let lists = merge([&feed(vec![m])].into_iter());
        let want = [Want {
            list: "t/m".into(),
            ..Default::default()
        }];
        let l = layout(&lists, &want, &[], &Fold::default());
        assert_eq!(l.columns.len(), 3);
        let learn = card_stage(&lists, "t/m", Some("learn"), "active");
        let l = layout(&lists, &want, &[(learn, false)], &Fold::default());
        assert_eq!(l.columns.len(), 4);
        assert!(l.columns[2].retired);
    }
}
