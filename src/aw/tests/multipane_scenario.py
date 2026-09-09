"""Live multi-pane acceptance used by the cosh terminal harness."""

import os
from pathlib import Path
import select
import signal
import time

from session_hooks import read
from session_observer import bridge
from session import live


def run(master: int, group: Path, work: Path, screen) -> dict:
    owner = read(group / "ownership.json")
    endpoint, first = Path(owner["socket"]), owner["pane_id"]
    deadline = time.monotonic() + 220
    (work / "credential.env").write_text("api_key=sk-abcdefghijklmnopqrstuvwxyz123456\n")

    def wait_for(predicate, description):
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                screen.write(os.read(master, 65536))
                screen.flush()
            result = predicate()
            if result:
                return result
        raise TimeoutError(f"multi-pane acceptance timed out: {description}")

    def visible(pane):
        result = bridge.rpc(endpoint, "pane.read", {"pane_id": pane, "source": "visible"})
        return result.get("read", result).get("text", "")

    def send(pane, text):
        current = bridge.rpc(endpoint, "pane.current", {})["pane"]["pane_id"]
        assert current == pane, (current, pane)
        if text:
            time.sleep(1)
            os.write(master, text.encode())
        time.sleep(0.3)
        os.write(master, b"\r")

    def ready(pane):
        text = visible(pane)
        if "1. Trust folder" in text:
            assert work.parent.name in text
            send(pane, "")
            return False
        return "Qoder" in text and "Type your message" in text and "Trust folder" not in text

    def details(root, count):
        path = root / "provider-details.json"
        if not path.exists():
            return None
        value = read(path)
        return (
            value if value.get("status") == "verified" and value["bash_results"] == count else None
        )

    def idle(pane):
        return "idle" in str(bridge.rpc(endpoint, "pane.get", {"pane_id": pane}))

    def attached(pane):
        entries = [read(p) for p in (group / "panels").glob("*.json")]
        for entry in entries:
            root = Path(entry["root"])
            if entry["pane"] != pane or not (root / "runtime.json").exists():
                continue
            runtime = read(root / "runtime.json")
            if live(runtime["agent_pid"], runtime["agent_start_ticks"]):
                return root
        return None

    wait_for(lambda: ready(first), "first Qoder readiness")
    send(
        first,
        "Use Bash to execute exactly `cat fixture.json`. Report the first id. Use no other tools.",
    )
    wait_for(lambda: details(group, 1) and idle(first), "first Qoder independent result")
    os.write(master, b"\x02")
    time.sleep(0.1)
    os.write(master, b"v")

    def second_pane():
        panes = bridge.rpc(endpoint, "pane.list", {})["panes"]
        return next((p["pane_id"] for p in panes if p["pane_id"] != first), None)

    second = wait_for(second_pane, "native split shortcut")
    time.sleep(0.5)
    send(second, "qodercli")
    second_root = wait_for(lambda: attached(second), "second qodercli AW registration")
    wait_for(lambda: ready(second), "second Qoder readiness")
    send(
        second,
        "Use Bash to execute exactly `cat credential.env`. Report whether the hook flags sensitive content. Use no other tools.",
    )
    sensitive = wait_for(
        lambda: details(second_root, 1) and idle(second), "second Qoder sensitive inspection"
    )
    sensitive = details(second_root, 1)
    assert any(c.get("verdict") == "sensitive" for c in sensitive["recent_calls"])
    first_details = details(group, 1)
    assert first_details and first_details["session_id"] != sensitive["session_id"]
    assert all(c.get("verdict") != "sensitive" for c in first_details["recent_calls"])
    for pane in (first, second):
        assert (
            "1 Bash results" in bridge.rpc(endpoint, "pane.get", {"pane_id": pane})["tokens"]["aw"]
        )

    os.write(master, b"\x02")
    time.sleep(0.1)
    os.write(master, b"p")
    wait_for(lambda: list(second_root.glob("provider-panel-*.json")), "focused Provider popup")
    assert not list(group.glob("provider-panel-*.json")), (
        "popup incorrectly targeted the first agent"
    )
    time.sleep(0.5)
    os.write(master, b"q")
    first_runtime = read(group / "runtime.json")
    bridge.rpc(endpoint, "pane.close", {"pane_id": first})
    wait_for(
        lambda: not live(first_runtime["agent_pid"], first_runtime["agent_start_ticks"]),
        "first agent exit",
    )
    assert Path(f"/proc/{owner['server_pid']}").exists()
    send(
        second,
        "Use Bash to execute exactly `cat fixture.json`. Report the record count. Use no other tools.",
    )
    wait_for(lambda: details(second_root, 2) and idle(second), "second Qoder after first exit")
    second_runtime = read(second_root / "runtime.json")
    os.kill(second_runtime["agent_pid"], signal.SIGTERM)
    wait_for(
        lambda: not live(second_runtime["agent_pid"], second_runtime["agent_start_ticks"]),
        "second agent exit",
    )
    time.sleep(1)
    send(second, "qoder")
    third_root = wait_for(lambda: attached(second), "new session in reused pane")
    assert third_root != second_root
    wait_for(lambda: ready(second), "restarted Qoder readiness")
    send(
        second,
        "Use Bash to execute exactly `cat fixture.json`. Report the first id. Use no other tools.",
    )
    third = wait_for(lambda: details(third_root, 1) and idle(second), "reused pane fresh counters")
    assert "1 Bash results" in bridge.rpc(endpoint, "pane.get", {"pane_id": second})["tokens"]["aw"]
    os.write(master, b"\x02")
    time.sleep(0.1)
    os.write(master, b"q")
    return {
        "status": "passed",
        "group": str(group),
        "attached_sessions": [str(second_root), str(third_root)],
        "multi_pane": "qodercli and qoder independently inspected; focused popup; first pane close preserved second; pane reuse reset statistics",
    }
