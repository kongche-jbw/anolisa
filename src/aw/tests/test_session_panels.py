"""Protect native pane selection and the owned registration boundary."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
from session_hooks import write
from session_panels import Panels, focused_root, register, registrations


class PanelTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="panels-unit-", dir=AW / "target")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.group, self.second = self.directory / "group", self.directory / "second"
        (self.group / "panels").mkdir(parents=True)
        self.second.mkdir()
        write(self.group / "ownership.json", {"socket": str(self.directory / "api.sock")})
        for root, pane, pid in ((self.group, "w1:p1", 101), (self.second, "w1:p2", 102)):
            register(self.group, root, pane)
            write(
                root / "runtime.json",
                {"agent_pid": pid, "agent_start_ticks": pid, "agent_kind": "qoder"},
            )

    def test_removed_pane_is_cleaned_even_while_its_agent_is_exiting(self):
        panels = Panels(self.group, self.directory / "api.sock", self.directory, 60, None)
        first, second = Mock(), Mock()
        first.poll.return_value = None
        second.poll.return_value = None
        panels.observers = {self.group: first, self.second: second}
        with (
            patch("session.live", return_value=True),
            patch("session.stop_agent") as stop_agent,
            patch("session_panels.stop_observer") as stop_observer,
        ):
            panels.refresh({"w1:p2"})
        stop_agent.assert_called_once_with(self.group)
        stop_observer.assert_called_once_with(first)
        self.assertEqual(panels.ended, {self.group})
        self.assertIs(panels.observers[self.second], second)

    def test_natural_exit_cleans_group_before_marking_ended(self):
        panels = Panels(self.group, self.directory / "api.sock", self.directory, 60, None)
        with patch("session.live", return_value=False), patch("session.stop_agent") as stop:
            panels.refresh({"w1:p1", "w1:p2"})
            self.assertEqual(stop.call_count, 2)
            panels.close()
            self.assertEqual(stop.call_count, 2)  # Do not reuse stale group IDs later.
        self.assertEqual(panels.ended, {self.group, self.second})

    def test_details_follow_focus_without_falling_back_to_first_agent(self):
        with (
            patch("session_panels.bridge.rpc", return_value={"pane": {"pane_id": "w1:p2"}}),
            patch("session.live", return_value=True),
        ):
            self.assertEqual(focused_root(self.group), self.second)
        with (
            patch("session_panels.bridge.rpc", return_value={"pane": {"pane_id": "w1:p3"}}),
            patch("session.live", return_value=True),
        ):
            with self.assertRaisesRegex(ValueError, "No AW agent"):
                focused_root(self.group)

    def test_dead_agent_is_not_a_current_provider_target(self):
        with (
            patch("session_panels.bridge.rpc", return_value={"pane": {"pane_id": "w1:p2"}}),
            patch("session.live", return_value=False),
        ):
            with self.assertRaisesRegex(ValueError, "No AW agent"):
                focused_root(self.group)

    def test_registration_cannot_point_outside_owned_session_directory(self):
        write(self.group / "panels/foreign.json", {"root": "/foreign/foreign", "pane": "w1:p3"})
        with self.assertRaisesRegex(ValueError, "outside"):
            registrations(self.group)


if __name__ == "__main__":
    unittest.main()
