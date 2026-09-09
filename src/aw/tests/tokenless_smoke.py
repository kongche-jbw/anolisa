#!/usr/bin/env python3
"""Run one synthetic Qoder projection and independently inspect its local history."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import time
import uuid

import native_smoke


def checked(command, **kwargs):
    return subprocess.run(
        command, check=True, capture_output=True, timeout=20, **kwargs
    )


def run(args):
    root = args.output_dir.resolve()
    root.mkdir(mode=0o700, parents=False, exist_ok=False)
    work, home = root / "work", root / "temporary-home"
    work.mkdir(mode=0o700)
    checked(["git", "init", "--quiet", str(work)])
    home.mkdir(mode=0o700)
    (home / "tmp").mkdir()
    (root / "tokenless-data").mkdir(mode=0o700)
    user_settings_path = Path.home() / ".qoder" / "settings.json"
    previous_trust = (
        json.loads(user_settings_path.read_text())
        .get("permissions", {})
        .get("trustDirectories", [])
    )
    if str(work) in previous_trust:
        raise RuntimeError("Fresh test directory is already trusted")
    plugins = json.loads(checked([args.qoder, "plugins", "list", "--json"]).stdout)
    if plugins or json.loads(user_settings_path.read_text()).get("hooks"):
        raise RuntimeError(
            "This smoke requires no Qoder user hooks/plugins; do not modify user settings"
        )
    native_smoke.write_json(root / "plugins-before.json", plugins)
    session = str(uuid.uuid4())
    project = (
        Path.home()
        / ".qoder"
        / "projects"
        / str(work).replace("/", "-").replace("_", "-")
    )
    transcript = project / f"{session}.jsonl"
    state = project / session
    if transcript.exists() or state.exists():
        raise RuntimeError("Synthetic session already exists")
    fixture = {
        "records": [
            {
                "id": n,
                "status": "success",
                "message": "deterministic synthetic build record",
                "duration_ms": 1250,
            }
            for n in range(32)
        ]
    }
    text = (
        "unchanged\n"
        if args.case == "no-gain"
        else json.dumps(fixture, indent=2) + "\n"
    )
    (work / "fixture.json").write_text(text)
    native_smoke.write_json(
        root / "synthetic-fixture.json",
        {
            "command": "cat fixture.json",
            "source_file_digest": hashlib.sha256(text.encode()).hexdigest(),
            "synthetic": True,
        },
    )
    settings = root / "settings.json"
    capture = root / "capture.py"
    capture.write_text(
        "import os,sys\nraw=sys.stdin.buffer.read(1048577)\nif len(raw)>1048576: raise SystemExit(1)\nwith open(sys.argv[1],'xb') as f: f.write(raw)\nwith open(sys.argv[1],'rb') as f: os.dup2(f.fileno(),0)\nos.execv(sys.argv[2],sys.argv[2:])\n"
    )
    hook = shlex.join(
        [
            sys.executable,
            str(capture),
            str(root / "native-event.json"),
            str(args.hook_bin),
            "qoder",
            str(settings),
        ]
    )
    (work / ".qoder").mkdir()
    native_smoke.write_json(
        work / ".qoder" / "settings.json",
        {
            "general": {"enableAutoUpdate": False},
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": hook, "timeout": 30}],
                    }
                ]
            },
        },
    )
    command = [
        args.qoder,
        "--model",
        "auto",
        "--setting-sources",
        "project",
        "--session-id",
        session,
        "--strict-mcp-config",
        "--mcp-config",
        '{"mcpServers":{}}',
        "--tools",
        "Bash",
        "--allowed-tools",
        "Bash",
        "--max-model-request-retries",
        "0",
        "-i" if args.interactive else "-p",
        "Execute exactly one Bash call with this command: `cat fixture.json`. This is a read-only synthetic AW integration fixture. Do not run any other tools. After the result, say done and finish.",
    ]
    prompt = command[-1]
    if args.interactive:
        del command[-2:]
        index = command.index("--setting-sources")
        del command[index : index + 2]
    launcher = root / "launch.py"
    launcher.write_text(
        "import os,sys,time\nfrom pathlib import Path\nend=time.monotonic()+5\nwhile not Path(sys.argv[1]).exists():\n if time.monotonic()>end: raise SystemExit(90)\n time.sleep(.02)\nos.execv(sys.argv[2],sys.argv[2:])\n"
    )
    lifecycle = {
        "runner_pid": os.getpid(),
        "runner_start_ticks": native_smoke.start_ticks(os.getpid()),
        "command": command,
        "working_directory": str(work),
        "expected_lifetime_seconds": 150 + args.hold_seconds,
        "session_id": session,
        "transcript": str(transcript),
        "authentication": "existing login in place, not copied",
        "started_at_ms": int(time.time() * 1000),
        "qoder_version": checked([args.qoder, "--version"]).stdout.decode().strip(),
        "tokenless_version": checked([str(args.tokenless_bin), "--version"])
        .stdout.decode()
        .strip(),
    }
    native_smoke.write_json(root / "lifecycle.json", lifecycle)
    process = None
    result = {"status": "failed"}
    try:
        with (root / "qoder.stdout.log").open("wb") as stdout, (
            root / "qoder.stderr.log"
        ).open("wb") as stderr:
            terminal = (
                open("/dev/tty", "r+b", buffering=0) if args.interactive else None
            )
            process = subprocess.Popen(
                [sys.executable, str(launcher), str(settings), *command],
                cwd=work,
                env=native_smoke.qoder_environment(home),
                stdin=terminal if terminal is not None else subprocess.DEVNULL,
                stdout=terminal if terminal is not None else stdout,
                stderr=terminal if terminal is not None else stderr,
                start_new_session=not args.interactive,
            )
            if terminal is not None:
                terminal.close()
            lifecycle.update(
                agent_pid=process.pid,
                agent_start_ticks=native_smoke.start_ticks(process.pid),
                stop_command=(
                    f"kill -TERM {process.pid}"
                    if args.interactive
                    else f"kill -TERM -- -{process.pid}"
                ),
            )
            native_smoke.write_json(root / "lifecycle.json", lifecycle)
            print(json.dumps(lifecycle), flush=True)
            config = native_smoke.make_settings(
                argparse.Namespace(
                    case="clean",
                    host="qoder",
                    provider_version=args.provider_version,
                    provider_python=args.provider_python,
                    provider_source=args.provider_source,
                ),
                process,
                root,
            )
            config["scope"]["session_id"] = session
            config["tokenless"] = {
                "provider_id": "tokenless",
                "allow_unrecoverable": True,
                "provider_version": args.tokenless_version,
                "program": str(args.tokenless_bin),
                "args": ["compress"],
                "environment": {"TOKENLESS_DATA_DIR": str(root / "tokenless-data")},
            }
            if args.case == "provider-failure":
                config["tokenless"].update(program="/bin/sh", args=["-c", "exit 23"])
            native_smoke.write_json(settings, config)
            if args.interactive:
                sys.path.insert(
                    0,
                    str(Path(__file__).resolve().parents[1] / "integrations" / "herdr"),
                )
                import bridge

                ready_deadline = time.monotonic() + 20
                trust_accepted = False
                while time.monotonic() < ready_deadline:
                    pane = bridge.rpc(
                        args.herdr_socket, "pane.get", {"pane_id": args.herdr_pane_id}
                    )
                    screen = bridge.rpc(
                        args.herdr_socket,
                        "pane.read",
                        {"pane_id": args.herdr_pane_id, "source": "visible"},
                    )
                    screen = screen.get("read", screen)
                    visible = screen.get("text", "")
                    if "1. Trust folder" in visible and not trust_accepted:
                        if root.name not in visible:
                            raise RuntimeError(
                                "Trust dialog does not identify owned workspace"
                            )
                        bridge.rpc(
                            args.herdr_socket,
                            "pane.send_input",
                            {"pane_id": args.herdr_pane_id, "keys": ["Enter"]},
                        )
                        trust_accepted = True
                        lifecycle["owned_workspace_trust_accepted"] = True
                        native_smoke.write_json(root / "lifecycle.json", lifecycle)
                        time.sleep(0.5)
                        continue
                    if "Qoder" in visible and "Trust folder" not in visible:
                        native_smoke.write_json(
                            root / "qoder-ready-screen.json", screen
                        )
                        break
                    time.sleep(0.1)
                else:
                    raise RuntimeError(
                        "Qoder interactive prompt did not become visible"
                    )
                time.sleep(1)
                bridge.rpc(
                    args.herdr_socket,
                    "pane.send_input",
                    {"pane_id": args.herdr_pane_id, "text": prompt},
                )
                time.sleep(0.2)
                bridge.rpc(
                    args.herdr_socket,
                    "pane.send_keys",
                    {"pane_id": args.herdr_pane_id, "keys": ["Enter"]},
                )
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                if (
                    args.interactive
                    and transcript.is_file()
                    and (root / "native-event.json").is_file()
                ):
                    captured = json.loads((root / "native-event.json").read_text())
                    found = False
                    for line in transcript.read_text().splitlines():
                        try:
                            row = json.loads(line)
                        except ValueError:
                            continue
                        content = row.get("message", {}).get("content", [])
                        if row.get("type") == "user" and isinstance(content, list):
                            found |= any(
                                isinstance(block, dict)
                                and block.get("type") == "tool_result"
                                and block.get("tool_use_id")
                                == captured.get("tool_use_id")
                                for block in content
                            )
                    if found:
                        break
                observed = os.waitid(
                    os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT
                )
                if observed is not None:
                    if observed.si_code != os.CLD_EXITED or observed.si_status != 0:
                        raise RuntimeError("Qoder failed; inspect owned CLI logs")
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("Qoder exceeded the 120 second deadline")
        native = json.loads((root / "native-event.json").read_text())
        if (
            native["session_id"] != session
            or native["cwd"] != str(work)
            or native["transcript_path"] != str(transcript)
        ):
            raise RuntimeError(
                "Native session/work/transcript differs from owned launch"
            )
        files = list((root / "evidence").glob("*.json"))
        if len(files) != 1:
            raise RuntimeError("Expected exactly one Core event")
        event = json.loads(files[0].read_text())
        scope = event["execution"]["scope"]
        binding = {
            k: config[k]
            for k in [
                "runtime",
                "agent_pid",
                "agent_start_ticks",
                "journal",
                "evidence",
            ]
        }
        binding["scope"] = scope
        adoption_binding = {
            **binding,
            "started_at_ms": lifecycle["started_at_ms"],
            "workspace": str(work),
            "native_event": str(root / "native-event.json"),
            "adoption_journal": str(root / "adoption-journal"),
        }
        native_smoke.write_json(root / "adoption-binding.json", adoption_binding)
        recorded = checked(
            [
                str(args.adoption_bin),
                "record",
                str(root / "adoption-binding.json"),
                event["event_key"],
            ]
        )
        (root / "adoption-record.json").write_bytes(recorded.stdout)
        verified = checked(
            [
                str(args.adoption_bin),
                "verify",
                str(root / "adoption-binding.json"),
                event["event_key"],
            ]
        )
        (root / "adoption-verified.json").write_bytes(verified.stdout)
        binding["adoption_bindings"] = {
            event["event_key"]: str(root / "adoption-binding.json")
        }
        native_smoke.write_json(root / "view-binding.json", binding)
        displayed = checked([str(args.view_bin), str(root / "view-binding.json")])
        (root / "view.json").write_bytes(displayed.stdout)
        adoption = json.loads(verified.stdout)
        view = json.loads(displayed.stdout)
        projection = [
            c for c in event["calls"] if c["receipt"]["provider_id"] == "tokenless"
        ]
        security = [
            c for c in event["calls"] if c["receipt"]["provider_id"] == "sec-core"
        ]
        if (
            len(projection) != 1
            or len(security) != 1
            or security[0]["receipt"]["disposition"] != "produced"
        ):
            raise RuntimeError("Expected one real SecCore and one Tokenless call")
        counts = [
            row for row in view["providers"] if row["provider_id"] == "tokenless"
        ][0]
        if args.case == "compressed":
            if (
                projection[0]["receipt"]["disposition"] != "produced"
                or adoption["observation"]["decision"] != "adopted"
                or adoption["saved_bytes"] <= 0
                or counts["adopted"] != 1
                or counts["saved_bytes"] != adoption["saved_bytes"]
            ):
                raise RuntimeError(
                    "Actual history did not adopt a smaller Tokenless candidate"
                )
        else:
            if (
                adoption["observation"]["decision"] != "preserved"
                or adoption["saved_bytes"] != 0
                or counts["adopted"] != 0
                or counts["saved_bytes"] != 0
            ):
                raise RuntimeError("Fallback did not preserve the real original result")
            if args.case == "provider-failure" and (
                projection[0]["receipt"]["disposition"] != "failed"
                or event["execution"]["decision"] != "preserve"
            ):
                raise RuntimeError("Provider fault was not recorded as a failed call")
        if args.herdr_socket:
            (root / "herdr-view.json").write_bytes(displayed.stdout)
            sys.path.insert(
                0, str(Path(__file__).resolve().parents[1] / "integrations" / "herdr")
            )
            import bridge

            bridge.rpc(
                args.herdr_socket,
                "pane.report_agent_session",
                {
                    "pane_id": args.herdr_pane_id,
                    "source": "herdr:qodercli",
                    "seq": 1,
                    "session_start_source": "startup",
                    "agent": "qodercli",
                    "agent_session_id": session,
                },
            )
            # Use the actual verified view and the matching live pane process.
            native_smoke.write_json(
                root / "herdr-process.json",
                bridge.rpc(
                    args.herdr_socket,
                    "pane.process_info",
                    {"pane_id": args.herdr_pane_id},
                ),
            )
            native_smoke.write_json(
                root / "herdr-before-publish.json",
                bridge.rpc(
                    args.herdr_socket, "pane.get", {"pane_id": args.herdr_pane_id}
                ),
            )
            published = bridge.refresh(
                args.herdr_socket,
                args.herdr_pane_id,
                args.view_bin,
                root / "view-binding.json",
                time.time_ns(),
            )
            native_smoke.write_json(root / "herdr-published.json", published)
            if not published:
                raise RuntimeError("Herdr rejected the live pane/session binding")
            native_smoke.write_json(
                root / "herdr-metadata.json",
                bridge.rpc(
                    args.herdr_socket, "pane.get", {"pane_id": args.herdr_pane_id}
                ),
            )
            hold_deadline = time.monotonic() + args.hold_seconds
            while time.monotonic() < hold_deadline:
                time.sleep(min(1, max(0, hold_deadline - time.monotonic())))
                if not bridge.refresh(
                    args.herdr_socket,
                    args.herdr_pane_id,
                    args.view_bin,
                    root / "view-binding.json",
                    time.time_ns(),
                ):
                    raise RuntimeError(
                        "Herdr lost the live binding during presentation"
                    )
        result = {
            "status": "passed",
            "case": args.case,
            "event_key": event["event_key"],
            "adoption": json.loads(verified.stdout),
            "view": json.loads(displayed.stdout),
        }
    except (
        OSError,
        ValueError,
        RuntimeError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ) as error:
        result = {"status": "failed", "reason": str(error) or "interrupted"}
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if process is not None:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    if args.interactive:
                        os.kill(process.pid, sig)
                    else:
                        os.killpg(process.pid, sig)
                except ProcessLookupError:
                    pass
                time.sleep(0.1)
            process.wait(timeout=5)
            lifecycle["agent_returncode"] = process.returncode
        # Exact fresh UUID and unique workspace establish cleanup ownership.
        if transcript.exists():
            if (
                transcript.is_symlink()
                or transcript.stat().st_uid != os.getuid()
                or transcript.stat().st_mtime * 1000 < lifecycle["started_at_ms"] - 1000
            ):
                result = {
                    "status": "failed",
                    "reason": "Transcript ownership check failed; retained for inspection",
                }
            else:
                transcript.unlink()
        if state.exists():
            if state.is_symlink() or state.stat().st_uid != os.getuid():
                result = {
                    "status": "failed",
                    "reason": "State ownership check failed; retained for inspection",
                }
            else:
                shutil.rmtree(state)
        try:
            project.rmdir()
        except (FileNotFoundError, OSError):
            pass
        # Remove only the fresh workspace trust entry added by this test's UI.
        current_stat = user_settings_path.stat()
        current_settings = json.loads(user_settings_path.read_text())
        trust = current_settings.get("permissions", {}).get("trustDirectories", [])
        if str(work) in trust and str(work) not in previous_trust:
            current_settings["permissions"]["trustDirectories"] = [
                item for item in trust if item != str(work)
            ]
            temporary_settings = user_settings_path.with_name(
                ".aw-test-settings-" + session
            )
            try:
                with temporary_settings.open("x") as output:
                    os.chmod(temporary_settings, current_stat.st_mode & 0o777)
                    json.dump(current_settings, output, indent=2)
                    output.write("\n")
                if user_settings_path.stat().st_mtime_ns != current_stat.st_mtime_ns:
                    raise RuntimeError(
                        "Settings changed concurrently; owned trust entry retained"
                    )
                os.replace(temporary_settings, user_settings_path)
            finally:
                temporary_settings.unlink(missing_ok=True)
        lifecycle["owned_workspace_trust_removed"] = str(work) not in json.loads(
            user_settings_path.read_text()
        ).get("permissions", {}).get("trustDirectories", [])
        shutil.rmtree(home)
        lifecycle.update(
            temporary_home_removed=not home.exists(),
            owned_session_removed=not transcript.exists() and not state.exists(),
            finished_at_ms=int(time.time() * 1000),
        )
        native_smoke.write_json(root / "lifecycle.json", lifecycle)
        native_smoke.write_json(root / "result.json", result)
    print(json.dumps(result, indent=2), flush=True)
    return 0 if result["status"] == "passed" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in [
        "hook-bin",
        "view-bin",
        "adoption-bin",
        "tokenless-bin",
        "provider-python",
        "provider-source",
        "output-dir",
    ]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--qoder", default=shutil.which("qodercli"))
    parser.add_argument("--provider-version", default="0.11.0")
    parser.add_argument("--tokenless-version", default="0.8.0")
    parser.add_argument(
        "--case", choices=["compressed", "no-gain", "provider-failure"], required=True
    )
    parser.add_argument("--interactive", action="store_true")
    parser.add_argument("--herdr-socket", type=Path)
    parser.add_argument("--herdr-pane-id")
    parser.add_argument(
        "--hold-seconds", type=int, choices=range(1, 121), default=3, metavar="1..120"
    )
    args = parser.parse_args()
    if bool(args.herdr_socket) != bool(args.herdr_pane_id) or (
        args.herdr_socket and not args.interactive
    ):
        parser.error("Herdr requires socket, pane ID and interactive mode together")

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
