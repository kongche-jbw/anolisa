"""Bounded conversation transitions using native peers and actual AW binaries."""

import copy
import json
from pathlib import Path
import signal
import stat
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

import test_session as fixture

INTEGRATION = fixture.INTEGRATION

import conversation
import session


class ConversationTests(unittest.TestCase):
    # Reuse fixture setup without inheriting and rerunning SessionTests.
    tearDown = fixture.SessionTests.tearDown
    projection = fixture.SessionTests.projection

    def setUp(self):
        fixture.SessionTests.setUp(self)
        self.config.pop("prompt")
        self.conversation = {
            "format": 1,
            "launch": self.config,
            "turns": [{"prompt": "ok"}],
        }
        self.settings = self.root / "conversation-input.json"

    def run_conversation(self, settings=None):
        path = settings or self.settings
        if not path.exists():
            session.write(path, self.conversation)
        return subprocess.run(
            [sys.executable, "-B", str(INTEGRATION / "conversation.py"), str(path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            timeout=120,
        )

    def summary(self, root=None):
        return json.loads(((root or self.root / "run") / "summary.json").read_text())

    def test_resume_and_reset_preserve_distinct_turn_adoption(self):
        self.projection()
        self.conversation["turns"] = [
            {"prompt": "ok"},
            {"prompt": "ok"},
            {"prompt": "ok", "reset": True},
        ]
        result = self.run_conversation()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.summary()
        self.assertEqual(summary["status"], "completed")
        identities = [entry["identity"] for entry in summary["turns"]]
        self.assertEqual(identities[0]["session_id"], identities[1]["session_id"])
        self.assertNotEqual(identities[1]["session_id"], identities[2]["session_id"])
        self.assertEqual([value["resume"] for value in identities], [False, True, False])
        self.assertEqual(len({value["turn_id"] for value in identities}), 3)
        self.assertEqual(len({value["runtime_id"] for value in identities}), 1)
        self.assertEqual(len({value["environment_id"] for value in identities}), 1)
        for number, identity in enumerate(identities, 1):
            root = self.root / "run" / f"turn-{number:04d}"
            hook = json.loads((root / "hook.json").read_text())["hook"]
            scope = hook["scope"]
            self.assertEqual(scope["session_id"], identity["session_id"])
            self.assertEqual(scope["turn_id"], identity["turn_id"])
            self.assertEqual(scope["runtime_id"], summary["conversation_id"])
            self.assertEqual(scope["environment_id"], summary["conversation_id"])
            self.assertEqual(scope["runtime_generation"], number)
            self.assertEqual(hook["runtime"]["generation"], number)
            agent = json.loads((root / "agent.json").read_text())
            self.assertFalse(Path(f'/proc/{agent["pid"]}').exists())
            observations = json.loads((root / "result.json").read_text())["observations"]
            self.assertEqual(len(observations), 1)
            self.assertEqual(observations[0]["observation_status"], "adopted")
            self.assertTrue(observations[0]["returned"])
        self.assertNotIn("observations", json.dumps(summary))
        for name in (
            "conversation.json",
            "summary.json",
            "turn-0001/identity.json",
            "turn-0001/result.json",
        ):
            path = self.root / "run" / name
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600, str(path))
        self.assertEqual(stat.S_IMODE((self.root / "run").stat().st_mode), 0o700)

    def assert_failed_resume_stops(self, prompt):
        self.projection()
        self.conversation["turns"] = [
            {"prompt": "ok"},
            {"prompt": prompt},
            {"prompt": "ok", "reset": True},
        ]
        result = self.run_conversation()
        self.assertEqual(result.returncode, 4, result.stderr)
        summary = self.summary()
        self.assertEqual(summary["status"], "stopped")
        self.assertEqual([turn["status"] for turn in summary["turns"]], ["completed", "failed"])
        self.assertTrue(summary["turns"][1]["identity"]["resume"])
        self.assertEqual(
            json.loads((self.root / "run/turn-0002/result.json").read_text())["observations"], []
        )
        self.assertFalse((self.root / "run/turn-0003").exists())

    def test_failed_resume_stops_without_next_turn(self):
        self.assert_failed_resume_stops("wrong_session")

    def test_stale_turn_hook_cannot_act_for_resumed_agent(self):
        self.assert_failed_resume_stops("stale_turn")

    def test_timeout_keeps_attempted_evidence_and_stops(self):
        self.config["timeout_seconds"] = 1
        self.conversation["turns"] = [{"prompt": "timeout"}, {"prompt": "ok"}]
        result = self.run_conversation()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(len(self.summary()["turns"]), 1)
        self.assertTrue((self.root / "run/turn-0001/failure.json").exists())
        self.assertFalse((self.root / "run/turn-0002").exists())
        pid = int((self.work / "descendant.pid").read_text())
        self.assertFalse(Path(f"/proc/{pid}").exists())

    def test_cancellation_reaps_children_and_stops_conversation(self):
        self.conversation["turns"] = [{"prompt": "timeout"}, {"prompt": "ok"}]
        session.write(self.settings, self.conversation)
        child = subprocess.Popen(
            [sys.executable, "-B", str(INTEGRATION / "conversation.py"), str(self.settings)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 15
            while not (self.work / "descendant.pid").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.02)
            child.send_signal(signal.SIGTERM)
            self.assertEqual(child.wait(timeout=8), 1)
            self.assertEqual(self.summary()["status"], "stopped")
            self.assertEqual(len(self.summary()["turns"]), 1)
            self.assertFalse((self.root / "run/turn-0002").exists())
            pid = int((self.work / "descendant.pid").read_text())
            self.assertFalse(Path(f"/proc/{pid}").exists())
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)

    def test_invalid_later_turn_is_rejected_before_creating_root(self):
        variants = [
            {"turns": []},
            {"turns": [{"prompt": "ok"}] * 9},
            {"turns": [{"prompt": "ok"}, {"prompt": "ok", "reset": 1}]},
            {"turns": [{"prompt": "ok"}, {"prompt": ""}]},
            {"turns": [{"prompt": "ok", "session_id": "external"}]},
            {"format": True},
            {"unknown": True},
            {"launch": {**self.config, "prompt": "ambiguous"}},
        ]
        for override in variants:
            with self.subTest(override=override), self.assertRaises(ValueError):
                conversation.launch({**self.conversation, **override})
            self.assertFalse((self.root / "run").exists())
            self.assertFalse((self.work / "agent-started").exists())

    def test_existing_root_is_preserved(self):
        root = self.root / "run"
        root.mkdir()
        owned = root / "user-owned"
        owned.write_text("preserve")
        result = self.run_conversation()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(owned.read_text(), "preserve")
        self.assertEqual(list(root.iterdir()), [owned])
        self.assertFalse((self.work / "agent-started").exists())

    def test_separate_conversations_do_not_resume_each_other(self):
        first = self.run_conversation()
        self.assertEqual(first.returncode, 0, first.stderr)
        first_summary = self.summary()
        before = (self.root / "run/summary.json").read_bytes()
        self.config["session_directory"] = str(self.root / "other")
        second = self.run_conversation(self.root / "second.json")
        self.assertEqual(second.returncode, 0, second.stderr)
        second_summary = self.summary(self.root / "other")
        self.assertNotEqual(first_summary["conversation_id"], second_summary["conversation_id"])
        one = first_summary["turns"][0]["identity"]
        two = second_summary["turns"][0]["identity"]
        self.assertNotEqual(one["session_id"], two["session_id"])
        self.assertFalse(two["resume"])
        self.assertEqual((self.root / "run/summary.json").read_bytes(), before)
        repeated = self.run_conversation()
        self.assertEqual(repeated.returncode, 1)
        self.assertEqual((self.root / "run/summary.json").read_bytes(), before)

    def test_incomplete_resume_history_stops_before_next_launch(self):
        self.conversation["turns"] = [{"prompt": "ok"}, {"prompt": "ok"}]

        def leave_partial_history(config, *, identity, processes):
            history = session.history_path(config, identity["session_id"])
            history.parent.mkdir(parents=True)
            history.write_text('{"partial":')
            return 0

        with patch.object(session, "launch", side_effect=leave_partial_history) as launch:
            with self.assertRaisesRegex(ValueError, "complete native history"):
                conversation.launch(copy.deepcopy(self.conversation))
        self.assertEqual(launch.call_count, 1)
        self.assertEqual(self.summary()["status"], "stopped")
        self.assertFalse((self.root / "run/turn-0002").exists())

    def test_cancellation_between_turns_cannot_start_another_agent(self):
        self.conversation["turns"] = [{"prompt": "ok"}, {"prompt": "ok"}]

        def cancel_after_first(config, *, identity, processes):
            processes.cancel()
            return 0

        with patch.object(session, "launch", side_effect=cancel_after_first) as launch:
            with self.assertRaisesRegex(RuntimeError, "cancelled"):
                conversation.launch(copy.deepcopy(self.conversation))
        self.assertEqual(launch.call_count, 1)
        self.assertEqual(self.summary()["status"], "stopped")
        self.assertEqual(len(self.summary()["turns"]), 1)


if __name__ == "__main__":
    unittest.main()
