//! Opt-in actions sent as JSON to a command on the card's home machine.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Machine;
use crate::transport::{self, Target};

const MAX_JSON_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub confirmation: bool,
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
}

pub fn run(machine: &Machine, command: &str, request: &Value) -> Result<Response, String> {
    run_with_deadline(machine, command, request, Duration::from_secs(120))
}

fn run_with_deadline(
    machine: &Machine,
    command: &str,
    request: &Value,
    deadline: Duration,
) -> Result<Response, String> {
    if command.trim().is_empty() {
        return Err("actions are disabled; configure [actions] command".into());
    }
    let json = serde_json::to_string(request).map_err(|e| format!("the action request: {e}"))?;
    if json.len() > MAX_JSON_BYTES {
        return Err("the action request is too large (maximum 64 KiB)".into());
    }
    let target = match &machine.ssh {
        Some(host) => Target::Ssh(host.clone()),
        None => Target::Local,
    };
    // The configured command is source; user data is JSON on stdin, never source.
    let output = transport::run_adapter(&target, command, Some(&json), deadline);
    if output.timed_out {
        return Err("the action timed out; its result is unknown. Check the canonical record before trying again. Do not retry automatically.".into());
    }
    if output.stdout.len() > MAX_JSON_BYTES {
        return Err("the action's JSON response is too large; its result is unknown. Do not retry automatically.".into());
    }
    // A backend can request confirmation with a nonzero exit code. Its JSON is the contract,
    // so parse it before interpreting process status.
    serde_json::from_str(&output.stdout).map_err(|error| {
        let status = output.status.map_or_else(|| "no exit status".into(), |s| format!("exit {s}"));
        let detail: String = output.stderr.trim().chars().take(1024).collect();
        let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
        format!("the action's JSON response is invalid ({status}): {error}{detail}. Its result is unknown. Do not retry automatically.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::transport::quote;
    use serde_json::json;
    use std::fs;

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
        let error = run_with_deadline(
            &machine(),
            &command,
            &json!({"action":"dispatch","id":"4"}),
            Duration::from_millis(60),
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(error.contains("result is unknown"), "{error}");
        assert!(error.contains("Do not retry automatically"), "{error}");
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
