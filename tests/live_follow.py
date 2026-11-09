#!/usr/bin/env python3
"""Runs a real Kerdr against a fake action backend that answers with `follow` and `progress`
(docs/feed.md, "Following work that goes on after its answer"), and against one that doesn't, in
a throwaway herdr session zz-follow-<pid> that it starts, and stops and deletes after. Kerdr runs
in a pane of that session with every HERDR_ variable unset, and is read with `herdr pane read`.
Run it by hand before a release that touches following:

    tests/live_follow.py [path to kerdr] [folder for its screens]

It checks that ⏎ on a card whose conversation runs outside herdr sends one reopen, shows the
backend's words while it follows, goes in on `done`, shows a `failed` in plain words, and that a
backend without `follow` is never asked `progress`."""
import json, os, re, shutil, subprocess, sys, tempfile, time

KERDR = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/kerdr")
OUT = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="kerdr-follow-")
S = f"zz-follow-{os.getpid()}"
ENV = {k: v for k, v in os.environ.items() if not k.startswith("HERDR_")}
os.makedirs(OUT, exist_ok=True)
results = []

FEED = r'''#!/usr/bin/env python3
"""A fake card feed for Kerdr's end-to-end check of following (intent 0591): one card, #91,
whose conversation runs in foot outside herdr until the fake backend has moved it in."""
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
state_path = os.path.join(here, "state.json")
try:
    state = json.load(open(state_path))
except (OSError, ValueError):
    state = {}
session = state.get("session") or os.environ.get("E2E_SESSION", "")
card = {
    "id": "0091", "ref": "#91", "title": "Follow check card", "stage": "build", "state": "active",
    "home": {"session": session}, "resume": {"agent": "codex", "conversation": "c-91"},
    "attention": "idle", "status": "Under way",
}
if state.get("workspace"):
    card["workspace"] = {"session": session, "id": state["workspace"], "label": "91-e2e"}
elif state.get("outside", True):
    card["outside_herdr"] = {"terminal": "foot", "since": "2026-11-08T09:00:00+00:00"}
print(json.dumps({"kerdr_feed": 1, "source": "e2e", "sessions": {session: {"used": 0, "limit": 3}},
                  "cards": [card], "warnings": []}))
'''

BACKEND = r'''#!/usr/bin/env python3
"""A fake action backend for Kerdr's end-to-end check of following (intent 0591). Its scenario
file says how it answers: follow-done, follow-failed or legacy (no follow at all)."""
import json, os, subprocess, sys, time
here = os.path.dirname(os.path.abspath(__file__))
state_path = os.path.join(here, "state.json")
TOKEN = "f-0123456789abcdef0123"

def load():
    try:
        return json.load(open(state_path))
    except (OSError, ValueError):
        return {}

def save(s):
    tmp = state_path + ".tmp"
    json.dump(s, open(tmp, "w"))
    os.replace(tmp, state_path)

def workspace(s):
    if s.get("workspace"):
        return s["workspace"]
    env = {k: v for k, v in os.environ.items() if not k.startswith("HERDR_")}
    out = subprocess.run(["herdr", "--session", s["session"], "workspace", "create", "--label", "91-e2e",
                          "--no-focus"], capture_output=True, text=True, env=env).stdout
    import re
    m = re.search(r'"workspace_id":"([^"]+)"', out)
    if not m:
        raise SystemExit(json.dumps({"ok": False, "message": "couldn't make the workspace: " + out[-200:]}))
    w = m.group(1)
    s["workspace"] = w
    save(s)
    return w

request = json.loads(sys.stdin.read())
s = load()
scenario = s.get("scenario", "follow-done")
with open(os.path.join(here, "requests.log"), "a") as log:
    log.write(json.dumps({"t": round(time.time(), 1), "request": request}) + "\n")
action = request.get("action")
if action == "reopen":
    if scenario == "legacy":
        w = workspace(s)
        print(json.dumps({"ok": True, "message": "Reopened #91 in herdr.", "id": "0091", "session": s["session"],
                          "workspace": w, "label": "91-e2e"}))
    else:
        s["began"] = time.time()
        save(s)
        print(json.dumps({"ok": True, "id": "0091", "session": s["session"],
                          "message": "Moving #91's conversation into herdr; ⏎ again in a minute goes in.",
                          "follow": {"token": TOKEN, "what": "move-in", "id": "0091", "session": s["session"],
                                     "say": "Moving #91's conversation into herdr…", "limit": 60}}))
elif action == "progress":
    if request.get("follow") != TOKEN:
        print(json.dumps({"ok": False, "state": "unknown", "message": "No step by that token."}))
    else:
        took = time.time() - s.get("began", time.time())
        if took < 6:
            print(json.dumps({"ok": True, "state": "running", "say": f"Waiting for #91's turn to end ({int(took)} s)…"}))
        elif scenario == "follow-failed":
            print(json.dumps({"ok": False, "state": "failed",
                              "message": "#91's agent outside herdr wasn't stopped, and nothing was changed: its turn didn't end within a minute."}))
        else:
            w = workspace(s)
            s["outside"] = False
            save(s)
            print(json.dumps({"ok": True, "state": "done", "message": "Moved #91's conversation into herdr.",
                              "id": "0091", "session": s["session"], "workspace": w, "label": "91-e2e"}))
else:
    print(json.dumps({"ok": False, "message": f"The e2e backend doesn't do {action}."}))
'''

def h(*args, check=True):
    r = subprocess.run(["herdr", "--session", S, *args], capture_output=True, text=True, env=ENV, timeout=30)
    if check and r.returncode:
        raise RuntimeError(f"herdr {' '.join(args)}: {r.stderr.strip() or r.stdout.strip()}")
    return r.stdout


def screen(pane):
    return h("pane", "read", pane, "--source", "visible", check=False)


def wait_for(pane, pattern, seconds):
    end = time.time() + seconds
    last = ""
    while time.time() < end:
        last = screen(pane)
        if re.search(pattern, last):
            return last
        time.sleep(0.5)
    raise AssertionError(f"never saw {pattern!r}; last screen:\n{last}")


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("PASS " if ok else "FAIL ") + name + (f": {detail}" if detail and not ok else ""), flush=True)


def requests(folder):
    try:
        return [json.loads(l)["request"] for l in open(os.path.join(folder, "requests.log"))]
    except OSError:
        return []


def scenario(name):
    folder = os.path.join(OUT, name)
    shutil.rmtree(folder, ignore_errors=True)
    os.makedirs(folder)
    open(os.path.join(folder, "feed.py"), "w").write(FEED)
    open(os.path.join(folder, "act.py"), "w").write(BACKEND)
    json.dump({"scenario": name, "session": S, "outside": True}, open(os.path.join(folder, "state.json"), "w"))
    with open(os.path.join(folder, "config.toml"), "w") as c:
        c.write(f'[machines]\npicker = false\n[board]\nonly = ["{S}"]\n'
                f'[feed]\ncommand = "python3 {folder}/feed.py"\nevery = 2\n'
                f'[actions]\ncommand = "python3 {folder}/act.py"\n')
    made = json.loads(h("workspace", "create", "--label", f"00-kerdr-{name}", "--no-focus", "--cwd", folder))
    w = made["result"]["workspace"]["workspace_id"]
    pane = made["result"]["root_pane"]["pane_id"]
    # A helper's pane, so tools that list agents leave it out.
    h("pane", "report-metadata", pane, "--source", "arkship.ark-helpers", "--token", "role=helper", check=False)
    unset = " ".join(f"-u {k}" for k in ("HERDR_ENV", "HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_TAB_ID",
                                          "HERDR_WORKSPACE_ID", "HERDR_SESSION", "HERDR_BIN_PATH", "HERDR_CONFIG_PATH"))
    h("pane", "send-text", pane, f"env {unset} KERDR_CONFIG={folder}/config.toml {KERDR}\n")
    return folder, w, pane


def workspaces():
    return json.loads(h("workspace", "list"))["result"]["workspaces"]


def focused_label():
    return next((w["label"] for w in workspaces() if w.get("focused")), None)


def run_follow_done():
    folder, w, pane = scenario("follow-done")
    first = wait_for(pane, r"#91", 30)
    try:
        # The card is narrow: "outside herdr · foot" may be cut short.
        first = wait_for(pane, r"outside her", 10)
        shown = True
    except AssertionError:
        shown = False
    open(os.path.join(folder, "screen-board.txt"), "w").write(first)
    check("follow-done: the board shows #91 running outside herdr", shown, first)
    check("follow-done: its hint says Enter moves it into herdr", "⏎ move it into herdr" in first, first)
    h("pane", "send-keys", pane, "Enter")
    moving = wait_for(pane, r"Moving #91|Waiting for #91", 10)
    open(os.path.join(folder, "screen-moving.txt"), "w").write(moving)
    check("follow-done: Enter says it's moving #91 into herdr", re.search(r"Moving #91|Waiting for #91", moving))
    running = wait_for(pane, r"Waiting for #91's turn to end", 15)
    open(os.path.join(folder, "screen-running.txt"), "w").write(running)
    check("follow-done: a running answer's words show", True)
    # A second Enter meanwhile sends nothing more.
    h("pane", "send-keys", pane, "Enter")
    time.sleep(1.5)
    reopens = [r for r in requests(folder) if r["action"] == "reopen"]
    check("follow-done: one reopen, even after a second Enter", len(reopens) == 1, f"{reopens}")
    end = time.time() + 30
    label = None
    while time.time() < end:
        label = focused_label()
        if label == "91-e2e":
            break
        time.sleep(0.5)
    sent = requests(folder)
    open(os.path.join(folder, "requests.json"), "w").write(json.dumps(sent, indent=1))
    progress = [r for r in sent if r["action"] == "progress"]
    check("follow-done: Kerdr asked how it stands with progress", len(progress) >= 2 and
          all(r == {"action": "progress", "follow": "f-0123456789abcdef0123"} for r in progress), f"{progress}")
    check("follow-done: on done Kerdr went into 91-e2e", label == "91-e2e", f"focused {label}")
    open(os.path.join(folder, "screen-after.txt"), "w").write(screen(pane))


def run_follow_failed():
    folder, w, pane = scenario("follow-failed")
    wait_for(pane, r"#91", 30)
    h("pane", "send-keys", pane, "Enter")
    failed = wait_for(pane, r"wasn't stopped", 25)
    open(os.path.join(folder, "screen-failed.txt"), "w").write(failed)
    check("follow-failed: the failure shows in plain words", "#91 didn't move into herdr" in failed and "Traceback" not in failed, failed)
    check("follow-failed: nothing was gone into", focused_label() != "91-e2e", f"focused {focused_label()}")
    open(os.path.join(folder, "requests.json"), "w").write(json.dumps(requests(folder), indent=1))


def run_legacy():
    folder, w, pane = scenario("legacy")
    wait_for(pane, r"#91", 30)
    h("pane", "send-keys", pane, "Enter")
    end = time.time() + 20
    label = None
    while time.time() < end:
        label = focused_label()
        if label == "91-e2e":
            break
        time.sleep(0.5)
    sent = requests(folder)
    open(os.path.join(folder, "requests.json"), "w").write(json.dumps(sent, indent=1))
    check("legacy: a backend without follow is asked no progress", not [r for r in sent if r["action"] == "progress"])
    check("legacy: its answer's workspace is gone into, as before", label == "91-e2e", f"focused {label}")


def main():
    subprocess.run(["herdr", "--session", S, "remote-client-bridge"], stdin=subprocess.DEVNULL,
                   capture_output=True, env=ENV, timeout=30)
    end = time.time() + 20
    while subprocess.run(["herdr", "--session", S, "workspace", "list"], capture_output=True, env=ENV).returncode:
        if time.time() > end:
            raise SystemExit(f"the {S} herdr server didn't start")
        time.sleep(0.3)
    try:
        for step in (run_follow_done, run_follow_failed, run_legacy):
            # Each scenario has its own workspace; going into 91-e2e is only checked by focus.
            for ws in workspaces():
                if ws["label"] == "91-e2e":
                    h("workspace", "close", ws["workspace_id"], check=False)
            before = {ws["workspace_id"] for ws in workspaces()}
            try:
                step()
            except Exception as e:  # noqa: BLE001
                check(step.__name__, False, repr(e))
            # Its Kerdr, and the herdr client Kerdr opened going in, end with its workspace: an
            # attached client would size every pane after it to fit inside itself.
            for ws in workspaces():
                if ws["workspace_id"] not in before or ws["label"] == "91-e2e":
                    h("workspace", "close", ws["workspace_id"], check=False)
            time.sleep(1)
    finally:
        subprocess.run(["herdr", "session", "stop", S], capture_output=True, env=ENV)
        subprocess.run(["herdr", "session", "delete", S], capture_output=True, env=ENV)
    failed = [r for r in results if not r[1]]
    print(f"{len(results) - len(failed)} passed, {len(failed)} failed; screens and requests in {OUT}", flush=True)
    raise SystemExit(1 if failed else 0)


main()
