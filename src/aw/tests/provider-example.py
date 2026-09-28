#!/usr/bin/env python3
"""Contract tests and optional isolated, real sec-core smoke for policy.py.

Run with ``python3 src/aw/tests/provider-example.py``. Add
``--sec-core-cli /absolute/agent-sec-cli --evidence-dir /absolute/test-directory``
to verify the installed scanner without executing the scanned command text.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "examples/providers/policy.py"
LAB = SOURCE.parents[4] / "target/provider-lab"
API_VERSION = "aw-provider/v1alpha1"
REAL_CLI: str | None = None
EVIDENCE: Path | None = None


def request(method: str = "invoke", **updates: object) -> dict:
    value = {"api_version": API_VERSION, "method": method, "request_id": "request-1"}
    if method != "describe":
        value["config"] = {}
    if method == "invoke":
        value.update(
            operation="check",
            config_revision="revision-1",
            budget_ms=2000,
            allowed_effects=["observe", "block"],
            input_digest="opaque-input-digest-1",
            event={
                "name": "tool.before",
                "agent": {"adapter": "qoder"},
                "tool": {
                    "name": "bash",
                    "native_name": "Bash",
                    "input": {"command": "printf '%s\\n' allowed"},
                },
            },
        )
    value.update(updates)
    return value


class ProviderExampleTests(unittest.TestCase):
    def setUp(self) -> None:
        LAB.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="contract-", dir=LAB)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def call(
        self, value: object, *, raw: bool = False, environment: dict | None = None
    ) -> dict:
        process = subprocess.run(
            [sys.executable, str(SOURCE)],
            input=value if raw else json.dumps(value),
            text=True,
            capture_output=True,
            timeout=8,
            check=False,
            env=environment,
        )
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertEqual(process.stderr, "")
        self.assertEqual(len(process.stdout.splitlines()), 1)
        response = json.loads(process.stdout)
        self.assertEqual(response["api_version"], API_VERSION)
        return response

    def scanner(self, body: str) -> dict:
        fake = self.root / "scanner.py"
        fake.write_text("import json, os, sys, time\n" + body)
        return {"argv": [sys.executable, str(fake)], "timeout_ms": 1000}

    def test_discovery_and_empty_configuration(self) -> None:
        result = self.call(request("describe"))
        self.assertEqual(result["status"], "ok")
        self.assertEqual(
            result["operations"],
            [
                {
                    "name": "check",
                    "events": ["tool.before"],
                    "effects": ["observe", "block"],
                },
                {"name": "record", "events": ["tool.after"], "effects": ["observe"]},
            ],
        )
        self.assertEqual(
            self.call(request("validate_config")),
            {
                "api_version": API_VERSION,
                "request_id": "request-1",
                "status": "ok",
            },
        )

    def test_rejects_unknown_or_malformed_configuration(self) -> None:
        for config in [
            None,
            [],
            {"secret": "must-not-echo"},
            {"blocked_tools": [1]},
            {"blocked_substrings": [""]},
            {"sec_core": {}},
            {"sec_core": {"argv": ["relative"]}},
            {"sec_core": {"argv": [sys.executable], "timeout_ms": True}},
            {"sec_core": {"argv": [sys.executable], "block_verdicts": ["pass"]}},
            {
                "sec_core": {
                    "argv": [sys.executable],
                    "block_verdicts": ["warn", "warn"],
                }
            },
        ]:
            with self.subTest(config=config):
                result = self.call(request("validate_config", config=config))
                self.assertEqual(result["error_code"], "invalid_config")
                self.assertNotIn("must-not-echo", json.dumps(result))

    def test_malformed_json_and_envelope_are_structured_errors(self) -> None:
        for payload in [
            '{"secret":"never-print",',
            "{} {}",
            "null",
            "[]",
            '{"x":1,"x":2}',
            '{"x":NaN}',
        ]:
            result = self.call(payload, raw=True)
            self.assertEqual(result["status"], "error")
            self.assertNotIn("never-print", json.dumps(result))
        for changes in [
            {"api_version": "wrong"},
            {"request_id": ""},
            {"method": []},
            {"unexpected": True},
            {"operation": []},
            {"budget_ms": False},
            {"allowed_effects": ["ask"]},
            {"input_digest": ""},
        ]:
            self.assertEqual(self.call(request(**changes))["status"], "error")

    def test_request_and_digest_are_bound_on_every_success(self) -> None:
        for identifier, digest in [
            ("request-1", "digest-1"),
            ("request-2", "digest-2"),
        ]:
            result = self.call(request(request_id=identifier, input_digest=digest))
            self.assertEqual(result["request_id"], identifier)
            self.assertEqual(result["input_digest"], digest)
            self.assertEqual(result["effects"], [])

    def test_allow_and_exact_tool_or_explicit_text_block(self) -> None:
        for native_name in [
            "Bash",
            "exec",
            "terminal",
            "execute_shell_command",
            "custom.tool",
        ]:
            value = request()
            value["event"]["tool"]["native_name"] = native_name
            self.assertEqual(self.call(value)["effects"], [])
            value["config"] = {"blocked_tools": [native_name]}
            self.assertEqual(
                self.call(value)["effects"],
                [{"type": "block", "reason_code": "policy_match"}],
            )
        value = request(config={"blocked_substrings": ["allowed"]})
        self.assertEqual(self.call(value)["effects"][0]["type"], "block")

    def test_effect_and_operation_event_admission(self) -> None:
        value = request(config={"blocked_tools": ["bash"]}, allowed_effects=["observe"])
        self.assertEqual(self.call(value)["error_code"], "effect_not_allowed")
        self.assertEqual(
            self.call(request(operation="record"))["error_code"], "unsupported_event"
        )
        value = request(operation="record", allowed_effects=["observe"])
        value["event"]["name"] = "tool.after"
        value["event"]["tool"]["result"] = {"sensitive": "never-echo"}
        result = self.call(value)
        self.assertEqual(result["effects"], [{"type": "observe"}])
        self.assertNotIn("never-echo", json.dumps(result))

    def test_scanner_receives_literal_argv_and_zero_exit_is_not_allow(self) -> None:
        capture = self.root / "arguments.json"
        config = self.scanner(
            f"open({str(capture)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
            "print(json.dumps({'ok': True, 'verdict': 'warn', 'findings': []}))\n"
        )
        value = request(config={"sec_core": config})
        command = f"printf '%s' '$(touch {self.root / 'unexpected'})'; `printf unsafe`"
        value["event"]["tool"]["input"]["command"] = command
        self.assertEqual(self.call(value)["effects"][0]["type"], "block")
        self.assertEqual(
            json.loads(capture.read_text()),
            ["scan-code", "--code", command, "--language", "bash", "--mode", "regex"],
        )
        self.assertFalse((self.root / "unexpected").exists())

    def test_scanner_pass_and_explicit_deny_only_threshold(self) -> None:
        for verdict in ["pass", "warn", "deny"]:
            config = self.scanner(
                f"print(json.dumps({{'ok': True, 'verdict': {verdict!r}, 'findings': []}}))\n"
            )
            config["block_verdicts"] = ["deny"]
            result = self.call(request(config={"sec_core": config}))
            self.assertEqual(bool(result["effects"]), verdict == "deny")

    def test_scanner_errors_do_not_become_policy_matches(self) -> None:
        for body, expected in [
            ("print('secret-invalid-output')\n", "invalid_scanner_output"),
            (
                "print(json.dumps({'ok': False, 'verdict': 'error', 'findings': []}))\n",
                "invalid_scanner_output",
            ),
            (
                "print(json.dumps({'ok': True, 'verdict': 'unknown', 'findings': []}))\n",
                "invalid_scanner_output",
            ),
            (
                "print(json.dumps({'ok': True, 'verdict': [], 'findings': []}))\n",
                "invalid_scanner_output",
            ),
            (
                "print('scanner secret', file=sys.stderr)\nsys.exit(7)\n",
                "scanner_failed",
            ),
        ]:
            result = self.call(request(config={"sec_core": self.scanner(body)}))
            self.assertEqual(result["error_code"], expected)
            self.assertNotIn("effects", result)
            self.assertNotIn("secret", json.dumps(result))

    def test_scanner_timeout_is_bounded_and_child_is_reaped(self) -> None:
        pidfile = self.root / "child.pid"
        config = self.scanner(
            f"open({str(pidfile)!r}, 'w').write(str(os.getpid()))\ntime.sleep(5)\n"
        )
        config["timeout_ms"] = 100
        self.assertEqual(
            self.call(request(config={"sec_core": config}))["error_code"],
            "scanner_timeout",
        )
        pid = int(pidfile.read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertEqual(
            self.call(request(config={"sec_core": config}, budget_ms=1))["error_code"],
            "budget_exhausted",
        )

    def test_unknown_tool_names_never_trigger_shell_scanning(self) -> None:
        config = self.scanner("raise RuntimeError('must not run')\n")
        for name in ["bash_custom", "custom.exec", "remote_terminal", "run_arbitrary"]:
            value = request(config={"sec_core": config})
            value["event"]["tool"]["native_name"] = name
            self.assertEqual(self.call(value)["effects"], [])
        for adapter, name in [
            ("custom", "exec"),
            ("qoder", "terminal"),
            ("qwenpaw", "Bash"),
        ]:
            value = request(config={"sec_core": config})
            value["event"]["agent"]["adapter"] = adapter
            value["event"]["tool"]["native_name"] = name
            self.assertEqual(self.call(value)["effects"], [])

    def test_all_confirmed_adapter_tool_pairs_use_scanner(self) -> None:
        config = self.scanner(
            "print(json.dumps({'ok': True, 'verdict': 'warn', 'findings': []}))\n"
        )
        for adapter, name in [
            ("qoder", "Bash"),
            ("openclaw", "exec"),
            ("hermes", "terminal"),
            ("qwenpaw", "execute_shell_command"),
        ]:
            value = request(config={"sec_core": config})
            value["event"]["agent"]["adapter"] = adapter
            value["event"]["tool"]["native_name"] = name
            self.assertEqual(self.call(value)["effects"][0]["type"], "block")

    def test_scanner_output_is_bounded_on_both_streams(self) -> None:
        for stream in ["stdout", "stderr"]:
            config = self.scanner(
                f"sys.{stream}.write('x' * (1024 * 1024 + 1))\nsys.{stream}.flush()\ntime.sleep(5)\n"
            )
            self.assertEqual(
                self.call(request(config={"sec_core": config}))["error_code"],
                "scanner_output_limit",
            )

    def test_installed_sec_core_allow_and_block(self) -> None:
        if REAL_CLI is None or EVIDENCE is None:
            self.skipTest(
                "pass --sec-core-cli and --evidence-dir for an isolated real scan"
            )
        EVIDENCE.mkdir(parents=True, exist_ok=True, mode=0o700)
        environment = {**os.environ, "AGENT_SEC_DATA_DIR": str(EVIDENCE / "audit")}
        cases = [
            ("allow", "printf '%s\\n' aw-sec-allow", False),
            ("block", "printf '%s\\n' 'rm -rf /aw-never-executed'", True),
        ]
        ledger = EVIDENCE / "processes.json"
        records = json.loads(ledger.read_text()) if ledger.exists() else []
        for name, command, blocked in cases:
            value = request(
                config={"sec_core": {"argv": [REAL_CLI], "timeout_ms": 5000}},
                budget_ms=6000,
            )
            value["event"]["tool"]["input"]["command"] = command
            log = EVIDENCE / f"{name}.log"
            with log.open("w") as output:
                child = subprocess.Popen(
                    [sys.executable, str(SOURCE)],
                    stdin=subprocess.PIPE,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    text=True,
                    env=environment,
                )
                record = {
                    "name": name,
                    "command": [sys.executable, str(SOURCE)],
                    "cwd": os.getcwd(),
                    "pid": child.pid,
                    "pgid": os.getpgid(child.pid),
                    "ports": [],
                    "log": str(log),
                    "timeout_seconds": 8,
                    "stop": f"kill -TERM {child.pid}",
                }
                records.append(record)
                (EVIDENCE / "processes.json").write_text(
                    json.dumps(records, indent=2) + "\n"
                )
                try:
                    child.communicate(json.dumps(value), timeout=8)
                finally:
                    if child.poll() is None:
                        child.kill()
                    child.wait(timeout=5)
                record["exit_code"] = child.returncode
            self.assertEqual(child.returncode, 0, log.read_text())
            result = json.loads(log.read_text())
            self.assertEqual(result["status"], "ok", result)
            self.assertEqual(bool(result["effects"]), blocked, result)
            with self.assertRaises(ProcessLookupError):
                os.kill(child.pid, 0)
            record["pid_absent"] = True
            record["policy_blocked"] = blocked
            (EVIDENCE / "processes.json").write_text(
                json.dumps(records, indent=2) + "\n"
            )
        audit = EVIDENCE / "audit/security-events.jsonl"
        events = [json.loads(line) for line in audit.read_text().splitlines()]
        for event in events:
            with self.assertRaises(ProcessLookupError):
                os.kill(event["pid"], 0)
        summary = {
            "cli": REAL_CLI,
            "scanner_pids": [event["pid"] for event in events],
            "scanner_pids_absent": True,
            "cases": [
                {
                    "verdict": event["details"]["result"]["verdict"],
                    "engine_version": event["details"]["result"]["engine_version"],
                    "rule_ids": [
                        finding["rule_id"]
                        for finding in event["details"]["result"]["findings"]
                    ],
                }
                for event in events[-2:]
            ],
        }
        (EVIDENCE / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--sec-core-cli")
    parser.add_argument("--evidence-dir", type=Path)
    options, remaining = parser.parse_known_args()
    REAL_CLI, EVIDENCE = options.sec_core_cli, options.evidence_dir
    if (REAL_CLI is None) != (EVIDENCE is None):
        parser.error("--sec-core-cli and --evidence-dir must be supplied together")
    unittest.main(argv=[sys.argv[0], *remaining])
