//! Opt-in actions sent as JSON to a command on the card's home machine.

use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Machine;
use crate::transport::{self, Target};

const MAX_JSON_BYTES: usize = 64 * 1024;

/// One of your open questions on background work, for the Kickoff Questions box (intent 0409).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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
    /// With a start on an account `s` picked: true when nothing was restarted (its agent was
    /// running already, or its session is being brought back), so that account wasn't applied
    /// and the message says so (intent 0580).
    #[serde(default)]
    pub account_unchanged: bool,
    /// Work that goes on after this answer, for Kerdr to follow with `progress` requests to the
    /// same machine (intent 0591): starting a herdr session first, a restore, a move into herdr.
    #[serde(default, deserialize_with = "follow")]
    pub follow: Option<Follow>,
    /// A `progress` answer's state: `running`, `done`, `asks`, `failed` or `unknown`.
    pub state: Option<String>,
    /// With `running`: what the followed work is on now.
    pub say: Option<String>,
    /// On a refusal that will likely clear by itself (a lock held, a push race, herdr slow to
    /// answer, the same op's earlier send still running): seconds to wait before sending the same
    /// request, with the same op, again (intent 0591).
    #[serde(default, deserialize_with = "seconds")]
    pub retry: Option<u64>,
    /// The answer an earlier send of the same op got, given again: nothing was done twice.
    #[serde(default)]
    pub repeat: bool,
    /// With `ok: false`: part of the change happened, and the message says which part.
    #[serde(default)]
    pub partial: bool,
    /// On a sign-in that stopped: a fresh one is what's wanted, its login having ended by itself.
    #[serde(default)]
    pub again: bool,
    /// On a sign-in that stopped: a newer sign-in of the same account took its place, which isn't
    /// a failure (intent 0591).
    #[serde(default)]
    pub replaced: bool,
    /// On a failed test: the sign-in is what's wrong, so the way to fix it can open (intent 0591).
    #[serde(default)]
    pub sign_in: bool,
}

impl Response {
    /// A sign-in still going, its link or the browser step yet to come. `starting` is a slow
    /// program that hasn't given its link yet (intent 0591).
    pub fn signin_under_way(&self) -> bool {
        matches!(self.status.as_deref(), Some("waiting" | "starting"))
    }
}

/// `retry` as a backend gives it: whole or fractional seconds, rounded up; anything else is none.
fn seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(value
        .as_f64()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| s.ceil().min(3600.0) as u64))
}

/// Whether a request can change something, and so carries an `op` (intent 0591): every action
/// except `progress`, `kickoff` and an `edit` that only reads its text.
pub fn changes(request: &Value) -> bool {
    match request["action"].as_str() {
        None | Some("progress" | "kickoff") => false,
        Some("edit") => request.get("text").is_some(),
        Some(_) => true,
    }
}

/// A fresh id for one request Andrew makes, `k-` and 16 lowercase hex digits: the backend does
/// a request with an op it has seen once, and answers a second send of it from its record.
pub fn new_op() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    // RandomState is seeded from the system's randomness, afresh for each one made.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    format!("k-{:016x}", h.finish())
}

/// The request with an op of its own when it can change something, in place of any it carried:
/// each key Andrew presses is a new request, a yes to a question included, which would otherwise
/// get the question back from the record. Only an automatic resend keeps its op.
pub fn with_op(mut request: Value) -> Value {
    if let Some(fields) = request.as_object_mut() {
        fields.remove("op");
    }
    if changes(&request) {
        request["op"] = Value::String(new_op());
    }
    request
}

/// The request as Andrew asked for it, without the op Kerdr gave it: what the backend fingerprints.
pub fn without_op(request: &Value) -> Value {
    let mut request = request.clone();
    if let Some(fields) = request.as_object_mut() {
        fields.remove("op");
    }
    request
}

/// How long Kerdr keeps sending a request again by itself, counted from its first answer (intent
/// 0591): every wait and every resend's own time come out of it, and none goes past it.
pub const RETRY_MOST: Duration = Duration::from_secs(120);

/// The time by two clocks, for RETRY_MOST: the steady one (`Instant`), which a change of the
/// computer's clock doesn't move, and the wall clock, which goes on while the computer sleeps, as
/// Linux's steady clock doesn't. Retrying's two minutes are up when either says so (Codex's third
/// look, intent 0591): a laptop that slept through a wait sends nothing more when it wakes.
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    pub steady: Instant,
    pub wall: SystemTime,
}

impl Clocks {
    pub fn now() -> Clocks {
        Clocks {
            steady: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    /// What's left of RETRY_MOST from `start` to now, by whichever clock has less left. A wall
    /// clock set back counts nothing as gone, so the steady clock's count stands.
    fn left_of_retrying(start: Clocks, now: Clocks) -> Duration {
        let steady = now.steady.saturating_duration_since(start.steady);
        let wall = now.wall.duration_since(start.wall).unwrap_or_default();
        RETRY_MOST.saturating_sub(steady.max(wall))
    }
}

/// The least a resend is given to answer in, within RETRY_MOST: most answers come in a second or
/// two. With less left, Kerdr sends nothing more and says the last answer as it is, rather than wait
/// a shorter while than was asked or send one that can't hear back.
pub const RESEND_ROOM: Duration = Duration::from_secs(10);

/// The waits before each resend of a request whose answer was lost (no answer in time, the
/// connection dropped, the answer cut off), to a backend that keeps ops: three, over about 30 s.
pub const LOST_WAITS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(15),
];

/// What the words of an error say when nobody can tell whether the request happened: its answer
/// was lost. Every such error ends so, and the board tells them apart by it.
pub const UNKNOWN: &str = "Kerdr can't tell whether it happened";

/// How every error ends that Kerdr gives for a request it never sent: known not to have happened.
pub const NOT_SENT: &str = "so nothing was sent";

/// What a request says when Kerdr has no action backend to send it to.
pub const NO_BACKEND: &str =
    "Kerdr has no action backend set ([actions] command in its config), so nothing was sent";

/// Whether an error leaves the request's result unknown: its answer was lost, or it says nothing
/// that shows it wasn't sent. Only an error that knows the request never left Kerdr (NOT_SENT)
/// says it didn't happen.
pub fn result_unknown(error: &str) -> bool {
    !error.contains(NOT_SENT)
}

/// Why a request is being sent again, as the board is told meanwhile.
#[derive(Clone, Debug, PartialEq)]
pub enum Resend {
    /// Refused for something passing: sent again after `wait`, the backend's words meanwhile.
    Retry { wait: Duration, message: String },
    /// Its answer was lost: sent again after `wait`, the `attempt`th time.
    Lost {
        wait: Duration,
        error: String,
        attempt: usize,
    },
}

/// What an answer with `follow` says Kerdr should wait for (intent 0591, docs/feed.md).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Follow {
    /// Opaque, sent back in each `progress` request.
    pub token: String,
    /// `start`, `recover`, `move-in`, `dispatch` or `reopen`.
    #[serde(default)]
    pub what: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    /// What it's doing, in words for the card: "Starting ARK on arknet-one, then #581…".
    #[serde(default)]
    pub say: String,
    /// Seconds the step may take; past that and a margin, Kerdr stops following and says so.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// A token Kerdr sends back: short and plain, so nothing odd goes into a request.
fn follow_token_ok(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `follow` as the answer gives it: an object with a usable token, else none at all, so a backend
/// that sends something Kerdr can't follow is read as one that doesn't follow.
fn follow<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Follow>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(serde_json::from_value::<Follow>(value)
        .ok()
        .filter(|f| follow_token_ok(&f.token)))
}

/// The request asking how followed work stands: read-only, so Kerdr may send it as often as it
/// likes (intent 0591).
pub fn progress_request(token: &str) -> Value {
    serde_json::json!({"action": "progress", "follow": token})
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

/// A `progress` answer: `running` carries `say` and may leave out `message`, which every other
/// answer must have (intent 0591).
fn parse_progress(text: &str) -> Result<Response, serde_json::Error> {
    let mut value: Value = serde_json::from_str(text)?;
    if value.is_object() && value.get("message").is_none() {
        value["message"] = Value::String(String::new());
    }
    if value.is_object() && value.get("ok").is_none() {
        value["ok"] = Value::Bool(value["state"] == "running" || value["state"] == "done");
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

/// The states a `progress` answer gives (docs/feed.md).
const PROGRESS_STATES: &[&str] = &["running", "done", "asks", "failed", "unknown"];

/// The states other answers give (an account move's, an account removal's), kept as they are by the
/// credential scrub, as KNOWN_STATUSES are.
const ANSWER_STATES: &[&str] = &[
    "stale",
    "queued",
    "waiting",
    "moved",
    "already",
    "unchanged",
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
    run_with_deadline(machine, command, request, deadline_for(request), None).result
}

/// As run_handing, sending the same request again, with the same op, by itself (intent 0591), only
/// with `resends` (its machine's feed says its backend keeps ops, so nothing is done twice there):
/// after a refusal with `retry`, once that many seconds have passed, and after an answer that was
/// lost, up to three times over about 30 seconds; all of it within RETRY_MOST of the first answer.
/// A change without an op is never sent again; a read (an item's text, a kickoff check), which
/// changes nothing, is as if it had one. `tell` hears each resend before its wait, and `pause` does
/// the waiting (a test's own, which needn't).
pub fn run_resending(
    machine: &Machine,
    command: &str,
    request: &Value,
    handed: Option<transport::Handed>,
    resends: bool,
    tell: &mut dyn FnMut(&Resend),
    pause: &dyn Fn(Duration),
) -> Answered {
    run_resending_within(
        machine,
        command,
        request,
        handed,
        resends,
        tell,
        pause,
        &Clocks::now,
        deadline_for(request),
    )
}

/// As run_resending, the first send given `deadline` to answer, and each resend no more than that;
/// `clock` tells the time (a test's own, which `pause` moves on).
#[allow(clippy::too_many_arguments)]
fn run_resending_within(
    machine: &Machine,
    command: &str,
    request: &Value,
    handed: Option<transport::Handed>,
    resends: bool,
    tell: &mut dyn FnMut(&Resend),
    pause: &dyn Fn(Duration),
    clock: &dyn Fn() -> Clocks,
    deadline: Duration,
) -> Answered {
    resending(
        request,
        handed,
        resends,
        tell,
        pause,
        clock,
        deadline,
        &mut |handed, deadline| {
            let ran = run_with_deadline(machine, command, request, deadline, handed);
            let lost = ran.lost;
            Sent {
                answered: keep_stderr(ran, request),
                lost,
            }
        },
    )
}

/// One send, as `resending` sees it.
struct Sent {
    answered: Answered,
    /// Its answer was lost on the way: the request may or may not have happened.
    lost: bool,
}

/// The resends themselves: `send` sends the request once, given the deadline it names. Only with
/// `resends` is anything sent twice (intent 0591): a backend that doesn't keep ops could do a
/// change twice. Every resend fits in RETRY_MOST from the first answer, as `clock` tells it: its
/// wait whole and at least RESEND_ROOM for it to answer in; one that doesn't isn't sent, and the
/// last answer stands. What's left is told again after each wait, which can take longer than asked
/// (a machine asleep or busy), and a resend is given no more than that.
#[allow(clippy::too_many_arguments)]
fn resending(
    request: &Value,
    handed: Option<transport::Handed>,
    resends: bool,
    tell: &mut dyn FnMut(&Resend),
    pause: &dyn Fn(Duration),
    clock: &dyn Fn() -> Clocks,
    deadline: Duration,
    send: &mut dyn FnMut(Option<transport::Handed>, Duration) -> Sent,
) -> Answered {
    let mut handed = handed;
    // Safe to send again: the backend keeps ops and answers a second send of one from its record,
    // and a read changes nothing however often it's sent.
    let may = resends && (request.get("op").is_some() || !changes(request));
    let mut this_deadline = deadline;
    // When retrying started: at the first answer, fixed then. It ends RETRY_MOST later.
    let mut start: Option<Clocks> = None;
    let mut lost = 0;
    loop {
        let sent = send(handed.take(), this_deadline);
        if !may {
            return sent.answered;
        }
        let start = *start.get_or_insert_with(clock);
        let why = match &sent.answered.result {
            Ok(r) if !r.ok && !r.confirmation && r.retry.is_some() => Resend::Retry {
                // As long as it asked, never less; at least a second.
                wait: Duration::from_secs(r.retry.unwrap_or(1).max(1)),
                message: r.message.clone(),
            },
            Err(error) if sent.lost && lost < LOST_WAITS.len() => Resend::Lost {
                wait: LOST_WAITS[lost],
                error: error.clone(),
                attempt: lost + 1,
            },
            _ => return sent.answered,
        };
        let wait = match &why {
            Resend::Retry { wait, .. } | Resend::Lost { wait, .. } => *wait,
        };
        // The wait is set against what's left before anything is added to it, so no wait asked
        // for, however long, can overflow.
        let left = Clocks::left_of_retrying(start, clock());
        if wait > left || left - wait < RESEND_ROOM {
            return sent.answered;
        }
        if matches!(why, Resend::Lost { .. }) {
            lost += 1;
        }
        tell(&why);
        pause(wait);
        let left = Clocks::left_of_retrying(start, clock());
        if left < RESEND_ROOM {
            return sent.answered;
        }
        this_deadline = deadline.min(left);
    }
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
    } else if request["action"] == "progress" {
        // A read of a record: a slow one is asked again in a few seconds anyway (intent 0591).
        Duration::from_secs(20)
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
    keep_stderr(
        run_with_deadline(machine, command, request, deadline, handed),
        request,
    )
}

/// How much of what a backend printed in place of a proper answer goes to the error log: its
/// beginning, which says what it is.
const MOST_PRINTED: usize = 4 * 1024;

/// An answer, with the end of what its command printed on standard error, and the beginning of a
/// malformed answer it printed, made fit for the error log: nothing typed into the request in it,
/// nor a credential. The board never sees this.
fn keep_stderr(ran: Ran, request: &Value) -> Answered {
    let printed = ran
        .printed
        .as_deref()
        .map(|p| redact_credentials(p.trim(), request))
        .map(|p| beginning(&p, MOST_PRINTED).to_string())
        .filter(|p| !p.is_empty());
    // Its end says what finally went wrong; room is left for what it printed instead of an answer.
    let room = crate::errlog::MOST_STDERR - printed.as_ref().map_or(0, |_| MOST_PRINTED + 64);
    let mut stderr = crate::errlog::last_part(ran.stderr.trim_end(), room).to_string();
    if let Some(printed) = printed {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str("[its answer, which Kerdr couldn't read] ");
        stderr.push_str(&printed);
    }
    let typed = typed_in(request);
    let stderr = crate::errlog::scrub(&redact_credentials(&stderr, request), &typed, &typed);
    Answered {
        result: ran.result,
        stderr,
    }
}

/// The first `most` bytes of `text`, never half a word: no token is left half there, where the
/// error log's scrub might not know it.
fn beginning(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text;
    }
    let mut end = most;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = &text[..end];
    if text[end..].starts_with(|c: char| c.is_alphanumeric()) {
        cut.trim_end_matches(|c: char| c.is_alphanumeric())
    } else {
        cut
    }
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

/// One send of a request, as its command answered it.
struct Ran {
    result: Result<Response, String>,
    /// What the command printed on standard error, unscrubbed.
    stderr: String,
    /// What it printed on standard output, unscrubbed, when that wasn't a proper answer: its error
    /// says that's in Kerdr's error log (intent 0591).
    printed: Option<String>,
    /// Whether its answer was lost (none in time, or the connection dropped): the request may or
    /// may not have happened.
    lost: bool,
}

impl Ran {
    /// A request never sent: nothing printed, nothing lost.
    fn unsent(error: String) -> Ran {
        Ran {
            result: Err(error),
            stderr: String::new(),
            printed: None,
            lost: false,
        }
    }
}

fn run_with_deadline(
    machine: &Machine,
    command: &str,
    request: &Value,
    deadline: Duration,
    handed: Option<transport::Handed>,
) -> Ran {
    if command.trim().is_empty() {
        return Ran::unsent(NO_BACKEND.into());
    }
    let json = match serde_json::to_string(request) {
        Ok(json) => json,
        Err(e) => {
            return Ran::unsent(format!(
                "Kerdr couldn't build that request (a fault in Kerdr: {e}), {NOT_SENT}"
            ));
        }
    };
    if json.len() > most_bytes(request) {
        let said = if most_bytes(request) > MAX_JSON_BYTES {
            format!(
                "the request is too large to send (64 KiB at most, and {} of images), so nothing was sent",
                crate::images::size(crate::images::MAX_IMAGES_BYTES)
            )
        } else {
            "the request is too large to send (64 KiB at most), so nothing was sent".into()
        };
        return Ran::unsent(said);
    }
    let target = match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    };
    // The configured command is source; user data is JSON on stdin, never source.
    let output = transport::run_adapter_handing(&target, command, Some(&json), deadline, handed);
    let result = answer_of(
        &output,
        request,
        deadline,
        &machine.name,
        machine.ssh.is_some(),
    );
    let lost = result.is_err() && lost(&output, machine.ssh.is_some());
    // An answer it couldn't read, or one far too long: what it printed goes to the error log, as
    // its error says (intent 0591). A lost answer's error promises nothing of the kind.
    let printed = (result.is_err() && !lost && !output.stdout.trim().is_empty())
        .then(|| output.stdout.clone());
    Ran {
        result,
        stderr: output.stderr,
        printed,
        lost,
    }
}

/// Whether an answer was lost on its way: none came in time, or the connection to its machine
/// dropped (ssh's own exit, 255). Either way the request may or may not have happened. A backend
/// that stopped without a proper answer, or sent one far too long, isn't lost: sent again, it
/// would most likely do the same.
fn lost(output: &transport::Output, ssh: bool) -> bool {
    output.timed_out || (ssh && output.status == Some(255))
}

/// What a command's output means.
fn answer_of(
    output: &transport::Output,
    request: &Value,
    deadline: Duration,
    machine: &str,
    over_ssh: bool,
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
                "{verb} {} is taking more than {how_long}; it may still open, so {UNKNOWN} yet, and its card shows how it goes.",
                reference(request)
            ));
        }
        let minutes = deadline.as_secs().div_ceil(60);
        let within = match minutes {
            0 | 1 => "a minute".to_string(),
            2 => "two minutes".into(),
            n => format!("{n} minutes"),
        };
        return Err(format!(
            "no answer came from {machine} within {within}; {UNKNOWN}, and its card shows how it stands."
        ));
    }
    if output.stdout.len() > most_answer_bytes(request) {
        return Err(format!(
            "{machine}'s action backend sent an answer far too long to read (a fault there; it's in the error log); {UNKNOWN}, and its card shows how it stands."
        ));
    }
    // A backend can request confirmation with a nonzero exit code. Its JSON is the contract,
    // so parse it before interpreting process status.
    let parsed = if request["action"] == "progress" {
        parse_progress(&output.stdout)
    } else {
        parse_response(&output.stdout)
    };
    let ssh = over_ssh && output.status == Some(255);
    parsed.map(|mut response| {
        response.message = redact(&response.message);
        for text in [&mut response.yes, &mut response.no, &mut response.url, &mut response.ticket, &mut response.confirm, &mut response.session, &mut response.id, &mut response.workspace, &mut response.label, &mut response.tab, &mut response.home, &mut response.machine, &mut response.user_code, &mut response.expires, &mut response.text, &mut response.was, &mut response.asked, &mut response.note, &mut response.say].into_iter().flatten() {
            *text = redact(text);
        }
        if let Some(follow) = response.follow.as_mut() {
            follow.say = redact(&follow.say);
            for text in [&mut follow.id, &mut follow.session].into_iter().flatten() {
                *text = redact(text);
            }
        }
        // A progress answer's state: only those the protocol knows, anything else reading as one
        // Kerdr can't follow. Any other answer's (an account move's `stale`, `already`) is its own,
        // and stays as it is when it's one the protocol knows (intent 0591).
        if request["action"] == "progress" {
            if response
                .state
                .as_deref()
                .is_some_and(|s| !PROGRESS_STATES.contains(&s))
            {
                response.state = Some("unknown".into());
            }
        } else if let Some(state) = response.state.as_mut().filter(|s| !ANSWER_STATES.contains(&s.as_str())) {
            *state = redact(state);
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
    }).map_err(|_| {
        // ssh's own exit when the connection failed or dropped: nothing of the backend's.
        if ssh {
            return format!(
                "the connection to {machine} dropped before its answer came; {UNKNOWN}, and its card shows how it stands."
            );
        }
        // What it printed may be a traceback, or quote what it was sent: it goes to the error
        // log, scrubbed, never to the board (intent 0591).
        let how = match output.status {
            Some(code) => format!("exit {code}"),
            None => "stopped by a signal".into(),
        };
        format!(
            "{machine}'s action backend stopped without a proper answer ({how}, and what it printed is in Kerdr's error log); {UNKNOWN}, and its card shows how it stands."
        )
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
                text = crate::errlog::credential_out(&text, secret);
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

    fn answered(request: &Value, stdout: &str) -> Result<Response, String> {
        let output = transport::Output {
            stdout: stdout.into(),
            stderr: String::new(),
            status: Some(0),
            timed_out: false,
            took: Duration::from_millis(5),
        };
        answer_of(&output, request, deadline_for(request), "arknet-one", true)
    }

    #[test]
    fn an_answer_with_follow_names_what_to_ask_about_and_how_long_it_may_take() {
        let r = answered(
            &json!({"action":"reopen","id":"0581"}),
            r#"{"ok":true,"message":"Starting ARK; ⏎ again in a minute goes in.","id":"0581","session":"ARK",
                "follow":{"token":"f-0123456789abcdef0123","what":"start","id":"0581","session":"ARK",
                          "say":"Starting ARK on arknet-one, then #581…","limit":300}}"#,
        )
        .unwrap();
        let f = r.follow.unwrap();
        assert_eq!(f.token, "f-0123456789abcdef0123");
        assert_eq!(f.what, "start");
        assert_eq!(f.say, "Starting ARK on arknet-one, then #581…");
        assert_eq!(f.limit, Some(300));
    }

    #[test]
    fn a_follow_kerdr_cant_send_back_reads_as_none_and_the_answer_stands() {
        for follow in [
            json!({"token":"f-1 ; rm -rf ~","say":"x"}),
            json!({"token":"","say":"x"}),
            json!("f-0123"),
            json!({"say":"no token"}),
        ] {
            let reply = json!({"ok":true,"message":"Started.","follow":follow}).to_string();
            let r = answered(&json!({"action":"dispatch","id":"0581"}), &reply).unwrap();
            assert!(r.follow.is_none(), "{follow}");
            assert!(r.ok);
        }
    }

    #[test]
    fn a_progress_answer_may_leave_out_message_and_only_known_states_are_read() {
        let request = progress_request("f-0123456789abcdef0123");
        assert_eq!(deadline_for(&request), Duration::from_secs(20));
        let r = answered(
            &request,
            r#"{"ok":true,"state":"running","say":"Waiting for ARK's restore…"}"#,
        )
        .unwrap();
        assert_eq!(r.state.as_deref(), Some("running"));
        assert_eq!(r.say.as_deref(), Some("Waiting for ARK's restore…"));
        let r = answered(
            &request,
            r#"{"state":"done","message":"Reopened.","workspace":"w4","session":"ARK"}"#,
        )
        .unwrap();
        assert!(r.ok, "a done answer without ok is a success");
        assert_eq!(r.workspace.as_deref(), Some("w4"));
        let r = answered(&request, r#"{"ok":false,"state":"exploded","message":"?"}"#).unwrap();
        assert_eq!(r.state.as_deref(), Some("unknown"));
        // Any other answer still needs its message.
        assert!(answered(&json!({"action":"reopen","id":"1"}), r#"{"ok":true}"#).is_err());
    }

    fn deadline_answer(
        machine: &Machine,
        command: &str,
        request: &Value,
        deadline: Duration,
        handed: Option<transport::Handed>,
    ) -> Result<Response, String> {
        run_with_deadline(machine, command, request, deadline, handed).result
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

    /// A fake backend that keeps a record of each op as release 2's backends do (intent 0591):
    /// a request it has an answer for gets it again, with `repeat`, and nothing is done twice.
    /// Every request goes in requests.log and every change in done.log; the first answer of all
    /// is lost on its way (it sleeps past Kerdr's deadline) once the change is done.
    #[cfg(unix)]
    fn op_keeping_backend(dir: &std::path::Path) -> String {
        let d = quote(dir.to_str().unwrap());
        fixture(
            dir,
            &format!(
                r#"req=$(cat)
dir={d}
printf '%s\n' "$req" >> "$dir/requests.log"
op=$(printf '%s' "$req" | sed -n 's/.*"op":"\(k-[0-9a-f]*\)".*/\1/p')
if [ -n "$op" ] && [ -f "$dir/$op.answer" ]; then
  sed 's/}}$/,"repeat":true}}/' "$dir/$op.answer"
  exit 0
fi
echo done >> "$dir/done.log"
answer='{{"ok":true,"message":"Marked #41 done.","id":"0041"}}'
[ -n "$op" ] && printf '%s' "$answer" > "$dir/$op.answer"
if [ ! -f "$dir/lost-once" ]; then : > "$dir/lost-once"; sleep 4; fi
printf '%s' "$answer"
"#
            ),
            "",
        )
    }

    #[cfg(unix)]
    fn lines(dir: &std::path::Path, name: &str) -> Vec<String> {
        fs::read_to_string(dir.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Resends as run_resending makes them, each send given `deadline`: what it told, and the
    /// waits it asked for, which pass at once on its clock (real time, moved on by each wait).
    #[cfg(unix)]
    #[allow(clippy::type_complexity)]
    fn resent(
        command: &str,
        request: &Value,
        resends: bool,
        deadline: Duration,
    ) -> (Answered, Vec<Resend>, Vec<Duration>) {
        let mut told = vec![];
        let waits = std::cell::RefCell::new(vec![]);
        let waited = std::cell::Cell::new(Duration::ZERO);
        let answered = run_resending_within(
            &machine(),
            command,
            request,
            None,
            resends,
            &mut |r| told.push(r.clone()),
            &|d| {
                waits.borrow_mut().push(d);
                waited.set(waited.get() + d);
            },
            &|| later(Clocks::now(), waited.get()),
            deadline,
        );
        (answered, told, waits.into_inner())
    }

    #[test]
    fn a_request_that_can_change_something_gets_a_fresh_op_and_a_read_gets_none() {
        for request in [
            json!({"action":"dispatch","id":"0041"}),
            json!({"action":"close","id":"0041"}),
            json!({"action":"edit","id":"0041","text":"New words.","was":"sha256:aa"}),
            json!({"action":"account-move","account":"ARK_C"}),
            json!({"action":"create","words":"Back up the NAS"}),
        ] {
            let a = with_op(request.clone());
            let b = with_op(a.clone());
            let op = a["op"].as_str().unwrap();
            assert!(
                op.len() == 18
                    && op.starts_with("k-")
                    && op[2..]
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{op}"
            );
            // A new key press is a new request: never the op of the one before (a question's).
            assert_ne!(a["op"], b["op"]);
            assert_eq!(without_op(&b), request);
        }
        for read in [
            json!({"action":"progress","follow":"f-0123456789abcdef0123"}),
            json!({"action":"kickoff","id":"0041"}),
            json!({"action":"edit","id":"0041"}),
        ] {
            assert_eq!(with_op(read.clone()), read, "a read carries no op");
        }
    }

    #[test]
    fn retry_is_read_as_whole_seconds_and_anything_else_is_none() {
        let read = |retry: &str| {
            parse_response(&format!(
                r#"{{"ok":false,"message":"locked","retry":{retry}}}"#
            ))
            .unwrap()
            .retry
        };
        assert_eq!(read("5"), Some(5));
        assert_eq!(read("2.5"), Some(3));
        assert_eq!(read("\"5\""), None);
        assert_eq!(read("-1"), None);
        assert_eq!(read("null"), None);
        let r = parse_response(
            r#"{"ok":false,"partial":true,"repeat":true,"message":"Saved here, but not pushed."}"#,
        )
        .unwrap();
        assert!(r.partial && r.repeat && r.retry.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_reply_lost_after_the_change_went_through_is_asked_again_and_never_done_twice() {
        let dir = tempfile::tempdir().unwrap();
        let command = op_keeping_backend(dir.path());
        let request = with_op(json!({"action":"close","id":"0041"}));
        let (answered, told, waits) = resent(&command, &request, true, Duration::from_millis(1500));
        // The resend got the recorded answer: the change stands, done once.
        let r = answered.result.unwrap();
        assert!(r.ok && r.repeat, "{r:?}");
        assert_eq!(r.message, "Marked #41 done.");
        assert_eq!(lines(dir.path(), "done.log").len(), 1, "done twice");
        // Sent twice, the same request with the same op.
        let sent = lines(dir.path(), "requests.log");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
        assert!(sent[0].contains(request["op"].as_str().unwrap()));
        assert_eq!(waits, [Duration::from_secs(5)]);
        assert!(
            matches!(&told[..], [Resend::Lost { attempt: 1, .. }]),
            "{told:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_old_backend_that_keeps_no_ops_is_never_sent_a_request_again() {
        let dir = tempfile::tempdir().unwrap();
        let command = op_keeping_backend(dir.path());
        // Its feed doesn't list "op": a lost answer's result is said to be unknown, as before.
        let request = with_op(json!({"action":"close","id":"0041"}));
        let (answered, told, waits) =
            resent(&command, &request, false, Duration::from_millis(1500));
        let error = answered.result.unwrap_err();
        assert!(result_unknown(&error), "{error}");
        assert!(error.contains("no answer came from here"), "{error}");
        assert_eq!(lines(dir.path(), "requests.log").len(), 1);
        assert!(told.is_empty() && waits.is_empty());
        // Nor is a request without an op, whatever its machine says.
        let dir = tempfile::tempdir().unwrap();
        let command = op_keeping_backend(dir.path());
        let (answered, told, _) = resent(
            &command,
            &json!({"action":"close","id":"0041"}),
            true,
            Duration::from_millis(1500),
        );
        assert!(answered.result.is_err());
        assert_eq!(lines(dir.path(), "requests.log").len(), 1);
        assert!(told.is_empty());
    }

    /// A send whose answer is what `outputs` holds next, read as from arknet-one over SSH.
    fn over_ssh(
        request: &Value,
        resends: bool,
        outputs: Vec<transport::Output>,
    ) -> (Answered, Vec<Resend>, usize) {
        let (answered, told, sends, _, _) = over_ssh_timed(request, resends, outputs);
        (answered, told, sends.len())
    }

    /// As over_ssh, with the deadline each send was given, the waits asked for, and the time from
    /// the first answer to the last.
    #[allow(clippy::type_complexity)]
    fn over_ssh_timed(
        request: &Value,
        resends: bool,
        outputs: Vec<transport::Output>,
    ) -> (
        Answered,
        Vec<Resend>,
        Vec<Duration>,
        Vec<Duration>,
        Duration,
    ) {
        over_ssh_late(request, resends, outputs, &[])
    }

    /// As over_ssh_timed, on a clock of its own: each send takes its output's `took` (one that
    /// timed out, its whole deadline, as a real one does), and each wait as long as it asked, and
    /// then as much longer as `late` holds for it (a machine asleep, or busy).
    #[allow(clippy::type_complexity)]
    fn over_ssh_late(
        request: &Value,
        resends: bool,
        outputs: Vec<transport::Output>,
        late: &[Duration],
    ) -> (
        Answered,
        Vec<Resend>,
        Vec<Duration>,
        Vec<Duration>,
        Duration,
    ) {
        let mut outputs = outputs.into_iter();
        let mut deadlines = vec![];
        let mut told = vec![];
        let waits = std::cell::RefCell::new(vec![]);
        let now = std::cell::Cell::new(Clocks::now());
        let first = std::cell::Cell::new(None::<Clocks>);
        let answered = resending(
            request,
            None,
            resends,
            &mut |r| told.push(r.clone()),
            &|d| {
                let mut waits = waits.borrow_mut();
                let over = late.get(waits.len()).copied().unwrap_or_default();
                waits.push(d);
                now.set(later(now.get(), d + over));
            },
            &|| now.get(),
            deadline_for(request),
            &mut |_, deadline| {
                deadlines.push(deadline);
                let mut out = outputs.next().expect("sent more often than expected");
                if out.timed_out {
                    out.took = deadline;
                }
                now.set(later(now.get(), out.took));
                if first.get().is_none() {
                    first.set(Some(now.get()));
                }
                let result = answer_of(&out, request, deadline, "arknet-one", true);
                let lost = result.is_err() && lost(&out, true);
                Sent {
                    answered: Answered {
                        result,
                        stderr: out.stderr.clone(),
                    },
                    lost,
                }
            },
        );
        let spent = now.get().steady - first.get().expect("sent at least once").steady;
        (answered, told, deadlines, waits.into_inner(), spent)
    }

    /// Both clocks, `d` later: time that passes while the computer is awake.
    fn later(c: Clocks, d: Duration) -> Clocks {
        Clocks {
            steady: c.steady + d,
            wall: c.wall + d,
        }
    }

    fn timed_out() -> transport::Output {
        transport::Output {
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            took: Duration::ZERO,
        }
    }

    #[test]
    fn no_request_is_ever_sent_again_to_a_backend_that_keeps_no_ops_even_after_retry() {
        // Kerdr gives every change an op, an old backend's included; only its feed saying "op"
        // makes a resend safe (intent 0591). A released backend could do the change twice.
        let change = with_op(json!({"action":"close","id":"0041"}));
        let refused = || {
            output(
                0,
                r##"{"ok":false,"retry":3,"message":"#41 is locked by another request."}"##,
                "",
            )
        };
        let (answered, told, sends) = over_ssh(&change, false, vec![refused()]);
        assert_eq!(sends, 1);
        assert!(told.is_empty());
        let r = answered.result.unwrap();
        assert!(!r.ok);
        assert_eq!(r.message, "#41 is locked by another request.");
        // Nor a read: no resend of any kind goes to it.
        let read = json!({"action":"kickoff","id":"0041"});
        let (_, told, sends) = over_ssh(&read, false, vec![refused()]);
        assert_eq!((sends, told.len()), (1, 0));
        // The same refusal from a backend that keeps ops is sent again, with the same op.
        let (answered, told, sends) = over_ssh(
            &change,
            true,
            vec![
                refused(),
                output(0, r#"{"ok":true,"message":"Marked #41 done."}"#, ""),
            ],
        );
        assert!(answered.result.unwrap().ok);
        assert_eq!((sends, told.len()), (2, 1));
    }

    #[test]
    fn one_two_minute_deadline_covers_all_the_retrying_and_a_wait_asked_for_is_never_cut_short() {
        let reopen = with_op(json!({"action":"reopen","id":"0041"}));
        let retry = |n: u64| {
            output(
                0,
                &format!(r#"{{"ok":false,"retry":{n},"message":"herdr isn't answering yet."}}"#),
                "",
            )
        };
        // A quick retry: 5, then a resend that hangs. It's given only what's left of the two
        // minutes, not a reopen's five, and once that's spent nothing more is sent, lost or not.
        let (answered, told, deadlines, waits, spent) =
            over_ssh_timed(&reopen, true, vec![retry(5), timed_out()]);
        assert_eq!(
            deadlines,
            [Duration::from_secs(300), Duration::from_secs(115)]
        );
        assert_eq!(waits, [Duration::from_secs(5)]);
        assert_eq!(told.len(), 1);
        assert!(spent <= RETRY_MOST, "{spent:?}");
        let error = answered.result.unwrap_err();
        assert!(result_unknown(&error), "{error}");
        // A lost answer's resends come out of the same two minutes.
        let close = with_op(json!({"action":"close","id":"0041"}));
        let (_, told, deadlines, waits, spent) = over_ssh_timed(
            &close,
            true,
            vec![timed_out(), timed_out(), timed_out(), timed_out()],
        );
        assert!(spent <= RETRY_MOST, "{spent:?}");
        assert_eq!(waits, [Duration::from_secs(5)], "{told:?}");
        assert_eq!(
            deadlines,
            [Duration::from_secs(120), Duration::from_secs(115)]
        );
        // A long wait asked for is waited whole: 90 s is 90 s, and the resend gets what's left.
        let (answered, _, deadlines, waits, _) = over_ssh_timed(
            &close,
            true,
            vec![
                retry(90),
                output(0, r#"{"ok":true,"message":"Marked #41 done."}"#, ""),
            ],
        );
        assert!(answered.result.unwrap().ok);
        assert_eq!(waits, [Duration::from_secs(90)]);
        assert_eq!(deadlines[1], Duration::from_secs(30));
        // One that leaves no room for the resend to answer isn't cut short: the refusal stands.
        let (answered, told, _, waits, _) = over_ssh_timed(&close, true, vec![retry(115)]);
        assert!(told.is_empty() && waits.is_empty());
        assert_eq!(answered.result.unwrap().retry, Some(115));
    }

    #[test]
    fn the_two_minutes_are_real_time_so_a_wait_that_runs_long_comes_out_of_them() {
        let reopen = with_op(json!({"action":"reopen","id":"0041"}));
        let retry = |n: u64| {
            output(
                0,
                &format!(r#"{{"ok":false,"retry":{n},"message":"herdr isn't answering yet."}}"#),
                "",
            )
        };
        // 5 s asked for, and 100 s gone by the time it's over (a machine asleep, or busy): the
        // resend is given the 20 s left of the two minutes, not 115.
        let (answered, _, deadlines, waits, spent) = over_ssh_late(
            &reopen,
            true,
            vec![retry(5), timed_out()],
            &[Duration::from_secs(95)],
        );
        assert_eq!(waits, [Duration::from_secs(5)]);
        assert_eq!(
            deadlines,
            [Duration::from_secs(300), Duration::from_secs(20)]
        );
        assert!(spent <= RETRY_MOST, "{spent:?}");
        assert!(result_unknown(&answered.result.unwrap_err()));
        // Waking with too little left for a resend to answer in, nothing more is sent: the last
        // refusal stands.
        let (answered, _, deadlines, _, _) =
            over_ssh_late(&reopen, true, vec![retry(5)], &[Duration::from_secs(107)]);
        assert_eq!(deadlines.len(), 1);
        assert_eq!(answered.result.unwrap().retry, Some(5));
    }

    #[test]
    fn a_wait_asked_for_however_long_is_set_against_what_is_left_and_never_added_up() {
        // However many seconds an answer asks for (one built by hand, past what's read from a
        // backend), they're set against what's left of the two minutes before anything is added:
        // u64::MAX of them is only too long, and that refusal stands.
        let close = with_op(json!({"action":"close","id":"0041"}));
        let refusal = |retry: u64| Answered {
            result: Ok(Response {
                ok: false,
                retry: Some(retry),
                message: "#41 is locked by another request.".into(),
                ..Default::default()
            }),
            stderr: String::new(),
        };
        let mut answers = vec![refusal(5), refusal(u64::MAX)].into_iter();
        let now = std::cell::Cell::new(Clocks::now());
        let waits = std::cell::RefCell::new(vec![]);
        let mut sends = 0;
        let answered = resending(
            &close,
            None,
            true,
            &mut |_| {},
            &|d| {
                waits.borrow_mut().push(d);
                now.set(later(now.get(), d));
            },
            &|| now.get(),
            deadline_for(&close),
            &mut |_, _| {
                sends += 1;
                now.set(later(now.get(), Duration::from_secs(1)));
                Sent {
                    answered: answers.next().expect("sent more often than expected"),
                    lost: false,
                }
            },
        );
        assert_eq!(sends, 2);
        assert_eq!(waits.into_inner(), [Duration::from_secs(5)]);
        assert_eq!(answered.result.unwrap().retry, Some(u64::MAX));
    }

    #[test]
    fn a_computer_asleep_through_a_wait_sends_nothing_more_when_it_wakes() {
        // Linux's steady clock stops while the computer sleeps; the wall clock doesn't. A laptop
        // shut for ten minutes during a 5 s wait wakes past the two minutes, and that refusal
        // stands, unsent again (Codex's third look, intent 0591).
        let close = with_op(json!({"action":"close","id":"0041"}));
        let refused = || Answered {
            result: Ok(Response {
                ok: false,
                retry: Some(5),
                message: "#41 is locked by another request.".into(),
                ..Default::default()
            }),
            stderr: String::new(),
        };
        let run = |asleep: Duration, wall_set: Duration| {
            let now = std::cell::Cell::new(Clocks::now());
            let mut sends = 0;
            let answered = resending(
                &close,
                None,
                true,
                &mut |_| {},
                &|d| {
                    // Awake for the wait asked for, then asleep: the steady clock stops.
                    let c = later(now.get(), d);
                    now.set(Clocks {
                        steady: c.steady,
                        wall: c.wall + asleep - wall_set,
                    });
                },
                &|| now.get(),
                deadline_for(&close),
                &mut |_, _| {
                    sends += 1;
                    let answered = if sends == 1 {
                        refused()
                    } else {
                        Answered {
                            result: Ok(Response {
                                ok: true,
                                message: "Marked #41 done.".into(),
                                ..Default::default()
                            }),
                            stderr: String::new(),
                        }
                    };
                    Sent {
                        answered,
                        lost: false,
                    }
                },
            );
            (sends, answered.result.unwrap())
        };
        let (sends, r) = run(Duration::from_secs(600), Duration::ZERO);
        assert_eq!(sends, 1, "sent again ten minutes later");
        assert!(!r.ok && r.retry == Some(5));
        // Awake all along, it's sent again, as ever.
        let (sends, r) = run(Duration::ZERO, Duration::ZERO);
        assert_eq!(sends, 2);
        assert!(r.ok);
        // A wall clock set back an hour meanwhile counts nothing as gone: the steady clock's
        // count stands, and it's sent again.
        let (sends, r) = run(Duration::ZERO, Duration::from_secs(3600));
        assert_eq!(sends, 2);
        assert!(r.ok);
    }

    #[test]
    fn an_account_answer_keeps_its_own_state_and_only_a_progress_answer_s_is_narrowed() {
        let stale = answered(
            &json!({"action":"account-move","pane":"w1:p1","to":"ARK_C"}),
            r#"{"ok":true,"state":"stale","message":"that pane runs another agent now; nothing moved"}"#,
        )
        .unwrap();
        assert_eq!(stale.state.as_deref(), Some("stale"));
        for state in ["queued", "waiting", "moved", "already"] {
            let r = answered(
                &json!({"action":"account-remove","code":"ARK_C"}),
                &json!({"ok":true,"state":state,"message":"m"}).to_string(),
            )
            .unwrap();
            assert_eq!(r.state.as_deref(), Some(state));
        }
        // A state that echoes a credential is scrubbed like any other field.
        let r = answered(
            &json!({"action":"account-signin","step":"paste","token":"fixture-private-value"}),
            r#"{"ok":false,"state":"fixture-private-value","message":"no"}"#,
        )
        .unwrap();
        assert_eq!(r.state.as_deref(), Some("[redacted]"));
        // A progress answer's is still only one Kerdr can follow.
        let r = answered(
            &progress_request("f-0123456789abcdef0123"),
            r#"{"ok":true,"state":"stale","message":"m"}"#,
        )
        .unwrap();
        assert_eq!(r.state.as_deref(), Some("unknown"));
    }

    fn output(status: i32, stdout: &str, stderr: &str) -> transport::Output {
        transport::Output {
            status: Some(status),
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
            took: Duration::from_millis(5),
        }
    }

    #[test]
    fn an_ssh_drop_in_the_middle_of_an_action_is_sent_again_once_the_connection_is_back() {
        let request = with_op(json!({"action":"move","id":"0041","stage":"build"}));
        let dropped = || {
            output(
                255,
                r#"{"ok":true,"mess"#,
                "client_loop: send disconnect: Broken pipe\n",
            )
        };
        // Back after the second try: the backend's record answers, with `repeat`.
        let (answered, told, sends) = over_ssh(
            &request,
            true,
            vec![
                dropped(),
                output(
                    255,
                    "",
                    "ssh: connect to host arknet-one port 22: Connection refused\n",
                ),
                output(
                    0,
                    r#"{"ok":true,"message":"Moved #41 to build.","repeat":true}"#,
                    "",
                ),
            ],
        );
        let r = answered.result.unwrap();
        assert!(r.ok && r.repeat);
        assert_eq!(sends, 3);
        assert!(
            matches!(
                &told[..],
                [
                    Resend::Lost { attempt: 1, .. },
                    Resend::Lost { attempt: 2, .. }
                ]
            ),
            "{told:?}"
        );
        // Three resends at most, about 30 s in all; then its result is said to be unknown.
        let (answered, told, sends) = over_ssh(&request, true, (0..4).map(|_| dropped()).collect());
        assert_eq!(sends, 4);
        assert_eq!(LOST_WAITS.iter().sum::<Duration>(), Duration::from_secs(30));
        assert_eq!(told.len(), 3);
        assert_eq!(
            answered.result.unwrap_err(),
            "the connection to arknet-one dropped before its answer came; Kerdr can't tell whether it happened, and its card shows how it stands."
        );
        // To a backend that keeps no ops: said at once, never resent.
        let (answered, told, sends) = over_ssh(&request, false, vec![dropped()]);
        assert_eq!(sends, 1);
        assert!(told.is_empty());
        assert!(result_unknown(&answered.result.unwrap_err()));
    }

    #[test]
    fn a_read_refused_for_something_passing_is_asked_again_though_it_has_no_op() {
        // An item's text, or a kickoff check: nothing changes however often it's sent.
        let read = json!({"action":"kickoff","id":"0041"});
        let (answered, told, sends) = over_ssh(
            &read,
            true,
            vec![
                output(
                    0,
                    r#"{"ok":false,"retry":3,"message":"herdr isn't answering yet."}"#,
                    "",
                ),
                output(0, r#"{"ok":true,"message":"","questions":[]}"#, ""),
            ],
        );
        assert!(answered.result.unwrap().ok);
        assert_eq!(sends, 2);
        assert_eq!(told.len(), 1);
        // A change with no op isn't, whatever it's told.
        let change = json!({"action":"close","id":"0041"});
        let (answered, _, sends) = over_ssh(
            &change,
            true,
            vec![output(
                0,
                r#"{"ok":false,"retry":3,"message":"locked"}"#,
                "",
            )],
        );
        assert!(!answered.result.unwrap().ok);
        assert_eq!(sends, 1);
    }

    #[test]
    fn a_backend_that_crashed_isnt_sent_the_request_again_and_isnt_called_a_dropped_connection() {
        let request = with_op(json!({"action":"close","id":"0041"}));
        let crashed = output(
            1,
            "",
            "Traceback (most recent call last):\nKeyError: 'stage'\n",
        );
        let (answered, told, sends) = over_ssh(&request, true, vec![crashed]);
        assert_eq!(sends, 1, "a crash would most likely crash again");
        assert!(told.is_empty());
        let error = answered.result.unwrap_err();
        assert!(
            error.starts_with(
                "arknet-one's action backend stopped without a proper answer (exit 1,"
            ),
            "{error}"
        );
        assert!(
            !error.contains("KeyError") && !error.contains("Traceback"),
            "{error}"
        );
        assert!(
            answered.stderr.contains("KeyError"),
            "kept for the error log"
        );
        // Exit 255 from a backend on this machine is its own, not ssh's.
        let local = answer_of(
            &output(255, "", ""),
            &request,
            deadline_for(&request),
            "here",
            false,
        );
        assert!(
            local
                .unwrap_err()
                .starts_with("here's action backend stopped")
        );
        assert!(!lost(&output(255, "", ""), false));
    }

    #[cfg(unix)]
    #[test]
    fn a_held_lock_answered_with_retry_is_sent_again_quietly_with_the_same_op() {
        let dir = tempfile::tempdir().unwrap();
        let d = quote(dir.path().to_str().unwrap());
        let command = fixture(
            dir.path(),
            &format!(
                r##"req=$(cat)
printf '%s\n' "$req" >> {d}/requests.log
if [ "$(wc -l < {d}/requests.log)" -lt 3 ]; then
  printf '%s' '{{"ok":false,"retry":2,"message":"#41 is being changed by another request; Kerdr asks again shortly."}}'
  exit 0
fi
echo done >> {d}/done.log
printf '%s' '{{"ok":true,"message":"Marked #41 done."}}'
"##
            ),
            "",
        );
        let request = with_op(json!({"action":"close","id":"0041"}));
        // A backend whose feed doesn't say it keeps ops is never sent it again: the refusal stands.
        let (answered, told, waits) = resent(&command, &request, false, Duration::from_secs(20));
        assert!(!answered.result.unwrap().ok);
        assert!(told.is_empty() && waits.is_empty());
        assert_eq!(lines(dir.path(), "requests.log").len(), 1);
        fs::remove_file(dir.path().join("requests.log")).unwrap();
        // One that does is, quietly, with the same op, and does it once.
        let (answered, told, waits) = resent(&command, &request, true, Duration::from_secs(20));
        assert!(answered.result.unwrap().ok);
        assert_eq!(waits, [Duration::from_secs(2); 2]);
        assert!(
            matches!(&told[0], Resend::Retry { message, .. } if message.contains("being changed")),
            "{told:?}"
        );
        let sent = lines(dir.path(), "requests.log");
        assert_eq!(sent.len(), 3);
        assert!(
            sent.iter().all(|s| *s == sent[0]),
            "the same request, the same op"
        );
        assert_eq!(lines(dir.path(), "done.log").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn retries_stop_after_two_minutes_and_the_last_refusal_is_shown_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let d = quote(dir.path().to_str().unwrap());
        let command = fixture(
            dir.path(),
            &format!(
                r##"cat >> {d}/requests.log
echo >> {d}/requests.log
printf '%s' '{{"ok":false,"retry":50,"message":"#41 is still locked by a pull under way."}}'
"##
            ),
            "",
        );
        let request = with_op(json!({"action":"close","id":"0041"}));
        let (answered, told, waits) = resent(&command, &request, true, Duration::from_secs(20));
        // 50 s, 50 s, and a third wait would pass two minutes.
        assert_eq!(waits, [Duration::from_secs(50); 2]);
        assert_eq!(told.len(), 2);
        let r = answered.result.unwrap();
        assert!(!r.ok && r.retry == Some(50));
        assert_eq!(r.message, "#41 is still locked by a pull under way.");
        // Without an op it isn't followed at all.
        let (_, told, waits) = resent(
            &command,
            &json!({"action":"close","id":"0041"}),
            true,
            Duration::from_secs(20),
        );
        assert!(told.is_empty() && waits.is_empty());
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
        // A save's is kept too, for its cause, without its text.
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"edit","id":"1","text":"my private secret words","was":"x"}),
        );
        assert!(answered.stderr.contains("the push failed: rejected"));
        assert!(!answered.stderr.contains("private"), "{}", answered.stderr);
        // A sign-in's token doesn't come back either.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'bad code private-value' >&2\nprintf '{\"ok\":false,\"message\":\"bad code private-value\"}'\n",
            "",
        );
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"account-signin","token":"fixture-private-value"}),
        );
        // Quoted in part: not in its standard error, nor in the message, which is shown too.
        assert!(!answered.stderr.contains("private"), "{}", answered.stderr);
        let message = answered.result.unwrap().message;
        assert!(!message.contains("private"), "{message}");
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
    fn a_backend_that_stops_without_an_answer_says_so_plainly_and_logs_what_it_printed() {
        let dir = tempfile::tempdir().unwrap();
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'not JSON'\necho 'Traceback: KeyError tracker unavailable' >&2\nexit 7\n",
            "[Console]::In.ReadToEnd() | Out-Null; Write-Output 'not JSON'; [Console]::Error.WriteLine('Traceback: KeyError tracker unavailable'); exit 7",
        );
        let request = json!({"action":"refresh","id":"4"});
        let answered = run_keeping_stderr(&machine(), &command, &request);
        let error = answered.result.unwrap_err();
        assert_eq!(
            error,
            "here's action backend stopped without a proper answer (exit 7, and what it printed is in Kerdr's error log); Kerdr can't tell whether it happened, and its card shows how it stands."
        );
        assert!(result_unknown(&error));
        // What it printed goes to the error log, never the board: no traceback, no class name.
        assert!(!error.contains("KeyError"), "{error}");
        assert!(
            answered.stderr.contains("tracker unavailable"),
            "{}",
            answered.stderr
        );
    }

    #[cfg(unix)]
    #[test]
    fn what_a_backend_printed_in_place_of_an_answer_is_kept_for_the_log_as_its_error_says() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing on standard error at all: what it printed instead is all there is to go on.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'ark-card-action: printed a warning before its answer'\necho '{\"ok\":tru'\n",
            "",
        );
        let request = json!({"action":"refresh","id":"4"});
        let answered = run_keeping_stderr(&machine(), &command, &request);
        let error = answered.result.unwrap_err();
        assert!(error.contains("in Kerdr's error log"), "{error}");
        assert!(!error.contains("printed a warning"), "{error}");
        assert!(
            answered
                .stderr
                .contains("printed a warning before its answer"),
            "{}",
            answered.stderr
        );
        // Scrubbed as standard error is: neither what was typed nor a credential.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'not an answer: my private secret words, fixture-private-value'\n",
            "",
        );
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"create","words":"my private secret words"}),
        );
        assert!(
            answered.stderr.contains("not an answer"),
            "{}",
            answered.stderr
        );
        assert!(
            !answered.stderr.contains("private secret"),
            "{}",
            answered.stderr
        );
        let answered = run_keeping_stderr(
            &machine(),
            &command,
            &json!({"action":"account-signin","step":"paste","token":"fixture-private-value"}),
        );
        assert!(
            !answered.stderr.contains("fixture-private-value"),
            "{}",
            answered.stderr
        );
        // One far too long: its beginning, bounded, after the end of its standard error.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\necho 'the push failed' >&2\nprintf '{\"ok\":true,\"message\":\"'\nhead -c 100000 /dev/zero | tr '\\0' 'y'\n",
            "",
        );
        let answered = run_keeping_stderr(&machine(), &command, &request);
        assert!(answered.result.unwrap_err().contains("far too long"));
        assert!(
            answered.stderr.starts_with("the push failed"),
            "{}",
            &answered.stderr[..80]
        );
        assert!(answered.stderr.contains(r#"{"ok":true,"message":""#));
        assert!(answered.stderr.len() <= crate::errlog::MOST_STDERR);
        // A good answer keeps nothing of what it printed.
        let command = fixture(
            dir.path(),
            "cat >/dev/null\nprintf '%s' '{\"ok\":true,\"message\":\"Refreshed.\"}'\n",
            "",
        );
        assert!(
            run_keeping_stderr(&machine(), &command, &request)
                .stderr
                .is_empty()
        );
    }

    #[test]
    fn a_timeout_says_plainly_that_nobody_knows_whether_it_happened() {
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
        assert_eq!(
            error,
            "no answer came from here within a minute; Kerdr can't tell whether it happened, and its card shows how it stands."
        );
        assert!(result_unknown(&error));
        // Words for Andrew, not for an agent.
        for agentish in ["canonical", "Do not retry", "result is unknown"] {
            assert!(!error.contains(agentish), "{error}");
        }
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
        let answered = run_keeping_stderr(&machine(), &command, &save);
        let error = answered.result.unwrap_err();
        assert!(!error.contains("kerdr-fixture-secret-line"), "{error}");
        assert!(error.contains("(exit 1,"), "{error}");
        // Nor the error log: what was typed is scrubbed from what the save's command printed.
        assert!(
            !answered.stderr.contains("kerdr-fixture-secret-line"),
            "{}",
            answered.stderr
        );
        // A read's error output goes to the error log as any action's does, never the board.
        let read = json!({"action":"edit","id":"0414"});
        let answered = run_keeping_stderr(&machine(), &command, &read);
        let error = answered.result.unwrap_err();
        assert!(!error.contains("leak"), "{error}");
        assert!(answered.stderr.contains("leak: kerdr-fixture-secret-line"));
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
