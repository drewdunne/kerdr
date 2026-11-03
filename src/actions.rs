//! Opt-in actions sent as JSON to a command on the card's home machine.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Machine;
use crate::transport::{self, Target};

const MAX_JSON_BYTES: usize = 64 * 1024;

/// One of your open questions on background work, for the Kickoff Questions box (intent 0409).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Question {
    pub num: String,
    pub question: String,
    /// What you need to know to answer it.
    #[serde(default)]
    pub background: String,
    /// The recommended answer, and why.
    #[serde(default)]
    pub pick: String,
    #[serde(default)]
    pub why: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    #[serde(deserialize_with = "confirmation")]
    pub confirmation: bool,
    pub yes: Option<String>,
    pub no: Option<String>,
    pub status: Option<String>,
    pub url: Option<String>,
    pub ticket: Option<String>,
    #[serde(default)]
    pub needs_code: bool,
    /// For a sign-in typed on the provider's page: the code to type there, and when it expires.
    pub user_code: Option<String>,
    pub expires: Option<String>,

    pub used: Option<u32>,
    pub limit: Option<u32>,
    pub session: Option<String>,
    pub id: Option<String>,
    /// A herdr workspace to go into now, in `session` on the machine the action ran on: what a
    /// reopen answers with.
    pub workspace: Option<String>,
    /// That workspace's name, for messages.
    pub label: Option<String>,
    /// With a confirmation: a token of what it asked about, sent back with the yes.
    pub confirm: Option<String>,
    /// The tab in `workspace` to land on, when a reopen opened one in a workspace already open.
    pub tab: Option<String>,
    /// With a refusal: the intent's home, `<Machine>/<Session>`, when it lives on another machine
    /// than the one asked. Kerdr asks that machine instead, once (intent 0411).
    pub home: Option<String>,
    /// With `account-current`: the current account as the machine asked sees it now, null once
    /// cleared.
    pub current: Option<crate::accounts::Current>,
    /// With a start: the machine it was asked for on, when that isn't the one asked (intent
    /// 0445): it starts there within minutes.
    pub machine: Option<String>,
    /// For an edit: a backlog item's text, as read or as it is now when a save found it changed,
    /// and its fingerprint, sent back with a save of it (intent 0414).
    pub text: Option<String>,
    pub was: Option<String>,
    /// With a start refused for your open questions: those questions, whole, and a token of what
    /// was asked, sent back with your answers (intent 0409).
    #[serde(default)]
    pub questions: Vec<Question>,
    pub asked: Option<String>,
    /// Something for the Kickoff Questions box to say.
    pub note: Option<String>,
}

impl Response {
    /// A refusal naming another home: Kerdr may send the change on there, in its place (intents
    /// 0411 and 0416).
    pub fn sends_on(&self) -> bool {
        !self.ok && !self.confirmation && self.home.is_some()
    }
}

fn confirmation<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let value = Value::deserialize(d)?;
    match value {
        Value::Bool(b) => Ok(b),
        Value::String(s) => Ok(!s.is_empty()),
        _ => Err(serde::de::Error::custom(
            "confirmation must be boolean or question text",
        )),
    }
}

pub fn parse_response(text: &str) -> Result<Response, serde_json::Error> {
    let mut value: Value = serde_json::from_str(text)?;
    if let Some(question) = value["confirmation"].as_str().map(str::to_string) {
        value["message"] = Value::String(question);
    }
    serde_json::from_value(value)
}

/// The statuses actions answer with (docs/feed.md), kept as they are by the credential scrub.
const KNOWN_STATUSES: &[&str] = &[
    "waiting",
    "done",
    "failed",
    "cancelled",
    "starting",
    "ready",
    "queued",
    "stale",
    "moved",
    "saved",
    "unchanged",
    "partial",
];

/// Metadata returned to the UI after sending; credentials never enter its result queue, and a
/// new intent's images, and an edited item's text, are only counted there: the board keeps the
/// text itself until it's saved (intent 0414).
pub fn request_metadata(request: &Value) -> Value {
    let mut value = request.clone();
    if value["action"] == "edit"
        && let Some(text) = value["text"].as_str().map(|t| t.chars().count())
    {
        value["text"] = Value::from(text);
    }
    if value["action"] == "account-signin"
        && let Some(fields) = value.as_object_mut()
    {
        fields.remove("token");
        fields.remove("page_code");
    }
    if let Some(images) = value["images"].as_array().map(Vec::len) {
        value["images"] = Value::from(images);
    }
    value
}

/// Whether a request is past what Kerdr sends: 64 KiB, and room for a new intent's images.
pub fn too_large(request: &Value) -> bool {
    serde_json::to_string(request).map_or(true, |json| json.len() > most_bytes(request))
}

/// The most an answer may be: 64 KiB, and for an edit room for an item's text (at most 48 KiB)
/// however much of it JSON escapes.
fn most_answer_bytes(request: &Value) -> usize {
    if request["action"] == "edit" {
        3 * MAX_JSON_BYTES
    } else {
        MAX_JSON_BYTES
    }
}

/// The most a request may be: 64 KiB, and room for the images a new intent may carry.
fn most_bytes(request: &Value) -> usize {
    if request["action"] == "create" && request["images"].is_array() {
        MAX_JSON_BYTES + crate::images::REQUEST_ALLOWANCE
    } else {
        MAX_JSON_BYTES
    }
}

pub fn run(machine: &Machine, command: &str, request: &Value) -> Result<Response, String> {
    run_with_deadline(machine, command, request, deadline_for(request), None).0
}

/// As run, calling `handed` once the adapter has the whole request: from then on it carries on
/// if Kerdr quits (review of intent 0445).
pub fn run_handing(
    machine: &Machine,
    command: &str,
    request: &Value,
    handed: transport::Handed,
) -> Answered {
    answer_with_stderr(
        machine,
        command,
        request,
        deadline_for(request),
        Some(handed),
    )
}

/// As run, with what the command printed on standard error, for the error log.
pub fn run_keeping_stderr(machine: &Machine, command: &str, request: &Value) -> Answered {
    answer_with_stderr(machine, command, request, deadline_for(request), None)
}

/// How long an action may take before Kerdr stops waiting. Starting or reopening work can take
/// minutes: a start on another machine waits up to four for it there, so those two get five, and
/// the backend always answers first (intent 0411).
pub fn deadline_for(request: &Value) -> Duration {
    if starts_work(request) {
        Duration::from_secs(300)
    } else {
        Duration::from_secs(120)
    }
}

/// Whether a request starts work: a start (d) or a reopen (⏎).
fn starts_work(request: &Value) -> bool {
    matches!(request["action"].as_str(), Some("dispatch" | "reopen"))
}

/// How a message names the record a request is for: `#391` for `0391`.
fn reference(request: &Value) -> String {
    let id = request["id"].as_str().unwrap_or_default();
    let trimmed = id.trim_start_matches('0');
    if id.is_empty() {
        "this work".into()
    } else if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        id.to_string()
    } else {
        format!("#{trimmed}")
    }
}

/// An action's answer, and what its command printed on standard error, ready for the error log:
/// nothing in it that was typed into the request, or a credential.
pub struct Answered {
    pub result: Result<Response, String>,
    pub stderr: String,
}

fn answer_with_stderr(
    machine: &Machine,
    command: &str,
    request: &Value,
    deadline: Duration,
    handed: Option<transport::Handed>,
) -> Answered {
    let (result, stderr) = run_with_deadline(machine, command, request, deadline, handed);
    // An edit's error output may quote the text it was sent, a piece too small to recognise
    // included, so a save's is left out here as it is from the message.
    let stderr = if request["action"] == "edit" && request.get("text").is_some() {
        String::new()
    } else {
        crate::errlog::scrub(&redact_credentials(&stderr, request), &typed_in(request))
    };
    Answered { result, stderr }
}

/// What was typed into a request: a new intent's words, an edit, an answer.
pub fn typed_in(request: &Value) -> Vec<&str> {
    let mut typed: Vec<&str> = ["words", "title", "text", "token", "page_code"]
        .iter()
        .filter_map(|field| request[*field].as_str())
        .collect();
    if let Some(answers) = request["answers"].as_array() {
        typed.extend(answers.iter().filter_map(|a| a["text"].as_str()));
    }
    typed
}

/// The answer, and what the command printed on standard error, unscrubbed.
fn run_with_deadline(
    machine: &Machine,
    command: &str,
    request: &Value,
    deadline: Duration,
    handed: Option<transport::Handed>,
) -> (Result<Response, String>, String) {
    if command.trim().is_empty() {
        return (
            Err("actions are disabled; configure [actions] command".into()),
            String::new(),
        );
    }
    let json = match serde_json::to_string(request) {
        Ok(json) => json,
        Err(e) => return (Err(format!("the action request: {e}")), String::new()),
    };
    if json.len() > most_bytes(request) {
        let said = if most_bytes(request) > MAX_JSON_BYTES {
            format!(
                "the action request is too large (maximum 64 KiB, and {} of images)",
                crate::images::size(crate::images::MAX_IMAGES_BYTES)
            )
        } else {
            "the action request is too large (maximum 64 KiB)".into()
        };
        return (Err(said), String::new());
    }
    let target = match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    };
    // The configured command is source; user data is JSON on stdin, never source.
    let output = transport::run_adapter_handing(&target, command, Some(&json), deadline, handed);
    let stderr = output.stderr.clone();
    (answer_of(&output, request, deadline), stderr)
}

/// What a command's output means.
fn answer_of(
    output: &transport::Output,
    request: &Value,
    deadline: Duration,
) -> Result<Response, String> {
    let redact = |text: &str| redact_credentials(text, request);
    if output.timed_out {
        if starts_work(request) {
            let verb = if request["action"] == "reopen" {
                "Reopening"
            } else {
                "Starting"
            };
            let minutes = deadline.as_secs().div_ceil(60);
            let how_long = match minutes {
                0 | 1 => "a minute".to_string(),
                2 => "two minutes".into(),
                5 => "five minutes".into(),
                n => format!("{n} minutes"),
            };
            return Err(format!(
                "{verb} {} is taking more than {how_long}; it may still open. Look at its card before trying again.",
                reference(request)
            ));
        }
        return Err("the action timed out; its result is unknown. Check the canonical record before trying again. Do not retry automatically.".into());
    }
    if output.stdout.len() > most_answer_bytes(request) {
        return Err("the action's JSON response is too large; its result is unknown. Do not retry automatically.".into());
    }
    // A backend can request confirmation with a nonzero exit code. Its JSON is the contract,
    // so parse it before interpreting process status.
    parse_response(&output.stdout).map(|mut response| {
        response.message = redact(&response.message);
        for text in [&mut response.yes, &mut response.no, &mut response.url, &mut response.ticket, &mut response.confirm, &mut response.session, &mut response.id, &mut response.workspace, &mut response.label, &mut response.tab, &mut response.home, &mut response.machine, &mut response.user_code, &mut response.expires, &mut response.text, &mut response.was, &mut response.asked, &mut response.note].into_iter().flatten() {
            *text = redact(text);
        }
        for question in &mut response.questions {
            for text in [&mut question.num, &mut question.question, &mut question.background, &mut question.pick, &mut question.why] {
                *text = redact(text);
            }
        }
        // A status the protocol knows stays as it is, even when it matches what was sent (a code
        // "done"); anything else is scrubbed like the rest.
        if let Some(status) = response.status.as_mut().filter(|s| !KNOWN_STATUSES.contains(&s.as_str())) {
            *status = redact(status);
        }
        response
    }).map_err(|error| {
        let status = output.status.map_or_else(|| "no exit status".into(), |s| format!("exit {s}"));
        // An edit's error output may quote the text it was sent (a secret its check found, say),
        // so it's left out for a save.
        let detail: String = if request["action"] == "edit" && request.get("text").is_some() {
            String::new()
        } else {
            redact(output.stderr.trim()).chars().take(1024).collect()
        };
        let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
        let error = redact(&error.to_string());
        format!("the action's JSON response is invalid ({status}): {error}{detail}. Its result is unknown. Do not retry automatically.")
    })
}

fn redact_credentials(text: &str, request: &Value) -> String {
    let mut text = text.to_string();
    if request["action"] == "account-signin" {
        for field in ["token", "page_code"] {
            if let Some(secret) = request[field].as_str().filter(|s| !s.is_empty()) {
                text = text.replace(secret, "[redacted]");
                if secret != secret.trim() && !secret.trim().is_empty() {
                    text = text.replace(secret.trim(), "[redacted]");
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::transport::quote;
    use serde_json::json;
    use std::fs;

    fn deadline_answer(
        machine: &Machine,
        command: &str,
        request: &Value,
        deadline: Duration,
        handed: Option<transport::Handed>,
    ) -> Result<Response, String> {
        run_with_deadline(machine, command, request, deadline, handed).0
    }

    fn machine() -> Machine {
        Config::default().machines("here", &[]).remove(0)
    }

    fn fixture(dir: &std::path::Path, body: &str, windows: &str) -> String {
        let (name, body) = if cfg!(windows) {
            ("action fixture.ps1", format!("\u{feff}{windows}"))
        } else {
            ("action fixture.sh", body.to_string())
        };
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        if cfg!(windows) {
            format!("& {}", transport::powershell_quote(path.to_str().unwrap()))
        } else {
            format!("sh {}", quote(path.to_str().unwrap()))
        }
    }

    #[test]
    fn a_backend_cannot_echo_the_submitted_token_into_a_result_or_error() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "fixture-private-credential";
        let command = fixture(
            dir.path(),
            "cat >/dev/null\nprintf '%s' '{\"ok\":false,\"message\":\"fixture-private-credential refused\"}'\n",
            "[Console]::In.ReadToEnd() | Out-Null; Write-Output '{\"ok\":false,\"message\":\"fixture-private-credential refused\"}'",
        );
        let request =
            json!({"action":"account-signin","step":"paste","code":"EXAMPLE","token":secret});
        let response = run(&machine(), &command, &request).unwrap();
        assert!(!response.message.contains(secret));
        assert!(response.message.contains("[redacted]"));
        // Nor into a sign-in's code to type, or its expiry.
        let reply = r#"{"ok":true,"status":"waiting","message":"m","user_code":"fixture-private-credential","expires":"until fixture-private-credential"}"#;
        let command = fixture(
            dir.path(),
            &format!("cat >/dev/null\nprintf '%s' '{reply}'\n"),
            &format!("[Console]::In.ReadToEnd() | Out-Null; Write-Output '{reply}'"),
        );
        let response = run(&machine(), &command, &request).unwrap();
        assert_eq!(response.user_code.as_deref(), Some("[redacted]"));
        assert!(!format!("{response:?}").contains(secret));
        // A code that happens to be a status word leaves the status as the protocol's.
        let reply = r#"{"ok":true,"status":"done","message":"signed in"}"#;
        let command = fixture(
            dir.path(),
            &format!("cat >/dev/null\nprintf '%s' '{reply}'\n"),
            &format!("[Console]::In.ReadToEnd() | Out-Null; Write-Output '{reply}'"),
        );
        let finish =
            json!({"action":"account-signin","step":"finish","ticket":"t","page_code":"done"});
        let response = run(&machine(), &command, &finish).unwrap();
        assert_eq!(response.status.as_deref(), Some("done"));
    }

    #[test]
    fn short_and_json_escaped_credentials_do_not_corrupt_response_structure() {
        for secret in ["ok", "true", "a", "quote\"and\\slash"] {
            let dir = tempfile::tempdir().unwrap();
            let reply =
                json!({"ok":true,"status":"done","message":format!("echo {secret}")}).to_string();
            let command = fixture(
                dir.path(),
                &format!(
                    "cat >/dev/null\nprintf '%s' {}\n",
                    crate::transport::quote(&reply)
                ),
                &format!(
                    "[Console]::In.ReadToEnd() | Out-Null; Write-Output '{}'",
                    reply.replace('\'', "''")
                ),
            );
            let response = run(
                &machine(),
                &command,
                &json!({"action":"account-signin","step":"finish","page_code":secret}),
            )
            .unwrap();
            assert!(response.ok);
            assert_eq!(response.status.as_deref(), Some("done"));
            assert!(response.message.contains("[redacted]"));
        }
    }

    #[test]
    fn every_account_action_uses_the_platform_adapter_contract() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
            "tests/fixtures/accounts.ps1"
        } else {
            "tests/fixtures/accounts.sh"
        });
        let command = if cfg!(windows) {
            format!("& {}", transport::powershell_quote(file.to_str().unwrap()))
        } else {
            format!("sh {}", quote(file.to_str().unwrap()))
        };
        for action in [
            "account-move",
            "account-move-waiting",
            "account-models",
            "account-add",
            "account-test",
            "account-remove",
        ] {
            let response=run(&machine(),&command,&json!({"action":action,"code":"EXAMPLE","pane":"w1:p1","session":"web","conversation":"fixture-conversation","to":"EXAMPLE","owner":"studio","models":["EXAMPLE"]})).unwrap();
            if action == "account-move" {
                assert!(response.confirmation);
                assert_eq!(response.confirm.as_deref(), Some("fixture-confirm"));
                assert!(response.message.starts_with("Mid-step"));
            } else {
                assert!(response.ok, "{action}: {response:?}");
            }
        }
        for (conversation, message) in [
            ("fixture-conversation", "moving to ARK"),
            ("stale-conversation", "the conversation changed"),
        ] {
            let response = run(&machine(), &command, &json!({"action":"account-move","pane":"w1:p1","session":"web","conversation":conversation,"to":"ARK","confirm":"fixture-confirm"})).unwrap();
            assert!(response.ok && !response.confirmation);
            assert_eq!(response.message, message);
        }
        for step in ["start", "poll", "finish", "cancel", "paste"] {
            let response=run(&machine(),&command,&json!({"action":"account-signin","step":step,"code":"EXAMPLE","ticket":"fixture-ticket","page_code":"page-code"})).unwrap();
            assert!(response.ok);
            assert!(response.status.is_some());
            if step == "cancel" {
                assert_eq!(response.status.as_deref(), Some("cancelled"));
            }
            if step == "start" {
                assert!(response.needs_code);
                assert_eq!(response.ticket.as_deref(), Some("fixture-ticket"));
            }
        }
        // A copy reaches the adapter: its start gets its own ticket, its test its own answer,
        // and a paste meant for it is refused rather than landing in another copy.
        let copy = |request: Value| {
            let mut request = request;
            request["copy"] = json!("tenants");
            run(&machine(), &command, &request).unwrap()
        };
        let start = copy(json!({"action":"account-signin","step":"start","code":"EXAMPLE"}));
        assert!(start.ok && start.needs_code);
        assert_eq!(start.ticket.as_deref(), Some("fixture-tenants-ticket"));
        let test = copy(json!({"action":"account-test","code":"EXAMPLE"}));
        assert!(test.ok);
        assert_eq!(test.message, "The tenants copy works");
        let paste =
            copy(json!({"action":"account-signin","step":"paste","code":"EXAMPLE","token":"t"}));
        assert!(!paste.ok);
        assert_eq!(paste.status.as_deref(), Some("failed"));
        let agents = run(
            &machine(),
            &command,
            &json!({"action":"account-signin","step":"start","code":"EXAMPLE","copy":"agents"}),
        )
        .unwrap();
        assert_eq!(agents.ticket.as_deref(), Some("fixture-ticket"));
        // A provider whose page takes a code answers with it and asks nothing back.
        let device = run(
            &machine(),
            &command,
            &json!({"action":"account-signin","step":"start","code":"DEVICE_X"}),
        )
        .unwrap();
        assert!(device.ok && !device.needs_code);
        assert_eq!(device.user_code.as_deref(), Some("K7QD-3MPWX"));
        assert_eq!(device.expires.as_deref(), Some("2026-10-31T18:15:00Z"));
        // A move to an account of another program is a hand-over, asked first.
        let move_to = |confirm: Option<&str>| {
            let mut request = json!({"action":"account-move","pane":"w1:p1","session":"web","conversation":"fixture-conversation","to":"HAND_X"});
            if let Some(token) = confirm {
                request["confirm"] = json!(token);
            }
            run(&machine(), &command, &request).unwrap()
        };
        let asked = move_to(None);
        assert!(asked.confirmation);
        assert_eq!(asked.yes.as_deref(), Some("hand it over"));
        assert_eq!(asked.confirm.as_deref(), Some("fixture-handover"));
        let queued = move_to(Some("fixture-handover"));
        assert!(queued.ok && !queued.confirmation);
        assert_eq!(queued.message, "handing over to HAND_X");
    }

    #[cfg(not(windows))]
    #[test]
    fn stderr_comes_back_for_the_log_without_what_was_typed() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'the push failed: rejected' >&2\necho 'quoting my private secret words' >&2\nprintf '%s' '{\"ok\":false,\"message\":\"no\"}'\n",
            "",
        );
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"create","words":"my private secret words"}),
        );
        assert!(!answered.result.unwrap().ok);
        assert!(answered.stderr.contains("the push failed: rejected"));
        assert!(!answered.stderr.contains("private"), "{}", answered.stderr);
        // A save's is left out whole: it may quote its text in pieces too small to recognise.
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"edit","id":"1","text":"my text","was":"x"}),
        );
        assert_eq!(answered.stderr, "");
        // A sign-in's token doesn't come back either.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'bad token fixture-private-value' >&2\nprintf 'not json'\n",
            "",
        );
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"account-signin","token":"fixture-private-value"}),
        );
        assert!(answered.result.is_err());
        assert!(
            !answered.stderr.contains("fixture-private"),
            "{}",
            answered.stderr
        );
    }

    #[test]
    fn redaction_covers_trimmed_credential_echoes() {
        assert_eq!(
            redact_credentials(
                "echo: fixture-private-value",
                &json!({"action":"account-signin","token":"  fixture-private-value  "})
            ),
            "echo: [redacted]"
        );
    }

    #[test]
    fn sends_user_text_as_json_without_executing_shell_punctuation() {
        let dir = tempfile::tempdir().unwrap();
        let captured = dir.path().join("captured.json");
        let sentinel = dir.path().join("injected");
        let title = format!(
            "it's `touch {0}`; $(touch {0})\n100% 🦀",
            sentinel.display()
        );
        let request = json!({"action":"create", "title":title, "words":"two\nlines"});
        let command = fixture(
            dir.path(),
            &format!(
                "cat > {}\nprintf '%s' '{{\"ok\":true,\"message\":\"created\",\"id\":\"0004\"}}'\n",
                quote(captured.to_str().unwrap())
            ),
            &format!(
                "[IO.File]::WriteAllText({}, [Console]::In.ReadToEnd(), (New-Object Text.UTF8Encoding $false)); Write-Output '{{\"ok\":true,\"message\":\"created\",\"id\":\"0004\"}}'",
                transport::powershell_quote(captured.to_str().unwrap())
            ),
        );
        let response = run(&machine(), &command, &request).unwrap();
        assert!(response.ok);
        assert_eq!(response.id.as_deref(), Some("0004"));
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(captured).unwrap()).unwrap(),
            request
        );
        assert!(!sentinel.exists());
    }

    #[test]
    fn returns_confirmation_json_even_when_command_exits_nonzero() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\nprintf '%s' '{\"ok\":false,\"message\":\"session full\",\"confirmation\":true,\"used\":3,\"limit\":3,\"session\":\"web\"}'\nexit 3\n",
            "[Console]::In.ReadToEnd() | Out-Null; Write-Output '{\"ok\":false,\"message\":\"session full\",\"confirmation\":true,\"used\":3,\"limit\":3,\"session\":\"web\"}'; exit 3",
        );
        let response = run(&machine(), &command, &json!({"action":"dispatch","id":"4"})).unwrap();
        assert!(!response.ok);
        assert!(response.confirmation);
        assert_eq!(response.used, Some(3));
        assert_eq!(response.limit, Some(3));
        assert_eq!(response.session.as_deref(), Some("web"));
    }

    #[test]
    fn malformed_output_is_reported_with_command_failure_detail() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'not JSON'\necho 'tracker unavailable' >&2\nexit 7\n",
            "[Console]::In.ReadToEnd() | Out-Null; Write-Output 'not JSON'; [Console]::Error.WriteLine('tracker unavailable'); exit 7",
        );
        let error = run(&machine(), &command, &json!({"action":"refresh","id":"4"})).unwrap_err();
        assert!(error.contains("JSON"), "{error}");
        assert!(error.contains("7"), "{error}");
        assert!(error.contains("tracker unavailable"), "{error}");
    }

    #[test]
    fn a_timeout_warns_that_the_result_is_unknown_and_must_not_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\nsleep 5\n",
            "[Console]::In.ReadToEnd() | Out-Null; Start-Sleep 5",
        );
        let error = deadline_answer(
            &machine(),
            &command,
            &json!({"action":"refresh","id":"4"}),
            Duration::from_millis(60),
            None,
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(error.contains("result is unknown"), "{error}");
        assert!(error.contains("Do not retry automatically"), "{error}");
    }

    #[test]
    fn starting_or_reopening_work_gets_five_minutes_and_says_it_may_still_open() {
        assert_eq!(
            deadline_for(&json!({"action":"dispatch","id":"4"})),
            Duration::from_secs(300)
        );
        assert_eq!(
            deadline_for(&json!({"action":"reopen","id":"4"})),
            Duration::from_secs(300)
        );
        for action in ["close", "create", "refresh", "account-test"] {
            assert_eq!(
                deadline_for(&json!({"action":action})),
                Duration::from_secs(120),
                "{action}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\nsleep 5\n",
            "[Console]::In.ReadToEnd() | Out-Null; Start-Sleep 5",
        );
        for (action, verb) in [("dispatch", "Starting #391"), ("reopen", "Reopening #391")] {
            let error = deadline_answer(
                &machine(),
                &command,
                &json!({"action":action,"id":"0391"}),
                Duration::from_millis(60),
                None,
            )
            .unwrap_err();
            assert!(error.starts_with(verb), "{error}");
            assert!(error.contains("is taking more than"), "{error}");
            assert!(error.contains("may still open"), "{error}");
            assert!(!error.contains("canonical record"), "{error}");
            assert!(!error.contains("Do not retry"), "{error}");
        }
    }

    #[test]
    fn a_reopen_names_its_new_tab_and_a_refusal_the_intents_home() {
        let reopened = parse_response(
            r#"{"ok":true,"message":"m","session":"S","workspace":"w4","tab":"w4:t2"}"#,
        )
        .unwrap();
        assert_eq!(reopened.tab.as_deref(), Some("w4:t2"));
        let plain =
            parse_response(r#"{"ok":true,"message":"m","session":"S","workspace":"w4"}"#).unwrap();
        assert_eq!(plain.tab, None);
        assert_eq!(plain.home, None);
        let moved = parse_response(
            r##"{"ok":false,"message":"#7 lives on build-box now (ARK); ⏎ again opens it there.","home":"build-box/ARK"}"##,
        )
        .unwrap();
        assert!(!moved.ok);
        assert_eq!(moved.home.as_deref(), Some("build-box/ARK"));
    }

    #[test]
    fn a_new_intent_carries_its_images_on_stdin_and_only_counts_them_afterwards() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let captured = dir.path().join("captured.json");
        // A screenshot's worth of bytes: far past the 64 KiB words limit, within the images'.
        let data = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 3_000_000]);
        let request = json!({"action":"create","words":"see [Image #1]","session":"default",
            "images":[{"tag":1,"type":"image/png","data":data}]});
        let command = fixture(
            dir.path(),
            &format!(
                "cat > {}\nprintf '%s' '{{\"ok\":true,\"message\":\"created\",\"id\":\"0004\"}}'\n",
                quote(captured.to_str().unwrap())
            ),
            &format!(
                "[IO.File]::WriteAllText({}, [Console]::In.ReadToEnd(), (New-Object Text.UTF8Encoding $false)); Write-Output '{{\"ok\":true,\"message\":\"created\",\"id\":\"0004\"}}'",
                transport::powershell_quote(captured.to_str().unwrap())
            ),
        );
        let response = run(&machine(), &command, &request).unwrap();
        assert!(response.ok);
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(captured).unwrap()).unwrap(),
            request
        );
        assert_eq!(request_metadata(&request)["images"], 1);
        // Images beyond what a new intent may carry are still refused before anything runs.
        let sentinel = dir.path().join("ran");
        let too_many = "x".repeat(crate::images::REQUEST_ALLOWANCE + MAX_JSON_BYTES);
        let error = run(
            &machine(),
            &format!("touch {}", quote(sentinel.to_str().unwrap())),
            &json!({"action":"create","words":"x","images":[{"data":too_many}]}),
        )
        .unwrap_err();
        assert!(error.contains("20 MB of images"), "{error}");
        assert!(!sentinel.exists());
    }

    #[test]
    fn an_edit_carries_its_text_on_stdin_and_only_counts_it_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let captured = dir.path().join("captured.json");
        let text = "Line one, it's `quoted`; $(touch x)\n\n### A heading 🦀";
        let request =
            json!({"action":"edit","id":"0414","text":text,"was":"sha256:aa","from":"amd-lzm4"});
        let reply = r#"{"ok":false,"status":"stale","message":"The text changed while you had it open.","text":"Newer.","was":"sha256:bb"}"#;
        let command = fixture(
            dir.path(),
            &format!(
                "cat > {}\nprintf '%s' '{reply}'\n",
                quote(captured.to_str().unwrap())
            ),
            &format!(
                "[IO.File]::WriteAllText({}, [Console]::In.ReadToEnd(), (New-Object Text.UTF8Encoding $false)); Write-Output '{reply}'",
                transport::powershell_quote(captured.to_str().unwrap())
            ),
        );
        let response = run(&machine(), &command, &request).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(captured).unwrap()).unwrap(),
            request
        );
        assert!(!response.ok);
        assert_eq!(response.status.as_deref(), Some("stale"));
        assert_eq!(response.text.as_deref(), Some("Newer."));
        assert_eq!(response.was.as_deref(), Some("sha256:bb"));
        // What comes back to the board names no text, only how long it was.
        let metadata = request_metadata(&request);
        assert_eq!(metadata["text"], text.chars().count());
        assert!(!metadata.to_string().contains("Line one"));
        // A read has no text to count.
        let read = json!({"action":"edit","id":"0414","from":"amd-lzm4"});
        assert_eq!(request_metadata(&read), read);
        // Its statuses are the protocol's, kept as they are.
        for status in ["saved", "unchanged", "partial", "stale"] {
            assert!(KNOWN_STATUSES.contains(&status), "{status}");
        }
        // A save past 64 KiB is refused before anything runs, and said to be too large.
        let big = json!({"action":"edit","id":"0414","text":"x".repeat(70_000),"was":"sha256:aa"});
        assert!(too_large(&big));
        assert!(!too_large(&request));
        let sentinel = dir.path().join("ran");
        let error = run(
            &machine(),
            &format!("touch {}", quote(sentinel.to_str().unwrap())),
            &big,
        )
        .unwrap_err();
        assert!(error.contains("too large"), "{error}");
        assert!(!sentinel.exists());
    }

    #[test]
    fn an_edit_saves_error_output_never_reaches_the_board() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >&2\necho 'leak: kerdr-fixture-secret-line' >&2\nexit 1\n",
            "[Console]::Error.WriteLine([Console]::In.ReadToEnd()); [Console]::Error.WriteLine('leak: kerdr-fixture-secret-line'); exit 1",
        );
        let save = json!({"action":"edit","id":"0414","text":"kerdr-fixture-secret-line","was":"sha256:aa"});
        let error = run(&machine(), &command, &save).unwrap_err();
        assert!(!error.contains("kerdr-fixture-secret-line"), "{error}");
        assert!(error.contains("invalid (exit 1)"), "{error}");
        // A read sends no text, so its error output is shown as any action's is.
        let read = json!({"action":"edit","id":"0414"});
        let error = run(&machine(), &command, &read).unwrap_err();
        assert!(error.contains("leak: kerdr-fixture-secret-line"), "{error}");
    }

    #[test]
    fn rejects_oversized_requests_before_running_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("ran");
        let command = format!("touch {}", quote(sentinel.to_str().unwrap()));
        let error = run(
            &machine(),
            &command,
            &json!({"action":"create","words":"x".repeat(70_000)}),
        )
        .unwrap_err();
        assert!(error.contains("too large"), "{error}");
        assert!(!sentinel.exists());
    }
}
