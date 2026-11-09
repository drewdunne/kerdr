//! What the board tells the error log (`errlog`): every error shown goes through `say` or `fail`
//! on its way to the screen, and these say which machine, card and action it came from.

use serde_json::Value;

use super::account_controls::AccountScreen;
use super::{App, Mode};
use crate::actions::Response;
use crate::errlog::Entry;
use crate::poll::Conn;

/// The answer being taken in, for whatever it leads Kerdr to say.
#[derive(Clone, Debug, Default)]
pub struct Answering {
    machine: String,
    card: Option<String>,
    action: String,
    answer: String,
    stderr: String,
    /// What was typed into the request, in what came back of it.
    typed: Vec<String>,
    /// A failure, which is kept even if nothing on screen says so.
    failed: bool,
    /// What's been kept of what it led Kerdr to say.
    kept: Vec<String>,
}

impl Answering {
    fn said(&self, text: &str) -> bool {
        self.kept.iter().any(|k| k == text)
    }

    fn sent(&self) -> impl Iterator<Item = &str> {
        self.typed.iter().map(String::as_str)
    }
}

impl App {
    /// An answer is here: what's said until `answer_done` is about it.
    pub(super) fn answer_arrives(
        &mut self,
        machine: usize,
        request: &Value,
        result: &Result<Response, String>,
    ) {
        let (answer, failed, mut typed) = match result {
            Ok(r) => (
                r.message.clone(),
                !r.ok && !r.confirmation && !r.sends_on() && r.questions.is_empty(),
                // An edit's text is only counted in `request`; this is what the record holds.
                r.text.iter().cloned().collect(),
            ),
            Err(e) => (e.clone(), true, vec![]),
        };
        typed.extend(
            crate::actions::typed_in(request)
                .into_iter()
                .map(str::to_string),
        );
        // A save's text comes only counted, and is held here.
        if request["action"] == "edit" {
            let id = request["id"].as_str().unwrap_or_default();
            typed.extend(
                self.edits
                    .values()
                    .filter(|h| h.id == id)
                    .filter_map(|h| h.mine.as_ref())
                    .map(|m| m.text.clone()),
            );
        }
        self.answering = Some(Answering {
            machine: self
                .machines
                .get(machine)
                .map(|m| m.name())
                .unwrap_or_default(),
            card: request["id"].as_str().map(|id| self.reference_for(id)),
            action: request["action"].as_str().unwrap_or_default().to_string(),
            answer,
            stderr: std::mem::take(&mut self.answer_stderr),
            typed,
            failed,
            kept: vec![],
        });
    }

    /// The answer has been taken in. A failure nothing said anything about is kept all the same.
    pub(super) fn answer_done(&mut self) {
        self.keep_box_errors();
        if let Some(a) = self.answering.clone()
            && a.failed
            && a.kept.is_empty()
        {
            self.keep_error(&a.answer, "quiet");
        }
        self.answering = None;
    }

    /// The sign-in box says its failures itself, from many places: each new one is kept once.
    /// Called whenever an answer has been taken in, and before each drawing of the board.
    pub fn keep_box_errors(&mut self) {
        let failure = match &self.mode {
            Mode::Accounts {
                screen: AccountScreen::Signin(s),
                ..
            } if s.failed && !s.message.is_empty() => {
                Some((s.message.clone(), s.machines.get(s.current).copied()))
            }
            _ => None,
        };
        let Some((message, machine)) = failure else {
            self.signin_kept = None;
            return;
        };
        if self.signin_kept.as_deref() == Some(message.as_str()) {
            return;
        }
        self.signin_kept = Some(message.clone());
        // The answer that said it already may have kept it, and any other isn't what it's about.
        let by_answer = self
            .answering
            .as_ref()
            .is_some_and(|a| a.action.starts_with("account-"));
        if by_answer {
            if !self.answering.as_ref().is_some_and(|a| a.said(&message)) {
                self.keep_error(&message, "fail");
            }
            return;
        }
        let other = self.answering.take();
        self.answering = Some(Answering {
            machine: machine
                .and_then(|m| self.machines.get(m))
                .map(|m| m.name())
                .unwrap_or_default(),
            action: "account-signin".into(),
            ..Default::default()
        });
        self.keep_error(&message, "fail");
        self.answering = other;
    }

    /// Keeps an error being shown, with where it came from.
    pub(crate) fn keep_error(&mut self, text: &str, shown: &'static str) {
        if self.demo {
            return;
        }
        let entry = match &self.answering {
            Some(a) => Entry {
                source: "action",
                shown,
                machine: Some(a.machine.clone()),
                card: a.card.clone(),
                action: Some(a.action.clone()),
                message: text.to_string(),
                answer: Some(a.answer.clone()),
                stderr: Some(a.stderr.clone()),
            },
            None => {
                let card = self.selected();
                Entry {
                    source: "kerdr",
                    shown,
                    machine: Some(card.map_or_else(|| self.this_machine(), |c| c.machine.clone())),
                    card: card.map(|c| c.reference.clone()),
                    message: text.to_string(),
                    ..Default::default()
                }
            }
        };
        let sent: Vec<&str> = self.answering.iter().flat_map(Answering::sent).collect();
        self.error_log.write(&entry, &self.typed_now(), &sent);
        if let Some(a) = &mut self.answering {
            a.kept.push(text.to_string());
        }
    }

    /// Everything Kerdr holds that was typed: none of it may reach the log, however a message
    /// came to quote it.
    fn typed_now(&self) -> Vec<&str> {
        let mut typed = vec![self.draft.as_str()];
        typed.extend(self.submissions.iter().map(|s| s.words.as_str()));
        typed.extend(self.returned.iter().map(|r| r.words.as_str()));
        typed.extend(self.draft_came_back.iter().map(|s| s.words.as_str()));
        typed.extend(
            self.kickoff_kept
                .values()
                .flatten()
                .map(|(_, _, words)| words.as_str()),
        );
        typed.extend(
            self.edits
                .values()
                .filter_map(|h| h.mine.as_ref())
                .map(|m| m.text.as_str()),
        );
        typed.extend(self.edits.values().map(|h| h.current.as_str()));
        typed.extend(kickoff_drafts(&self.mode));
        if let Mode::Accounts {
            screen: AccountScreen::Signin(s),
            ..
        } = &self.mode
        {
            typed.extend([s.code.as_str(), s.page_code.as_str()]);
        }
        if let Some(a) = &self.answering {
            typed.extend(a.typed.iter().map(String::as_str));
        }
        typed
    }

    /// Keeps what the board shows wrong with a machine: once when it starts or changes, never for
    /// every read that finds it still so.
    pub(super) fn keep_machine_problems(&mut self, machine: usize) {
        if self.demo {
            return;
        }
        let Some(m) = self.machines.get(machine) else {
            return;
        };
        let name = m.name();
        // Only what has lasted (intent 0591): a short drop, or a read that failed once, is kept
        // quiet here as on the board.
        let at = self.now();
        let mut now: Vec<(String, String)> = vec![];
        if let Some(problem) = conn_problem(&m.conn, &name).filter(|_| m.conn.lasting(at)) {
            now.push(("conn".into(), problem));
        }
        if let Some(e) = m.feed_problem(at) {
            now.push(("feed".into(), format!("{name}'s feed failed: {e}")));
        }
        if let Some(why) = m.sessions_problem(at) {
            now.push(("sessions".into(), format!("{name}: {why}")));
        }
        for failed in m.read_failures_lasting(at) {
            now.push((
                format!("read {}", failed.key()),
                format!("{name}'s feed: {}", failed.why),
            ));
        }
        if let Some(data) = &m.data {
            let sessions = data
                .snapshots
                .keys()
                .chain(m.snapshot_errors.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for session in sessions {
                if let Some(e) = m.session_problem(session, at) {
                    now.push((
                        format!("session {session}"),
                        format!("{name}'s herdr session {session} didn't answer: {e}"),
                    ));
                }
            }
        }
        self.machine_problems
            .retain(|(i, what), _| *i != machine || now.iter().any(|(w, _)| w == what));
        let mut new = vec![];
        for (what, text) in now {
            let key = (machine, what);
            if self.machine_problems.get(&key) != Some(&text) {
                self.machine_problems.insert(key, text.clone());
                new.push(text);
            }
        }
        for text in new {
            let entry = Entry {
                source: "machine",
                shown: "board",
                machine: Some(name.clone()),
                message: text,
                ..Default::default()
            };
            self.error_log.write(&entry, &self.typed_now(), &[]);
        }
    }
}

/// What's typed in the Kickoff Questions box, wherever under other boxes it is.
fn kickoff_drafts(mode: &Mode) -> Vec<&str> {
    match mode {
        Mode::Kickoff { form, back, .. } => {
            let mut drafts: Vec<&str> = form.drafts.iter().map(String::as_str).collect();
            drafts.extend(kickoff_drafts(back));
            drafts
        }
        Mode::Peek { back, .. }
        | Mode::Palette { back, .. }
        | Mode::Compose { back, .. }
        | Mode::Filter { back, .. }
        | Mode::Accounts { back, .. }
        | Mode::Confirm { back, .. } => kickoff_drafts(back),
        _ => vec![],
    }
}

/// What the board shows wrong with a machine's connection, if anything.
fn conn_problem(conn: &Conn, name: &str) -> Option<String> {
    match conn {
        Conn::Connecting | Conn::Live { .. } | Conn::Hidden => None,
        Conn::Reconnecting { detail, .. } => {
            Some(format!("{name} stopped answering; retrying: {detail}"))
        }
        Conn::Unreachable { detail, .. } => Some(format!("{name} can't be reached: {detail}")),
        Conn::HostKey { detail } => Some(format!(
            "SSH wants a decision about {name}'s host key: {detail}. On the machines screen (M), ⏎ on it answers it."
        )),
        Conn::Auth { detail } => Some(format!(
            "SSH wants a password or another key for {name}: {detail}. On the machines screen (M), ⏎ on it lets you type it."
        )),
        Conn::HerdrMissing => Some(format!("{name} has no herdr.")),
        Conn::HerdrTooOld { version } => Some(format!("{name}'s herdr {version} is too old.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tone;
    use crate::errlog::tests::{lines, log};
    use crate::poll::Event;
    use chrono::Local;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// A board on real-looking machines, its log in a folder of its own.
    fn board() -> (tempfile::TempDir, App, PathBuf) {
        let (dir, log, path) = log();
        let mut app = crate::app::tests::demo_app();
        app.demo = false;
        app.error_log = log;
        (dir, app, path)
    }

    fn one(path: &Path) -> Value {
        let mut got = lines(path);
        assert_eq!(got.len(), 1, "{got:?}");
        got.remove(0)
    }

    fn refused(message: &str) -> Result<Response, String> {
        Ok(Response {
            ok: false,
            message: message.into(),
            ..Default::default()
        })
    }

    #[test]
    fn a_refused_save_is_kept_with_its_machine_card_action_and_stderr() {
        let (_dir, mut app, path) = board();
        let machine = 1;
        let name = app.machines[machine].name();
        app.answer_stderr = "ark-card-action: the save failed: index.lock exists".into();
        app.finish_action(
            machine,
            json!({"action":"dispatch","id":"0143"}),
            refused("the store is locked"),
        );
        let l = one(&path);
        assert_eq!(l["source"], "action");
        assert_eq!(l["shown"], "fail");
        assert_eq!(l["machine"], name.as_str());
        assert_eq!(l["action"], "dispatch");
        assert!(l["card"].as_str().unwrap().contains("143"), "{l}");
        assert!(
            l["message"]
                .as_str()
                .unwrap()
                .contains("the store is locked")
        );
        assert_eq!(
            l["stderr"],
            "ark-card-action: the save failed: index.lock exists"
        );
        // What was said is still on screen, unchanged by any of this.
        assert!(app.failing());
    }

    #[test]
    fn stderr_belongs_only_to_its_own_answer() {
        let (_dir, mut app, path) = board();
        app.answer_stderr = "first's stderr".into();
        app.finish_action(
            0,
            json!({"action":"dispatch","id":"0143"}),
            refused("first"),
        );
        app.finish_action(
            0,
            json!({"action":"dispatch","id":"0144"}),
            refused("second"),
        );
        let got = lines(&path);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["stderr"], "first's stderr");
        assert!(got[1].get("stderr").is_none(), "{}", got[1]);
    }

    #[test]
    fn a_message_cut_short_on_screen_is_whole_in_the_log() {
        let (_dir, mut app, path) = board();
        let long = format!("the cause {}and the end", "of it ".repeat(200));
        app.finish_action(0, json!({"action":"dispatch","id":"0143"}), refused(&long));
        let l = one(&path);
        assert!(l["message"].as_str().unwrap().ends_with('…'), "{l}");
        assert_eq!(l["answer"], long.as_str());
    }

    #[test]
    fn an_answer_that_never_came_is_kept() {
        let (_dir, mut app, path) = board();
        app.finish_action(
            0,
            json!({"action":"dispatch","id":"0143"}),
            Err("the action timed out; its result is unknown.".into()),
        );
        let l = one(&path);
        assert_eq!(l["source"], "action");
        assert!(l["message"].as_str().unwrap().contains("timed out"), "{l}");
    }

    #[test]
    fn a_new_intents_words_never_reach_the_log() {
        let (_dir, mut app, path) = board();
        let words = "buy the domain for the Zephyr launch before anyone sees it";
        app.answer_stderr = format!("create failed for: {words}");
        app.finish_action(
            0,
            json!({"action":"create","words":words,"submission":"k-1"}),
            refused(&format!(
                "couldn't save: {words}, nor intents/0099-buy-domain-zephyr-launch"
            )),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("wasn't saved"), "{text}");
        assert!(!text.to_lowercase().contains("zephyr"), "{text}");
        assert!(!text.contains("buy the domain"), "{text}");
        // The screen still says what it said, words included: only the log leaves them out.
        assert!(app.message().unwrap().0.contains("Zephyr"));
    }

    #[test]
    fn a_refused_edit_keeps_its_cause_and_not_its_text() {
        let (_dir, mut app, path) = board();
        app.edits.insert(
            "k".into(),
            crate::app::edit_controls::Held {
                id: "0143".into(),
                machine: 0,
                current: "what it said before".into(),
                was: "sha256:x".into(),
                read_at: Instant::now(),
                mine: Some(crate::app::edit_controls::Mine {
                    text: "Zephyr's private plan".into(),
                    from: "what it said before".into(),
                }),
                check_first: false,
                read_only: None,
            },
        );
        app.answer_stderr = "index.lock exists; couldn't save Zephyr".into();
        app.finish_action(
            0,
            json!({"action":"edit","id":"0143","text":21,"was":"sha256:x"}),
            refused("couldn't save Zephyr"),
        );
        let l = one(&path);
        assert_eq!(l["action"], "edit");
        assert_eq!(l["stderr"], "index.lock exists; couldn't save [typed]");
        assert!(!l.to_string().contains("Zephyr"), "{l}");
    }

    #[test]
    fn what_a_later_failure_quotes_from_the_box_stays_out_too() {
        let (_dir, mut app, path) = board();
        app.draft = "the words still in the box being typed".into();
        app.fail("something quoted the words still in the box being typed here");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("still in the box"), "{text}");
    }

    #[test]
    fn a_kickoff_answer_stays_out() {
        let (_dir, mut app, path) = board();
        app.finish_action(
            0,
            json!({"action":"dispatch","id":"0143","answers":[{"num":"1","text":"only on the old laptop"}]}),
            refused("couldn't record: only on the old laptop"),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("old laptop"), "{text}");
    }

    #[test]
    fn a_token_in_a_message_stays_out() {
        let (_dir, mut app, path) = board();
        app.finish_action(
            0,
            json!({"action":"dispatch","id":"0143"}),
            refused("remote: bad credentials glpat-abcDEF123456xyz789"),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("abcDEF123456"), "{text}");
    }

    #[test]
    fn a_warning_inside_kerdr_is_kept_once_even_when_held_back_and_shown_later() {
        let (_dir, mut app, path) = board();
        app.fail("first");
        app.say("That image is too big.", Tone::Warn);
        app.say("All well.", Tone::Info);
        app.say("Done.", Tone::Good);
        assert_eq!(lines(&path).len(), 2);
        let l = &lines(&path)[1];
        assert_eq!(l["source"], "kerdr");
        assert_eq!(l["shown"], "warn");
        assert_eq!(l["message"], "That image is too big.");
        assert!(l["machine"].is_string(), "{l}");
        // Taking the failure down shows what was held; the log already has it.
        if let Some(m) = app.message.as_mut() {
            m.2 = Instant::now() - Duration::from_secs(60);
        }
        crate::app::tests::press(&mut app, 'j');
        assert!(app.message().unwrap().0.contains("too big"));
        assert_eq!(lines(&path).len(), 2);
    }

    #[test]
    fn a_failure_with_no_message_on_screen_is_kept() {
        let (_dir, mut app, path) = board();
        let request = json!({"action":"dispatch","id":"0143"});
        app.answer_arrives(0, &request, &refused("nothing said this"));
        app.answer_done();
        let l = one(&path);
        assert_eq!(l["shown"], "quiet");
        assert_eq!(l["message"], "nothing said this");
    }

    #[test]
    fn a_question_or_a_refusal_sent_on_to_its_home_is_no_error() {
        let (_dir, mut app, path) = board();
        let request = json!({"action":"dispatch","id":"0143"});
        for answer in [
            Response {
                confirmation: true,
                message: "sure?".into(),
                ..Default::default()
            },
            Response {
                home: Some("elsewhere/ARK".into()),
                message: "lives elsewhere".into(),
                ..Default::default()
            },
        ] {
            app.answer_arrives(0, &request, &Ok(answer));
            app.answer_done();
        }
        assert!(lines(&path).is_empty());
    }

    #[test]
    fn a_machine_that_stops_answering_is_kept_once_until_it_changes() {
        let (_dir, mut app, path) = board();
        let name = app.machines[1].name();
        let dropped = |attempt, ago: i64| Conn::Reconnecting {
            since: Local::now() - chrono::Duration::seconds(ago),
            attempt,
            retry_at: Local::now(),
            detail: "ssh: connect to host lab port 22: Connection timed out".into(),
        };
        let event = |conn| Event {
            machine: 1,
            conn,
            data: None,
            at: Local::now(),
        };
        // A short drop is kept quiet: most come back within a try or two (intent 0591).
        app.apply(event(dropped(1, 2)));
        app.apply(event(dropped(2, 6)));
        assert!(lines(&path).is_empty(), "{:?}", lines(&path));
        // Once it lasts, it's kept, once.
        app.apply(event(dropped(3, crate::poll::QUIET_FOR)));
        app.apply(event(dropped(4, crate::poll::QUIET_FOR + 15)));
        let l = one(&path);
        assert_eq!(l["source"], "machine");
        assert_eq!(l["shown"], "board");
        assert_eq!(l["machine"], name.as_str());
        assert!(
            l["message"]
                .as_str()
                .unwrap()
                .contains("Connection timed out")
        );
        // Worse is a change; back is not an error; dropping again is a new one, once it lasts.
        app.apply(event(Conn::Unreachable {
            since: Local::now(),
            retry_at: Local::now(),
            detail: "ssh: connect to host lab port 22: Connection timed out".into(),
        }));
        assert_eq!(lines(&path).len(), 2);
        app.apply(event(Conn::Live {
            latency: Duration::from_millis(5),
        }));
        assert_eq!(lines(&path).len(), 2);
        app.apply(event(dropped(1, 2)));
        assert_eq!(lines(&path).len(), 2);
        app.apply(event(dropped(5, crate::poll::QUIET_FOR + 1)));
        assert_eq!(lines(&path).len(), 3);
    }

    #[test]
    fn a_feed_or_a_session_that_fails_once_is_kept_quiet_until_it_lasts() {
        let (_dir, mut app, path) = board();
        let read = |feed: Result<crate::feed::Feed, String>, session: Result<(), String>, at| {
            let mut data = crate::poll::MachineData {
                feed: Some(feed),
                ..Default::default()
            };
            data.snapshots.insert(
                "web".into(),
                session.map(|_| crate::herdr::Snapshot::default()),
            );
            Event {
                machine: 1,
                conn: Conn::Live {
                    latency: Duration::from_millis(5),
                },
                data: Some(data),
                at,
            }
        };
        let good = || crate::feed::parse(r#"{"kerdr_feed":1}"#);
        let now = Local::now();
        app.apply(read(good(), Ok(()), now));
        // One failed feed read, re-read at once and good: nothing said, nothing kept.
        app.apply(read(Err(crate::poll::FEED_EMPTY.into()), Ok(()), now));
        app.apply(read(good(), Ok(()), now));
        assert!(app.machines[1].feed_problem(now).is_none());
        // A session slow once under load: the same.
        app.apply(read(good(), Err(crate::poll::SESSION_SILENT.into()), now));
        assert!(app.machines[1].session_problem("web", now).is_none());
        assert!(lines(&path).is_empty(), "{:?}", lines(&path));
        // Three feed reads in a row fail: now it's said, with its reason, and kept once.
        for _ in 0..3 {
            app.apply(read(
                Err(crate::poll::FEED_CUT_OFF.into()),
                Err(crate::poll::SESSION_SILENT.into()),
                now,
            ));
        }
        assert_eq!(
            app.machines[1].feed_problem(now),
            Some(crate::poll::FEED_CUT_OFF)
        );
        let kept = lines(&path);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0]["message"].as_str().unwrap().contains("cut off"));
        // The session's failure, once it has lasted QUIET_FOR, is said too.
        let later = now + chrono::Duration::seconds(crate::poll::QUIET_FOR);
        assert_eq!(
            app.machines[1].session_problem("web", later).as_deref(),
            Some(crate::poll::SESSION_SILENT)
        );
    }

    #[test]
    fn the_demo_keeps_nothing() {
        let (_dir, mut app, path) = board();
        app.demo = true;
        app.fail("made up");
        app.say("made up too", Tone::Warn);
        assert!(lines(&path).is_empty());
    }

    #[test]
    fn a_log_that_cant_be_written_leaves_the_screen_as_it_was() {
        let (dir, mut app, _path) = board();
        let in_the_way = dir.path().join("a file");
        std::fs::write(&in_the_way, "").unwrap();
        app.error_log = crate::errlog::ErrorLog::at(in_the_way.join("errors.log"));
        app.fail("#9 didn't start: refused.");
        assert_eq!(
            app.message().map(|(t, tone)| (t.to_string(), tone)),
            Some(("#9 didn't start: refused.".to_string(), Tone::Fail))
        );
    }
}
