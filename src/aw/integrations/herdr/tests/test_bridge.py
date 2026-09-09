"""Presentation tests use explicit fixtures; they are not AW adoption evidence."""

import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bridge", Path(__file__).parents[1] / "bridge.py"
)
bridge = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bridge)


class BridgeTests(unittest.TestCase):
    def view(self) -> dict:
        return {
            "format": 1,
            "scope": {"session_id": "current"},
            "runtime_alive": True,
            "verification": "journal_verified",
            "adoption": "not_observed",
            "providers": [
                {
                    "provider_id": "example",
                    "kind": "projection",
                    "calls": 1,
                    "candidates": 1,
                    "adopted": 0,
                    "saved_bytes": 0,
                    "failed": 0,
                    "bypassed": 0,
                }
            ],
        }

    def test_candidates_are_never_adoption(self) -> None:
        view = self.view()
        tokens = bridge.format_view(view, view["scope"])
        self.assertIn("1 candidates", tokens["aw_tokenless"])
        self.assertIn("Adoption unknown", tokens["aw_usage"])
        self.assertNotIn("saved", str(tokens))

    def test_unverified_or_stale_views_fail(self) -> None:
        for key, value in [
            ("scope", {"session_id": "old"}),
            ("runtime_alive", False),
            ("verification", "unverified"),
            ("adoption", "adopted"),
        ]:
            with self.subTest(key=key):
                view = self.view()
                view[key] = value
                with self.assertRaises(ValueError):
                    bridge.format_view(view, {"session_id": "current"})

    def test_unsupported_adoption_and_counters_fail(self) -> None:
        for key, value in [
            ("adopted", 1),
            ("saved_bytes", 256),
            ("failed", -1),
            ("calls", True),
            ("candidates", 2),
        ]:
            with self.subTest(key=key):
                view = self.view()
                view["providers"][0][key] = value
                with self.assertRaises(ValueError):
                    bridge.format_view(view, view["scope"])

    def test_failure_and_bypass_are_visible(self) -> None:
        for key in ("failed", "bypassed"):
            view = self.view()
            view["providers"][0].update({"candidates": 0, key: 1})
            self.assertIn(key, bridge.format_view(view, view["scope"])["aw_tokenless"])

    def test_verified_history_is_labelled_as_history(self) -> None:
        view = self.view()
        view["adoption"] = "local_history"
        view["providers"][0].update({"adopted": 1, "saved_bytes": 256})
        tokens = bridge.format_view(view, view["scope"])
        self.assertIn("history adopted / -256 B", tokens["aw_tokenless"])
        self.assertNotIn("model", str(tokens))

    def test_mixed_results_keep_adoption_savings_and_failures(self) -> None:
        view = self.view()
        view["adoption"] = "local_history"
        view["providers"][0].update(
            {"calls": 3, "adopted": 1, "saved_bytes": 256, "failed": 1, "bypassed": 1}
        )
        tokens = bridge.format_view(view, view["scope"])
        for text in ("1 history adopted", "-256 B", "1 failed", "1 bypassed"):
            self.assertIn(text, tokens["aw_tokenless"])
        self.assertIn("1 failed / 1 bypassed", tokens["aw_usage"])
        self.assertIn("AW failure / 3 calls", tokens["aw"])

    def test_mixed_candidate_failure_and_bypass_remain_visible(self) -> None:
        view = self.view()
        view["providers"][0].update({"calls": 3, "failed": 1, "bypassed": 1})
        tokens = bridge.format_view(view, view["scope"])
        for text in ("1 candidates", "1 failed", "1 bypassed"):
            self.assertIn(text, tokens["aw_tokenless"])
        self.assertIn("Adoption unknown", tokens["aw_usage"])

    def test_empty_session_does_not_inherit_counts(self) -> None:
        view = self.view()
        view["providers"] = []
        tokens = bridge.format_view(view, view["scope"])
        self.assertEqual(tokens["aw"], "AW verified / 0 calls")
        self.assertEqual(tokens["aw_tokenless"], "Tokenless: no calls")

    def test_pane_must_match_session_and_process(self) -> None:
        binding = {"agent_pid": 42, "scope": {"session_id": "current"}}
        pane = {"agent_session": {"value": "current"}}
        process = {"shell_pid": 41, "foreground_processes": [{"pid": 42}]}
        bridge.verify_pane(binding, pane, process)
        with self.assertRaises(ValueError):
            bridge.verify_pane(binding, {"agent_session": {"value": "old"}}, process)
        with self.assertRaises(ValueError):
            bridge.verify_pane(binding, pane, {"shell_pid": 99})

    def test_refresh_error_clears_previous_counters(self) -> None:
        with patch.object(bridge, "publish") as publish:
            self.assertFalse(
                bridge.refresh(
                    Path("unused"),
                    "pane",
                    Path("unused"),
                    Path("/missing-aw-test-binding"),
                    100,
                )
            )
        tokens = publish.call_args.args[2]
        self.assertIsNone(tokens["aw_tokenless"])
        self.assertIsNone(tokens["aw_sec"])
        self.assertIn("unknown", tokens["aw"])

    def test_fixed_source_and_expiring_metadata(self) -> None:
        with patch.object(bridge, "rpc") as rpc:
            bridge.publish(Path("unused"), "pane", {"aw": "unknown"}, 12)
        params = rpc.call_args.args[2]
        self.assertEqual(params["source"], "anolisa.aw")
        self.assertEqual(params["seq"], 12)
        self.assertEqual(params["ttl_ms"], 5000)

    def test_session_switch_during_verification_clears_view(self) -> None:
        binding = {"agent_pid": 42, "scope": {"session_id": "current"}}
        pane = {"agent_session": {"value": "current"}}
        process = {"shell_pid": 42}
        reply = type("Reply", (), {"stdout": json.dumps(self.view()).encode()})()
        with patch.object(Path, "read_text", return_value=json.dumps(binding)):
            with patch.object(bridge.subprocess, "run", return_value=reply):
                with patch.object(
                    bridge,
                    "rpc",
                    side_effect=[
                        pane,
                        process,
                        {"agent_session": {"value": "new"}},
                        process,
                    ],
                ):
                    with patch.object(bridge, "publish") as publish:
                        self.assertFalse(
                            bridge.refresh(
                                Path("socket"),
                                "pane",
                                Path("verify"),
                                Path("binding"),
                                100,
                            )
                        )
        self.assertIsNone(publish.call_args.args[2]["aw_tokenless"])


if __name__ == "__main__":
    unittest.main()
