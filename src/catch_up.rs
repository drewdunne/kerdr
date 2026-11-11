//! Catching up once something has passed (intent 0591, release 4, round 2). A change refused by
//! two copies of a record that disagree about where it lives goes once more by itself, after both
//! have been read and agree (K1-53). A change held a long while for its machine is said once
//! (K1-13), and so is a machine that answers again after it stopped (K1-117). An old backend that
//! reads an item's text without giving it is asked to update, then read again (K1-97). A stop signal
//! while your editor was open sends the save first, or keeps it (K3-31).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::holding::{held_what, span};
use super::{Action, App, Tone};
use crate::actions::Response;
use crate::kept::HeldRequest;
use crate::model::CardSnapshot;
use crate::poll::Conn;

/// How long a change whose copies disagree waits for them to agree before Kerdr says so.
pub const AGREE_WITHIN: Duration = Duration::from_secs(120);

/// How long a change waits in the hold file for its machine before Kerdr says it still waits.
pub const NUDGE_AFTER: i64 = 600;

/// How long an old backend asked to update for e counts as just asked.
const UPDATED_FOR: Duration = Duration::from_secs(120);

/// What Kerdr keeps while it catches up.
#[derive(Debug, Default)]
pub struct CatchUp {
    /// Changes whose copies disagreed about where their card lives, waiting for them to agree.
    pub disagreed: Vec<Disagreed>,
    /// The ops of changes sent once more after their copies agreed: never a second time.
    pub agreed_once: Vec<String>,
    /// Held changes already said to still wait, by op.
    pub nudged: Vec<String>,
    /// Cards whose text an old backend read without giving it, by key: read again once after it
    /// was asked to update, and when.
    pub edit_updated: HashMap<String, Instant>,
}

/// A change refused by a copy that named another home, after the copy it was sent to first named
/// this one: its two copies disagree about where its card lives.
#[derive(Clone, Debug)]
pub struct Disagreed {
    pub key: String,
    pub id: String,
    pub request: Value,
    pub from: Option<CardSnapshot>,
    /// The machine whose answer named another home, and the one it named.
    pub asked: usize,
    pub named: usize,
    pub since: Instant,
    /// Whether each has been read since.
    pub read_asked: bool,
    pub read_named: bool,
}

impl App {
    /// A change sent on to the home a refusal named was refused there too, naming yet another
    /// home: it waits for both copies to be read and agree, then goes once more by itself, with
    /// its op (K1-53). False when it went once more already, so Kerdr says they disagree now.
    pub(crate) fn wait_for_agreement(
        &mut self,
        asked: usize,
        named: usize,
        id: &str,
        key: &str,
        request: Value,
        from: Option<CardSnapshot>,
    ) -> bool {
        let op = request["op"].as_str().unwrap_or_default().to_string();
        if !op.is_empty() && self.catch_up.agreed_once.contains(&op) {
            return false;
        }
        let reference = self.reference_for(id);
        let (a, b) = (self.machines[asked].name(), self.machines[named].name());
        self.catch_up.disagreed.retain(|d| d.key != key);
        self.catch_up.disagreed.push(Disagreed {
            key: key.to_string(),
            id: id.to_string(),
            request,
            from,
            asked,
            named,
            since: Instant::now(),
            read_asked: false,
            read_named: false,
        });
        self.say(
            format!(
                "{reference}'s copies on {a} and {b} disagree about where it lives: Kerdr sends it again once both have been read and agree."
            ),
            Tone::Info,
        );
        true
    }

    /// The home a machine's own copy of this card gives, as one of Kerdr's machines: its feed's
    /// `home`, else that machine itself. None when its feed doesn't show the card.
    fn home_on(&self, machine: usize, key: &str) -> Option<Option<usize>> {
        let feed = self.machines.get(machine)?.feed.as_ref()?;
        let source = feed.source.as_deref().unwrap_or("feed");
        let card = feed
            .cards
            .iter()
            .find(|c| format!("{source}:{}", c.id) == key)?;
        Some(
            match card.home.as_ref().and_then(|h| h.machine.as_deref()) {
                Some(name) => self.machine_index(name),
                None => Some(machine),
            },
        )
    }

    /// After each of a machine's events: changes whose copies disagreed go once both have been read
    /// and agree, or are said after AGREE_WITHIN; a change held long for its machine is said once.
    pub(crate) fn catch_up_read(&mut self, machine: usize, read: bool) {
        if read {
            for d in self.catch_up.disagreed.iter_mut() {
                d.read_asked |= d.asked == machine;
                d.read_named |= d.named == machine;
            }
        }
        let waiting = std::mem::take(&mut self.catch_up.disagreed);
        for d in waiting {
            let homes = (d.read_asked && d.read_named)
                .then(|| (self.home_on(d.asked, &d.key), self.home_on(d.named, &d.key)));
            match homes {
                Some((Some(Some(a)), Some(Some(b)))) if a == b => self.agreed(d, a),
                _ if d.since.elapsed() >= AGREE_WITHIN => self.still_disagree(&d),
                _ => self.catch_up.disagreed.push(d),
            }
        }
        self.nudge_held();
    }

    /// Both copies name the same home now: the change goes there once more, with its op.
    fn agreed(&mut self, d: Disagreed, home: usize) {
        let mut request = d.request;
        let op = request["op"].as_str().unwrap_or_default().to_string();
        if request["action"] == "edit" {
            self.edit_resend(&mut request, Some(&d.key));
        }
        if request["action"] == "reopen"
            && let Some(session) = self.machines[home]
                .feed
                .as_ref()
                .and_then(|f| {
                    let source = f.source.as_deref().unwrap_or("feed");
                    f.cards
                        .iter()
                        .find(|c| format!("{source}:{}", c.id) == d.key)
                })
                .and_then(|c| c.home.as_ref())
                .and_then(|h| h.session.clone())
        {
            request["session"] = json!(session);
        }
        // A hold-file record: it goes from the file, as itself, for that machine now, and is
        // checked there as any held change is (holding::send_held).
        if !op.is_empty() && self.kept.find(&op).is_some() {
            self.catch_up.agreed_once.push(op.clone());
            self.resent.push((home, d.key));
            let name = self.machines[home].name();
            let request = self.record_request(&op, &request);
            self.kept.retarget(&op, &name, request);
            if let Some(entry) = self.kept.find(&op).cloned() {
                self.send_held(home, entry);
            }
            return;
        }
        self.action_from = d.from.or_else(|| self.card(&d.key).map(CardSnapshot::of));
        self.action_key = Some(d.key.clone());
        let before = self.resolver.last_added();
        self.queue_action(home, request);
        let n = self.resolver.last_added();
        if n != before && !op.is_empty() {
            self.resolver.keep_op(n, &op);
        }
        if !op.is_empty() {
            self.catch_up.agreed_once.push(op);
        }
        // Its answer naming yet another home isn't sent on again.
        self.resent.push((home, d.key));
    }

    /// Two minutes, and its copies still disagree: said, with what each says, and nothing sent.
    fn still_disagree(&mut self, d: &Disagreed) {
        let reference = self.reference_for(&d.id);
        let says = |app: &App, m: usize| {
            let name = app.machines[m].name();
            match app.home_on(m, &d.key) {
                Some(Some(home)) => {
                    format!("{name}'s says it lives on {}", app.machines[home].name())
                }
                Some(None) => format!("{name}'s names a machine Kerdr doesn't have"),
                None => format!("{name}'s doesn't show it"),
            }
        };
        let kept = self.edit_kept(&d.request);
        // A hold-file record it was leaves the file: nothing more is sent.
        if let Some(op) = d.request["op"].as_str()
            && self.kept.find(op).is_some()
        {
            self.kept.release(op);
        }
        let (a, b) = (says(self, d.asked), says(self, d.named));
        self.fail(format!(
            "{reference}'s copies still disagree about where it lives after two minutes: {a}, and {b}. One of them is behind, so nothing was changed. Try again once they agree.{kept}"
        ));
    }

    /// A change in the hold file that has waited NUDGE_AFTER for its machine: said once, the
    /// longest waiting first (K1-13).
    fn nudge_held(&mut self) {
        let now = self.now().timestamp();
        let due = self
            .kept
            .held
            .iter()
            .filter(|h| h.after.is_none() && now - h.pressed >= NUDGE_AFTER)
            .filter(|h| !self.catch_up.nudged.iter().any(|op| op == h.op()))
            .min_by_key(|h| h.pressed)
            .cloned();
        let Some(h) = due else {
            return;
        };
        self.catch_up.nudged.push(h.op().to_string());
        self.say(
            format!(
                "{} on {} still waits for {} ({} so far): it goes when {} answers, and m then D on its card drops it.",
                h.what,
                h.reference,
                h.machine,
                span(now - h.pressed),
                h.machine
            ),
            Tone::Info,
        );
    }

    /// Whether this machine had stopped answering long enough for it to show: a drop past its quiet
    /// first moments, or a machine that can't be reached.
    pub(crate) fn was_away(&self, machine: usize) -> bool {
        self.machines.get(machine).is_some_and(|m| {
            matches!(m.conn, Conn::Reconnecting { .. } | Conn::Unreachable { .. })
                && m.conn.lasting(self.now())
        })
    }

    /// A machine that had stopped answering answers again, before what's held for it goes: the line
    /// that says so, with how many held changes go now (K1-117). None when it isn't back.
    pub(crate) fn back_line(&self, machine: usize, was_away: bool, read: bool) -> Option<String> {
        let m = self.machines.get(machine)?;
        if !was_away || !m.conn.is_live() || self.demo {
            return None;
        }
        // Only what goes from here: not what another Kerdr open now is sending, nor anything for a
        // backend that no longer checks a held change (Codex's review 60).
        let held = self
            .kept
            .held
            .iter()
            .filter(|h| {
                h.after.is_none()
                    && m.answers_to(&h.machine)
                    && !self.kept.taken_elsewhere(h)
                    && self.holds_on(machine)
            })
            .count();
        let goes = match (read, held) {
            (false, _) | (true, 0) => String::new(),
            (true, 1) => ": the change held for it goes now".to_string(),
            (true, n) => format!(": the {n} changes held for it go now"),
        };
        Some(format!("{} is answering again{goes}.", m.name()))
    }

    /// Says that line once what's held has gone: before what sending it said, when that's only
    /// news; a warning or failure said meanwhile speaks alone. `before` is what the message line
    /// said before.
    pub(crate) fn say_back(&mut self, line: Option<String>, before: Option<String>) {
        let Some(line) = line else {
            return;
        };
        let now = self.message().map(|(t, tone)| (t.to_string(), tone));
        match now {
            Some((text, tone)) if Some(&text) != before.as_ref() => {
                if matches!(tone, Tone::Info | Tone::Good) {
                    self.say(format!("{line} {text}"), Tone::Info);
                }
            }
            _ => self.say(line, Tone::Good),
        }
    }

    /// An item's text read from an old backend, which answered without it: that backend is asked to
    /// bring its ark-sdlc up to date, which a read does first, and the text is read again, once
    /// (K1-97). False when it was read again already: the caller says it still can't be.
    pub(crate) fn read_after_update(
        &mut self,
        key: &str,
        id: &str,
        machine: usize,
        away: u64,
        from: Option<CardSnapshot>,
    ) -> bool {
        if self
            .catch_up
            .edit_updated
            .remove(key)
            .is_some_and(|at| at.elapsed() < UPDATED_FOR)
        {
            return false;
        }
        self.catch_up
            .edit_updated
            .insert(key.to_string(), Instant::now());
        let name = self.machines[machine].name();
        let reference = self.reference_for(id);
        let said = format!(
            "{name}'s ark-sdlc is too old to edit text; Kerdr asked it to update, and reads {reference}'s text again."
        );
        self.say(said.clone(), Tone::Info);
        self.edit_flights.push(super::edit_controls::EditFlight {
            id: id.to_string(),
            key: key.to_string(),
            at: Instant::now(),
            away,
            said,
        });
        self.action_from = from;
        self.action_key = Some(key.to_string());
        self.queue_action(
            machine,
            json!({"action":"edit","id":id,"from":crate::config::hostname()}),
        );
        true
    }

    /// A stop signal came while your editor was open, and it has closed on `action` (K3-31): the
    /// save goes now if its machine answers (`run` sends it and waits for its answer), else it's
    /// held for that machine when its backend holds, else your text stays in the words file. What
    /// became of it, in words for the line Kerdr ends with; None when there was nothing to save.
    pub(crate) fn save_before_stopping(
        &mut self,
        key: &str,
        action: Action,
        run: &mut dyn FnMut(usize, &Value) -> Result<Response, String>,
    ) -> Option<String> {
        let words_file = |app: &App, why: String| {
            let held = app.edits.get(key)?;
            held.mine.as_ref()?;
            let reference = app.reference_for(&held.id);
            let place = app
                .words_dir
                .as_ref()
                .map(|d| format!(" ({})", d.join("words.json").display()))
                .unwrap_or_default();
            Some(format!(
                "Your new text for {reference} wasn't saved{why}, so it's kept in Kerdr's words file{place}: e on it in the next Kerdr opens it."
            ))
        };
        let Action::Mutate { machine, request } = action else {
            // Not sent at all (its machine isn't answering, say): your version is kept all the same.
            return words_file(self, String::new());
        };
        if request["action"] != "edit" || request.get("text").is_none() {
            return None;
        }
        let request = crate::actions::with_op(request);
        let id = request["id"].as_str().unwrap_or_default().to_string();
        let reference = self.reference_for(&id);
        let name = self.machines.get(machine).map(|m| m.name())?;
        let live = self.machines[machine].conn.is_live();
        // Sent, and answered: done, or refused for good (its text changed meanwhile, say), when
        // only your words file can keep it.
        let refused = if live && self.actions_on() && !self.demo {
            match run(machine, &request) {
                Ok(r) if r.ok => {
                    self.edits.remove(key);
                    return Some(format!("Your new text for {reference} was saved first."));
                }
                // A question needs you: it isn't sent again by itself.
                Ok(r) if r.confirmation => Some("its backend asks about it first".to_string()),
                Ok(r) if r.retry.is_none() => Some(r.message),
                // Something passing, or its answer lost: it may yet go, with the same op.
                _ => None,
            }
        } else {
            None
        };
        if refused.is_none() && self.holds_on(machine) {
            let rev = self.card_rev(key);
            self.kept.hold(HeldRequest {
                machine: name.clone(),
                what: held_what(&request),
                request,
                rev,
                key: Some(key.to_string()),
                reference: reference.clone(),
                pressed: self.now().timestamp(),
                after: None,
                by: None,
                stays: None,
                unanswered: false,
            });
            return Some(format!(
                "Your new text for {reference} is held for {name}: the next Kerdr sends it once {name} answers."
            ));
        }
        let why = refused
            .map(|m| format!(" ({})", crate::errlog::last_part(&m, 200)))
            .unwrap_or_default();
        words_file(self, why).or_else(|| {
            Some(format!(
                "Your new text for {reference} wasn't saved, and Kerdr holds no copy of it."
            ))
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::tests::demo_app;
    use crate::kept::Kept;

    fn said(app: &App) -> String {
        app.message()
            .map(|(t, _)| t.to_string())
            .unwrap_or_default()
    }

    fn away(app: &mut App, machine: usize, secs: i64) {
        let since = app.now() - chrono::Duration::seconds(secs);
        app.machines[machine].conn = Conn::Reconnecting {
            since,
            attempt: 3,
            retry_at: since,
            detail: "no answer".into(),
        };
    }

    /// Both of the first two machines' copies of this card say it lives on `home`.
    pub(crate) fn homes_say(app: &mut App, key: &str, home: &str) {
        let template = (0..2)
            .flat_map(|m| app.machines[m].feed.iter())
            .flat_map(|f| {
                let source = f.source.clone().unwrap_or_else(|| "feed".into());
                f.cards
                    .iter()
                    .filter(move |c| format!("{source}:{}", c.id) == key)
            })
            .next()
            .cloned()
            .expect("a copy of the card");
        for m in 0..2 {
            let feed = app.machines[m].feed.as_mut().unwrap();
            let source = feed.source.clone().unwrap_or_else(|| "feed".into());
            if !feed
                .cards
                .iter()
                .any(|c| format!("{source}:{}", c.id) == key)
            {
                feed.cards.push(template.clone());
            }
            for c in feed.cards.iter_mut() {
                if format!("{source}:{}", c.id) == key {
                    c.home = Some(crate::feed::Home {
                        machine: Some(home.into()),
                        session: Some("ARK".into()),
                    });
                }
            }
        }
    }

    #[test]
    fn copies_still_disagreeing_after_two_minutes_are_said_with_what_each_says() {
        let mut app = demo_app();
        let card = app.board.cards[0].clone();
        let id = card.detail.as_ref().unwrap().id.clone();
        let reference = app.reference_for(&id);
        homes_say(&mut app, &card.key, "laptop");
        let request = crate::actions::with_op(json!({"action":"dispatch","id":id}));
        assert!(app.wait_for_agreement(1, 0, &id, &card.key, request, None));
        // Read, but each still says the other: it waits.
        for c in app.machines[1].feed.as_mut().unwrap().cards.iter_mut() {
            if c.id == id {
                c.home.as_mut().unwrap().machine = Some("build-box".into());
            }
        }
        app.catch_up_read(0, true);
        app.catch_up_read(1, true);
        assert_eq!(app.catch_up.disagreed.len(), 1);
        assert!(!app.failing());
        // Two minutes on: said, and nothing sent.
        app.catch_up.disagreed[0].since = Instant::now() - AGREE_WITHIN;
        app.catch_up_read(0, false);
        assert!(app.catch_up.disagreed.is_empty());
        assert!(app.resolver.ready().is_empty());
        assert_eq!(
            said(&app),
            format!(
                "{reference}'s copies still disagree about where it lives after two minutes: build-box's says it lives on build-box, and laptop's says it lives on laptop. One of them is behind, so nothing was changed. Try again once they agree."
            )
        );
    }

    #[test]
    fn a_change_held_ten_minutes_for_its_machine_is_said_once() {
        let mut app = demo_app();
        let name = app.machines[1].name();
        app.kept = Kept::in_memory();
        let pressed = app.now().timestamp() - NUDGE_AFTER - 5;
        app.kept.hold(HeldRequest {
            machine: name.clone(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":"41"})),
            rev: None,
            key: None,
            reference: "#41".into(),
            what: "d".into(),
            pressed,
            after: None,
            by: None,
            stays: None,
            unanswered: false,
        });
        app.catch_up_read(0, true);
        assert_eq!(
            said(&app),
            format!(
                "d on #41 still waits for {name} (10 min so far): it goes when {name} answers, and m then D on its card drops it."
            )
        );
        // Once only.
        app.say("Something else.", Tone::Info);
        app.catch_up_read(0, true);
        assert_eq!(said(&app), "Something else.");
    }

    /// The machine's next event, as its poller brings it: answering, with its feed read.
    fn answers(app: &mut App, machine: usize) {
        let feed = app.machines[machine].feed.clone().map(Ok);
        app.apply(crate::poll::Event {
            machine,
            conn: Conn::Live {
                latency: Default::default(),
            },
            data: Some(crate::poll::MachineData {
                feed,
                ..Default::default()
            }),
            at: app.now(),
        });
    }

    #[test]
    fn a_machine_that_answers_again_says_so_and_what_held_goes() {
        let mut app = demo_app();
        let name = app.machines[1].name();
        // A blip too short to have shown says nothing.
        away(&mut app, 1, 2);
        answers(&mut app, 1);
        assert_eq!(said(&app), "");
        // One that showed: said, with what's held for it, for its backend that holds.
        away(&mut app, 1, 600);
        let feed = app.machines[1].feed.as_mut().unwrap();
        feed.supports.push(crate::app::holding::HOLD.into());
        app.kept.hold(HeldRequest {
            machine: name.clone(),
            request: crate::actions::with_op(json!({"action":"dispatch","id":"41"})),
            rev: None,
            key: None,
            reference: "#41".into(),
            what: "d".into(),
            pressed: app.now().timestamp(),
            after: None,
            by: None,
            stays: None,
            unanswered: false,
        });
        answers(&mut app, 1);
        // Before what sending it says.
        assert!(
            said(&app).starts_with(&format!(
                "{name} is answering again: the change held for it goes now. "
            )),
            "{}",
            said(&app)
        );
        // Answering still: nothing more.
        app.say("Something else.", Tone::Info);
        answers(&mut app, 1);
        assert_eq!(said(&app), "Something else.");
    }

    fn edit_save(id: &str) -> Action {
        Action::Mutate {
            machine: 0,
            request: json!({"action":"edit","id":id,"text":"New words.","was":"sha256:aa","from":"here"}),
        }
    }

    #[test]
    fn a_stop_while_the_editor_was_open_sends_the_save_or_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = demo_app();
        app.demo = false;
        app.cfg.actions.command = Some("test-adapter".into());
        app.kept = Kept::at(dir.path(), app.now().timestamp());
        app.words_dir = Some(dir.path().to_path_buf());
        // Answering: sent first, and said.
        let mut sent = vec![];
        let went = app.save_before_stopping("feed:41", edit_save("41"), &mut |m, r| {
            sent.push((m, r.clone()));
            Ok(crate::actions::parse_response(r#"{"ok":true,"message":""}"#).unwrap())
        });
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1["op"].is_string(), "{}", sent[0].1);
        assert!(went.unwrap().contains("was saved first"));
        // Its answer lost, to a backend that holds: in the hold file, with the op it went with.
        app.machines[0]
            .feed
            .as_mut()
            .unwrap()
            .supports
            .push("hold".into());
        let mut op = String::new();
        let went = app
            .save_before_stopping("feed:41", edit_save("41"), &mut |_, r| {
                op = r["op"].as_str().unwrap().to_string();
                Err("ssh: connection dropped".into())
            })
            .unwrap();
        assert!(went.contains("is held for"), "{went}");
        let next = Kept::at(dir.path(), app.now().timestamp());
        assert_eq!(next.held.len(), 1);
        assert_eq!(next.held[0].op(), op);
        assert_eq!(next.held[0].request["text"], "New words.");
        // Refused for good: your version, which the editor's close kept, stays in the words file,
        // named.
        app.edits.insert(
            "feed:41".into(),
            crate::app::edit_controls::Held {
                id: "41".into(),
                machine: 0,
                current: "Old words.".into(),
                was: "sha256:aa".into(),
                read_at: Instant::now(),
                mine: Some(crate::app::edit_controls::Mine {
                    text: "New words.".into(),
                    from: "Old words.".into(),
                }),
                check_first: false,
                read_only: None,
            },
        );
        let went = app
            .save_before_stopping("feed:41", edit_save("41"), &mut |_, _| {
                Ok(crate::actions::parse_response(
                    r#"{"ok":false,"message":"its text changed since you read it"}"#,
                )
                .unwrap())
            })
            .unwrap();
        assert!(
            went.contains("wasn't saved (its text changed since you read it)"),
            "{went}"
        );
        assert!(went.contains("words.json"), "{went}");
        // Not answering, and its backend doesn't hold: the words file too. As the editor closes
        // on it, your version is what the words file keeps for the next Kerdr.
        app.machines[0].feed.as_mut().unwrap().supports.clear();
        let card = app.board.cards[0].clone();
        let id = card.detail.as_ref().unwrap().id.clone();
        let machine = card.machine_index;
        app.edits.insert(
            card.key.clone(),
            crate::app::edit_controls::Held {
                id: id.clone(),
                machine,
                current: "Old words.".into(),
                was: "sha256:aa".into(),
                read_at: Instant::now(),
                mine: None,
                check_first: false,
                read_only: None,
            },
        );
        let action = app.edit_ended(
            &card.key,
            crate::edit::Ended::Wrote {
                text: "My new words.".into(),
                run: Default::default(),
                touched: true,
            },
        );
        assert!(matches!(action, Action::Mutate { .. }), "{action:?}");
        away(&mut app, machine, 60);
        let went = app
            .save_before_stopping(&card.key, action, &mut |_, _| {
                panic!("nothing is sent to a machine that isn't answering")
            })
            .unwrap();
        assert!(went.contains("kept in Kerdr's words file"), "{went}");
        app.keep_on_quit().unwrap();
        let words = Kept::at(dir.path(), app.now().timestamp()).take_words();
        let mine = words.edits.iter().find(|e| e.key == card.key);
        assert_eq!(
            mine.map(|e| e.text.as_str()),
            Some("My new words."),
            "{words:?}"
        );
        // Nothing to save: nothing said.
        assert_eq!(
            app.save_before_stopping("feed:99", Action::None, &mut |_, _| unreachable!()),
            None
        );
    }
}
