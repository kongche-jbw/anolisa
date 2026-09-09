"""Observe completed native history independently and publish verified session totals."""

import json
from pathlib import Path
import subprocess
import sys
import time

from session_hooks import read, write
import provider_details

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "integrations/herdr"))
import bridge


def history_ready(native: dict) -> bool:
    path = Path(native["transcript_path"])
    if not path.exists():
        return False
    with path.open("rb") as stream:
        raw = stream.read(64 * 1024 * 1024 + 1)
    if len(raw) > 64 * 1024 * 1024:
        raise ValueError("native history exceeds 64 MiB")
    for line in raw.splitlines(keepends=True):
        if not line.endswith(b"\n"):
            continue
        row = json.loads(line)
        if row.get("sessionId") != native["session_id"] or row.get("type") != "user":
            continue
        blocks = row.get("message", {}).get("content")
        if isinstance(blocks, list) and any(
            item.get("type") == "tool_result" and item.get("tool_use_id") == native["tool_use_id"]
            for item in blocks
            if isinstance(item, dict)
        ):
            return True
    return False


class Observer:
    def __init__(self, root: Path, socket: Path, pane: str, bins: Path):
        self.root, self.socket, self.pane, self.bins = root, socket, pane, bins
        self.finished: set[str] = set()
        self.bindings: dict[str, str] = {}
        self.reported_session = None
        self.sequence = time.monotonic_ns()

    def refresh(self) -> None:
        self.sequence += 1
        config = read(self.root / "runtime.json")
        state = read(self.root / "state.json")
        if state["session_id"] != config["session_id"]:
            tokens = {
                **{key: None for key in bridge.TOKEN_NAMES},
                "aw": "AW unavailable: session changed",
                "aw_sec": "SecCore: unverified",
                "aw_tokenless": "Tokenless: unverified",
                "aw_usage": "Restart AW launcher | Ctrl+B Q: exit",
            }
            bridge.publish(self.socket, self.pane, tokens, self.sequence)
            (self.root / "view.json").unlink(missing_ok=True)
            write(self.root / "display.json", tokens)
            write(
                self.root / "provider-details.json", {"status": "session changed; restart required"}
            )
            return
        for call in sorted((self.root / "calls").iterdir()):
            if call.name in self.finished or not (call / "completed.json").exists():
                continue
            completed = read(call / "completed.json")
            if completed["returncode"]:
                self.finished.add(call.name)
                write(call / "observation.json", {"status": "unhandled"})
                continue
            try:
                if not history_ready(read(call / "native.json")):
                    # A parallel tool can wait on a human permission prompt before
                    # Qoder persists the whole batch. The session deadline bounds polling.
                    continue
                self.finished.add(call.name)
                key = completed["event_key"]
                binding = str(call / "adoption-binding.json")
                for action in ("record", "verify"):
                    result = subprocess.run(
                        [str(self.bins / "aw-adoption-cli"), action, binding, key],
                        check=True,
                        capture_output=True,
                        timeout=10,
                    )
                    (call / f"adoption-{action}.json").write_bytes(result.stdout)
                self.bindings[key] = binding
                write(call / "observation.json", {"status": "verified"})
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                self.finished.add(call.name)
                write(call / "observation.json", {"status": "unverified", "error": str(error)})
                if isinstance(error, subprocess.CalledProcessError) and error.stderr:
                    (call / "verification-error.log").write_bytes(error.stderr)
        session = state["session_id"]
        if self.reported_session != session:
            bridge.rpc(
                self.socket,
                "pane.report_agent_session",
                {
                    "pane_id": self.pane,
                    "source": "herdr:qodercli",
                    "seq": self.sequence,
                    "session_start_source": state.get("session_start_source", "startup"),
                    "agent": "qodercli",
                    "agent_session_id": session,
                },
            )
            self.reported_session = session
        binding = {
            key: config["aw"][key]
            for key in ("runtime", "scope", "agent_pid", "agent_start_ticks", "journal")
        }
        binding["scope"] = {**binding["scope"], "session_id": session}
        binding.update(evidence=str(self.root / "evidence"), adoption_bindings=self.bindings)
        write(self.root / "view-binding.json", binding)
        failures = sum(
            read(path).get("session_id") in (None, session)
            for path in (self.root / "errors").glob("*.json")
        )
        failures += sum(
            read(path)["status"] != "verified"
            for path in (self.root / "calls").glob("*/observation.json")
            if read(path.parent / "before.json")["session_id"] == session
        )
        pending = sum(
            path.parent.name not in self.finished
            and read(path.parent / "before.json")["session_id"] == session
            for path in (self.root / "calls").glob("*/completed.json")
        )
        # Rust verifies all counters; Python adds only coverage/exit instructions.
        try:
            bridge.verify_pane(
                binding,
                bridge.rpc(self.socket, "pane.get", {"pane_id": self.pane}),
                bridge.rpc(self.socket, "pane.process_info", {"pane_id": self.pane}),
            )
            result = subprocess.run(
                [str(self.bins / "aw-view-cli"), str(self.root / "view-binding.json")],
                check=True,
                capture_output=True,
                timeout=10,
            )
            view = json.loads(result.stdout)
            tokens = bridge.format_view(view, binding["scope"])
            details = provider_details.snapshot(self.root, config, view)
            tokens.update(provider_details.sidebar(details))
            bridge.verify_pane(
                binding,
                bridge.rpc(self.socket, "pane.get", {"pane_id": self.pane}),
                bridge.rpc(self.socket, "pane.process_info", {"pane_id": self.pane}),
            )
            tokens["aw_usage"] = (
                f"Bash | {pending} pending | {failures} unverified"
                if pending or failures
                else "Ctrl+B P: Providers | Q: exit"
            )
            bridge.publish(self.socket, self.pane, tokens, self.sequence)
            write(self.root / "view.json", view)
            write(self.root / "display.json", tokens)
            write(self.root / "provider-details.json", details)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            bridge.publish(
                self.socket,
                self.pane,
                {
                    **{key: None for key in bridge.TOKEN_NAMES},
                    "aw": "AW unknown",
                    "aw_sec": "SecCore: unverified",
                    "aw_tokenless": "Tokenless: unverified",
                    "aw_usage": "Bash only | Ctrl+B Q: exit",
                },
                self.sequence,
            )
            write(self.root / "view-error.json", {"message": str(error)})
            write(self.root / "provider-details.json", {"status": "verification unavailable"})


if __name__ == "__main__":
    root, endpoint, pane, bins = (
        Path(sys.argv[1]),
        Path(sys.argv[2]),
        sys.argv[3],
        Path(sys.argv[4]),
    )
    observer = Observer(root, endpoint, pane, bins)
    deadline = time.monotonic() + int(sys.argv[5])
    while time.monotonic() < deadline:
        observer.refresh()
        time.sleep(1)
