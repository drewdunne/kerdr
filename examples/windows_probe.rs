//! Test driver for the Windows CI console and adapter checks; not part of the release package.
#[cfg(windows)]
fn main() {
    use std::fs;
    use std::process::Command;
    use std::time::Duration;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    let args: Vec<String> = std::env::args().collect();
    if args[1] == "paths" {
        println!(
            "{}",
            serde_json::json!({
                "herdr": kerdr::herdr_keys::herdr_config_path(),
                "kerdr": kerdr::config::config_path(), "home":kerdr::config::home(),
                "state":kerdr::config::state_dir(), "cache":kerdr::config::cache_dir()
            })
        );
        return;
    }
    if args[1] == "adapter" {
        let out = kerdr::transport::run_adapter(
            &kerdr::transport::Target::Local,
            &args[2],
            Some("{}"),
            Duration::from_millis(args[3].parse().unwrap()),
        );
        print!("{}", out.stdout);
        eprint!("{}", out.stderr);
        std::process::exit(if out.timed_out {
            64
        } else {
            out.status.unwrap_or(1)
        });
    }
    unsafe extern "system" fn control(event: u32) -> i32 {
        i32::from(event == 0 || event == 1)
    }
    // This test parent stays alive while Ctrl+C/Break are delivered to the shared console.
    unsafe {
        SetConsoleCtrlHandler(Some(control), 1);
    }
    let baseline = kerdr::windows::ConsoleModes::current_values();
    let mut child = Command::new(&args[2]).spawn().unwrap();
    let pid = child.id();
    loop {
        if let Ok(event) = fs::read_to_string(&args[4]) {
            let _ = fs::remove_file(&args[4]);
            unsafe {
                windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(
                    event.trim().parse().unwrap(),
                    0,
                );
            }
        }
        let modes = kerdr::windows::ConsoleModes::current_values();
        let status = child.try_wait().unwrap();
        fs::write(
            &args[3],
            serde_json::to_vec(&serde_json::json!({
                "pid":pid, "baseline":baseline, "modes":modes, "exited":status.is_some()
            }))
            .unwrap(),
        )
        .unwrap();
        if let Some(status) = status {
            std::process::exit(status.code().unwrap_or(1));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(not(windows))]
fn main() {}
