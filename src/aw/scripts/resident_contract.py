"""Share container suite selection and evidence checks with nightly aggregation."""

import json
from pathlib import Path

from native_reports import validate_case, verify_case

BINARIES = {"aw", "aw-provider-tokenless", "tokenless"}


def suites(manifest: dict) -> dict:
    """Reuse every resident command and its exact report from the native inventory."""
    selected = {}
    for agent in ("claude-sdk", "pi"):
        row = manifest["agents"][agent]
        cases = [
            case for case in row["cases"] if "tests/sdk_resident.py" in case["argv"]
        ]
        if not cases or len({case["name"] for case in cases}) != len(cases):
            raise ValueError(f"{agent}: missing or duplicate resident suites")
        for case in cases:
            if case["argv"][:2] != ["{python}", "tests/sdk_resident.py"]:
                raise ValueError("resident suite must execute the shared harness")
            if case["report"] != "json":
                raise ValueError("resident container requires named JSON evidence")
            validate_case(case)
        selected[agent] = {**row, "cases": cases}
    if len({row["python"] for row in selected.values()}) != 1:
        raise ValueError("resident image requires a shared pinned Python runtime")
    return selected


def verify(directory: Path, manifest: dict, expected: str) -> dict:
    """Recheck individual native reports instead of trusting the container exit code."""
    report = json.loads((directory / "result.json").read_text())
    if report.get("candidate_sha") != expected or report.get("passed") is not True:
        raise ValueError("resident container failed or belongs to another candidate")
    selected = suites(manifest)
    keys = {
        (agent, case["name"])
        for agent, row in selected.items()
        for case in row["cases"]
    }
    completed = report.get("suites", [])
    if (
        len(completed) != len(keys)
        or {(item["agent"], item["name"]) for item in completed} != keys
    ):
        raise ValueError("resident container has missing, duplicate or unknown suites")
    for agent, row in selected.items():
        for case in row["cases"]:
            verify_case(case, "", directory / agent)
    runtime = report["runtime"]
    if runtime["python"] != next(iter(selected.values()))["python"]:
        raise ValueError("resident Python version differs from the native inventory")
    if (
        runtime["node"] != "v24.16.0"
        or runtime["claude_cli"] != "2.1.161 (Claude Code)"
    ):
        raise ValueError("resident native runtime version changed")
    for name, version in selected["claude-sdk"]["versions"].items():
        if runtime["python_packages"].get(name) != version:
            raise ValueError(f"resident dependency version mismatch: {name}")
    if runtime["pi"] != "0.84.2":
        raise ValueError("resident Pi version changed")
    cleanup = json.loads((directory / "container.json").read_text())
    if (
        cleanup.get("candidate_sha") != expected
        or cleanup.get("network") != "none"
        or cleanup.get("mounts") != []
        or cleanup.get("removed") is not True
        or cleanup.get("image_removed") is not True
    ):
        raise ValueError(
            "resident container isolation or cleanup evidence is incomplete"
        )
    return report
