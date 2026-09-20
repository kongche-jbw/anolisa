"""Fixed Herdr and synthetic Qoder; product startup must not need this script."""

import ctypes
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import signal
import socket
import struct
import sys
import tempfile
import termios
import time


BINARY = Path(sys.argv[1]).resolve(strict=True)
HERDR = Path(sys.argv[2]).resolve(strict=True)
FIXTURE = Path(__file__).with_name("aw_fixture.py")
spec = importlib.util.spec_from_file_location("aw_synthetic_peer", FIXTURE)
peer = importlib.util.module_from_spec(spec)
saved_argv = sys.argv[:]
sys.argv = [str(FIXTURE), str(BINARY)]
spec.loader.exec_module(peer)
sys.argv = saved_argv

UPSTREAM = Path(__file__).resolve().parents[5] / "aw/integrations/herdr/upstream.json"
PIN = json.loads(UPSTREAM.read_text())["assets"][platform.machine()]["sha256"]
assert hashlib.sha256(HERDR.read_bytes()).hexdigest() == PIN, "Herdr differs from official pin"
TASK = Path(tempfile.mkdtemp(prefix="awh-", dir=BINARY.parent.parent))
PANES = []
REGISTERED = {}


def identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return fields[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def register():
    roots = [pane.pid for pane in PANES]
    # This fixture is a subreaper and only starts its own test trees.
    roots.extend(int(pid) for pid in Path(f"/proc/{os.getpid()}/task/{os.getpid()}/children").read_text().split())
    for root in roots:
        try:
            children = peer.descendants(root)
        except ProcessLookupError:
            continue
        for pid, ticks in children:
            if (pid, ticks) in REGISTERED:
                continue
            try:
                command = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
                cwd = os.readlink(f"/proc/{pid}/cwd")
            except (FileNotFoundError, ProcessLookupError):
                command, cwd = "exited during registration", "unknown"
            row = {"pid": pid, "ticks": ticks, "command": command, "cwd": cwd,
                   "lifetime": "one bounded fixture case", "ports": [],
                   "stop": f"kill -TERM {pid}; wait {pid}"}
            REGISTERED[pid, ticks] = row
            with (TASK / "processes.jsonl").open("a") as output:
                output.write(json.dumps(row) + "\n")


def wait(predicate, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for pane in PANES:
            if not pane.reaped:
                pane.drain()
        register()
        result = predicate()
        if result:
            return result
        time.sleep(0.025)
    raise AssertionError("bounded Herdr fixture wait expired")


def start(name, digest=PIN):
    os.environ.update(COSH_AW_HERDR=str(HERDR), COSH_AW_HERDR_SHA256=digest,
                      TERM="xterm-256color", PYTHONDONTWRITEBYTECODE="1")
    pane = peer.Pane(TASK / name, owner=True)
    PANES.append(pane)
    fcntl.ioctl(pane.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
    wait(lambda: b"AWTEST>" in pane.output)
    pane.send('AW_TEST_LOCAL=preserved; printf "%s\\n%s\\n%s\\n" "$$" "$PWD" "$COSH_AW_ROOT" > outer-before\n')
    wait(lambda: (pane.root / "outer-before").exists())
    before = (pane.root / "outer-before").read_text().splitlines()
    assert len(before) == 3
    assert not (pane.root / "ready.json").exists()
    assert not [row for row in REGISTERED.values()
                if row["command"].startswith(str(HERDR) + " ") and identity(row["pid"]) == row["ticks"]], "ordinary shell started Herdr"
    return pane, before


def finish_shell(pane, before):
    pane.send("exit\n")
    wait(lambda: os.waitpid(pane.pid, os.WNOHANG)[0])
    pane.reaped = True
    os.close(pane.fd)
    assert not Path(before[2]).exists(), "outer AW scope survived normal exit"


def snapshot(pane, suffix):
    pane.send(f'printf "%s\\n%s\\n%s\\n" "$$" "$PWD" "$AW_TEST_LOCAL" > outer-{suffix}\n')
    path = pane.root / f"outer-{suffix}"
    wait(path.exists)
    return path.read_text().splitlines()


def metadata(ready):
    session = Path(ready["root"]).parents[2]
    descriptor = json.loads((session / "launch.json").read_text())
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(1)
        client.connect(descriptor["socket"])
        client.sendall(b'{"id":"fixture","method":"pane.list","params":{}}\n')
        data = bytearray()
        for _ in range(128):
            data.extend(client.recv(8192))
            if b"\n" in data:
                response = json.loads(data.split(b"\n", 1)[0])
                assert response["id"] == "fixture"
                panes = response["result"]["panes"]
                assert len(panes) == 1
                tokens = panes[0]["tokens"]
                return tokens if "callback_observed" in tokens.get("aw", "") else None
        raise AssertionError("bounded metadata reply exceeded")


def launch_and_return(pane, before, sequence):
    ready_path = pane.root / "ready.json"
    ready_path.unlink(missing_ok=True)
    prompt = "two words ' \" ; $(touch aw-injected-dollar) `touch aw-injected-backtick`"
    argv = ["--model", "auto", prompt]
    output_path = pane.root / f"exit-{sequence}"
    pane.send("qoder " + " ".join(map(shlex.quote, argv))
              + f'; printf "%s\\n" "$?" > exit-{sequence}\n')
    wait(ready_path.exists, timeout=25)
    ready = json.loads(ready_path.read_text())
    assert ready["tty"] and ready["argv"][-3:] == argv, ready
    assert not (pane.root / "aw-injected-dollar").exists()
    assert not (pane.root / "aw-injected-backtick").exists()
    binding = json.loads((Path(ready["root"]) / "binding.json").read_text())
    inner_owner = binding["prepared"]["owner_pid"]
    assert inner_owner != pane.pid, "Herdr pane reused outer AW owner"
    native_stat = Path(f'/proc/{ready["pid"]}/stat').read_text().rsplit(")", 1)[1].split()
    native_parent = int(native_stat[1])
    parent_stat = Path(f"/proc/{native_parent}/stat").read_text().rsplit(")", 1)[1].split()
    assert int(parent_stat[1]) == inner_owner, "native Bash lost sole Qoder wait/reap ownership"
    viewers = [row for row in REGISTERED.values()
               if row["command"].startswith(str(HERDR) + " ") and identity(row["pid"]) == row["ticks"]]
    assert viewers, "Qoder did not start Herdr"
    tokens = wait(lambda: metadata(ready), timeout=6)
    register()
    pane.send("quit\n")
    wait(output_path.exists, timeout=20)
    assert output_path.read_text().strip() == "7", "native exit status was replaced by viewer status"
    assert snapshot(pane, str(sequence)) == before[:2] + ["preserved"]
    assert not Path(ready["root"]).exists(), "inner AW scope survived pane exit"
    assert not Path(f'/proc/{ready["pid"]}').exists(), "native Qoder survived return"
    assert not [row["pid"] for row in viewers if identity(row["pid"]) == row["ticks"]], "Herdr survived shell return"
    (TASK / f"launch-{sequence}.json").write_text(json.dumps({
        "outer_owner": pane.pid, "outer_bash": int(before[0]),
        "inner_owner": inner_owner, "inner_bash": native_parent,
        "native_pid": ready["pid"], "argv_preserved": True, "exit_code": 7, "metadata": tokens,
    }, indent=2) + "\n")
    return inner_owner


def interrupted_session(name, hangup=False):
    pane, before = start(name)
    pane.send('qoder; printf "%s\\n" "$?" > interrupt-status\n')
    wait(lambda: (pane.root / "ready.json").exists(), timeout=25)
    ready = json.loads((pane.root / "ready.json").read_text())
    descriptors = [Path(ready["root"]).parents[2] / "launch.json"]
    assert len(descriptors) == 1, "expected one owned Herdr session"
    descriptor = json.loads(descriptors[0].read_text())
    launcher = descriptor["launcher_pid"]
    ticks = str(descriptor["launcher_start_ticks"])
    assert identity(launcher) == ticks, "launcher identity changed before interruption"
    stat = Path(f"/proc/{launcher}/stat").read_text().rsplit(")", 1)[1].split()
    assert int(stat[1]) == int(before[0]), "launcher does not belong to outer Bash"
    register()
    owned = dict(peer.descendants(launcher))
    assert ready["pid"] in owned, "native process absent from owned session tree"
    if hangup:
        # Target precisely the registered foreground launcher, not the outer shell.
        os.kill(launcher, signal.SIGHUP)
    else:
        pane.send("\x03")
    status_path = pane.root / "interrupt-status"
    wait(status_path.exists, timeout=20)
    status = int(status_path.read_text())
    assert (status != 0 if hangup else status == 130), f"unexpected interruption status: {status}"
    assert snapshot(pane, "interrupted") == before[:2] + ["preserved"]

    def no_owned_processes():
        remaining = []
        for pid, expected in owned.items():
            if identity(pid) != expected:
                continue
            # The fixture's subreaper may inherit already-dead grandchildren.
            # Reap them without signalling; never mask a live product leak.
            try:
                os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                pass
            if identity(pid) == expected:
                remaining.append(pid)
        return not remaining

    wait(no_owned_processes, timeout=4)
    assert not descriptors[0].parent.exists(), "Herdr private session survived interruption"
    assert not Path(ready["root"]).exists(), "AW pane scope survived interruption"
    (TASK / f"interrupt-{name}.json").write_text(json.dumps({
        "signal": "SIGHUP" if hangup else "Ctrl-C",
        "launcher_pid": launcher, "launcher_start_ticks": ticks,
        "outer_bash": int(before[0]), "registered_session_pids": sorted(owned),
        "exit_code": status, "remaining_pids": [], "original_shell_restored": True,
    }, indent=2) + "\n")
    finish_shell(pane, before)


def startup_timeout():
    pane, before = start("e")
    (pane.home / ".bashrc").write_text("sleep 8\n")
    pane.send('export COSH_SHELL_ISOLATED=0; qoder; printf "%s\n" "$?" > startup-status\n')
    wait(lambda: (pane.root / "startup-status").exists(), timeout=16)
    assert int((pane.root / "startup-status").read_text()) != 0
    assert not (pane.root / "ready.json").exists(), "startup reader received Agent launch"
    assert snapshot(pane, "startup") == before[:2] + ["preserved"]
    assert not [row for row in REGISTERED.values()
                if row["command"].startswith(str(HERDR) + " ") and identity(row["pid"]) == row["ticks"]]
    (TASK / "startup-timeout.json").write_text(json.dumps({
        "agent_not_started": True, "original_shell_restored": True,
    }) + "\n")
    finish_shell(pane, before)


def cases():
    pane, before = start("a")
    first = launch_and_return(pane, before, 1)
    second = launch_and_return(pane, before, 2)
    assert first != second, "second invocation reused finished pane owner"
    finish_shell(pane, before)
    pane, before = start("b", "0" * 64)
    pane.send('qoder; printf "%s\\n" "$?" > refused-status\n')
    wait(lambda: (pane.root / "refused-status").exists())
    assert (pane.root / "refused-status").read_text().strip() != "0"
    assert not (pane.root / "ready.json").exists(), "wrong Herdr pin still started native Qoder"
    assert not [row for row in REGISTERED.values()
                if row["command"].startswith(str(HERDR) + " ") and identity(row["pid"]) == row["ticks"]], "wrong pin started Herdr"
    assert snapshot(pane, "refused") == before[:2] + ["preserved"]
    finish_shell(pane, before)
    interrupted_session("c")
    interrupted_session("d", hangup=True)
    startup_timeout()


def login_pane():
    pane, _ = start("login")
    profile = pane.root / "profile-count"
    bashrc = pane.root / "bashrc-count"
    (pane.home / ".bash_profile").write_text(
        f'printf "%s\\n" "$$" >> {shlex.quote(str(profile))}\nPS1="AWLOGIN> "\n'
    )
    (pane.home / ".bashrc").write_text(
        f'printf "bashrc\\n" >> {shlex.quote(str(bashrc))}\n'
    )
    # Re-enter through the actual login entry, retaining the fixture's private
    # HOME and AW configuration. The enclosing PTY still owns the whole tree.
    pane.send(f'export COSH_SHELL_ISOLATED=0; exec {shlex.quote(str(BINARY))} raw fake --login\n')
    wait(lambda: b"AWLOGIN>" in pane.output)
    pane.send('AW_TEST_LOCAL=preserved; printf "%s\\n%s\\n%s\\n" "$$" "$PWD" "$COSH_AW_ROOT" > login-before\n')
    wait(lambda: (pane.root / "login-before").exists())
    before = (pane.root / "login-before").read_text().splitlines()
    profiles_before = profile.read_text().splitlines()
    assert profiles_before.count(before[0]) == 1, "outer Bash must load its profile once"
    # Existing cosh PATH discovery also runs a login probe before the owner.
    # The new pane must add no profile execution to that established baseline.
    launch_and_return(pane, before, "login")
    counts = {"profiles_before": profiles_before, "profiles_after": profile.read_text().splitlines(),
              "bashrc": bashrc.read_text().splitlines() if bashrc.exists() else []}
    (TASK / "login-startup.json").write_text(json.dumps(counts, indent=2) + "\n")
    assert counts["profiles_after"] == profiles_before and counts["bashrc"] == ["bashrc"], counts
    finish_shell(pane, before)


def cleanup():
    register()
    for pane in PANES:
        if not pane.reaped:
            pane.drain()
        (TASK / f"terminal-{pane.root.name}.log").write_bytes(pane.output)
    for sig, duration in ((signal.SIGTERM, 1), (signal.SIGKILL, 2)):
        register()
        for pid, ticks in REGISTERED:
            if identity(pid) == ticks:
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            pending = []
            for pid, ticks in REGISTERED:
                if identity(pid) != ticks:
                    continue
                try:
                    os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    pass
                if identity(pid) == ticks:
                    pending.append(pid)
            if not pending:
                break
            time.sleep(0.02)
    remaining = [pid for pid, ticks in REGISTERED if identity(pid) == ticks]
    for pane in PANES:
        if not pane.reaped:
            os.close(pane.fd)
            pane.reaped = True
        shutil.rmtree(pane.root)
    (TASK / "cleanup.json").write_text(json.dumps({"registered": len(REGISTERED),
                                                 "remaining_pids": remaining}) + "\n")
    assert not remaining, f"owned processes survived cleanup: {remaining}"


if __name__ == "__main__":
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "fixture subreaper unavailable"

    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("fixed-Herdr fixture interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    print(f"Fixed-Herdr evidence: {TASK}", flush=True)
    try:
        if sys.argv[3:] == ["login"]:
            login_pane()
            print("PASS: login profile once, non-login pane bashrc, same shell return", flush=True)
        else:
            assert not sys.argv[3:], "unknown fixture case"
            cases()
            print("PASS: on-demand launch, native argv, same shell return, second launch, wrong pin, Ctrl-C, launcher HUP, startup timeout", flush=True)
    finally:
        cleanup()
