"""Lifecycle/identity integration using real AW binaries and synthetic Agents."""

import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

INTEGRATION = Path(__file__).resolve().parents[1]
AW = INTEGRATION.parents[1]
sys.path.insert(0, str(INTEGRATION))
import session


def pin(path):
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="qoder-test-", dir=AW / "target")
        self.root = Path(self.temp.name)
        self.work = self.root / "workspace"
        self.native = self.root / "native"
        self.work.mkdir()
        self.native.mkdir()
        peer = Path(__file__).with_name("native_peer.py").read_text()
        self.binaries = {}
        for name, filename in (("qoder", "qoder"), ("cosh_shell", "cosh-shell")):
            path = self.root / filename
            path.write_text(f"#!{sys.executable}\n" + peer)
            path.chmod(0o700)
            self.binaries[name] = pin(path)
        target = Path(os.environ.get("CARGO_TARGET_DIR", str(AW / "target")))
        if not target.is_absolute():
            target = AW / target
        for name, binary in (
            ("aw_hook", "aw-hook-cli"),
            ("aw_preflight", "aw-preflight-cli"),
            ("aw_adoption", "aw-adoption-cli"),
        ):
            self.binaries[name] = pin(target / "debug" / binary)
        scanner = self.root / "scanner.py"
        scanner.write_text((AW / "crates/aw-sec-host/tests/native_fixture.py").read_text())
        provider = {
            "provider_id": "security",
            "provider_version": "fixture",
            "program": sys.executable,
            "program_sha256": pin(Path(sys.executable))["sha256"],
            "cwd": str(self.work),
            "args": [str(scanner)],
            "environment": {},
            "pins": [],
            "limits": {
                "timeout_ms": 2000,
                "input_bytes": 1048576,
                "output_bytes": 131072,
                "stderr_bytes": 65536,
            },
        }
        self.config = {
            "format": 1,
            "workspace": str(self.work),
            "config_directory": str(self.native),
            "session_directory": str(self.root / "run"),
            "binaries": self.binaries,
            "preflight": {"mode": "inspect", "security": provider},
            "prompt": "ok",
            "timeout_seconds": 20,
            "permission_mode": "dont_ask",
        }
        golden = json.loads((AW / "tests/fixtures/tokenless-native.json").read_text())["cases"][0]
        (self.work / "golden.json").write_text(json.dumps(golden))

    def tearDown(self):
        self.temp.cleanup()

    def run_session(self):
        with open(os.devnull, "w") as output:
            settings = self.root / "settings.json"
            session.write(settings, self.config)
            return subprocess.run(
                [sys.executable, "-B", str(INTEGRATION / "session.py"), str(settings)],
                stdout=output,
                stderr=subprocess.PIPE,
                timeout=40,
                text=True,
            )

    def projection(self):
        self.config["preflight"]["mode"] = "project"
        tokenless = json.loads(json.dumps(self.config["preflight"]["security"]))
        tokenless["provider_id"] = "tokenless"
        tokenless["environment"] = {
            "TOKENLESS_STATS_ENABLED": "0",
            "TOKENLESS_SLS_ENABLED": "0",
            "TOKENLESS_COMPRESSION_ENABLED": "1",
        }
        tokenless["args"] = [
            "-c",
            "import sys,json,pathlib\nif sys.argv[-1]=='--version': print('tokenless 0.8.1');sys.exit(0)\nr=json.load(sys.stdin);g=json.loads(pathlib.Path('golden.json').read_text())\nprint(json.dumps({'protocol_version':2,'operation':'post_tool','attribution':r['attribution'],'result':g['response']['result']}))",
        ]
        self.config["preflight"]["tokenless"] = tokenless
        self.config["projection"] = {
            "retention": "source_and_candidate",
            "accepted_reversibility": ["unrecoverable"],
            "allow_text_reencoding": True,
        }

    def test_inspect_binds_exec_identity_and_preserves_native_settings(self):
        native = self.native / "settings.json"
        native.write_text('{"theme":"dark"}')
        before = native.read_bytes()
        result = self.run_session()
        self.assertEqual(result.returncode, 0, result.stderr)
        root = self.root / "run"
        agent = json.loads((root / "agent.json").read_text())
        hook = json.loads((root / "hook.json").read_text())
        self.assertEqual(hook["agent_pid"], agent["pid"])
        self.assertEqual((self.work / "hook-exit").read_text(), "0")
        self.assertEqual(native.read_bytes(), before)
        self.assertFalse(Path(f'/proc/{agent["pid"]}').exists())
        self.assertEqual(json.loads((root / "result.json").read_text())["observations"], [])

    def test_projection_returns_candidate_and_observes_independent_history(self):
        self.projection()
        result = self.run_session()
        self.assertEqual(result.returncode, 0, result.stderr)
        record = json.loads((self.root / "run/result.json").read_text())["observations"][0]
        self.assertTrue(record["returned"])
        self.assertEqual(record["observation_status"], "adopted")
        self.assertGreater(record["saved_bytes"], 0)

    def test_wrong_session_cannot_produce_observation(self):
        self.projection()
        self.config["prompt"] = "wrong_session"
        result = self.run_session()
        self.assertEqual(result.returncode, 4, result.stderr)
        self.assertEqual(
            json.loads((self.root / "run/result.json").read_text())["observations"], []
        )

    def test_existing_native_hooks_fail_without_overwrite_or_agent_start(self):
        native = self.native / "settings.json"
        native.write_text('{"hooks":{"PostToolUse":[{"existing":"security"}]}}')
        before = native.read_bytes()
        result = self.run_session()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(native.read_bytes(), before)
        self.assertFalse((self.root / "run").exists())
        self.assertFalse((self.work / "agent-started").exists())

    def test_globally_disabled_hooks_fail_before_start_without_changing_settings(self):
        native = self.native / "settings.json"
        native.write_text('{"disableAllHooks":true}')
        before = native.read_bytes()
        result = self.run_session()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(native.read_bytes(), before)
        self.assertFalse((self.root / "run").exists())
        self.assertFalse((self.work / "agent-started").exists())

    def test_native_project_directory_override_is_rejected_before_start(self):
        with patch.dict(os.environ, {"QODER_CONFIG_DIR_NAME": ".alternate"}):
            result = self.run_session()
        self.assertEqual(result.returncode, 1)
        self.assertFalse((self.work / "agent-started").exists())
        self.assertFalse((self.root / "run").exists())

    def test_missing_security_protocol_stops_before_agent_and_cleans_setup(self):
        self.config["preflight"]["security"]["environment"] = {"MODE": "missing_command"}
        result = self.run_session()
        self.assertEqual(result.returncode, 1)
        self.assertFalse((self.work / "agent-started").exists())
        self.assertFalse((self.root / "run").exists())

    def test_existing_session_directory_is_preserved(self):
        root = self.root / "run"
        root.mkdir()
        (root / "user-owned").write_text("preserve")
        result = self.run_session()
        self.assertEqual(result.returncode, 1)
        self.assertEqual((root / "user-owned").read_text(), "preserve")

    def test_timeout_and_early_parent_exit_reap_owned_descendants(self):
        for mode in ("timeout", "early_exit"):
            with self.subTest(mode=mode):
                self.config["prompt"] = mode
                self.config["timeout_seconds"] = 1
                self.config["session_directory"] = str(self.root / mode)
                settings = self.root / (mode + ".json")
                session.write(settings, self.config)
                result = subprocess.run(
                    [sys.executable, "-B", str(INTEGRATION / "session.py"), str(settings)],
                    capture_output=True,
                    timeout=20,
                )
                self.assertEqual(result.returncode, 1 if mode == "timeout" else 7, result.stderr)
                pid = int((self.work / "descendant.pid").read_text())
                self.assertFalse(Path(f"/proc/{pid}").exists())

    def test_sigterm_reaps_group_without_touching_unrelated_child(self):
        self.config["prompt"] = "timeout"
        settings = self.root / "signal.json"
        session.write(settings, self.config)
        unrelated = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"])
        child = subprocess.Popen(
            [sys.executable, "-B", str(INTEGRATION / "session.py"), str(settings)],
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
            pid = int((self.work / "descendant.pid").read_text())
            self.assertFalse(Path(f"/proc/{pid}").exists())
            self.assertIsNone(unrelated.poll())
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            unrelated.terminate()
            unrelated.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
