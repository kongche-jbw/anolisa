"""Run with the pinned QwenPaw environment; no model credentials needed."""

import asyncio
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

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


class RegistrationReceiptTests(unittest.TestCase):
    def setUp(self) -> None:
        root = Path(__file__).resolve().parents[5] / "target/profile-lab/qwenpaw/receipt-tests"
        root.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=root)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.config = self.root / "hooks.json"
        self.config.write_text(
            json.dumps(
                {
                    "hooks": [
                        {"event": "tool.before", "provider": "policy"},
                        {"event": "tool.after", "provider": "policy"},
                    ]
                }
            )
        )
        self.receipt = self.root / "registered.json"

    def environment(self, **values):
        environment = {
            key: value
            for key, value in os.environ.items()
            if key not in {"AW_READY_FILE", "AW_READY_TOKEN"}
        }
        return patch.dict(
            os.environ,
            {**environment, "AW_NATIVE_CONFIG": str(self.config), **values},
            clear=True,
        )

    def test_receipt_is_private_complete_and_refreshable(self) -> None:
        registered = []
        api = SimpleNamespace(register_middleware=lambda factory, **kw: registered.append(factory))
        with self.environment(AW_READY_FILE=str(self.receipt), AW_READY_TOKEN="first-token"):
            MODULE.AwPlugin().register(api)
        self.assertEqual(len(registered), 2)
        self.assertEqual(
            json.loads(self.receipt.read_text()),
            {
                "version": 1,
                "adapter": "qwenpaw",
                "pid": os.getpid(),
                "token": "first-token",
                "hooks": 2,
            },
        )
        self.assertEqual(self.receipt.stat().st_mode & 0o777, 0o600)
        with self.environment(AW_READY_FILE=str(self.receipt), AW_READY_TOKEN="next-token"):
            MODULE.AwPlugin().register(api)
        self.assertEqual(json.loads(self.receipt.read_text())["token"], "next-token")
        self.assertEqual(list(self.root.glob(".aw-register-*")), [])

    def test_missing_receipt_partner_fails_before_registration(self) -> None:
        for settings in (
            {"AW_READY_FILE": str(self.receipt)},
            {"AW_READY_TOKEN": "token"},
        ):
            with self.environment(**settings), self.assertRaisesRegex(ValueError, "both"):
                MODULE.AwPlugin().register(SimpleNamespace())
        self.assertFalse(self.receipt.exists())

    def test_no_receipt_without_environment_or_after_registration_failure(self) -> None:
        api = SimpleNamespace(register_middleware=lambda *args, **kw: None)
        with self.environment():
            MODULE.AwPlugin().register(api)
        self.assertFalse(self.receipt.exists())
        with self.environment(AW_READY_FILE=str(self.receipt), AW_READY_TOKEN="token"):
            with self.assertRaises(AttributeError):
                MODULE.AwPlugin().register(SimpleNamespace())
        self.assertFalse(self.receipt.exists())


if __name__ == "__main__":
    unittest.main()
