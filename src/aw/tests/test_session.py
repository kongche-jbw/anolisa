"""Check native lifecycle binding without making model requests."""

import json
import subprocess
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
import session_hooks as hooks
from session_observer import Observer, history_ready


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="session-unit-", dir=AW / "target")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ("calls", "turns"):
            (self.root / name).mkdir()
        hooks.write(self.root / "state.json", {"session_id": "s1", "turn_id": None, "sequence": 0})
        self.config = {"session_id": "s1", "agent_pid": 123, "agent_start_ticks": 456}

    def event(self, kind, tool="tool-1", session="s1"):
        return {
            "hook_event_name": kind,
            "session_id": session,
            "tool_use_id": tool,
            "tool_name": "Bash",
            "tool_input": {"command": "cat fixture.json"},
        }

    def transition(self, kind, tool="tool-1", session="s1"):
        return hooks.transition(self.root, self.config, self.event(kind, tool, session))

    def test_activation_keeps_native_arguments_after_launcher_options(self):
        result = subprocess.run(
            [
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                'source "$1" --allow-unrecoverable >/dev/null; '
                'python3() { printf "%s\\0" "$@"; }; '
                'qoder --model auto "prompt with spaces" --duration 20',
                "bash",
                str(AW / "scripts/activate.sh"),
            ],
            check=True,
            capture_output=True,
            timeout=5,
        )
        self.assertEqual(
            result.stdout.decode().split("\0")[:-1],
            [
                str(AW / "scripts/session.py"),
                "--allow-unrecoverable",
                "--",
                "--model",
                "auto",
                "prompt with spaces",
                "--duration",
                "20",
            ],
        )

    def test_tool_requires_observed_prompt(self):
        with self.assertRaisesRegex(ValueError, "no observed"):
            self.transition("PreToolUse")

    def test_parallel_tools_keep_their_original_turn_after_next_prompt(self):
        self.transition("UserPromptSubmit")
        first = hooks.read(self.root / "state.json")["turn_id"]
        self.transition("PreToolUse", "a")
        self.transition("PreToolUse", "b")
        self.transition("UserPromptSubmit")
        second = hooks.read(self.root / "state.json")["turn_id"]
        self.assertNotEqual(first, second)
        for tool in ("a", "b"):
            call = self.transition("PostToolUse", tool)
            self.assertEqual(hooks.read(call / "before.json")["turn_id"], first)
        self.transition("PreToolUse", "c")
        call = self.transition("PostToolUse", "c")
        self.assertEqual(hooks.read(call / "before.json")["turn_id"], second)

    def test_duplicate_post_and_changed_input_are_rejected(self):
        self.transition("UserPromptSubmit")
        self.transition("PreToolUse")
        changed = self.event("PostToolUse")
        changed["tool_input"] = {"command": "different"}
        with self.assertRaisesRegex(ValueError, "changed"):
            hooks.transition(self.root, self.config, changed)
        self.transition("PostToolUse")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.transition("PostToolUse")

    def test_new_session_resets_turn_and_rejects_unbound_session(self):
        self.transition("UserPromptSubmit")
        with self.assertRaisesRegex(ValueError, "differs"):
            self.transition("PreToolUse", session="s2")
        event = self.event("SessionStart", session="s2")
        event["source"] = "resume"
        with self.assertRaisesRegex(ValueError, "unexpected"):
            hooks.transition(self.root, self.config, event)
        event["source"] = "new"
        hooks.transition(self.root, self.config, event)
        self.assertIsNone(hooks.read(self.root / "state.json")["turn_id"])
        with self.assertRaisesRegex(ValueError, "restart"):
            self.transition("PreToolUse", session="s2")

    def test_history_waits_for_complete_matching_result_line(self):
        history = self.root / "history.jsonl"
        native = {"session_id": "s1", "tool_use_id": "t1", "transcript_path": str(history)}
        row = {
            "sessionId": "s1",
            "type": "user",
            "message": {
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "synthetic"}]
            },
        }
        history.write_text(json.dumps(row))
        self.assertFalse(history_ready(native))
        history.write_text(json.dumps(row) + "\n")
        self.assertTrue(history_ready(native))
        native["tool_use_id"] = "unrelated"
        self.assertFalse(history_ready(native))

    def test_native_failure_never_invokes_a_provider(self):
        self.transition("UserPromptSubmit")
        self.transition("PreToolUse")
        event = self.event("PostToolUseFailure")
        call = hooks.transition(self.root, self.config, event)
        self.assertEqual(hooks.invoke(self.root, {}, call, event), b"{}")
        self.assertEqual(hooks.read(call / "completed.json")["returncode"], 1)

    def observer(self):
        (self.root / "errors").mkdir()
        hooks.write(
            self.root / "runtime.json",
            {
                "agent_pid": 123,
                "session_id": "s1",
                "aw": {
                    "runtime": {},
                    "scope": {"session_id": "s1"},
                    "agent_pid": 123,
                    "agent_start_ticks": 456,
                    "journal": str(self.root / "journal"),
                },
            },
        )
        return Observer(self.root, self.root / "api.sock", "pane-1", self.root)

    def refresh(self, observer):
        with (
            patch("session_observer.bridge.rpc", return_value={}) as rpc,
            patch("session_observer.bridge.verify_pane") as verify,
            patch("session_observer.bridge.format_view", return_value={}),
            patch("session_observer.provider_details.snapshot", return_value={}),
            patch("session_observer.provider_details.sidebar", return_value={}),
            patch("session_observer.bridge.publish"),
            patch("session_observer.subprocess.run") as command,
        ):
            command.return_value.stdout = b"{}"
            observer.refresh()
            return rpc, verify, command

    def test_permission_wait_does_not_expire_unpersisted_history(self):
        observer = self.observer()
        self.transition("UserPromptSubmit")
        self.transition("PreToolUse")
        event = self.event("PostToolUse")
        event["transcript_path"] = str(self.root / "not-yet-persisted.jsonl")
        call = hooks.transition(self.root, self.config, event)
        hooks.write(call / "completed.json", {"returncode": 0, "finished_at_ms": 1})
        _, _, command = self.refresh(observer)
        self.assertFalse((call / "observation.json").exists())
        self.assertNotIn(call.name, observer.finished)
        self.assertEqual(command.call_count, 1)  # Only view; no premature adoption record.
        self.assertEqual(
            hooks.read(self.root / "display.json")["aw_usage"],
            "Bash | 1 pending | 0 unverified",
        )

    def test_new_clears_stale_view_without_claiming_herdr_rebinding(self):
        observer = self.observer()
        observer.reported_session = "s1"
        hooks.write(self.root / "view.json", {"scope": {"session_id": "s1"}})
        hooks.write(
            self.root / "state.json",
            {"session_id": "s2", "session_start_source": "new"},
        )
        rpc, verify, command = self.refresh(observer)
        rpc.assert_not_called()
        verify.assert_not_called()
        command.assert_not_called()
        self.assertFalse((self.root / "view.json").exists())
        self.assertIn("Restart", hooks.read(self.root / "display.json")["aw_usage"])


if __name__ == "__main__":
    unittest.main()
