"""Bounded real-model QwenPawAgent test, using the native plugin loader.

Run from an isolated working directory with AW_* launcher variables set.
The model key is read only in memory from AW_TEST_MODEL_KEY_FILE.
This exercises the public runtime, not QwenPaw's full HTTP/TUI application.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

from agentscope.agent import ReActConfig
from agentscope.credential import OpenAICredential
from agentscope.message import Msg, TextBlock
from agentscope.model import OpenAIChatModel
from agentscope.tool import FunctionTool, Toolkit
from qwenpaw.agents.react_agent import QwenPawAgent
from qwenpaw.agents.tools.shell import execute_shell_command
from qwenpaw.config.config import AgentProfileConfig
from qwenpaw.plugins.loader import PluginLoader
from qwenpaw.plugins.registry import PluginRegistry


async def main() -> None:
    work = Path.cwd()
    profile = AgentProfileConfig(
        id="aw-native-test", name="AW native test", workspace_dir=str(work)
    )
    loader = PluginLoader([Path(os.environ["QWENPAW_WORKING_DIR"]) / "plugins"])
    source = Path(os.environ["QWENPAW_WORKING_DIR"]) / "plugins/aw-native"
    record = await loader.load_plugin_from_path(source)
    if not record.enabled or record.instance is None:
        raise RuntimeError("Native plugin did not load")
    middlewares = [
        entry.factory(None, profile) for entry in PluginRegistry().get_middleware_factories()
    ]

    async def terminal(command: str):
        """Execute a bounded shell command in the isolated coding workspace."""
        return await execute_shell_command(command, timeout=10, cwd=work)

    model = OpenAIChatModel(
        credential=OpenAICredential(
            api_key=Path(os.environ["AW_TEST_MODEL_KEY_FILE"]).read_text().strip(),
            base_url="https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        ),
        model="qwen3.7-plus",
        stream=False,
        max_retries=0,
        client_kwargs={"timeout": 60},
        extra_body={"enable_thinking": False},
    )
    agent = QwenPawAgent(
        name="aw-native-test",
        model=model,
        system_prompt="Complete the single coding verification task using terminal. If denied, stop. Do not retry or delegate.",
        toolkit=Toolkit(tools=[FunctionTool(terminal, is_concurrency_safe=True)]),
        react_config=ReActConfig(max_iters=3, stop_on_reject=True),
        middlewares=middlewares,
        agent_config=profile,
        workspace_dir=work,
    )
    await asyncio.wait_for(
        agent.reply(
            Msg(
                name="user",
                role="user",
                content=[
                    TextBlock(
                        text="Use terminal exactly once to run: printf '42\\n' > answer.txt. Then report completion. If denied, report the denial and stop."
                    )
                ],
            )
        ),
        timeout=150,
    )
    print(
        json.dumps(
            {
                "runtime": "QwenPawAgent",
                "model": "qwen3.7-plus",
                "middleware_count": len(middlewares),
                "marker_exists": (work / "answer.txt").exists(),
            }
        )
    )


if __name__ == "__main__":
    asyncio.run(main())
