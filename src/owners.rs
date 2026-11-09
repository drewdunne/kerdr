//! Who the work is for (intent 0595): the owners the feeds list (organizations, and the tenants
//! inside them), each with its teams, and how a card or a herdr session finds its owner. Kerdr's
//! top bar splits the board by these instead of by machine whenever a feed lists owners.

use crate::accounts::Team;
use crate::feed::Feed;

/// The key of work no owner on the list claims: its own entry at the end of the bar, `Unowned`.
pub const UNOWNED: &str = "";

/// One owner as the board knows it, from every machine's feed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerInfo {
    pub id: String,
    /// `organization` or `tenant`.
    pub kind: String,
    pub name: String,
    pub code: Option<String>,
    /// Its organization, for a tenant.
    pub parent: Option<String>,
    pub teams: Vec<Team>,
    /// The machine its new work runs on, as its feed says.
    pub agent_machine: Option<String>,
    /// The herdr sessions its work runs in.
    pub sessions: Vec<String>,
}

impl OwnerInfo {
    /// How the bar and cards name it in a word: its code (`TNY`), else its name.
    pub fn short(&self) -> &str {
        self.code
            .as_deref()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| self.label())
    }

    /// Its name, else its id.
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn team(&self, id: &str) -> Option<&Team> {
        self.teams.iter().find(|t| t.id == id)
    }

    /// The organization at the top of its chain: itself for an organization.
    pub fn organization<'a>(&'a self, owners: &'a [OwnerInfo]) -> &'a OwnerInfo {
        let mut at = self;
        let mut seen = vec![self.id.as_str()];
        while let Some(parent) = at.parent.as_deref().and_then(|p| find(owners, p)) {
            if seen.contains(&parent.id.as_str()) {
                break;
            }
            seen.push(&parent.id);
            at = parent;
        }
        at
    }
}

/// Every machine's owners as one list, by id: the first machine's copy (this one's) leads, and a
/// team or a field only another machine's copy has is added from it.
pub fn merge<'a>(feeds: impl Iterator<Item = &'a Feed>) -> Vec<OwnerInfo> {
    let mut out: Vec<OwnerInfo> = Vec::new();
    for feed in feeds {
        for o in &feed.owners {
            if o.id.trim().is_empty() {
                continue;
            }
            match out.iter_mut().find(|x| x.id == o.id) {
                Some(held) => {
                    for t in &o.teams {
                        if !t.id.is_empty() && held.team(&t.id).is_none() {
                            held.teams.push(t.clone());
                        }
                    }
                    if held.agent_machine.is_none() {
                        held.agent_machine = o.agent_machine.clone();
                    }
                    if held.code.is_none() {
                        held.code = o.code.clone();
                    }
                    for s in &o.sessions {
                        if !held.sessions.contains(s) {
                            held.sessions.push(s.clone());
                        }
                    }
                }
                None => out.push(OwnerInfo {
                    id: o.id.clone(),
                    kind: o.kind.clone(),
                    name: o.name.clone(),
                    code: o.code.clone().filter(|c| !c.is_empty()),
                    parent: o.parent.clone().filter(|p| !p.is_empty()),
                    teams: o
                        .teams
                        .iter()
                        .filter(|t| !t.id.is_empty())
                        .cloned()
                        .collect(),
                    agent_machine: o.agent_machine.clone().filter(|m| !m.is_empty()),
                    sessions: o.sessions.clone(),
                }),
            }
        }
    }
    out
}

pub fn find<'a>(owners: &'a [OwnerInfo], id: &str) -> Option<&'a OwnerInfo> {
    owners.iter().find(|o| o.id == id)
}

/// The owners in the bar's order: each organization, then the tenants inside it, as the feeds list
/// them; a tenant whose organization isn't listed comes after.
pub fn ordered(owners: &[OwnerInfo]) -> Vec<&OwnerInfo> {
    let mut out: Vec<&OwnerInfo> = Vec::new();
    let tops = owners.iter().filter(|o| {
        o.parent
            .as_deref()
            .is_none_or(|p| find(owners, p).is_none())
    });
    for top in tops {
        push_tree(owners, top, &mut out);
    }
    for o in owners {
        if !out.iter().any(|x| x.id == o.id) {
            out.push(o);
        }
    }
    out
}

fn push_tree<'a>(owners: &'a [OwnerInfo], at: &'a OwnerInfo, out: &mut Vec<&'a OwnerInfo>) {
    if out.iter().any(|x| x.id == at.id) {
        return;
    }
    out.push(at);
    for child in owners
        .iter()
        .filter(|o| o.parent.as_deref() == Some(&at.id))
    {
        push_tree(owners, child, out);
    }
}

/// The owner a card's record names: its tenant, else its organization, matched against the list by
/// id, else by code (an older feed said `ofw` for One Foot Whale). `UNOWNED` when the list has
/// neither.
pub fn record_owner(
    owners: &[OwnerInfo],
    tenant: Option<&str>,
    organization: Option<&str>,
) -> String {
    let named = tenant
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or_else(|| organization.map(str::trim).filter(|o| !o.is_empty()));
    let Some(named) = named else {
        return UNOWNED.into();
    };
    owners
        .iter()
        .find(|o| o.id == named)
        .or_else(|| {
            owners.iter().find(|o| {
                o.code
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(named))
            })
        })
        .map(|o| o.id.clone())
        .unwrap_or_else(|| UNOWNED.into())
}

/// The owner of a herdr session: the one whose sessions name it, else the one whose code its name
/// starts with (`TNY`, `TNY-Background`, `P4M-Marketing`). `UNOWNED` for any other.
pub fn session_owner(owners: &[OwnerInfo], session: &str) -> String {
    if let Some(o) = owners
        .iter()
        .find(|o| o.sessions.iter().any(|s| s == session))
    {
        return o.id.clone();
    }
    let code = session.split_once('-').map_or(session, |(c, _)| c);
    owners
        .iter()
        .find(|o| o.code.as_deref() == Some(code))
        .map(|o| o.id.clone())
        .unwrap_or_else(|| UNOWNED.into())
}

/// A card's team: the one its record names when its owner lists it, else none (No team).
pub fn record_team(owners: &[OwnerInfo], owner: &str, team: Option<&str>) -> Option<String> {
    let team = team.map(str::trim).filter(|t| !t.is_empty())?;
    find(owners, owner)?.team(team).map(|t| t.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Owner;

    fn owner(id: &str, parent: Option<&str>, code: Option<&str>, teams: &[&str]) -> Owner {
        Owner {
            id: id.into(),
            kind: if parent.is_some() {
                "tenant"
            } else {
                "organization"
            }
            .into(),
            name: id[..1].to_uppercase() + &id[1..],
            parent: parent.map(str::to_string),
            code: code.map(str::to_string),
            sessions: code.map(|c| vec![c.to_string()]).unwrap_or_default(),
            teams: teams
                .iter()
                .map(|t| Team {
                    id: t.to_string(),
                    name: t[..1].to_uppercase() + &t[1..],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn arknet() -> Vec<OwnerInfo> {
        let feed = Feed {
            kerdr_feed: 1,
            owners: vec![
                owner("arknet", None, Some("ANET"), &[]),
                owner(
                    "tinyhost",
                    Some("arkship"),
                    Some("TNY"),
                    &["dev", "marketing"],
                ),
                owner("arkship", None, Some("ARK"), &[]),
                owner("one-foot-whale", Some("arkship"), Some("OFW"), &[]),
                owner("kibbelz", None, Some("KBZ"), &[]),
                owner("dunneshire", Some("kibbelz"), None, &[]),
            ],
            ..Default::default()
        };
        merge(std::iter::once(&feed))
    }

    #[test]
    fn owners_come_grouped_under_their_organization() {
        let owners = arknet();
        let ids: Vec<&str> = ordered(&owners).iter().map(|o| o.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "arknet",
                "arkship",
                "tinyhost",
                "one-foot-whale",
                "kibbelz",
                "dunneshire"
            ]
        );
    }

    #[test]
    fn a_cards_owner_is_its_tenant_else_its_organization_by_id_or_code() {
        let owners = arknet();
        assert_eq!(
            record_owner(&owners, Some("tinyhost"), Some("arkship")),
            "tinyhost"
        );
        assert_eq!(record_owner(&owners, None, Some("arkship")), "arkship");
        assert_eq!(
            record_owner(&owners, Some("ofw"), Some("arkship")),
            "one-foot-whale"
        );
        assert_eq!(record_owner(&owners, Some("nobody"), None), UNOWNED);
        assert_eq!(record_owner(&owners, None, None), UNOWNED);
    }

    #[test]
    fn a_session_belongs_to_its_listed_owner_or_its_codes() {
        let owners = arknet();
        assert_eq!(session_owner(&owners, "TNY"), "tinyhost");
        assert_eq!(session_owner(&owners, "TNY-Background"), "tinyhost");
        assert_eq!(session_owner(&owners, "ARK-Marketing"), "arkship");
        assert_eq!(session_owner(&owners, "default"), UNOWNED);
    }

    #[test]
    fn a_team_its_owner_doesnt_list_is_no_team() {
        let owners = arknet();
        assert_eq!(
            record_team(&owners, "tinyhost", Some("dev")).as_deref(),
            Some("dev")
        );
        assert_eq!(record_team(&owners, "tinyhost", Some("sales")), None);
        assert_eq!(record_team(&owners, "arkship", Some("dev")), None);
        assert_eq!(record_team(&owners, "tinyhost", None), None);
    }

    #[test]
    fn machines_owners_merge_by_id_adding_what_one_lacks() {
        let mut a = owner("tinyhost", Some("arkship"), Some("TNY"), &["dev"]);
        a.agent_machine = None;
        let mut b = owner(
            "tinyhost",
            Some("arkship"),
            Some("TNY"),
            &["dev", "marketing"],
        );
        b.agent_machine = Some("arknet-one".into());
        let fa = Feed {
            kerdr_feed: 1,
            owners: vec![a],
            ..Default::default()
        };
        let fb = Feed {
            kerdr_feed: 1,
            owners: vec![b],
            ..Default::default()
        };
        let merged = merge([&fa, &fb].into_iter());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].teams.len(), 2);
        assert_eq!(merged[0].agent_machine.as_deref(), Some("arknet-one"));
    }

    #[test]
    fn a_tenants_organization_is_the_top_of_its_chain() {
        let owners = arknet();
        let t = find(&owners, "tinyhost").unwrap();
        assert_eq!(t.organization(&owners).id, "arkship");
        assert_eq!(
            find(&owners, "arknet").unwrap().organization(&owners).id,
            "arknet"
        );
    }
}
