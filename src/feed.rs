//! Card feeds: a command on a machine that prints its records of work as cards (docs/feed.md).
//! Kerdr runs it on that machine and merges its cards with the herdr workspaces it sees there.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Feed {
    pub kerdr_feed: u32,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub sessions: BTreeMap<String, FeedSlots>,
    #[serde(default)]
    pub cards: Vec<FeedCard>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FeedSlots {
    #[serde(default)]
    pub used: u32,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub max: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct FeedCard {
    pub id: String,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// When the record was first created; keeping it fresh must not change this.
    #[serde(default)]
    pub created: Option<String>,
    /// When the user last confirmed that the work is still wanted.
    #[serde(default)]
    pub refreshed_at: Option<String>,
    #[serde(default)]
    pub backlog_rank: Option<i64>,
    #[serde(default)]
    pub disposition: Option<String>,
    /// `pending` while an agent writes up an intent from the words it was made with, `failed`
    /// when none could; absent once it's written up.
    #[serde(default)]
    pub intake: Option<String>,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub story: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub home: Option<Home>,
    #[serde(default)]
    pub workspace: Option<FeedWorkspace>,
    #[serde(default)]
    pub attention: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub needs: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub waits_on: Vec<WaitsOn>,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
    #[serde(default)]
    pub timeline: Vec<TimelineEntry>,
    #[serde(default)]
    pub log: Vec<LogEntry>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub stale_days: Option<i64>,
    #[serde(default)]
    pub words: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub closed: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    /// For work under way: the agent conversation that worked it, which a reopen resumes.
    #[serde(default)]
    pub resume: Option<Resume>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Resume {
    /// The agent that held it, such as `claude` or `codex`.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub conversation: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Home {
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct FeedWorkspace {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct WaitsOn {
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub open: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Criterion {
    pub text: String,
    #[serde(default)]
    pub met: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct TimelineEntry {
    pub stage: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct LogEntry {
    #[serde(default)]
    pub date: Option<String>,
    pub text: String,
}

impl FeedCard {
    /// The handle drawn first on the card: the feed's `ref`, else `#` and the id without zeros.
    pub fn reference(&self) -> String {
        if let Some(r) = self.reference.as_deref().filter(|r| !r.trim().is_empty()) {
            return r.to_string();
        }
        let trimmed = self.id.trim_start_matches('0');
        if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
            self.id.clone()
        } else {
            format!("#{trimmed}")
        }
    }

    pub fn criteria_progress(&self) -> Option<(u32, u32)> {
        if self.criteria.is_empty() || self.criteria.iter().all(|c| c.met.is_none()) {
            return None;
        }
        let met = self.criteria.iter().filter(|c| c.met == Some(true)).count() as u32;
        Some((met, self.criteria.len() as u32))
    }
}

/// Reads a feed, accepting only the version this Kerdr knows.
pub fn parse(text: &str) -> Result<Feed, String> {
    let start = text.find('{').ok_or("the feed printed no JSON")?;
    let feed: Feed =
        serde_json::from_str(&text[start..]).map_err(|e| format!("the feed's JSON: {e}"))?;
    if feed.kerdr_feed != 1 {
        return Err(format!(
            "the feed is version {}; this Kerdr reads version 1",
            feed.kerdr_feed
        ));
    }
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_minimal_feed() {
        let feed =
            parse(r#"{"kerdr_feed":1,"cards":[{"id":"0143","title":"Find the next flywheel"}]}"#)
                .unwrap();
        assert_eq!(feed.cards.len(), 1);
        assert_eq!(feed.cards[0].reference(), "#143");
    }

    #[test]
    fn skips_noise_before_the_json() {
        let feed = parse("warning: something\n{\"kerdr_feed\":1}").unwrap();
        assert!(feed.cards.is_empty());
    }

    #[test]
    fn refuses_other_versions() {
        assert!(parse(r#"{"kerdr_feed":2}"#).is_err());
    }

    #[test]
    fn preserves_scope_order_and_distinct_creation_and_refresh_dates() {
        let feed = parse(r#"{"kerdr_feed":1,"cards":[{"id":"4","title":"t","tenant":"shop","organization":"org","project":"checkout","created":"2026-01-01","refreshed_at":"2026-10-22","backlog_rank":-2,"disposition":"kept"}]}"#).unwrap();
        let card = serde_json::to_value(&feed.cards[0]).unwrap();
        assert_eq!(card["tenant"], "shop");
        assert_eq!(card["organization"], "org");
        assert_eq!(card["project"], "checkout");
        assert_eq!(card["created"], "2026-01-01");
        assert_eq!(card["refreshed_at"], "2026-10-22");
        assert_eq!(card["backlog_rank"], -2);
        assert_eq!(card["disposition"], "kept");
    }

    #[test]
    fn reads_the_conversation_a_reopen_resumes() {
        let feed = parse(r#"{"kerdr_feed":1,"cards":[{"id":"41","title":"t","stage":"deploy","resume":{"agent":"codex","conversation":"4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41","new":1}}]}"#).unwrap();
        let resume = feed.cards[0].resume.as_ref().unwrap();
        assert_eq!(resume.agent.as_deref(), Some("codex"));
        assert_eq!(
            resume.conversation.as_deref(),
            Some("4f2b8c1e-5d3a-4b6f-9c2e-1a7d3e9b0c41")
        );
        assert!(
            parse(r#"{"kerdr_feed":1,"cards":[{"id":"1","title":"t"}]}"#)
                .unwrap()
                .cards[0]
                .resume
                .is_none()
        );
    }

    #[test]
    fn progress_needs_at_least_one_known_criterion() {
        let mut card = FeedCard {
            id: "1".into(),
            title: "t".into(),
            criteria: vec![
                Criterion {
                    text: "a".into(),
                    met: None,
                },
                Criterion {
                    text: "b".into(),
                    met: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(card.criteria_progress(), None);
        card.criteria[0].met = Some(true);
        assert_eq!(card.criteria_progress(), Some((1, 2)));
    }
}
