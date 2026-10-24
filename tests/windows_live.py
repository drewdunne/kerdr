"""Disposable native Windows integration using real Herdr and Microsoft's ConPTY."""
import ctypes
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


def run(args, check=True, **kw):
    p = subprocess.run(args, env=env, capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=45, **kw)
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
            p = subprocess.run([probe, "adapter", command, "5000"], env=env,
                               capture_output=True, text=True, encoding="utf-8", timeout=20)
            assert p.returncode == (64 if index else 0), (p.returncode, p.stdout, p.stderr)
            pid = int(pidfile.read_text())
            server_pids.append(pid)
            def ready():
                return subprocess.run(["herdr", "--session", name, "workspace", "list"],
                                      env=env, capture_output=True, timeout=10).returncode == 0
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
        console = PtyProcess.spawn([probe, "console", kerdr, str(modes_file), str(control_file)],
                                   env=env, dimensions=(40, 140), backend=1)
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
        def client_pids():
            query = f"(Get-CimInstance Win32_Process -Filter 'ParentProcessId = {kerdr_pid}' | Where-Object Name -eq 'herdr.exe' | Select-Object -ExpandProperty ProcessId) -join ','"
            return run(["powershell.exe", "-NoProfile", "-Command", query]).strip()
        def snapshot():
            return json.loads(herdr("api", "snapshot"))["result"]["snapshot"]
        def verify_identity():
            assert alive(server_pids[0]), "original persistent server exited"
            assert set(workspaces) <= {w["workspace_id"] for w in snapshot()["workspaces"]}
        expect("Misc")
        console.write("i")
        expect("Conversations without an intent")
        time.sleep(0.3)
        board_modes = state()["modes"]
        def enter():
            discard()
            console.write("\r")
            wait_for(client_pids, "attached client starts")
            time.sleep(1)
            return client_pids()
        def back(key):
            discard()
            console.write("\x02" + key)
            wait_for(lambda: not client_pids(), "only attached client exits")
            expect("Conversations without an intent")
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
                expect("Conversations without an intent")
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
        if kerdr_pid and alive(kerdr_pid):
            run(["taskkill", "/PID", str(kerdr_pid), "/T", "/F"], check=False)
        if console and console.isalive():
            console.terminate(force=True)
        for name in test_sessions:
            run(["herdr", "session", "stop", name], check=False)
            run(["herdr", "session", "delete", name], check=False)
        for pid in server_pids:
            wait_for(lambda: not alive(pid), "test server stops")
