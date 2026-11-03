#!/usr/bin/env python3
"""Runs a real Kerdr in a hidden terminal against a machine over real SSH, and checks that sessions
are held and released as the README says, and how long going in takes. CI has no second machine, so
run it by hand before a release that touches src/hold.rs or the jump:

    tests/live_holds.py <ssh host> [path to kerdr]

The host needs the same herdr version as this machine. It uses throwaway sessions named zz-holds-*
on the host, and deletes them after. Keep this machine free of sessions with those names."""
import fcntl, glob, os, pty, re, select, signal, struct, subprocess, sys, tempfile, termios, time

HOST = sys.argv[1]
KERDR = sys.argv[2] if len(sys.argv) > 2 else "target/debug/kerdr"
A, B = f"zz-holds-{os.getpid()}-a", f"zz-holds-{os.getpid()}-b"
HERDR = "HERDR_SOCKET_PATH HERDR_SESSION HERDR_PANE_ID HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_ENV HERDR_BIN_PATH".split()
PRE = f'PATH="$HOME/.local/bin:$PATH"; unset {" ".join(HERDR)}; '
failures = []


def there(cmd):
    return subprocess.run(["ssh", "-o", "BatchMode=yes", HOST, PRE + cmd], capture_output=True, text=True)


def token(session, *args):
    there(f"herdr --session {session} workspace report-metadata w1 --source kerdr " + " ".join(args))


def check(what, ok, detail=""):
    print(("ok    " if ok else "FAIL  ") + what + (f" ({detail})" if detail else ""))
    if not ok:
        failures.append(what)


for s in (A, B):
    # How herdr itself starts a server for another machine: `herdr --remote` asks to restart any other.
    there(f"herdr --session {s} remote-client-bridge </dev/null >/dev/null")
    there(f"herdr --session {s} workspace create --no-focus --label {s}")
    there(f"herdr --session {s} pane report-metadata w1:p1 --source kerdr --token role=helper")
    token(s, "--token kerdr_stage=test", f"--token kerdr_note={s}")

tmp = tempfile.mkdtemp(prefix="zz-holds-")
config = f"{tmp}/config.toml"
with open(config, "w") as f:
    f.write(f'[machines]\npicker=false\n[board]\nonly=["{A}","{B}"]\n[[machine]]\nname="there"\nssh="{HOST}"\n')
env = {k: v for k, v in os.environ.items() if k not in HERDR}
env.update(KERDR_CONFIG=config, TERM="xterm-256color", XDG_STATE_HOME=tmp, XDG_CACHE_HOME=tmp)
pid, fd = pty.fork()
if pid == 0:
    os.execvpe(KERDR, [KERDR], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 150, 0, 0))
os.kill(pid, signal.SIGWINCH)
screen = bytearray()


def pump(seconds):
    end = time.time() + seconds
    while time.time() < end:
        if select.select([fd], [], [], 0.02)[0]:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            screen.extend(data)
            if b"\x1b[6n" in data:
                os.write(fd, b"\x1b[1;1R")
            if b"\x1b[c" in data:
                os.write(fd, b"\x1b[?62;c")


FOLDER = (os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}") + "/kerdr"


def held():
    return len(glob.glob(f"{FOLDER}/h{pid}-*.sock"))


def children():
    return subprocess.run(["ps", "-o", "pid=,args=", "--ppid", str(pid)], capture_output=True, text=True).stdout


def until(ok, limit):
    begun = time.time()
    while time.time() - begun < limit:
        pump(0.1)
        if ok():
            return time.time() - begun
    return None


def enter():
    """Goes into the selected card in the misc list. How long the screen took, in seconds."""
    os.write(fd, b"i")
    pump(0.4)
    screen.clear()
    begun = time.time()
    os.write(fd, b"\r")
    while time.time() - begun < 15:
        pump(0.01)
        if len(screen) > 6000:
            return time.time() - begun
    return None


def leave():
    os.write(fd, b"\x00")
    pump(0.15)
    os.write(fd, b"q")
    pump(1.5)
    os.write(fd, b"\x1b")
    pump(0.3)


try:
    pump(5)
    check("nothing waits, nothing is held", held() == 0)
    token(A, "--token kerdr_status=needs_you")
    check("a card starts waiting: held", until(lambda: held() == 1, 15) is not None)
    token(A, "--clear-token kerdr_status")
    check("it stops waiting untouched: released", until(lambda: held() == 0, 15) is not None)
    token(A, "--token kerdr_status=needs_you")
    until(lambda: held() == 1, 15)
    pump(1)

    took = enter()
    check("going in uses the held connection", " client" in children() and "--remote" not in children())
    check("going in takes under 1.5 s", took is not None and took < 1.5, f"{took and round(took, 2)} s")
    os.write(fd, b"echo zz-typed\r")
    pump(1.5)
    check("what you type reaches the pane", "zz-typed" in there(f"herdr --session {A} pane read w1:p1 --source visible").stdout)
    token(B, "--token kerdr_status=needs_you")
    check("another session starts waiting while you're inside: held", until(lambda: held() == 2, 20) is not None)
    token(B, "--clear-token kerdr_status")
    check("it stops waiting while you're inside: released", until(lambda: held() == 1, 20) is not None)
    token(A, f"--token kerdr_jump=board:{int(time.time())}", "--ttl-ms 15000")
    check("Kerdr's key closes the window", until(lambda: " client" not in children(), 8) is not None)
    pump(1)
    os.write(fd, b"\x1b")
    pump(0.3)

    took = enter()
    leave()
    check("going back in within the minute is held too", took is not None and took < 1.5, f"{took and round(took, 2)} s")
    after = until(lambda: held() == 0, 70)
    check("released a minute after leaving, though it still shows as waiting", after is not None and after > 50, f"{after and round(after)} s")

    took = enter()
    check("with nothing held it goes in the usual way", "--remote" in children(), f"{took and round(took, 2)} s")
    leave()
    check("and that session is held for the minute after", until(lambda: held() == 1, 8) is not None)
    masters = [line.split()[0] for line in children().splitlines() if "ControlMaster=yes" in line]
    check("one connection", len(masters) == 1)
    os.kill(int(masters[0]), signal.SIGSTOP)
    took = enter()
    check("a connection that stopped answering goes the usual way", "--remote" in children(), f"{took and round(took, 2)} s")
    leave()
    if subprocess.run(["kill", "-0", masters[0]], capture_output=True).returncode == 0:
        os.kill(int(masters[0]), signal.SIGCONT)
    check("and it's replaced", until(lambda: held() == 1, 15) is not None)

    os.write(fd, b"q")
    pump(0.5)
    os.write(fd, b"q")
    pump(1.5)
    check("quitting leaves no socket behind", held() == 0 and not glob.glob(f"{FOLDER}/h{pid}-*"))
finally:
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass
    for s in (A, B):
        there(f"herdr session stop {s}; herdr session delete {s}")
    subprocess.run(["rm", "-rf", tmp])
print("FAILED: " + "; ".join(failures) if failures else "all passed")
raise SystemExit(1 if failures else 0)
