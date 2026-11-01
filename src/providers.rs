//! What Kerdr knows of providers and programs, all from the feeds' `providers` (docs/feed.md):
//! their names, letters, kinds, sign-in styles and usage windows. Kerdr names none of them in its
//! code, so a provider added later is listed, added, signed in and shown with no Kerdr release.
//! A feed without `providers` (an older machine) is read from what its accounts say, as before.

use crate::accounts::{Account, AccountCopy, Provider, legacy_windows};
use crate::feed::Feed;

/// The providers of one or more feeds. For names, every machine's feed, the viewed machine's
/// first, so a machine whose feed predates `providers` still shows names another's gives. For what
/// a machine can do (which providers `+` offers, code rules, sign-in, paste, which programs `s`
/// moves), that machine's feed alone, since another machine's backend may know more.
pub struct Providers<'a> {
    feeds: Vec<&'a Feed>,
}

impl<'a> Providers<'a> {
    pub fn new(feeds: impl IntoIterator<Item = &'a Feed>) -> Self {
        Providers {
            feeds: feeds.into_iter().collect(),
        }
    }

    fn accounts(&self) -> impl Iterator<Item = &'a Account> + '_ {
        self.feeds.iter().flat_map(|f| f.accounts.iter())
    }

    /// Whether any feed lists its providers; without, every account reads as it did before.
    pub fn listed(&self) -> bool {
        self.feeds.iter().any(|f| !f.providers.is_empty())
    }

    pub fn get(&self, id: &str) -> Option<&'a Provider> {
        self.feeds
            .iter()
            .flat_map(|f| f.providers.iter())
            .find(|p| p.id == id)
    }

    /// Every provider, in the feeds' order. A feed without `providers` gives one for each
    /// provider its accounts name, with what an older backend offered: a token account, signed in
    /// by pasting the page's code, with five-hour and weekly figures.
    pub fn list(&self) -> Vec<Provider> {
        let mut out: Vec<Provider> = Vec::new();
        for p in self.feeds.iter().flat_map(|f| f.providers.iter()) {
            if !out.iter().any(|o| o.id == p.id) {
                out.push(p.clone());
            }
        }
        for a in self.accounts() {
            if !a.provider.is_empty() && !out.iter().any(|o| o.id == a.provider) {
                out.push(legacy(a));
            }
        }
        out
    }

    /// The providers' ids in the feeds' order, for grouping accounts by provider.
    pub fn order(&self) -> Vec<String> {
        self.list().into_iter().map(|p| p.id).collect()
    }

    /// A provider's name: the feed's label, else what an account of it says, else its id with a
    /// capital, as Kerdr named the providers of feeds before `providers`.
    pub fn label(&self, id: &str) -> String {
        if let Some(p) = self.get(id).filter(|p| !p.label.trim().is_empty()) {
            return p.label.clone();
        }
        self.accounts()
            .filter(|a| a.provider == id)
            .find_map(|a| a.provider_label.clone().filter(|l| !l.trim().is_empty()))
            .unwrap_or_else(|| capitalized(id))
    }

    /// A program's name: a provider's `program_labels`, else what an account it runs says, else
    /// its id with a capital, as a reopen names the agent it resumes.
    pub fn program_label(&self, id: &str) -> String {
        if let Some(label) = self
            .feeds
            .iter()
            .flat_map(|f| f.providers.iter())
            .find_map(|p| p.program_labels.get(id).filter(|l| !l.trim().is_empty()))
        {
            return label.clone();
        }
        self.accounts()
            .filter(|a| a.runs == id)
            .find_map(|a| a.program_label.clone().filter(|l| !l.trim().is_empty()))
            .unwrap_or_else(|| capitalized(id))
    }

    /// A kind of account in its provider's words. A feed without `providers` keeps the words
    /// Kerdr used before.
    pub fn kind_label(&self, provider: &str, kind: &str) -> String {
        match self.get(provider) {
            Some(p) => p.kind_label(kind),
            None => match kind {
                "login" => "saved sign-in".into(),
                "token" => "token per machine".into(),
                other => other.into(),
            },
        }
    }

    /// The programs accounts run, which `s` can move: the providers' programs and every account's.
    pub fn programs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let named = self
            .feeds
            .iter()
            .flat_map(|f| f.providers.iter())
            .flat_map(|p| p.programs.iter().cloned())
            .chain(self.accounts().map(|a| a.runs.clone()));
        for program in named {
            if !program.is_empty() && !out.contains(&program) {
                out.push(program);
            }
        }
        out
    }

    /// Whether a sign-in of this account, or of one copy of it, takes a pasted token: what the
    /// copy says, else (for the copy launches use, or the account) what the account says, else a
    /// token account whose provider doesn't sign in by a typed code. A copy from a feed before
    /// `paste` other than the first takes none, as root's doesn't.
    pub fn takes_paste(&self, a: &Account, copy: Option<&AccountCopy>) -> bool {
        if let Some(c) = copy {
            if let Some(paste) = c.paste {
                return paste;
            }
            if !a.first_copy(c) {
                return false;
            }
        }
        a.paste.unwrap_or_else(|| {
            a.kind == "token" && !self.get(&a.provider).is_some_and(Provider::types_code)
        })
    }

    /// Whether accounts of this provider sign in at all: a provider whose `signin` is null (a
    /// local model, say) is only tested. One no feed describes signs in, as before.
    pub fn signs_in(&self, provider: &str) -> bool {
        self.get(provider).is_none_or(|p| p.signin.is_some())
    }

    /// Whether a code typed into a sign-in's code box should be checked for an emailed sign-in
    /// code: only for a provider no feed describes, whose older backend ends the sign-in on a
    /// wrong code. A backend that describes its providers checks codes itself and waits on.
    pub fn checks_codes_here(&self, provider: &str) -> bool {
        self.get(provider).is_none()
    }
}

/// What a feed without `providers` says of an account's provider: its name if the account gives
/// one, and an older backend's one way to add and sign in.
fn legacy(a: &Account) -> Provider {
    Provider {
        id: a.provider.clone(),
        label: a
            .provider_label
            .clone()
            .unwrap_or_else(|| capitalized(&a.provider)),
        kinds: vec!["token".into()],
        programs: if a.runs.is_empty() {
            vec![]
        } else {
            vec![a.runs.clone()]
        },
        signin: Some("page_code".into()),
        windows: legacy_windows(),
        ..Provider::default()
    }
}

fn capitalized(id: &str) -> String {
    let mut chars = id.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(json: serde_json::Value) -> Feed {
        serde_json::from_value(json).unwrap()
    }

    fn listed() -> Feed {
        feed(serde_json::json!({"kerdr_feed":1,
            "providers":[
                {"id":"alpha","label":"Alpha","letter":"A","kinds":["token","login"],
                 "kind_labels":{"token":"a token per machine","login":"the machine's saved sign-in"},
                 "programs":["alpha-cli"],"program_labels":{"alpha-cli":"Alpha CLI"},
                 "signin":"page_code","windows":["five","week"]},
                {"id":"beta","label":"Beta","letter":"B","kinds":["login","token"],
                 "kind_labels":{"login":"this machine's own sign-in"},
                 "programs":["beta-cli"],"program_labels":{"beta-cli":"Beta CLI"},
                 "signin":"device_code","windows":["five","week"]}],
            "accounts":[
                {"code":"KBZ_A","provider":"alpha","kind":"token","runs":"alpha-cli"},
                {"code":"KBZ_B","provider":"beta","kind":"token","runs":"beta-cli"}]}))
    }

    fn older() -> Feed {
        feed(serde_json::json!({"kerdr_feed":1,"accounts":[
            {"code":"KBZ","provider":"alpha","kind":"login","runs":"alpha-cli"},
            {"code":"ARK","provider":"alpha","kind":"token","runs":"alpha-cli","provider_label":"Alpha"}]}))
    }

    #[test]
    fn names_kinds_and_programs_come_from_the_feed() {
        let f = listed();
        let p = Providers::new([&f]);
        assert!(p.listed());
        assert_eq!(p.label("beta"), "Beta");
        assert_eq!(p.program_label("beta-cli"), "Beta CLI");
        assert_eq!(p.kind_label("beta", "login"), "this machine's own sign-in");
        // A kind the provider gives no words for shows as itself.
        assert_eq!(p.kind_label("beta", "token"), "token");
        assert_eq!(p.programs(), vec!["alpha-cli", "beta-cli"]);
        assert_eq!(p.order(), ["alpha", "beta"]);
        assert!(p.signs_in("alpha") && !p.checks_codes_here("alpha"));
        assert_eq!(
            p.list().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
    }

    #[test]
    fn a_code_typed_on_the_providers_page_means_no_paste_unless_the_feed_says() {
        let f = listed();
        let p = Providers::new([&f]);
        assert!(p.takes_paste(&f.accounts[0], None));
        assert!(!p.takes_paste(&f.accounts[1], None));
        let mut told = f.accounts[1].clone();
        told.paste = Some(true);
        assert!(p.takes_paste(&told, None));
        // A copy says for itself; else the first copy follows the account and others take none.
        told.copies = serde_json::from_value(serde_json::json!([
            {"id":"agents","stored":true},{"id":"tenants","stored":false},{"id":"spare","paste":true}
        ]))
        .unwrap();
        assert!(p.takes_paste(&told, Some(&told.copies[0])));
        assert!(!p.takes_paste(&told, Some(&told.copies[1])));
        assert!(p.takes_paste(&told, Some(&told.copies[2])));
        told.paste = Some(false);
        told.kind = "token".into();
        assert!(!p.takes_paste(&told, Some(&told.copies[0])));
    }

    #[test]
    fn a_provider_with_no_sign_in_is_only_tested() {
        let f =
            feed(serde_json::json!({"kerdr_feed":1,"providers":[{"id":"local","signin":null}]}));
        let p = Providers::new([&f]);
        assert!(!p.signs_in("local"));
        assert!(p.signs_in("unknown"));
    }

    #[test]
    fn a_feed_without_providers_reads_as_before() {
        let f = older();
        let p = Providers::new([&f]);
        assert!(!p.listed());
        assert_eq!(p.label("alpha"), "Alpha");
        assert_eq!(p.label("gamma"), "Gamma");
        assert_eq!(p.program_label("alpha-cli"), "Alpha-cli");
        assert_eq!(p.kind_label("alpha", "login"), "saved sign-in");
        assert_eq!(p.kind_label("alpha", "token"), "token per machine");
        assert!(p.signs_in("alpha") && p.checks_codes_here("alpha"));
        let list = p.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kinds, ["token"]);
        assert_eq!(list[0].signin.as_deref(), Some("page_code"));
        assert!(p.takes_paste(&f.accounts[1], None));
        assert!(!p.takes_paste(&f.accounts[0], None));
    }

    #[test]
    fn another_machines_feed_names_what_an_older_one_doesnt() {
        let (new, old) = (listed(), older());
        let p = Providers::new([&old, &new]);
        assert_eq!(p.label("alpha"), "Alpha");
        assert_eq!(p.program_label("alpha-cli"), "Alpha CLI");
        assert_eq!(p.get("beta").map(|p| p.letter.as_str()), Some("B"));
    }
}
