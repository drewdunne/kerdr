"""Disposable native Windows integration using real Herdr and Microsoft's ConPTY."""
import ctypes
import faulthandler
from ctypes import wintypes
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

from winpty import PtyProcess

sys.stdout.reconfigure(encoding="utf-8")
faulthandler.dump_traceback_later(180, exit=True)
kerdr = str(Path(sys.argv[1]).resolve())
probe = str(Path(kerdr).parent / "examples" / "windows_probe.exe")
results = Path("test-results").resolve()
results.mkdir(exist_ok=True)
env = {k: v for k, v in os.environ.items() if not k.startswith("HERDR_")}
session = f"zz-kerdr-windows-{os.getpid()}"
transcript = []
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.OpenProcess.restype = wintypes.HANDLE
kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel.CloseHandle.argtypes = [wintypes.HANDLE]


def alive(pid):
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return False
    code = wintypes.DWORD()
    kernel.GetExitCodeProcess(handle, ctypes.byref(code))
    kernel.CloseHandle(handle)
    return code.value == 259


def call(args, timeout=45, child_env=None):
    # File-backed captures cannot hang communicate() if a persistent backend retains a handle.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(args, env=child_env or env, stdout=stdout, stderr=stderr)
        try:
            code = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
            raise
        stdout.seek(0)
        stderr.seek(0)
        return subprocess.CompletedProcess(args, code, stdout.read().decode("utf-8", "replace"),
                                           stderr.read().decode("utf-8", "replace"))


def run(args, check=True, **kw):
    p = call(args, **kw)
    if check and p.returncode:
        raise AssertionError(f"{args}: {p.returncode}\n{p.stdout}\n{p.stderr}")
    return p.stdout


def herdr(*args, name=session):
    return run(["herdr", "--session", name, *args])


def wait_for(check, label, timeout=35):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.2)
    raise AssertionError(f"timed out: {label}")


def quote(text):
    return "'" + str(text).replace("'", "''") + "'"


for count in [0, 1, 2]:
    objects = ",".join("[pscustomobject]@{ProcessId=" + str(i) + ";Name='herdr.exe';CommandLine='fixture'}" for i in range(count))
    value = json.loads(run(["powershell.exe", "-NoProfile", "-Command",
                           "ConvertTo-Json -InputObject @(" + objects + ") -Compress"]))
    assert isinstance(value, list) and len(value) == count, value
print("PASS PowerShell process JSON for zero, one and multiple processes", flush=True)

# Compare the native defaults with Herdr itself, without changing any config.
default_env = env.copy()
for key in ["HERDR_CONFIG_PATH", "KERDR_CONFIG", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"]:
    default_env.pop(key, None)
paths = json.loads(subprocess.check_output([probe, "paths"], env=default_env, encoding="utf-8"))
herdr_help = subprocess.check_output(["herdr", "--help"], env=default_env, encoding="utf-8")
assert ("Config: " + paths["herdr"]).lower() in herdr_help.lower(), (paths, herdr_help)
assert Path(paths["kerdr"]) == Path(env["APPDATA"]) / "kerdr" / "config.toml"
assert Path(paths["home"]) == Path(env["USERPROFILE"])
print("PASS default Windows paths agree with native Herdr", flush=True)

with tempfile.TemporaryDirectory(prefix="kerdr windows café ") as temp:
    temp = Path(temp)
    config = temp / "herdr.toml"
    config.write_text('onboarding = false\n', encoding="utf-8")
    env["HERDR_CONFIG_PATH"] = str(config)
    run([kerdr, "herdr-keys", "--install", "--config", str(config)])
    env["KERDR_CONFIG"] = str(temp / "kerdr.toml")
    Path(env["KERDR_CONFIG"]).write_text(
        f'[machines]\npicker = false\n[board]\nonly = ["{session}"]\n', encoding="utf-8")
    env["XDG_CACHE_HOME"] = str(temp / "cache")
    env["XDG_STATE_HOME"] = str(temp / "state")
    console = None
    kerdr_pid = None
    server_pids = []
    test_sessions = [session, session + "-timeout"]
    try:
        # Actions can start persistent work. Exercise both success and unknown-result timeout
        # through the same library adapter transport used by real actions.
        for index, name in enumerate(test_sessions):
            pidfile = temp / f"server-{index}.pid"
            command = (
                f"$p = Start-Process (Get-Command herdr).Source -PassThru -WindowStyle Hidden "
                f"-ArgumentList '--session {name} server' "
                f"-RedirectStandardOutput {quote(results / f'server-{index}.txt')} "
                f"-RedirectStandardError {quote(results / f'server-{index}-err.txt')}; "
                f"[IO.File]::WriteAllText({quote(pidfile)}, [string]$p.Id); "
                "Write-Output '{\"ok\":true,\"message\":\"started\"}'; "
                + ("Start-Sleep 30" if index else "")
            )
            print(f"Starting persistent server through adapter {index}", flush=True)
            p = call([probe, "adapter", command, "5000"], timeout=20)
            print(f"Adapter {index} returned {p.returncode}", flush=True)
            assert p.returncode == (64 if index else 0), (p.returncode, p.stdout, p.stderr)
            pid = int(pidfile.read_text())
            server_pids.append(pid)
            def ready():
                return call(["herdr", "--session", name, "workspace", "list"], timeout=10).returncode == 0
            wait_for(ready, "action-created server is ready")
            assert alive(pid), "action completion/timeout killed its persistent backend"
            print(f"PASS persistent action-created server after {'timeout' if index else 'success'}", flush=True)

        workspaces = []
        for label in ["01-windows", "02-windows"]:
            created = json.loads(herdr("workspace", "create", "--label", label, "--no-focus"))
            wid = created["result"]["workspace"]["workspace_id"]
            workspaces.append(wid)
            herdr("workspace", "report-metadata", wid, "--source", "kerdr",
                  "--token", "kerdr_stage=test", "--token", "kerdr_status=needs_you",
                  "--token", "kerdr_note=Windows live check")
        feed = {"kerdr_feed": 1, "source": "windows-live", "cards": [
            {"id": str(index + 1), "ref": f"0{index + 1}-windows", "title": f"Windows card {index + 1}",
             "stage": "test", "state": "active", "attention": "needs_you",
             "story": "Native café 🦀", "workspace": {"session": session, "id": wid}}
            for index, wid in enumerate(workspaces)]}
        feed_command = "Write-Output " + quote(json.dumps(feed, ensure_ascii=False))
        with Path(env["KERDR_CONFIG"]).open("a", encoding="utf-8") as cfg:
            cfg.write("[feed]\ncommand = " + json.dumps(feed_command, ensure_ascii=False) + "\n")
        panes = json.loads(herdr("pane", "list"))
        for pane in panes.get("result", {}).get("panes", []):
            if pane.get("pane_id"):
                herdr("pane", "report-metadata", pane["pane_id"], "--source",
                      "arkship.ark-helpers", "--token", "role=helper")
        (results / "windows-sessions.json").write_text(run(["herdr", "session", "list", "--json"]), encoding="utf-8")
        (results / "windows-snapshot.json").write_text(herdr("api", "snapshot"), encoding="utf-8")
        print(run([kerdr, "doctor"]), flush=True)
        cards = json.loads(run([kerdr, "cards"]))
        assert cards["machines"][0]["connection"] == "live", cards
        for label in ["01-windows", "02-windows"]:
            assert any(c["ref"] == label and c["attention"] == "needs you" for c in cards["cards"]), cards
        print("PASS native discovery, snapshots, cards and doctor", flush=True)

        modes_file = temp / "console-modes.json"
        control_file = temp / "control-event"
        # String zero avoids PtyProcess.spawn treating numeric zero as an omitted backend.
        # winpty-rs defines ConPTY=0, WinPTY=1.
        print("Starting real ConPTY console probe", flush=True)
        console = PtyProcess.spawn([probe, "console", kerdr, str(modes_file), str(control_file)],
                                   env=env, dimensions=(40, 140), backend="0")
        output = queue.Queue()
        def drain():
            try:
                while True:
                    data = console.read(65536)
                    transcript.append(data)
                    output.put(data)
            except (EOFError, OSError):
                pass
        threading.Thread(target=drain, daemon=True).start()
        def state():
            try:
                return json.loads(modes_file.read_text())
            except (OSError, ValueError):
                return None
        wait_for(state, "console probe starts")
        kerdr_pid = state()["pid"]
        print(f"Console probe started Kerdr process {kerdr_pid}", flush=True)
        def expect(text, timeout=35):
            seen = ""
            until = time.monotonic() + timeout
            while time.monotonic() < until:
                try:
                    seen += output.get(timeout=0.5)
                except queue.Empty:
                    continue
                if text.lower() in seen.lower():
                    return
            raise AssertionError(f"console did not show {text!r}: {seen[-3000:]}")
        def discard():
            while True:
                try:
                    output.get_nowait()
                except queue.Empty:
                    return
        def processes():
            query = f"ConvertTo-Json -InputObject @(Get-CimInstance Win32_Process -Filter 'ParentProcessId = {kerdr_pid}' | Select-Object ProcessId,Name,CommandLine) -Compress"
            value = json.loads(run(["powershell.exe", "-NoProfile", "-Command", query]) or "[]")
            return [value] if isinstance(value, dict) else value or []
        def client_pids():
            import re
            pattern = r'--session\s+"?' + re.escape(session) + r'"?\s*$'
            return {p["ProcessId"] for p in processes()
                    if p["Name"].lower() == "herdr.exe" and re.search(pattern, p["CommandLine"] or "")}
        def snapshot():
            return json.loads(herdr("api", "snapshot"))["result"]["snapshot"]
        def verify_identity():
            assert alive(server_pids[0]), "original persistent server exited"
            assert set(workspaces) <= {w["workspace_id"] for w in snapshot()["workspaces"]}
        expect("Windows card 1")
        time.sleep(0.3)
        board_modes = state()["modes"]
        def enter():
            discard()
            print("Entering Herdr", flush=True)
            console.write("\r")
            wait_for(client_pids, "attached client starts")
            time.sleep(1)
            clients = client_pids()
            assert clients, "interactive client exited before terminal input"
            return clients
        def back(key):
            discard()
            print(f"Returning from Herdr with {key!r}", flush=True)
            console.write("\x02" + key)
            wait_for(lambda: not client_pids(), "only attached client exits")
            expect("MAINTAIN")
            time.sleep(0.2)
            assert state()["modes"] == board_modes, (state(), board_modes)
            verify_identity()

        enter()
        back("q")
        print("PASS Enter, ordinary detach, original server/workspaces and console flags", flush=True)
        enter()
        back("\x7f")
        print("PASS prefix Backspace: fresh Kerdr redraw, original server and console flags", flush=True)
        original_client = enter()
        original_focus = snapshot()["focused_workspace_id"]
        discard()
        console.write("\x02 ")
        wait_for(lambda: client_pids() and client_pids() != original_client
                 and snapshot()["focused_workspace_id"] in set(workspaces) - {original_focus},
                 "next opens a different workspace in a new client")
        verify_identity()
        back("\x7f")
        print("PASS prefix Space jumps directly to the other card; Backspace restores board modes", flush=True)
        for event in [0, 1]:
            enter()
            discard()
            control_file.write_text(str(event))
            wait_for(lambda: not control_file.exists(), "control event delivered")
            time.sleep(1)
            assert alive(kerdr_pid), "console control event killed Kerdr during handoff"
            if client_pids():
                back("q")
            else:
                expect("MAINTAIN")
                assert state()["modes"] == board_modes
                verify_identity()
            print(f"PASS console {'Ctrl+C' if event == 0 else 'Ctrl+Break'} during handoff", flush=True)
        discard()
        console.write("q")
        wait_for(lambda: not console.isalive(), "Kerdr exits")
        final_modes = state()
        assert final_modes["exited"] and final_modes["modes"] == final_modes["baseline"], final_modes
        (results / "console-modes.json").write_text(json.dumps(final_modes), encoding="utf-8")
        verify_identity()
        print("PASS Kerdr exit restores original console modes and preserves original sessions", flush=True)
    finally:
        (results / "windows-console.txt").write_text("".join(transcript), encoding="utf-8")
        if 'modes_file' in locals() and modes_file.exists():
            (results / "console-modes-last.json").write_bytes(modes_file.read_bytes())
        if kerdr_pid and alive(kerdr_pid):
            try:
                (results / "child-processes.json").write_text(json.dumps(processes()), encoding="utf-8")
            except Exception:
                pass
        if kerdr_pid and alive(kerdr_pid):
            run(["taskkill", "/PID", str(kerdr_pid), "/T", "/F"], check=False)
        if console and console.isalive():
            console.terminate(force=True)
        for pidfile in temp.glob("server-*.pid"):
            try:
                server_pids.append(int(pidfile.read_text()))
            except (OSError, ValueError):
                pass
        for name in test_sessions:
            for command in ["stop", "delete"]:
                try:
                    run(["herdr", "session", command, name], check=False, timeout=10)
                except subprocess.TimeoutExpired:
                    pass
        for pid in set(server_pids):
            if alive(pid):
                run(["taskkill", "/PID", str(pid), "/T", "/F"], check=False, timeout=10)
            wait_for(lambda: not alive(pid), "test server stops", timeout=10)
