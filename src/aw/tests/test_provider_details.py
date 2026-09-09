"""Keep provider presentation tied to verified events and explicit native outcomes."""

from pathlib import Path
import sys
import tempfile
import unittest

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
import provider_details as panel
from session_hooks import write


class ProviderDetailsTests(unittest.TestCase):
    def test_sensitive_result_exposes_rule_and_scanner_coverage(self):
        call = {
            "verdict": "sensitive",
            "findings": [{"rule_id": "api_key", "count": 1, "severity": "high"}],
            "coverage": {
                "input_bytes": 43,
                "scanned_bytes": 43,
                "complete": True,
                "ruleset_ids": ["sec-core/pii-regex/native-v1"],
            },
        }
        self.assertEqual(panel.outcome(call), "inspection: sensitive | 1 findings")
        detail = "\n".join(panel.inspection_lines(call))
        self.assertIn("api_key | count 1 | severity high", detail)
        self.assertIn("43/43 B | complete: True", detail)
        self.assertIn("command already ran", detail)

    def test_no_savings_is_distinct_from_unknown_bypass_reason(self):
        self.assertEqual(
            panel.outcome({"status": "bypassed", "native_disposition": "no_savings"}),
            "preserved: no savings",
        )
        self.assertEqual(
            panel.outcome({"status": "bypassed"}),
            "preserved: native reason not recorded",
        )
        self.assertEqual(
            panel.outcome({"status": "failed", "error": "provider_timeout"}),
            "failed: provider_timeout",
        )

    def test_newly_arrived_evidence_is_excluded_from_verified_snapshot(self):
        with tempfile.TemporaryDirectory(prefix="provider-panel-", dir=AW / "target") as directory:
            root = Path(directory)
            (root / "evidence").mkdir()
            write(root / "evidence/not-verified-yet.json", {"must_not_be_read": True})
            detail = panel.snapshot(
                root,
                {"providers": {}},
                {"scope": {"session_id": "s1"}, "providers": [], "events": []},
            )
            self.assertEqual(detail["bash_results"], 0)
            self.assertEqual(detail["recent_calls"], [])

    def test_sidebar_separates_provider_calls_and_adopted_savings(self):
        detail = {
            "bash_results": 5,
            "providers": {"sec-core": {"version": "0.11.0"}, "tokenless": {"version": "0.8.0"}},
            "counters": [
                {"provider_id": "sec-core", "calls": 5},
                {"provider_id": "tokenless", "calls": 5, "adopted": 0, "saved_bytes": 0},
            ],
            "recent_calls": [
                {
                    "provider_id": "tokenless",
                    "status": "bypassed",
                    "native_disposition": "no_savings",
                }
            ],
        }
        tokens = panel.sidebar(detail)
        self.assertEqual(tokens["aw"], "AW | 5 Bash results")
        self.assertIn("0.8.0", tokens["aw_tokenless"])
        self.assertEqual(tokens["aw_tokenless_result"], "preserved: no savings")
        self.assertEqual(tokens["aw_savings"], "History: 0 adopted | saved 0 B")


if __name__ == "__main__":
    unittest.main()
