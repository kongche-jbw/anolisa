#!/usr/bin/env python3
"""Probe the pinned Hermes profile contract without a model or user credentials."""

import argparse
import json
import os
from pathlib import Path
import shutil


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    root.mkdir()
    home = root / "profile"
    home.mkdir(mode=0o700)
    work = root / "work"
    work.mkdir()
    trace = root / "trace.jsonl"
    native = {"hooks": {"pre_tool_call": [{"command": "/usr/bin/python3 ./native-hook.py before"}]}}
    (home / "config.yaml").write_text(json.dumps(native))
    overlay = root / "overlay.json"
    overlay.write_text(json.dumps({"hooks": {"pre_tool_call": [{"command": "false"}]}}))
    shutil.copyfile(Path(__file__).with_name("profile_hook.py"), work / "native-hook.py")
    os.environ.update(HERMES_HOME=str(home), HERMES_CONFIG_PATH=str(overlay),
                      AW_HERMES_TEST_TRACE=str(trace))
    os.chdir(work)
    # Import only after the explicit isolated profile has been established.
    from hermes_constants import get_config_path, get_env_path, get_hermes_home
    from hermes_cli.config import load_config
    from hermes_cli.auth import _auth_file_path
    from hermes_state import _default_db_path
    from agent.shell_hooks import _make_callback, iter_configured_hooks

    config = load_config()
    specs = iter_configured_hooks(config)
    assert len(specs) == 1 and "native-hook.py" in specs[0].command
    _make_callback(specs[0])(tool_name="terminal", args={"command": "printf contract"})
    rows = [json.loads(line) for line in trace.read_text().splitlines()]
    result = {"config_path": str(get_config_path()), "overlay_path": str(overlay),
        "overlay_supported": get_config_path() == overlay,
        "home": str(get_hermes_home()), "env_path": str(get_env_path()),
        "auth_path": str(_auth_file_path()), "database_path": str(_default_db_path()),
        "native_hook_loaded": len(rows) == 1, "relative_hook_cwd": rows[0]["cwd"]}
    assert not result["overlay_supported"]
    assert get_config_path() == home / "config.yaml"
    assert get_env_path() == home / ".env" and _auth_file_path() == home / "auth.json"
    assert _default_db_path() == home / "state.db"
    assert result["relative_hook_cwd"] == str(work)
    (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
