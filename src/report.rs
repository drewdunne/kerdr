//! `kerdr report`: how an agent tells Kerdr where its work stands, from inside its herdr pane.
//! It writes herdr workspace tokens (kerdr_stage, kerdr_status, kerdr_note, kerdr_progress),
//! which herdr keeps until they change, the workspace closes or herdr restarts.

use std::process::Command;

use crate::model::{Attention, Stage};

#[derive(Debug, Default)]
pub struct Report {
    pub stage: Option<String>,
    pub status: Option<String>,
    pub note: Option<String>,
    pub progress: Option<String>,
    pub clear: bool,
    pub workspace: Option<String>,
    /// The workspace's herdr session, when not run from inside it.
    pub session: Option<String>,
}

pub fn args(r: &Report) -> Result<Vec<String>, String> {
    let workspace = r
        .workspace
        .clone()
        .or_else(|| std::env::var("HERDR_WORKSPACE_ID").ok())
        .filter(|w| !w.is_empty())
        .ok_or("run this inside a herdr pane, or pass --workspace")?;
    let mut a: Vec<String> = Vec::new();
    if let Some(s) = r.session.as_deref().filter(|s| !s.is_empty()) {
        a.push("--session".into());
        a.push(s.into());
    }
    a.extend([
        "workspace".to_string(),
        "report-metadata".into(),
        workspace,
        "--source".into(),
        "kerdr".into(),
    ]);
    if r.clear {
        for t in [
            "kerdr_stage",
            "kerdr_status",
            "kerdr_note",
            "kerdr_progress",
        ] {
            a.push("--clear-token".into());
            a.push(t.into());
        }
        return Ok(a);
    }
    let mut any = false;
    if let Some(s) = &r.stage {
        let stage = Stage::parse(s).ok_or_else(|| {
            format!(
                "unknown stage {s:?}: use backlog, plan, design, build, test, deploy or maintain"
            )
        })?;
        a.push("--token".into());
        a.push(format!("kerdr_stage={}", stage.id()));
        any = true;
    }
    if let Some(s) = &r.status {
        let att = Attention::parse(s).ok_or_else(|| {
            format!("unknown status {s:?}: use needs-you, waiting, running, blocked, idle or done")
        })?;
        let id = match att {
            Attention::NeedsYou => "needs_you",
            Attention::Blocked => "blocked",
            Attention::Running => "running",
            Attention::Waiting => "waiting",
            Attention::Idle => "idle",
            Attention::Done | Attention::Unreachable => "done",
        };
        a.push("--token".into());
        a.push(format!("kerdr_status={id}"));
        any = true;
    }
    if let Some(n) = &r.note {
        a.push("--token".into());
        a.push(format!("kerdr_note={}", crate::theme::trunc(n.trim(), 78)));
        any = true;
    }
    if let Some(p) = &r.progress {
        crate::board::parse_progress(p)
            .ok_or_else(|| format!("progress is met/total, like 1/3, not {p:?}"))?;
        a.push("--token".into());
        a.push(format!("kerdr_progress={p}"));
        any = true;
    }
    if !any {
        return Err(
            "say what to report: --stage, --status, --note or --progress (or --clear)".into(),
        );
    }
    Ok(a)
}

pub fn run(r: &Report) -> Result<(), String> {
    let a = args(r)?;
    let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let out = Command::new(&herdr)
        .args(&a)
        .output()
        .map_err(|e| format!("{herdr}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .next()
            .unwrap_or("herdr refused the report")
            .to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_herdr_arguments() {
        let r = Report {
            stage: Some("review".into()),
            status: Some("needs-you".into()),
            note: Some("pick a palette".into()),
            progress: Some("1/3".into()),
            workspace: Some("w4".into()),
            ..Default::default()
        };
        let a = args(&r).unwrap();
        assert!(a.contains(&"kerdr_stage=test".to_string()));
        assert!(a.contains(&"kerdr_status=needs_you".to_string()));
        assert!(a.contains(&"kerdr_note=pick a palette".to_string()));
        assert!(a.contains(&"kerdr_progress=1/3".to_string()));
    }

    #[test]
    fn refuses_nonsense() {
        let base = Report {
            workspace: Some("w1".into()),
            ..Default::default()
        };
        assert!(args(&base).is_err());
        assert!(
            args(&Report {
                stage: Some("vibes".into()),
                ..base
            })
            .is_err()
        );
    }
}
