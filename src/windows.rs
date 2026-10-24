//! Native Windows collection and bounded adapter processes. Herdr clients are deliberately
//! excluded from adapter jobs so a client timeout can never kill a persistent session.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

use crate::config::Machine;
use crate::transport::{self, Output, Target};

pub struct ProcessJob(HANDLE);

impl ProcessJob {
    pub fn assign(child: &Child) -> std::io::Result<Self> {
        // SAFETY: null security/name uses an unnamed job with default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(handle);
        // SAFETY: this Windows POD structure is valid when zero-initialized.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: both handles are live and the buffer and size describe limits.
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) == 0
        } {
            return Err(std::io::Error::last_os_error());
        }
        resume(child.id())?;
        Ok(job)
    }
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        // SAFETY: this job owns the handle; closing it also reaps its adapter descendants.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn resume(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: snapshot and thread handles are checked and closed on every path.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut found = Thread32First(snapshot, &mut entry);
        let mut result = Err(std::io::Error::other("suspended command thread not found"));
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                result = if thread.is_null() {
                    Err(std::io::Error::last_os_error())
                } else {
                    let resumed = ResumeThread(thread);
                    let error = std::io::Error::last_os_error();
                    CloseHandle(thread);
                    if resumed == u32::MAX {
                        Err(error)
                    } else {
                        Ok(())
                    }
                };
                break;
            }
            found = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        result
    }
}

/// Console flags belong to the shared console, rather than to an individual client process.
/// Restore them even when a client is forcibly closed before its own cleanup can run.
pub struct ConsoleModes(Vec<(HANDLE, u32)>);

impl ConsoleModes {
    pub fn capture() -> Self {
        use windows_sys::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        let mut modes = Vec::new();
        for stream in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE] {
            let mut mode = 0;
            // SAFETY: GetConsoleMode validates the standard handle before we store it.
            unsafe {
                let handle = GetStdHandle(stream);
                if GetConsoleMode(handle, &mut mode) != 0 {
                    modes.push((handle, mode));
                }
            }
        }
        Self(modes)
    }

    pub fn values(&self) -> Vec<u32> {
        self.0.iter().map(|(_, mode)| *mode).collect()
    }
}

impl Drop for ConsoleModes {
    fn drop(&mut self) {
        for (handle, mode) in &self.0 {
            // SAFETY: these are borrowed console handles; they remain open during handoff.
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleMode(*handle, *mode);
            }
        }
    }
}

pub fn poll(machine: &Machine, feed_due: bool) -> Output {
    poll_with_deadline(machine, feed_due, Duration::from_secs(25))
}

fn poll_with_deadline(machine: &Machine, feed_due: bool, deadline: Duration) -> Output {
    let started = Instant::now();
    let run = |args: &[&str]| {
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Output {
                timed_out: true,
                ..Default::default()
            };
        }
        transport::run_herdr(&Target::Local, &machine.herdr, args, remaining)
    };
    let version = run(&["--version"]);
    if version.status == Some(127) {
        return Output {
            status: Some(0),
            stdout: "@@KERDR missing\n".into(),
            took: started.elapsed(),
            ..Default::default()
        };
    }
    if !version.ok() {
        return version;
    }
    let sessions = run(&["session", "list", "--json"]);
    if !sessions.ok() {
        return sessions;
    }
    let keys = std::fs::read_to_string(crate::herdr_keys::herdr_config_path())
        .is_ok_and(|s| crate::herdr_keys::has_keys(&s));
    let mut stdout = format!(
        "@@KERDR host\n{}\n@@KERDR version\n{}\n@@KERDR keys\n{}\n@@KERDR sessions\n{}\n",
        crate::config::hostname(),
        version.stdout.trim(),
        if keys { "yes" } else { "no" },
        sessions.stdout.trim()
    );
    if let Ok(list) = crate::herdr::parse_sessions(&sessions.stdout) {
        for session in list.into_iter().filter(|s| s.running) {
            let snapshot = run(&["--session", &session.name, "api", "snapshot"]);
            if snapshot.timed_out {
                return snapshot;
            }
            let body = if snapshot.ok() {
                snapshot.stdout
            } else {
                format!(
                    "snapshot failed (exit {:?}): {}",
                    snapshot.status,
                    snapshot.stderr.trim()
                )
            };
            stdout.push_str(&format!("@@KERDR snapshot {}\n{}\n", session.name, body));
        }
    }
    if let Some(command) = machine.feed.as_deref().filter(|_| feed_due) {
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Output {
                timed_out: true,
                ..Default::default()
            };
        }
        let feed = transport::run_adapter(
            &Target::Local,
            command,
            None,
            Duration::from_secs(12).min(remaining),
        );
        if feed.ok() {
            stdout.push_str(&format!("@@KERDR feed\n{}\n", feed.stdout));
        } else {
            stdout.push_str(&format!(
                "@@KERDR feed_error\n{}\n",
                if feed.timed_out {
                    "the feed timed out".to_string()
                } else {
                    format!(
                        "the feed failed (exit {:?}): {}",
                        feed.status,
                        feed.stderr.trim()
                    )
                }
            ));
        }
    }
    stdout.push_str("@@KERDR end\n");
    Output {
        status: Some(0),
        stdout,
        took: started.elapsed(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposable_descendants_are_reaped_after_the_shell_exits() {
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        let script = format!(
            "$p = Start-Process powershell.exe -NoNewWindow -PassThru -ArgumentList '-NoProfile -Command Start-Sleep 30'; [IO.File]::WriteAllText({}, [string]$p.Id); Write-Output done",
            transport::powershell_quote(pid_file.to_str().unwrap())
        );
        let out = transport::run_script(&Target::Local, &script, Duration::from_secs(10));
        assert!(out.ok(), "{}", out.stderr);
        let pid: u32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        // SAFETY: query-only process handle, checked before use and closed after querying.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !handle.is_null() {
                let mut exit = 259;
                GetExitCodeProcess(handle, &mut exit);
                CloseHandle(handle);
                assert_ne!(exit, 259, "adapter descendant survived job cleanup");
            }
        }
    }

    #[test]
    fn valid_snapshot_output_before_a_timeout_is_not_a_successful_poll() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("slow herdr.cmd");
        std::fs::write(
            &bin,
            r#"@echo off
if "%~1"=="--version" (
echo herdr 0.9.5
exit /b 0
)
if "%~1"=="session" (
echo {"sessions":[{"name":"default","running":true}]}
exit /b 0
)
echo {"result":{"snapshot":{"workspaces":[]}}}
ping -n 6 127.0.0.1 >nul
"#,
        )
        .unwrap();
        let machine = Machine {
            name: "fixture".into(),
            herdr: bin.to_str().unwrap().into(),
            ssh: None,
            feed: None,
            hidden: false,
        };
        let out = poll_with_deadline(&machine, false, Duration::from_secs(1));
        assert!(out.timed_out, "{out:?}");
        assert!(!out.ok());
    }

    #[test]
    fn valid_feed_output_before_a_timeout_is_an_error_section() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("feed herdr.cmd");
        std::fs::write(&bin, "@echo off\nif \"%~1\"==\"--version\" (echo herdr 0.9.5 & exit /b 0)\necho {\"sessions\":[]}\n").unwrap();
        let machine = Machine {
            name: "fixture".into(),
            herdr: bin.to_str().unwrap().into(),
            ssh: None,
            feed: Some("Write-Output '{\"kerdr_feed\":1,\"cards\":[]}'; Start-Sleep 30".into()),
            hidden: false,
        };
        let out = poll_with_deadline(&machine, true, Duration::from_secs(2));
        let sections = crate::poll::split_sections(&out.stdout);
        assert_eq!(sections.feed_error.as_deref(), Some("the feed timed out"));
        assert!(sections.feed.is_none(), "timed-out valid JSON was accepted");
    }
}
