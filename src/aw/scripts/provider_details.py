#!/usr/bin/env python3
"""Inspect actual provider configuration and session-verified execution details."""

import argparse
import curses
import json
import os
import sys
from pathlib import Path
import textwrap
import time

from session_hooks import read, ticks, write


def snapshot(root: Path, config: dict, view: dict) -> dict:
    """Extract content-free detail only after Rust verifies the same session evidence."""
    calls = []
    events = set()
    for key in view["events"]:
        path = root / "evidence" / f"{key}.json"
        event = read(path)
        if event["execution"]["scope"]["session_id"] != view["scope"]["session_id"]:
            continue
        events.add(event["event_key"])
        for call in event["calls"]:
            receipt = call["receipt"]
            mapping = event.get("tokenless_mapping") or {}
            if mapping.get("invocation_id") != receipt["invocation_id"]:
                mapping = {}
            output = call.get("output") or {}
            calls.append(
                {
                    "provider_id": receipt["provider_id"],
                    "version": receipt["provider_version"],
                    "manifest_digest": receipt["manifest_digest"],
                    "invocation_id": receipt["invocation_id"],
                    "tool_use_id": receipt["scope"]["tool_use_id"],
                    "status": receipt["disposition"],
                    "completed_at_ms": receipt["completed_at_ms"],
                    "elapsed_ms": receipt["completed_at_ms"] - receipt["started_at_ms"],
                    "verdict": output.get("inspection", {}).get("verdict"),
                    "findings": output.get("inspection", {}).get("findings", []),
                    "coverage": output.get("inspection", {}).get("coverage"),
                    "error": receipt.get("error_code"),
                    "native_disposition": mapping.get("native_disposition"),
                    "projection_result": mapping.get("projection_result"),
                    "operations": mapping.get("applied_operations", []),
                    "source_bytes": event["source_bytes"],
                    "candidate_bytes": event.get("candidate_bytes"),
                }
            )
    calls.sort(key=lambda call: call["completed_at_ms"], reverse=True)
    return {
        "status": "verified",
        "verified_at_ms": int(time.time() * 1000),
        "session_id": view["scope"]["session_id"],
        "providers": config["providers"],
        "counters": view["providers"],
        "bash_results": len(events),
        "recent_calls": calls[:40],
    }


def outcome(call: dict) -> str:
    if call.get("error"):
        return "failed: " + call["error"]
    if call.get("verdict"):
        count = sum(f["count"] for f in call.get("findings", []))
        return "inspection: " + call["verdict"] + (f" | {count} findings" if count else "")
    if call.get("projection_result") == "output_not_smaller":
        return "preserved: output not smaller"
    native = call.get("native_disposition")
    if native:
        return {
            "no_savings": "preserved: no savings",
            "passthrough": "preserved: passthrough",
            "dry_run": "preserved: dry run",
            "recoverability_unavailable": "preserved: recovery unavailable",
            "applied": "compression candidate prepared",
        }.get(native, native)
    if call["status"] == "bypassed":
        return "preserved: native reason not recorded"
    return call["status"]


def sidebar(details: dict) -> dict:
    rows = {row["provider_id"]: row for row in details["counters"]}
    tokens = {"aw": f"AW | {details['bash_results']} Bash results"}
    for identity, key, label in (
        ("sec-core", "aw_sec", "SecCore"),
        ("tokenless", "aw_tokenless", "Tokenless"),
    ):
        provider = details["providers"][identity]
        row = rows.get(identity, {})
        tokens[key] = f"{label} {provider['version']} | {row.get('calls', 0)} calls"
        latest = next((c for c in details["recent_calls"] if c["provider_id"] == identity), None)
        tokens[key + "_result"] = outcome(latest) if latest else "ready | no calls yet"
    projection = rows.get("tokenless", {})
    tokens["aw_savings"] = (
        f"History: {projection.get('adopted', 0)} adopted | "
        f"saved {projection.get('saved_bytes', 0)} B"
    )
    return tokens


def inspection_lines(call: dict) -> list[str]:
    result = []
    coverage = call.get("coverage")
    if coverage:
        result += [
            f"Scanned: {coverage['scanned_bytes']}/{coverage['input_bytes']} B | complete: {coverage['complete']}"
        ]
        result += ["Ruleset: " + ", ".join(coverage["ruleset_ids"])]
    for finding in call.get("findings", []):
        result += [
            f"Finding: {finding['rule_id']} | count {finding['count']} | severity {finding['severity']}"
        ]
    if call.get("verdict"):
        result += ["Post-tool observation only; command already ran, original content retained."]
    return result


def lines(root: Path, page: int) -> list[str]:
    config = read(root / "launch.json")
    path = root / "provider-details.json"
    details = read(path) if path.exists() else {"status": "waiting"}
    status = details["status"]
    age = int(time.time() * 1000) - details.get("verified_at_ms", 0)
    heading = []
    heading += [f"Session: {details.get('session_id', config['session_id'])}"]
    heading += [
        f"Evidence: {status}" + (f" | checked {age // 1000}s ago" if status == "verified" else "")
    ]
    if config.get("agent_kind", "qoder") == "codex":
        heading += [
            "Codex native terminal attached. AW hooks are not connected in this entry.",
            "Provider configuration below is not evidence of execution.",
            "",
        ]
    else:
        heading += [
            "Coverage: Bash post-tool only; local history adoption, not model consumption.",
            "",
        ]
    if page == 1:
        for identity, provider in config["providers"].items():
            heading += [
                f"{identity}  {provider['version']}  | native protocol v{provider['native_protocol']}"
            ]
            heading += ["Program: " + provider["program"]]
            if "source" in provider:
                heading += ["Source: " + provider["source"]]
            row = next(
                (r for r in details.get("counters", []) if r["provider_id"] == identity), None
            )
            if status == "verified" and row:
                heading += [
                    f"Calls {row['calls']} | failed {row['failed']} | preserved {row['bypassed']}"
                ]
                if identity == "tokenless":
                    heading += [
                        f"Candidates {row['candidates']} | history adopted {row['adopted']} | saved {row['saved_bytes']} B"
                    ]
                latest = next(
                    (c for c in details["recent_calls"] if c["provider_id"] == identity), None
                )
                if latest:
                    heading += [
                        "Last: " + outcome(latest),
                        "Manifest digest: " + latest["manifest_digest"],
                    ]
                    heading += inspection_lines(latest)
            else:
                heading += ["Configuration discovered; execution is not currently verified."]
            heading += [""]
        heading += ["Evidence directory: " + str(root)]
    elif status == "verified":
        for call in details["recent_calls"]:
            heading += [
                f"{call['provider_id']} {call['version']} | {outcome(call)} | {call['elapsed_ms']} ms"
            ]
            heading += ["Invocation: " + call["invocation_id"], "Tool: " + call["tool_use_id"]]
            heading += inspection_lines(call)
            if call["provider_id"] == "tokenless":
                heading += [
                    f"Source {call['source_bytes']} B | candidate {call['candidate_bytes']} B"
                ]
                heading += ["Operations: " + (", ".join(call["operations"]) or "none recorded")]
            heading += [""]
    else:
        heading += [
            "No currently verified call details; wait for verification or restart the session."
        ]
    return heading


def show(screen, root: Path) -> None:
    curses.curs_set(0)
    curses.start_color()
    curses.use_default_colors()
    curses.init_pair(1, curses.COLOR_CYAN, -1)
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    curses.mouseinterval(0)
    screen.timeout(500)
    page, offset = 1, 0
    deadline = time.monotonic() + read(root / "launch.json")["duration_seconds"]
    while time.monotonic() < deadline:
        height, width = screen.getmaxyx()
        content = []
        for line in lines(root, page):
            safe = "".join(c if c.isprintable() else " " for c in line)
            content.extend(textwrap.wrap(safe, max(1, width - 2)) or [""])
        offset = min(offset, max(0, len(content) - max(0, height - 4)))
        screen.erase()
        header = [
            "AW PROVIDERS",
            "[1 Overview] [2 Recent calls] [q Close]",
            f"Page {page} | Click tabs / wheel scroll / Up Down",
        ]
        for y, line in enumerate(header[: max(0, height - 1)]):
            screen.addnstr(y, 0, line, max(0, width - 1), curses.color_pair(1) | curses.A_BOLD)
        for y, line in enumerate(content[offset : offset + max(0, height - 4)], 3):
            screen.addnstr(y, 0, line, max(0, width - 1))
        screen.refresh()
        key = screen.getch()
        if key == curses.KEY_MOUSE:
            _, x, y, _, buttons = curses.getmouse()
            if buttons & (curses.BUTTON1_PRESSED | curses.BUTTON1_CLICKED) and y == 1:
                if 0 <= x < 12:
                    page, offset = 1, 0
                elif 13 <= x < 29:
                    page, offset = 2, 0
                elif 30 <= x < 39:
                    return
            elif buttons & curses.BUTTON4_PRESSED:
                offset = max(0, offset - 3)
            elif buttons & curses.BUTTON5_PRESSED:
                offset += 3
        if key in (ord("q"), 27):
            return
        if key in (ord("1"), ord("2")):
            page, offset = key - ord("0"), 0
        elif key in (curses.KEY_DOWN, ord("j")):
            offset += 1
        elif key in (curses.KEY_UP, ord("k")):
            offset = max(0, offset - 1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("session", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.json:
        print(json.dumps(read(args.session / "provider-details.json"), indent=2))
    else:
        record = args.session / f"provider-panel-{os.getpid()}.json"
        ownership = {
            "pid": os.getpid(),
            "start_ticks": ticks(os.getpid()),
            "command": [sys.executable, str(Path(__file__).resolve()), str(args.session)],
            "cwd": str(Path.cwd()),
            "duration_seconds": read(args.session / "launch.json")["duration_seconds"],
            "stop": f"kill -TERM {os.getpid()}",
        }
        write(record, ownership)
        try:
            curses.wrapper(show, args.session)
        finally:
            write(record, {**ownership, "exited": True})
