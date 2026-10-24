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

pub fn poll(machine: &Machine, feed_due: bool) -> Output {
    let started = Instant::now();
    let deadline = Duration::from_secs(25);
    let run = |args: &[&str]| {
        transport::run_herdr(
            &Target::Local,
            &machine.herdr,
            args,
            deadline.saturating_sub(started.elapsed()),
        )
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
            stdout.push_str(&format!(
                "@@KERDR snapshot {}\n{}\n",
                session.name, snapshot.stdout
            ));
        }
    }
    if let Some(command) = machine.feed.as_deref().filter(|_| feed_due) {
        let feed = transport::run_adapter(
            &Target::Local,
            command,
            None,
            Duration::from_secs(12).min(deadline.saturating_sub(started.elapsed())),
        );
        stdout.push_str(&format!("@@KERDR feed\n{}\n", feed.stdout));
    }
    stdout.push_str("@@KERDR end\n");
    Output {
        status: Some(0),
        stdout,
        took: started.elapsed(),
        ..Default::default()
    }
}
