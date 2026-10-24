"""Native Windows integration: isolated Herdr, both custom keys and real console handoff.
Run after cargo build with Python + pywinpty, on a disposable Windows CI runner.
Never installs Kerdr or changes the user's Herdr configuration.
"""
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

kerdr = str(Path(sys.argv[1]).resolve())
results = Path("test-results")
results.mkdir(exist_ok=True)
env = {k: v for k, v in os.environ.items() if not k.startswith("HERDR_")}
session = f"zz-kerdr-windows-{os.getpid()}"
transcript = []


def run(args, check=True, **kw):
    p = subprocess.run(args, env=env, capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=40, **kw)
    if check and p.returncode:
        raise AssertionError(f"{args}: {p.returncode}\n{p.stdout}\n{p.stderr}")
    return p.stdout


def herdr(*args):
    return run(["herdr", "--session", session, *args])


def wait_for(check, label, timeout=30):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.2)
    raise AssertionError(f"timed out: {label}")


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
    server_log = open(temp / "server.log", "w", encoding="utf-8")
    server = subprocess.Popen(["herdr", "--session", session, "server"], env=env,
                              stdout=server_log, stderr=subprocess.STDOUT)
    console = None
    try:
        def ready():
            return subprocess.run(["herdr", "--session", session, "workspace", "list"],
                                  env=env, capture_output=True, timeout=10).returncode == 0
        wait_for(ready, "isolated server starts")
        created = json.loads(herdr("workspace", "create", "--label", "01-windows", "--no-focus"))
        wid = created["result"]["workspace"]["workspace_id"]
        # Mark test panes as helpers before any test agent could be started (none are).
        panes = json.loads(herdr("pane", "list"))
        for pane in panes.get("result", {}).get("panes", []):
            if pane.get("pane_id"):
                herdr("pane", "report-metadata", pane["pane_id"], "--source", "ark-helper", "--token", "role=helper")
        herdr("workspace", "report-metadata", wid, "--source", "kerdr",
              "--token", "kerdr_status=needs_you", "--token", "kerdr_note=Windows live check")
        cards = json.loads(run([kerdr, "cards"]))
        assert cards["machines"][0]["connection"] == "live", cards
        assert any(c["ref"] == "01-windows" and c["attention"] == "needs you" for c in cards["cards"]), cards
        run([kerdr, "doctor"])
        print("PASS native discovery, snapshots, cards and doctor", flush=True)

        # pywinpty backend 1 is Microsoft's real ConPTY, with native console input events.
        console = PtyProcess.spawn([kerdr], env=env, dimensions=(40, 140), backend=1)
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
        def expect(text, timeout=30):
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
        expect("Misc")
        console.write("i")
        expect("01-windows")
        # Ordinary Herdr detach returns to Kerdr without stopping the server.
        console.write("\r")
        time.sleep(3)
        console.write("\x02q")
        expect("01-windows")
        assert ready()
        print("PASS Enter -> Herdr -> prefix q -> Kerdr; server persists", flush=True)
        # Both installed custom commands must execute through Windows Herdr's cmd shell.
        for key, request in [("\x7f", "board"), (" ", "next")]:
            console.write("\r")
            time.sleep(3)
            console.write("\x02" + key)
            expect("01-windows")
            assert ready()
            print(f"PASS prefix {request}: native command, token, client-only exit and board redraw", flush=True)
        console.write("q")
        wait_for(lambda: not console.isalive(), "Kerdr exits cleanly")
        assert ready(), "quitting Kerdr killed the persistent server"
        print("PASS Kerdr exit leaves persistent Herdr server and workspace alive", flush=True)
    finally:
        (results / "windows-console.txt").write_text("".join(transcript), encoding="utf-8")
        if console and console.isalive():
            console.terminate(force=True)
        run(["herdr", "session", "stop", session], check=False)
        run(["herdr", "session", "delete", session], check=False)
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        server_log.close()
        (results / "windows-server.txt").write_text((temp / "server.log").read_text(encoding="utf-8", errors="replace"), encoding="utf-8")
