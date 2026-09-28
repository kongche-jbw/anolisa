"""Run with the pinned QwenPaw environment; no model credentials needed."""

import asyncio
import importlib.util
import unittest
from pathlib import Path
from types import SimpleNamespace

from agentscope.agent import Agent
from agentscope.message import TextBlock, ToolCallBlock, ToolResultState
from agentscope.tool import ToolResponse

SPEC = importlib.util.spec_from_file_location(
    "aw_qwenpaw",
    Path(__file__).resolve().parents[3] / "adapters/qwenpaw/plugin.py",
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class NativeMiddlewareTests(unittest.IsolatedAsyncioTestCase):
    async def test_native_onion_order_and_projection(self) -> None:
        seen = []
        call = ToolCallBlock(id="call-1", name="shell", input='{"command":"true"}')

        class Probe(MODULE.AwToolMiddleware):
            async def _invoke(self, payload):
                seen.append((self.provider, payload))
                return 0, b"", b""

        async def tool(tool_call):
            seen.append(("tool", tool_call.id))
            yield ToolResponse(content=[TextBlock(text="ok")])

        agent = SimpleNamespace(
            _acting_middlewares=[
                Probe("tool.before", "before-a"),
                Probe("tool.before", "before-b"),
                Probe("tool.after", "after-a"),
                Probe("tool.after", "after-b"),
            ],
            _acting_impl=tool,
        )
        result = [item async for item in Agent._acting(agent, call)]
        self.assertEqual(
            [name for name, _ in seen],
            [
                "before-a",
                "before-b",
                "tool",
                "after-b",
                "after-a",
            ],
        )
        self.assertEqual(seen[0][1]["tool_call"]["input"], '{"command":"true"}')
        self.assertEqual(seen[3][1]["tool_response"]["state"], "success")
        self.assertEqual(result[0].content[0].text, "ok")

    async def test_block_does_not_enter_tool(self) -> None:
        middleware = MODULE.AwToolMiddleware("tool.before", "deny")

        async def invoke(payload):
            return 2, b"", b"blocked fixture"

        async def tool():
            raise AssertionError("denied tool executed")
            yield

        middleware._invoke = invoke
        result = [
            item
            async for item in middleware.on_acting(
                None,
                {"tool_call": ToolCallBlock(id="denied", name="shell", input="{}")},
                tool,
            )
        ]
        self.assertEqual(result[0].state, ToolResultState.DENIED)

    async def test_explicit_ask_is_an_error(self) -> None:
        middleware = MODULE.AwToolMiddleware("tool.before", "ask")

        async def invoke(payload):
            return 0, b'{"action":"ask"}', b""

        middleware._invoke = invoke
        with self.assertRaisesRegex(RuntimeError, "does not support ask"):
            await middleware._run({})

    async def test_independent_calls_remain_concurrent(self) -> None:
        started = 0
        both_started = asyncio.Event()

        class Probe(MODULE.AwToolMiddleware):
            async def _invoke(self, payload):
                nonlocal started
                started += 1
                if started == 2:
                    both_started.set()
                await asyncio.wait_for(both_started.wait(), 1)
                return 0, b"", b""

        probe = Probe("tool.before", "overlap")
        await asyncio.gather(probe._run({}), probe._run({}))
        self.assertEqual(started, 2)


if __name__ == "__main__":
    unittest.main()
