#!/usr/bin/env python3
"""Run one governed Gateway checkpoint and retain content-free evidence."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import time
import uuid
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


os.umask(0o077)
POC_LINK = Path("/opt/anolisa-mvp/aw-provider-poc/current")
POC_ROOT = POC_LINK.resolve(strict=True)
if POC_ROOT.parent != POC_LINK.parent / "releases":
    raise RuntimeError(f"unsafe PoC release path: {POC_ROOT}")

GATEWAY = POC_ROOT / "bin/cosh-gateway"
WS_CKPT = Path("/opt/anolisa-mvp/bin/ws-ckpt")
GATEWAY_SOCKET = Path("/run/anolisa-aw-provider-poc/gateway.sock")
WS_CKPT_SOCKET = Path("/run/ws-ckpt-agent-work/ws-ckpt.sock")
WORKSPACE = Path("/var/lib/anolisa-agent-work/workspaces/interactive-agent")
AUDIT_LOG = Path("/var/lib/anolisa-aw-provider-poc/checkpoint-security.jsonl")
STATE_ROOT = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
RUNS_ROOT = STATE_ROOT / "aw-provider-poc" / "checkpoints"
EXPECTED_PROFILE = "workspace-checkpoint-v1"
EXPECTED_CHECKPOINT_MESSAGE = "COSH governed Task checkpoint"
CHECKPOINT_ID = re.compile(
    r"^ckp_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
DIGEST = re.compile(r"^[0-9a-f]{64}$")
UUID_BODY = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
TERMINAL_EVENTS = {"task_succeeded", "task_failed", "task_cancelled"}
EXPECTED_ORDER = [
    "approval_requested",
    "approval_resolved",
    "execution_planned",
    "execution_result_recorded",
    "task_succeeded",
]


def die(message: str) -> None:
    """Exit with one stable, user-facing error."""
    raise SystemExit(f"aw-checkpoint-demo: ERROR: {message}")


def is_canonical_id(value: Any, prefix: str) -> bool:
    """Check one public identity without accepting a prefix-only lookalike."""
    return isinstance(value, str) and re.fullmatch(f"{prefix}_{UUID_BODY}", value) is not None


def run(
    command: list[str],
    *,
    input_text: str | None = None,
    timeout_seconds: int = 10,
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run one bounded child command and retain diagnostics."""
    try:
        completed = subprocess.run(
            command,
            check=False,
            text=True,
            input=input_text,
            capture_output=True,
            timeout=timeout_seconds,
            env=environment,
        )
    except subprocess.TimeoutExpired as error:
        die(f"command timed out after {timeout_seconds}s: {command[0]} ({error})")
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or "no diagnostic"
        die(f"command failed ({completed.returncode}): {detail}")
    return completed


def run_json(
    command: list[str],
    *,
    input_text: str | None = None,
    timeout_seconds: int = 10,
) -> dict[str, Any]:
    """Run one command that must return exactly one JSON object."""
    completed = run(command, input_text=input_text, timeout_seconds=timeout_seconds)
    lines = [line for line in completed.stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        die(f"expected one JSON line from {command[0]}, received {len(lines)}")
    try:
        value = json.loads(lines[0])
    except json.JSONDecodeError as error:
        die(f"invalid JSON from {command[0]}: {error}")
    if not isinstance(value, dict):
        die(f"expected a JSON object from {command[0]}")
    if value.get("event") == "error":
        die(f"Gateway rejected the request: {value.get('code')}: {value.get('message')}")
    return value


def write_json(path: Path, value: Any) -> None:
    """Persist readable local evidence inside this invocation's private run."""
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def gateway_command(*arguments: str) -> list[str]:
    """Build one command against the dedicated PoC Gateway socket."""
    return [
        str(GATEWAY),
        "task",
        "--socket",
        str(GATEWAY_SOCKET),
        "--output",
        "jsonl",
        *arguments,
    ]


def require_capabilities() -> dict[str, Any]:
    """Fail before mutation when this build lacks the checkpoint profile."""
    if not GATEWAY.is_file() or not os.access(GATEWAY, os.X_OK):
        die(f"Gateway executable is unavailable: {GATEWAY}")
    if not WS_CKPT.is_file() or not os.access(WS_CKPT, os.X_OK):
        die(f"ws-ckpt executable is unavailable: {WS_CKPT}")
    help_text = run([str(GATEWAY), "serve", "--help"]).stdout
    required = [
        "--capability-profile",
        "workspace-checkpoint-v1",
        "--checkpoint-socket",
        "--security-audit",
    ]
    missing = [field for field in required if field not in help_text]
    if missing:
        die(f"Gateway build lacks checkpoint capabilities: {', '.join(missing)}")
    if not GATEWAY_SOCKET.is_socket():
        die(f"dedicated Gateway socket is not ready: {GATEWAY_SOCKET}")
    if not WS_CKPT_SOCKET.is_socket():
        die(f"ws-ckpt socket is not ready: {WS_CKPT_SOCKET}")
    admission = run_json(gateway_command("admission"))
    profile_identity = admission.get("capability_profile")
    workspace = admission.get("workspace")
    if not isinstance(profile_identity, dict) or not isinstance(workspace, dict):
        die("Gateway admission omitted its profile or workspace identity")
    profile = profile_identity.get("profile_id")
    if profile != EXPECTED_PROFILE:
        die(f"Gateway admitted profile {profile!r}, expected {EXPECTED_PROFILE!r}")
    for label, digest in [
        ("profile manifest", profile_identity.get("manifest_digest")),
        ("workspace scope", workspace.get("scope_digest")),
    ]:
        if not isinstance(digest, str) or not DIGEST.fullmatch(digest):
            die(f"Gateway admission has an invalid {label} digest")
    return admission


def snapshot_inventory() -> list[dict[str, Any]]:
    """Read the exact ws-ckpt inventory for the admitted registration path."""
    environment = dict(os.environ)
    environment["WS_CKPT_SOCKET"] = str(WS_CKPT_SOCKET)
    completed = run(
        [
            str(WS_CKPT),
            "list",
            "--workspace",
            str(WORKSPACE),
            "--format",
            "json",
        ],
        environment=environment,
    )
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        die(f"ws-ckpt returned invalid inventory JSON: {error}")
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        die("ws-ckpt returned an unsupported inventory shape")
    return value


def audit_records() -> list[dict[str, Any]]:
    """Read validated content-free security audit records."""
    try:
        lines = AUDIT_LOG.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        die(f"cannot read checkpoint audit log: {error}")
    if not lines or lines[0] != '{"schema":"cosh.gateway.security-audit-log.v1"}':
        die("checkpoint audit log has an invalid header")
    records: list[dict[str, Any]] = []
    for line in lines[1:]:
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            die(f"checkpoint audit log contains invalid JSON: {error}")
        if not isinstance(value, dict):
            die("checkpoint audit log contains a non-object record")
        records.append(value)
    return records


def event_kind(envelope: dict[str, Any]) -> str | None:
    """Return the closed Task event discriminator."""
    event = envelope.get("event")
    if not isinstance(event, dict):
        return None
    kind = event.get("event")
    return kind if isinstance(kind, str) else None


def poll_task(
    task_id: str, deadline_seconds: int, events_path: Path
) -> list[dict[str, Any]]:
    """Approve once, then follow bounded Task pages to one terminal state."""
    deadline = time.monotonic() + deadline_seconds
    cursor = 0
    events: list[dict[str, Any]] = []
    approval_id: str | None = None
    approval_resolved = False
    for _ in range(deadline_seconds * 4):
        if time.monotonic() >= deadline:
            break
        page = run_json(
            gateway_command("events", task_id, "--after", str(cursor), "--limit", "64")
        )
        page_events = page.get("events")
        if not isinstance(page_events, list):
            die("Gateway returned an invalid Task event page")
        for envelope in page_events:
            if not isinstance(envelope, dict):
                die("Gateway returned a non-object Task event")
            events.append(envelope)
            kind = event_kind(envelope)
            if kind == "approval_requested":
                value = envelope.get("event", {}).get("approval", {}).get("approval_id")
                if not is_canonical_id(value, "apr"):
                    die("approval request did not contain a canonical approval ID")
                if approval_id is not None and approval_id != value:
                    die("Task requested more than one checkpoint approval")
                approval_id = value
            if kind in TERMINAL_EVENTS:
                write_json(events_path, events)
                return events
        write_json(events_path, events)
        next_revision = page.get("next_revision")
        if not isinstance(next_revision, int) or next_revision < cursor:
            die("Gateway returned an invalid Task event cursor")
        cursor = next_revision
        if approval_id is not None and not approval_resolved:
            run_json(
                gateway_command(
                    "resolve-approval",
                    approval_id,
                    "--decision",
                    "approve",
                    "--idempotency-key",
                    f"aw-poc-approve-{uuid.uuid4().hex}",
                )
            )
            approval_resolved = True
        if not page.get("has_more"):
            time.sleep(0.25)
    die(f"Task {task_id} did not settle within {deadline_seconds}s")


def validate_event_order(events: list[dict[str, Any]]) -> dict[str, str]:
    """Require the complete approval, execution, and terminal sequence."""
    kinds = [kind for item in events if (kind := event_kind(item)) is not None]
    positions: list[int] = []
    for expected in EXPECTED_ORDER:
        if kinds.count(expected) != 1:
            die(f"Task evidence must contain exactly one {expected}: {kinds}")
        try:
            position = kinds.index(expected, positions[-1] + 1 if positions else 0)
        except ValueError:
            die(f"Task evidence is missing {expected}: {kinds}")
        positions.append(position)
    terminal = kinds[-1] if kinds else None
    if terminal != "task_succeeded":
        die(f"checkpoint Task ended as {terminal!r}")

    approval = next(item["event"] for item in events if event_kind(item) == "approval_requested")
    resolution = next(item["event"] for item in events if event_kind(item) == "approval_resolved")
    plan = next(item["event"] for item in events if event_kind(item) == "execution_planned")
    execution = next(
        item["event"] for item in events if event_kind(item) == "execution_result_recorded"
    )
    approval_id = approval["approval"]["approval_id"]
    if resolution.get("approval_id") != approval_id or resolution.get("decision") != "approve":
        die("checkpoint approval resolution is not the requested demo approval")
    execution_id = execution.get("execution_id")
    permit_id = plan.get("permit_id")
    if not is_canonical_id(execution_id, "exe"):
        die("checkpoint result did not contain a canonical execution ID")
    if plan.get("execution_id") != execution_id or not is_canonical_id(permit_id, "prm"):
        die("checkpoint permit and execution plan are not correlated to the result")
    outcome = execution.get("outcome", {})
    if outcome.get("outcome") != "succeeded":
        die(f"checkpoint execution did not succeed: {outcome}")
    evidence_ref = outcome.get("evidence_ref")
    if not isinstance(evidence_ref, str) or not DIGEST.fullmatch(evidence_ref):
        die("checkpoint execution did not retain a canonical receipt digest")
    return {
        "approval_id": approval_id,
        "approval_decision": resolution["decision"],
        "permit_id": permit_id,
        "execution_id": execution_id,
        "evidence_ref": evidence_ref,
    }


def create_marker(run_id: str) -> Path:
    """Create one exact live-workspace marker that the snapshot will capture."""
    marker_dir = WORKSPACE / ".aw-provider-poc"
    if marker_dir.is_symlink():
        die(f"refusing a symlink marker directory: {marker_dir}")
    marker_dir.mkdir(mode=0o700, exist_ok=True)
    marker = marker_dir / f"{run_id}.txt"
    descriptor = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.write(descriptor, f"AW checkpoint PoC marker {run_id}\n".encode())
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    return marker


def remove_marker(marker: Path) -> None:
    """Remove only the live marker created by this invocation."""
    try:
        marker.unlink(missing_ok=True)
        marker.parent.rmdir()
    except OSError:
        pass


def main() -> None:
    """Run and display one complete governed checkpoint trace."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--deadline-seconds", type=int, default=90, choices=range(15, 181))
    args = parser.parse_args()

    admission = require_capabilities()
    before_snapshots = snapshot_inventory()
    before_ids = {item.get("id") for item in before_snapshots if isinstance(item.get("id"), str)}
    before_audit = audit_records()

    RUNS_ROOT.mkdir(parents=True, mode=0o700, exist_ok=True)
    run_id = uuid.uuid4().hex
    run_dir = RUNS_ROOT / f"run.{run_id}"
    run_dir.mkdir(mode=0o700)
    marker = create_marker(run_id)
    try:
        submitted = run_json(
            gateway_command(
                "submit",
                "--idempotency-key",
                f"aw-poc-submit-{run_id}",
            ),
            input_text="Create one governed workspace checkpoint for this demonstration.\n",
        )
        task_id = submitted.get("task_id")
        if not is_canonical_id(task_id, "tsk"):
            die("Gateway submission did not return a canonical Task ID")
        write_json(run_dir / "submission.json", submitted)
        events = poll_task(task_id, args.deadline_seconds, run_dir / "task-events.json")
        identities = validate_event_order(events)
        task = run_json(gateway_command("get", task_id))
        if task.get("state") != "succeeded":
            die(f"checkpoint Task projection is {task.get('state')!r}")
        active_run_id = task.get("active_run_id")
        if not is_canonical_id(active_run_id, "run"):
            die("checkpoint Task did not retain a canonical active Run ID")

        after_snapshots = snapshot_inventory()
        created = [
            item
            for item in after_snapshots
            if isinstance(item.get("id"), str) and item["id"] not in before_ids
        ]
        if len(created) != 1:
            die(f"expected exactly one new checkpoint, observed {len(created)}")
        checkpoint = created[0]
        checkpoint_id = checkpoint.get("id")
        checkpoint_meta = checkpoint.get("meta")
        if not isinstance(checkpoint_id, str) or not CHECKPOINT_ID.fullmatch(checkpoint_id):
            die(f"new checkpoint has a non-canonical ID: {checkpoint_id!r}")
        if checkpoint.get("workspace") != str(WORKSPACE):
            die("new checkpoint is not bound to the configured registration path")
        if not isinstance(checkpoint_meta, dict) or checkpoint_meta.get(
            "message"
        ) != EXPECTED_CHECKPOINT_MESSAGE:
            die("new checkpoint does not carry the governed Gateway message")

        after_audit = audit_records()
        if after_audit[: len(before_audit)] != before_audit:
            die("security audit history changed instead of appending")
        new_audit = after_audit[len(before_audit) :]
        if len(new_audit) != 1:
            die(f"expected exactly one new security audit record, observed {len(new_audit)}")
        audit = new_audit[0]
        if audit.get("schema") != "cosh.gateway.security-audit.v1":
            die("security audit record has an unsupported schema")
        if (
            audit.get("execution_id") != identities["execution_id"]
            or audit.get("task_id") != task_id
            or audit.get("run_id") != active_run_id
        ):
            die("security audit identities do not match the Task and execution")
        for field in ["operation_digest", "target_identity_digest"]:
            digest = audit.get(field)
            if not isinstance(digest, str) or not DIGEST.fullmatch(digest):
                die(f"security audit contains an invalid {field}")

        summary = {
            "schema": "aw.provider.checkpoint-vm-demo/v1",
            "completed_at": datetime.now(UTC).isoformat(),
            "gateway": {
                "profile_id": admission["capability_profile"]["profile_id"],
                "manifest_digest": admission["capability_profile"]["manifest_digest"],
                "target": admission["target"],
                "workspace_scope_digest": admission["workspace"]["scope_digest"],
            },
            "task": {
                "task_id": task_id,
                "run_id": active_run_id,
                "state": task["state"],
                **identities,
            },
            "checkpoint": checkpoint,
            "security_audit": audit,
            "invariants": {
                "approval_precedes_execution": True,
                "guarded_v2_result_is_conclusive": True,
                "exactly_one_new_snapshot": True,
                "demo_runner_resolved_one_approval": True,
                "registration_path": str(WORKSPACE),
            },
            "coverage": {
                "governed_create_exercised": True,
                "recovery_protocol_fields_validated": True,
                "fault_or_restart_recovery_exercised": False,
            },
        }
        write_json(run_dir / "task-events.json", events)
        write_json(run_dir / "summary.json", summary)
    finally:
        remove_marker(marker)

    print("GOVERNED CHECKPOINT E2E")
    print("=" * 78)
    print(f"Evidence      {run_dir}")
    print(f"Profile       {summary['gateway']['profile_id']}")
    print(f"Task          {summary['task']['task_id']} · {summary['task']['state']}")
    print(
        f"Approval      {summary['task']['approval_id']} · "
        f"decision={summary['task']['approval_decision']} (demo runner)"
    )
    print(f"Execution     {summary['task']['execution_id']}")
    print(f"Permit        {summary['task']['permit_id']}")
    print(f"Checkpoint    {summary['checkpoint']['id']}")
    print(f"Operation     sha256={summary['security_audit']['operation_digest']}")
    print("Coverage      governed create + recovery fields; no fault injection")


if __name__ == "__main__":
    main()
