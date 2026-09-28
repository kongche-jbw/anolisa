"""Run inside the pinned Hermes environment; exercise its native dispatcher."""

import json
import os
import shlex
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from agent.shell_hooks import _make_callback, _parse_response, iter_configured_hooks
from hermes_cli.plugins import (
    PluginManager,
    _get_pre_tool_call_directive_details,
    _resolve_block_from_details,
)


class NativeShellHookTests(unittest.TestCase):
    def test_native_dispatch_runs_real_commands_in_order(self) -> None:
        home = Path(os.environ["HERMES_HOME"])
        home.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=home, prefix="hook-test-") as directory:
            trace = Path(directory) / "trace.jsonl"
            fixture = Path(__file__).with_name("hook_command.py")
            specs = iter_configured_hooks(
                {
                    "hooks": {
                        "pre_tool_call": [
                            {
                                "command": shlex.join(
                                    [sys.executable, str(fixture), label, "allow", str(trace)]
                                ),
                                "timeout": 3,
                            }
                            for label in ("first", "second")
                        ]
                    }
                }
            )
            manager = PluginManager(scope_key=directory)
            manager._hooks["pre_tool_call"] = [_make_callback(spec) for spec in specs]
            result = manager.invoke_hook(
                "pre_tool_call", tool_name="terminal", args={"command": "true"}, tool_call_id="one"
            )
            self.assertEqual(result, [])
            rows = [json.loads(line) for line in trace.read_text().splitlines()]
            self.assertEqual([row["label"] for row in rows], ["first", "second"])
            self.assertEqual(rows[0]["payload"]["tool_input"], {"command": "true"})

    def test_list_order_is_native_registration_order(self) -> None:
        specs = iter_configured_hooks(
            {
                "hooks": {
                    "pre_tool_call": [
                        {"command": "first", "timeout": 3},
                        {"command": "second", "timeout": 4},
                    ]
                }
            }
        )
        self.assertEqual([spec.command for spec in specs], ["first", "second"])
        self.assertEqual([spec.timeout for spec in specs], [3, 4])

    def test_native_approve_is_human_escalation(self) -> None:
        self.assertEqual(
            _parse_response("pre_tool_call", '{"action":"approve","message":"review"}'),
            {"action": "approve", "message": "review"},
        )
        self.assertIsNone(_parse_response("pre_tool_call", '{"decision":"approve"}'))

    def test_native_ask_fails_closed_without_human(self) -> None:
        from tools.approval import request_tool_approval

        self.assertIn("native-lab", os.environ["HERMES_HOME"])
        result = request_tool_approval(
            "terminal", "AW test requires a human", rule_key="aw-native-test"
        )
        self.assertFalse(result["approved"])
        self.assertIn("no interactive user or gateway", result["message"])

    def test_later_native_block_wins_over_approve(self) -> None:
        directives = [
            {"action": "approve", "message": "ask"},
            {"action": "block", "message": "stop"},
        ]
        with patch("hermes_cli.lifecycle.invoke_hook", return_value=directives):
            result = _get_pre_tool_call_directive_details("terminal", {"command": "true"})
        self.assertEqual((result.action, result.message), ("block", "stop"))

    def test_approval_denial_remains_blocked(self) -> None:
        with patch(
            "hermes_cli.lifecycle.invoke_hook",
            return_value=[{"action": "approve", "message": "ask"}],
        ):
            details = _get_pre_tool_call_directive_details("terminal", {})
        with patch(
            "tools.approval.request_tool_approval",
            return_value={"approved": False, "message": "denied"},
        ):
            self.assertEqual(_resolve_block_from_details(details, "terminal"), "denied")

    def test_post_does_not_accept_pre_control_directives(self) -> None:
        self.assertIsNone(
            _parse_response("post_tool_call", '{"action":"block","message":"too late"}')
        )


if __name__ == "__main__":
    unittest.main()
