"""Exercise the pinned Hermes dispatcher and directive aggregator without a model."""

import json
import os
import shlex
from pathlib import Path
from unittest.mock import patch

from agent.shell_hooks import _make_callback, iter_configured_hooks
from hermes_cli.plugins import PluginManager, _get_pre_tool_call_directive_details


def manager(socket, policy):
    command = [os.environ["AW_BIN"], "hook", "--socket", socket, "--agent", "hermes"]
    hooks = {}
    for native, event in [("pre_tool_call", "tool.before"), ("post_tool_call", "tool.after")]:
        failure = policy if event == "tool.before" else "report"
        hooks[native] = [{
            "command": shlex.join(command + ["--event", event, "--provider", "policy", "--adapter", "hermes", "--on-error", failure]),
            "timeout": 7, "fail_closed": failure == "block",
        }]
    result = PluginManager(scope_key=os.environ["HERMES_HOME"])
    for specification in iter_configured_hooks({"hooks": hooks}):
        result._hooks.setdefault(specification.event, []).append(_make_callback(specification))
    return result


live = manager(os.environ["AW_SOCKET"], "block")
with patch("hermes_cli.lifecycle.invoke_hook", live.invoke_hook):
    allowed = _get_pre_tool_call_directive_details("terminal", {"command": "printf allowed"}, tool_call_id="hermes-allow")
    denied = _get_pre_tool_call_directive_details("terminal", {"command": "printf AW_DENY_FIXTURE"}, tool_call_id="hermes-deny")
assert allowed.action is None, allowed
assert denied.action == "block" and denied.message == "AW policy blocked this tool call", denied
assert live.invoke_hook("post_tool_call", tool_name="terminal", args={"command": "printf allowed"}, result='{"output":"allowed"}', tool_call_id="hermes-allow") == []

for policy in ["block", "report"]:
    disconnected = manager(os.environ["AW_SOCKET"] + ".missing", policy)
    with patch("hermes_cli.lifecycle.invoke_hook", disconnected.invoke_hook):
        result = _get_pre_tool_call_directive_details("terminal", {"command": "printf allowed"})
    assert result.action == ("block" if policy == "block" else None), result

Path(os.environ["AW_BRIDGE_RESULT"]).write_text(json.dumps({
    "framework": "hermes", "scope": "native dispatcher and directive aggregator; no model",
    "allow_neutral": True, "native_block_adopted": True, "after_observed": True,
    "disconnected_block": True, "disconnected_report": True,
}) + "\n")
