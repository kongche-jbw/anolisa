#!/usr/bin/env python3
"""Run a bounded sequence of native Qoder prompts with explicit reset boundaries."""

import argparse
import json
import os
import stat
from pathlib import Path
import sys
import uuid

import session
from session_process import Processes


def validate(config):
    if (
        not isinstance(config, dict)
        or set(config) != {"format", "launch", "turns"}
        or type(config["format"]) is not int
        or config["format"] != 1
        or not isinstance(config["launch"], dict)
        or "prompt" in config["launch"]
        or not isinstance(config["turns"], list)
        or not 1 <= len(config["turns"]) <= 8
    ):
        raise ValueError("expected format 1, launch settings and one to eight turns")
    root = Path(config["launch"].get("session_directory", ""))
    for number, turn in enumerate(config["turns"], 1):
        if (
            not isinstance(turn, dict)
            or "prompt" not in turn
            or set(turn) - {"prompt", "reset"}
            or ("reset" in turn and type(turn["reset"]) is not bool)
        ):
            raise ValueError("turn requires a prompt and optional boolean reset")
        session.validate(
            {
                **config["launch"],
                "session_directory": str(root / f"turn-{number:04d}"),
                "prompt": turn["prompt"],
            }
        )


def require_history(config, session_id):
    path = session.history_path(config, session_id)
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or not 1 <= info.st_size <= 16777216:
            raise ValueError("resume requires a nonempty bounded native history")
        contents = stream.read(16777217)
    if len(contents) > 16777216 or not contents.endswith(b"\n"):
        raise ValueError("resume requires complete native history rows")
    for row in contents.splitlines():
        if not isinstance(json.loads(row, object_pairs_hook=session.unique), dict):
            raise ValueError("resume requires native history objects")


def launch(config):
    """Retain attempted turns, stopping before the next turn on failure or cancellation."""
    validate(config)
    root = Path(config["launch"]["session_directory"])
    root.mkdir(mode=0o700)
    conversation = str(uuid.uuid4())
    summary = {
        "format": 1,
        "conversation_id": conversation,
        "status": "stopped",
        "requested_turns": len(config["turns"]),
        "turns": [],
    }
    try:
        session.write(root / "conversation.json", config)
        with Processes() as processes:
            session_id = None
            for number, turn in enumerate(config["turns"], 1):
                if processes.cancelled:
                    raise RuntimeError("conversation cancelled")
                resume = session_id is not None and not turn.get("reset", False)
                if resume:
                    require_history(config["launch"], session_id)
                else:
                    session_id = str(uuid.uuid4())
                identity = {
                    "session_id": session_id,
                    "turn_id": str(uuid.uuid4()),
                    "runtime_id": conversation,
                    "environment_id": conversation,
                    "runtime_generation": number,
                    "resume": resume,
                }
                directory = f"turn-{number:04d}"
                entry = {
                    "identity": identity,
                    "directory": directory,
                    "status": "failed",
                }
                summary["turns"].append(entry)
                status = session.launch(
                    {
                        **config["launch"],
                        "session_directory": str(root / directory),
                        "prompt": turn["prompt"],
                    },
                    identity=identity,
                    processes=processes,
                )
                entry["agent_exit_code"] = status
                if status:
                    return status
                entry["status"] = "completed"
            if processes.cancelled:
                raise RuntimeError("conversation cancelled")
            summary["status"] = "completed"
        return 0
    finally:
        # This index carries no output body or replayable process authority.
        # Each turn retains its own result and independent adoption evidence.
        session.write(root / "summary.json", summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("settings", help="absolute private conversation JSON")
    args = parser.parse_args()
    path = Path(args.settings)
    if not path.is_absolute():
        parser.error("absolute settings path required")
    return launch(session.read(path, private=True))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, TypeError, KeyError) as error:
        print(f"aw-qoder-conversation: {error}", file=sys.stderr)
        raise SystemExit(1)
