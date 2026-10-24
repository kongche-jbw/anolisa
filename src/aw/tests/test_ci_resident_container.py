"""Keep the candidate image and fork profile isolated and evidence-based."""

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap
from unittest.mock import patch

from test_ci_checks import AW, GateFixture
import resident_container
from check_native import load, select
from native_acceptance import verify_bundle
from resident_container import source_files, stage
from resident_contract import BINARIES, suites, verify


class ResidentContainerTests(GateFixture):
    def setUp(self):
        super().setUp()
        self.manifest = load(AW)

    def reports(self):
        selected = suites(self.manifest)
        report = {
            "candidate_sha": "a" * 40,
            "passed": True,
            "suites": [],
            "runtime": {
                "python": selected["claude-sdk"]["python"],
                "node": "v24.16.0",
                "claude_cli": "2.1.161 (Claude Code)",
                "pi": "0.84.2",
                "python_packages": selected["claude-sdk"]["versions"],
            },
        }
        for agent, row in selected.items():
            for case in row["cases"]:
                report["suites"].append({"agent": agent, "name": case["name"]})
        (self.root / "result.json").write_text(json.dumps(report))
        cleanup = {
            "candidate_sha": "a" * 40,
            "network": "none",
            "mounts": [],
            "removed": True,
            "image_removed": True,
        }
        (self.root / "container.json").write_text(json.dumps(cleanup))
        return report, cleanup

    def test_fork_profile_preserves_all_credential_free_agents_and_full_default(self):
        self.assertEqual(
            set(select(self.manifest)["agents"]), set(self.manifest["agents"])
        )
        reduced = select(self.manifest, True)
        self.assertEqual(
            set(reduced["agents"]),
            set(self.manifest["agents"]) - {"qoder", "qoder-sdk"},
        )
        self.assertEqual(suites(reduced), suites(self.manifest))

    def test_container_tracks_inventory_additions_and_rejects_empty_or_invalid_commands(
        self,
    ):
        edited = copy.deepcopy(self.manifest)
        row = edited["agents"]["pi"]
        case = copy.deepcopy(
            next(c for c in row["cases"] if c["name"] == "resident-lifecycle")
        )
        case["name"] = "new-resident-case"
        row["cases"].append(case)
        self.assertIn(case, suites(edited)["pi"]["cases"])
        case["argv"].insert(1, "-c")
        with self.assertRaisesRegex(ValueError, "shared harness"):
            suites(edited)
        row["cases"] = []
        with self.assertRaisesRegex(ValueError, "missing"):
            suites(edited)

    def test_image_context_excludes_extra_payload_and_environment_files(self):
        binaries, context = self.root / "bin", self.root / "context"
        binaries.mkdir()
        context.mkdir()
        names = BINARIES | {
            "rtk",
            "policy",
            "aw-provider-sec-core",
            "agent-sec-cli",
            "agent-sec-daemon",
        }
        lines = []
        for name in names:
            (binaries / name).write_text(name)
            (binaries / name).chmod(0o700)
            lines.append(hashlib.sha256(name.encode()).hexdigest() + "  " + name)
        (binaries / "SHA256SUMS").write_text("\n".join(lines))
        (binaries / "source-sha.txt").write_text("a" * 40)
        for name in (".env", "credentials.json", "deployment.yaml"):
            (binaries / name).write_text("excluded-fixture-content")
        stage(AW, binaries, context, "a" * 40, self.manifest)
        self.assertEqual(
            {p.name for p in (context / "payload").iterdir()},
            BINARIES | {"SHA256SUMS", "source-sha.txt"},
        )
        verify_bundle(context / "payload", "a" * 40, BINARIES)
        for path in context.rglob("*"):
            if path.is_file():
                self.assertNotIn(b"excluded-fixture-content", path.read_bytes())
        self.assertTrue(
            all("target" not in path.relative_to(AW).parts for path in source_files(AW))
        )

    def test_result_requires_every_suite_actual_reports_versions_and_cleanup(self):
        report, cleanup = self.reports()
        # The outer success bit cannot replace real per-suite evidence.
        with self.assertRaises(ValueError):
            verify(self.root, self.manifest, "a" * 40)
        with patch("resident_contract.verify_case") as check:
            verify(self.root, self.manifest, "a" * 40)
            self.assertEqual(check.call_count, len(report["suites"]))
        for field, value in (
            ("passed", False),
            ("candidate_sha", "b" * 40),
            ("suites", report["suites"][:-1]),
        ):
            (self.root / "result.json").write_text(json.dumps({**report, field: value}))
            with self.subTest(field=field), self.assertRaises(ValueError):
                verify(self.root, self.manifest, "a" * 40)
        (self.root / "result.json").write_text(json.dumps(report))
        for field, value in (
            ("removed", False),
            ("image_removed", False),
            ("network", "bridge"),
            ("mounts", [{}]),
        ):
            (self.root / "container.json").write_text(
                json.dumps({**cleanup, field: value})
            )
            with self.subTest(field=field), patch(
                "resident_contract.verify_case"
            ), self.assertRaises(ValueError):
                verify(self.root, self.manifest, "a" * 40)

    def test_unmerged_profile_requires_manual_fork_exact_sha_and_no_secret(self):
        workflow = (AW.parents[1] / ".github/workflows/aw-preview.yml").read_text()
        block = workflow.split(
            "      - name: Require a trusted candidate before executing repository code\n",
            1,
        )[1]
        script = textwrap.dedent(
            block.split("        run: |\n", 1)[1].split(
                "      - name: Validate native coverage", 1
            )[0]
        )
        expected = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=AW, text=True
        ).strip()
        environment = {
            **os.environ,
            "CANDIDATE_SHA": expected,
            "WORKFLOW_SHA": expected,
            "WORKFLOW_REF": "refs/heads/feature/example",
            "REPOSITORY": "fixture/fork",
            "EVENT_NAME": "workflow_dispatch",
            "CREDENTIAL_FREE": "true",
        }
        for overrides, passed in (
            ({}, True),
            ({"REPOSITORY": "agentic-os-org/ANOLISA"}, False),
            ({"WORKFLOW_SHA": "b" * 40}, False),
            ({"EVENT_NAME": "pull_request"}, False),
            ({"WORKFLOW_REF": "refs/tags/fixture"}, False),
        ):
            result = subprocess.run(
                ["bash", "-euo", "pipefail", "-c", script],
                cwd=AW,
                env={**environment, **overrides},
                timeout=10,
                capture_output=True,
            )
            self.assertEqual(result.returncode == 0, passed, result.stderr)
        self.assertIn("!inputs.credential_free && (matrix.agent", workflow)
        caller = (AW.parents[1] / ".github/workflows/aw-ci.yml").read_text()
        self.assertIn(
            "!inputs.credential_free && secrets.QODER_PERSONAL_ACCESS_TOKEN", caller
        )
        container = workflow.split("  resident-container:\n", 1)[1].split(
            "  result:\n", 1
        )[0]
        self.assertNotIn("secrets.", container)

    def test_failed_container_execution_or_evidence_copy_still_removes_owned_resources(
        self,
    ):
        for failure in ("execution", "copy"):
            output = self.root / failure
            state = {"container": False, "image": False}

            def docker(argv, **kwargs):
                self.assertNotIn("QODER_PERSONAL_ACCESS_TOKEN", kwargs["env"])
                command = argv[1:]
                if command[0] == "create":
                    state["container"] = True
                    return "fixture-id"
                if command[0] == "inspect":
                    return json.dumps(
                        [
                            {
                                "Image": "fixture-image",
                                "Mounts": [],
                                "HostConfig": {"NetworkMode": "none"},
                            }
                        ]
                    )
                if command[0] == "cp" and failure == "copy":
                    raise subprocess.CalledProcessError(1, argv)
                if command[0] == "ps":
                    return "fixture-id" if state["container"] else ""
                if command[0] == "rm":
                    state["container"] = False
                if command[:2] == ["image", "ls"]:
                    return "fixture-image" if state["image"] else ""
                if command[:2] == ["image", "rm"]:
                    state["image"] = False
                return ""

            def run(_commands, argv, _cwd, _timeout):
                if argv[1] == "buildx":
                    state["image"] = True
                elif failure == "execution":
                    raise subprocess.TimeoutExpired(argv, 1)
                return ""

            argv = [
                "resident_container.py",
                "--candidate-sha",
                "a" * 40,
                "--bin",
                str(self.root),
                "--output",
                str(output),
            ]
            with patch.object(sys, "argv", argv), patch.object(
                resident_container, "git", side_effect=["a" * 40, ""]
            ), patch.object(resident_container, "stage", return_value={}), patch.object(
                resident_container.Commands, "run", autospec=True, side_effect=run
            ), patch.object(
                resident_container.subprocess, "check_output", side_effect=docker
            ), patch.dict(
                os.environ, {"QODER_PERSONAL_ACCESS_TOKEN": "excluded-fixture-token"}
            ):
                with self.assertRaises(
                    (subprocess.TimeoutExpired, subprocess.CalledProcessError)
                ):
                    resident_container.main()
            self.assertEqual(state, {"container": False, "image": False})
            cleanup = json.loads((output / "container.json").read_text())
            self.assertTrue(cleanup["removed"] and cleanup["image_removed"])


if __name__ == "__main__":
    import unittest

    unittest.main()
