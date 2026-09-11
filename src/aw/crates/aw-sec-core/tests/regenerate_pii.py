#!/usr/bin/env python3
"""Replay the native PII CLI oracle; isolate configuration, never replace scanners."""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]
FIXTURE = Path(__file__).with_name("fixtures") / "native-pii.json"
CASES = [
    ("clean", "hello world", False, None, "clean"),
    ("unicode", "备注🙂 alice@securecorp.cn\n", False, None, "suspicious"),
    ("credential", "api_key=sk-abcdefghijklmnopqrstuvwxyz123456", False, None, "sensitive"),
    ("low_hidden", "example email test@example.invalid", False, None, "clean"),
    ("low_included", "example email test@example.invalid", True, None, "suspicious"),
    (
        "custom",
        "ORDER-ABC12345",
        False,
        "- type: order_id\n  regex: 'ORDER-[A-Z0-9]{8}'\n  severity: deny\n",
        "sensitive",
    ),
    ("invalid_rules", "hello world", False, "- type: order_id\n  regex: '[invalid'\n", None),
]
CHILD = """
import os
from pathlib import Path
from unittest.mock import patch
from agent_sec_cli.cli import main
with patch('agent_sec_cli.pii_checker.custom_rules.default_custom_rules_path',
           return_value=Path(os.environ['AW_ORACLE_RULES'])):
    main()
"""


def check_audit(value: object, content: str, rules: str | None) -> None:
    """Reject raw input/configuration in decoded audit strings, including escapes."""
    if isinstance(value, dict):
        for key, child in value.items():
            if key in {"raw_evidence", "redacted_text", "text"}:
                raise RuntimeError("native audit contains a forbidden field")
            check_audit(child, content, rules)
    elif isinstance(value, list):
        for child in value:
            check_audit(child, content, rules)
    elif isinstance(value, str):
        if (content and content in value) or (rules and rules in value):
            raise RuntimeError("native audit leaked raw input/configuration")


class AuditTests(unittest.TestCase):
    """Keep the native oracle's disclosure checks effective without a scanner."""

    def test_nested_escaped_input_and_configuration_are_rejected(self):
        for secret in ['备注\n\t"private"\\value', "- rule:\n  regex: '[private'\n"]:
            decoded = json.loads(json.dumps({"result": [{"unexpected": secret}]}))
            for content, rules in [(secret, None), ("other input", secret)]:
                with self.subTest(secret=secret), self.assertRaises(RuntimeError):
                    check_audit(decoded, content, rules)

    def test_forbidden_fields_fail_and_sanitized_metadata_passes(self):
        for field in ["raw_evidence", "redacted_text", "text"]:
            with self.assertRaises(RuntimeError):
                check_audit({"result": {field: "partial"}}, "complete input", None)
        check_audit({"findings": [{"type": "email", "count": 1}]}, "private\ninput", None)


def collect() -> dict:
    """Run the real CLI and verify its native audit before keeping stable output."""
    revision = subprocess.run(
        ["git", "log", "-1", "--format=%H", "--", "src/agent-sec-core"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    ).stdout.strip()
    records = []
    build = ROOT / "src/aw/target"
    build.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="native-pii-", dir=build) as temporary:
        for name, content, include_low, rules, expected in CASES:
            directory = Path(temporary) / name
            directory.mkdir()
            rules_path = directory / "rules.yaml"
            if rules is not None:
                rules_path.write_text(rules, encoding="utf-8")
            environment = dict(os.environ)
            environment.update(
                {
                    "PYTHONPATH": str(ROOT / "src/agent-sec-core/agent-sec-cli/src"),
                    "PYTHONDONTWRITEBYTECODE": "1",
                    "AGENT_SEC_DATA_DIR": str(directory / "data"),
                    "AGENT_SEC_TELEMETRY_LOG_PATH": str(directory / "telemetry.jsonl"),
                    "AW_ORACLE_RULES": str(rules_path),
                }
            )
            arguments = ["scan-pii", "--stdin", "--format", "json", "--source", "tool_output"]
            if include_low:
                arguments.append("--include-low-confidence")
            result = subprocess.run(
                [sys.executable, "-c", CHILD, *arguments],
                input=content.encode("utf-8"),
                env=environment,
                cwd=directory,
                capture_output=True,
                timeout=30,
            )
            if result.returncode != 0:
                raise RuntimeError(f"{name}: native CLI failed with {result.returncode}")
            output = json.loads(result.stdout)
            output["elapsed_ms"] = 0
            events = [
                json.loads(line)
                for line in (directory / "data/security-events.jsonl").read_text().splitlines()
            ]
            if len(events) != 1 or events[0]["category"] != "pii_scan":
                raise RuntimeError(f"{name}: missing or duplicate native security event")
            check_audit(events[0]["details"], content, rules)
            records.append(
                {
                    "name": name,
                    "content": content,
                    "include_low_confidence": include_low,
                    "rules_yaml": rules,
                    "exit_code": result.returncode,
                    "output": output,
                    "expected_aw_verdict": expected,
                }
            )
    return {
        "profile": "agent-sec-cli/scan-pii/0.12.0",
        "native_source_commit": revision,
        "cases": records,
    }


def main() -> None:
    """Write a reviewed oracle or fail on native behavior drift."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="Replace reviewed golden fixtures")
    parser.add_argument(
        "--self-test", action="store_true", help="Test audit assertions without SecCore"
    )
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(AuditTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        if not result.wasSuccessful():
            raise SystemExit(1)
        return
    actual = collect()
    if args.write:
        FIXTURE.parent.mkdir(exist_ok=True)
        FIXTURE.write_text(
            json.dumps(actual, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    else:
        expected = json.loads(FIXTURE.read_text(encoding="utf-8"))
        if actual["cases"] != expected["cases"]:
            raise SystemExit("Native PII behavior drifted; review the compatibility change")
    print(f"Native PII: {len(actual['cases'])} CLI and sanitized audit cases passed")


if __name__ == "__main__":
    main()
