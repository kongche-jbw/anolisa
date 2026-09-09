"""Check native identity isolation and truthful inspection-only presentation."""

import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import codex_hooks
import provider_details
from session_hooks import read, write


class CodexHooksTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "calls").mkdir()
        write(self.root / "state.json", {"session_id": None})

    def event(self, session_id="native-1", turn="turn-1", tool="call-1"):
        return {
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "session_id": session_id,
            "turn_id": turn,
            "tool_use_id": tool,
            "tool_input": {"command": "printf clean"},
        }

    def test_native_turns_and_panes_remain_distinct(self):
        first = codex_hooks.transition(self.root, self.event())
        second = codex_hooks.transition(self.root, self.event(turn="turn-2", tool="call-2"))
        self.assertEqual(read(first / "before.json")["turn_id"], "turn-1")
        self.assertEqual(read(second / "before.json")["turn_id"], "turn-2")
        self.assertEqual(read(self.root / "state.json")["session_id"], "native-1")
        with self.assertRaisesRegex(ValueError, "session changed"):
            codex_hooks.transition(self.root, self.event(session_id="other-pane"))
        self.assertTrue(read(self.root / "state.json")["session_changed"])
        self.assertEqual(len(list((self.root / "calls").iterdir())), 2)

    def test_duplicate_and_unbound_turn_do_not_produce_second_call(self):
        with self.assertRaisesRegex(ValueError, "turn identity"):
            codex_hooks.transition(self.root, self.event(turn=""))
        self.assertIsNone(read(self.root / "state.json")["session_id"])
        codex_hooks.transition(self.root, self.event())
        with self.assertRaises(FileExistsError):
            codex_hooks.transition(self.root, self.event())
        self.assertEqual(len(list((self.root / "calls").iterdir())), 1)

    def test_cli_settings_do_not_bypass_trust(self):
        args = codex_hooks.arguments()
        self.assertNotIn("--dangerously-bypass-hook-trust", args)
        for index, arg in enumerate(args):
            if arg == "-c":
                hooks = tomllib.loads(args[index + 1])["hooks"]
                handler = next(iter(hooks.values()))[0]["hooks"][0]
                self.assertIn("codex_hooks.py", handler["command"])
                self.assertNotIn(str(self.root), handler["command"])

    def test_codex_does_not_claim_tokenless_execution_or_adoption(self):
        details = {
            "agent_kind": "codex",
            "bash_results": 1,
            "providers": {"sec-core": {"version": "0.11.0"}, "tokenless": {"version": "0.8.0"}},
            "counters": [{"provider_id": "sec-core", "calls": 1}],
            "recent_calls": [{"provider_id": "sec-core", "verdict": "sensitive"}],
        }
        tokens = provider_details.sidebar(details)
        self.assertIn("1 calls", tokens["aw_sec"])
        self.assertIn("unsupported", tokens["aw_tokenless"])
        self.assertNotIn("adopted", json.dumps(tokens))
        self.assertIn("output retained", tokens["aw_savings"])


if __name__ == "__main__":
    unittest.main()
