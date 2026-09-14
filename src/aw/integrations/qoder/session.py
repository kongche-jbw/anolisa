#!/usr/bin/env python3
"""Explicit one-prompt Qoder session through cosh-shell and existing AW CLIs."""

import argparse
from contextlib import nullcontext
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import stat
import sys
import uuid

from session_process import Processes

SCRIPT = Path(__file__).resolve()
PROFILE = "qoder-cli-1.1.47/jsonl-v1"


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def read(path, private=False):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > 1048576:
            raise ValueError("invalid settings file")
        if private and (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600):
            raise ValueError("settings require caller ownership and mode 0600")
        return json.loads(stream.read(1048577), object_pairs_hook=unique)


def write(path, value):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as stream:
        json.dump(value, stream, ensure_ascii=False)
        stream.write("\n")


def binary(pin):
    if set(pin) != {"path", "sha256"} or not re.fullmatch(r"[a-f0-9]{64}", pin["sha256"]):
        raise ValueError("invalid executable pin")
    path = Path(pin["path"])
    if not path.is_absolute() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("absolute executable required")
    with path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != pin["sha256"]:
            raise ValueError("executable changed")
    return str(path)


def validate(config):
    if os.environ.get("QODER_CONFIG_DIR_NAME", ".qoder") != ".qoder":
        raise ValueError("unsupported Qoder project settings directory override")
    required = {
        "format",
        "workspace",
        "config_directory",
        "session_directory",
        "binaries",
        "preflight",
        "prompt",
        "timeout_seconds",
        "permission_mode",
    }
    if not required <= config.keys() or config.keys() - required - {"projection", "model"}:
        raise ValueError("unknown or missing launcher settings")
    if (
        config["format"] != 1
        or type(config["timeout_seconds"]) is not int
        or not 1 <= config["timeout_seconds"] <= 240
    ):
        raise ValueError("invalid session format or deadline")
    for key in ("workspace", "config_directory", "session_directory"):
        if not Path(config[key]).is_absolute():
            raise ValueError("absolute session paths required")
    work = Path(config["workspace"])
    if (
        not work.is_dir()
        or str(work.resolve()) != str(work)
        or not re.fullmatch(r"/[A-Za-z0-9_/-]+", str(work))
    ):
        raise ValueError(
            "workspace must be canonical ASCII letters/digits/slashes/underscores/hyphens"
        )
    if not Path(config["config_directory"]).is_dir():
        raise ValueError("existing native Qoder config directory required")
    if not isinstance(config["prompt"], str) or not 1 <= len(config["prompt"].encode()) <= 32768:
        raise ValueError("bounded single prompt required")
    if config["permission_mode"] not in ("default", "dont_ask", "bypass_permissions"):
        raise ValueError("explicit supported native permission mode required")
    if "model" in config and (not isinstance(config["model"], str) or not config["model"]):
        raise ValueError("invalid model")
    expected = {"qoder", "cosh_shell", "aw_hook", "aw_preflight", "aw_adoption"}
    if set(config["binaries"]) != expected:
        raise ValueError("explicit Qoder, cosh-shell and AW executable pins required")
    for pin in config["binaries"].values():
        binary(pin)
    if Path(config["binaries"]["cosh_shell"]["path"]).name != "cosh-shell":
        raise ValueError("select the cosh-shell helper, not the cosh shell entry")
    preflight = config["preflight"]
    if preflight.get("mode") not in ("inspect", "project") or preflight["security"]["cwd"] != str(
        work
    ):
        raise ValueError("Provider cwd must match the native workspace")
    if (preflight["mode"] == "project") != ("projection" in config):
        raise ValueError("projection requires explicit retention/reversibility settings")
    if "projection" in config:
        options = config["projection"]
        if (
            set(options) != {"retention", "accepted_reversibility", "allow_text_reencoding"}
            or options["retention"] != "source_and_candidate"
            or options["accepted_reversibility"] != ["unrecoverable"]
            or type(options["allow_text_reencoding"]) is not bool
        ):
            raise ValueError(
                "explicit source/candidate retention and unrecoverable opt-in required"
            )


def coexistence(config, processes):
    work, native = Path(config["workspace"]), Path(config["config_directory"])
    paths = [native / "settings.json"]
    for parent in (work, *work.parents):
        paths.extend(parent / ".qoder" / name for name in ("settings.json", "settings.local.json"))
    for path in paths:
        if path.exists():
            settings = read(path)
            plugins = settings.get("enabledPlugins", {})
            if (
                not isinstance(plugins, dict)
                or settings.get("hooks")
                or settings.get("disableAllHooks")
                or any(value is not False for value in plugins.values())
            ):
                raise ValueError(
                    "existing Qoder hooks/plugins require a reviewed coexistence profile"
                )
    result = processes.capture(
        [
            binary(config["binaries"]["qoder"]),
            "--config-dir",
            str(native),
            "plugins",
            "list",
            "--json",
        ],
        work,
    )
    if json.loads(result, object_pairs_hook=unique) != []:
        raise ValueError("existing Qoder plugins require a reviewed coexistence profile")


def history_path(config, session):
    """Locate the fixed native profile's transcript for an owned session."""
    project = config["workspace"].replace("/", "-").replace("_", "-")
    return Path(config["config_directory"]) / "projects" / project / f"{session}.jsonl"


def bootstrap(root):
    config = read(root / "launch.json", private=True)
    validate(config)
    identity = read(root / "identity.json", private=True)
    session = identity["session_id"]
    pid = os.getpid()
    start = int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19])
    runtime = {
        "runtime_id": identity["runtime_id"],
        "generation": identity["runtime_generation"],
        "binding_revision": 1,
        "environment_id": identity["environment_id"],
        "process_ref": f"pid:{pid}@{start}",
        "observation_source": "owned_child",
        "owner_id": "aw-qoder-launcher",
        "state": "running",
        "sequence": 1,
        "session_id": session,
    }
    scope = {
        "environment_id": identity["environment_id"],
        "execution_context_id": session,
        "actor_id": "qoder",
        "runtime_id": identity["runtime_id"],
        "runtime_generation": identity["runtime_generation"],
        "binding_revision": 1,
        "session_id": session,
        "turn_id": identity["turn_id"],
    }
    hook = {
        "runtime": runtime,
        "scope": scope,
        "agent_pid": pid,
        "agent_start_ticks": start,
        "qoder_single_turn_id": identity["turn_id"],
        "journal": str(root / "journal"),
        "provider": config["preflight"]["security"],
        "include_low_confidence": False,
    }
    mode = "qoder"
    settings = hook
    if config["preflight"]["mode"] == "project":
        settings = {
            "hook": hook,
            "tokenless": config["preflight"]["tokenless"],
            "record_directory": str(root / "records"),
            "history_path": str(history_path(config, session)),
            "history_profile": PROFILE,
            "max_observation_delay_ms": 300000,
            **config["projection"],
        }
        mode = "qoder-project"
    write(root / "hook.json", settings)
    # The hook is the existing binary: native session/tool/cwd binding remains
    # in the Adapter and history reader, not in a second Python mapper.
    command = shlex.join([binary(config["binaries"]["aw_hook"]), mode, str(root / "hook.json")])
    budget = config["preflight"]["security"]["limits"]["timeout_ms"]
    if mode == "qoder-project":
        budget += config["preflight"]["tokenless"]["limits"]["timeout_ms"]
    write(
        root / "qoder.json",
        {
            "general": {"enableAutoUpdate": False},
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {
                                "type": "command",
                                "command": command,
                                "timeout": (budget * 2 + 999) // 1000 + 15,
                            }
                        ],
                    }
                ]
            },
        },
    )
    qoder = binary(config["binaries"]["qoder"])
    argv = [
        qoder,
        "--config-dir",
        config["config_directory"],
        "--cwd",
        config["workspace"],
        "--settings",
        str(root / "qoder.json"),
        "--resume" if identity["resume"] else "--session-id",
        session,
        "--permission-mode",
        config["permission_mode"],
        "--tools",
        "Bash",
        "--max-model-request-retries",
        "0",
    ]
    if "model" in config:
        argv.extend(["--model", config["model"]])
    argv.extend(["-p", config["prompt"]])
    write(root / "agent.json", {"pid": pid, "start_ticks": start, "session_id": session})
    os.execv(qoder, argv)


def launch(config, *, identity=None, processes=None):
    validate(config)
    if identity is None:
        session = str(uuid.uuid4())
        identity = {
            "session_id": session,
            "turn_id": session,
            "runtime_id": session,
            "environment_id": session,
            "runtime_generation": 1,
            "resume": False,
        }
    root, work = Path(config["session_directory"]), Path(config["workspace"])
    # Exclusive creation gives cleanup ownership without touching existing state.
    root.mkdir(mode=0o700)
    started = False
    try:
        # A bounded conversation shares cancellation across turns, while each
        # turn still waits for complete child cleanup before the next launch.
        with nullcontext(processes) if processes is not None else Processes() as processes:
            binaries = config["binaries"]
            for name, expected in (("qoder", "1.1.47"), ("cosh_shell", "cosh-shell 0.15.0")):
                if (
                    processes.capture([binary(binaries[name]), "--version"], work).strip()
                    != expected
                ):
                    raise ValueError("unsupported native launcher version")
            coexistence(config, processes)
            write(root / "preflight.json", config["preflight"])
            readiness = json.loads(
                processes.capture(
                    [binary(binaries["aw_preflight"]), str(root / "preflight.json")],
                    work,
                    2 * config["preflight"]["security"]["limits"]["timeout_ms"] / 1000
                    + config["preflight"]
                    .get("tokenless", {})
                    .get("limits", {})
                    .get("timeout_ms", 0)
                    / 1000
                    + 15,
                )
            )
            security = readiness.get("providers", [{}])[0]
            if (
                readiness.get("status") != "dependencies_ready"
                or security.get("protocol_probe") != "passed"
            ):
                raise ValueError("native security protocol admission required")
            write(root / "launch.json", config)
            write(root / "identity.json", identity)
            write(root / "readiness.json", readiness)
            # Recheck coexistence after potentially slow Provider preparation.
            coexistence(config, processes)
            started = True
            status = processes.run(
                [
                    binary(binaries["cosh_shell"]),
                    "--",
                    sys.executable,
                    "-B",
                    str(SCRIPT),
                    "_bootstrap",
                    str(root),
                ],
                work,
                config["timeout_seconds"],
            )
            observations = []
            if (root / "records").exists():
                records = sorted(
                    p
                    for p in (root / "records").iterdir()
                    if re.fullmatch(r"[a-f0-9]{64}\.json", p.name)
                )
                if len(records) > 128:
                    raise ValueError("session observation count exceeded")
                for record in records:
                    result = json.loads(
                        processes.capture(
                            [binary(binaries["aw_adoption"]), "observe", str(record)], work
                        )
                    )
                    observations.append(result)
            write(
                root / "result.json",
                {
                    "agent_exit_code": status,
                    "observations": observations,
                    "cleanup": "owned_group_reaped",
                    "proof_boundary": "local_history",
                },
            )
            return status
    except BaseException:
        if started:
            write(root / "failure.json", {"status": "session_failed", "adoption": "unverified"})
        raise
    finally:
        if not started:
            shutil.rmtree(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("settings", help="absolute private launcher JSON")
    args = parser.parse_args()
    path = Path(args.settings)
    if not path.is_absolute():
        parser.error("absolute settings path required")
    return launch(read(path, private=True))


if __name__ == "__main__":
    try:
        if len(sys.argv) == 3 and sys.argv[1] == "_bootstrap":
            bootstrap(Path(sys.argv[2]))
        else:
            raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"aw-qoder: {error}", file=sys.stderr)
        raise SystemExit(1)
