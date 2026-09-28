"""QwenPaw plugin: one native middleware registration per AW command."""

from __future__ import annotations

import asyncio
import json
import os
import signal
from pathlib import Path
from typing import Any, AsyncGenerator, Callable

from agentscope.message import TextBlock, ToolResultState
from agentscope.middleware import MiddlewareBase
from agentscope.tool import ToolResponse


class AwToolMiddleware(MiddlewareBase):
    """Observe tool I/O, or deny a before call when its command exits 2.

    Input is a JSON projection of native Pydantic tool objects. QwenPaw has
    no command hook dialect; exit 2 is this adapter's explicit convention.
    Approval is outside on_acting and is deliberately unsupported here.
    """

    def __init__(self, event: str, provider: str) -> None:
        self.event = event
        self.provider = provider

    async def _invoke(self, payload: dict[str, Any]) -> tuple[int, bytes, bytes]:
        proc = await asyncio.create_subprocess_exec(
            os.environ["AW_BIN"],
            "hook",
            "--socket",
            os.environ["AW_SOCKET"],
            "--agent",
            os.environ["AW_AGENT"],
            "--event",
            self.event,
            "--provider",
            self.provider,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            start_new_session=True,
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                proc.communicate(json.dumps(payload).encode()),
                timeout=60,
            )
        except BaseException:
            if proc.returncode is None:
                os.killpg(proc.pid, signal.SIGKILL)
            await proc.wait()
            raise
        return proc.returncode, stdout, stderr

    async def _run(self, payload: dict[str, Any]) -> ToolResponse | None:
        code, stdout, stderr = await self._invoke(payload)
        # This is not a portable approval protocol. Reject an explicit ask
        # instead of allowing it to look like a successfully applied policy.
        try:
            directive = json.loads(stdout) if stdout.strip() else None
        except (ValueError, UnicodeDecodeError):
            directive = None
        if isinstance(directive, dict) and (
            directive.get("action") in {"ask", "approve"} or directive.get("decision") == "ask"
        ):
            raise RuntimeError("AW QwenPaw on_acting does not support ask/approval")
        if code == 2 and self.event == "tool.before":
            reason = (stderr or stdout).decode(errors="replace").strip()
            return ToolResponse(
                state=ToolResultState.DENIED,
                content=[TextBlock(text=reason or "Denied by AW native middleware")],
            )
        if code != 0:
            raise RuntimeError(f"AW {self.event} provider {self.provider} exited {code}")
        return None

    async def on_acting(
        self,
        agent: Any,
        input_kwargs: dict[str, Any],
        next_handler: Callable[..., AsyncGenerator[Any, None]],
    ) -> AsyncGenerator[Any, None]:
        """Leave chain nesting and parallel tool dispatch to AgentScope."""
        call = input_kwargs["tool_call"]
        payload = {"event": self.event, "tool_call": call.model_dump(mode="json")}
        if self.event == "tool.before":
            denied = await self._run(payload)
            if denied is not None:
                yield denied
                return
        async for item in next_handler():
            if self.event == "tool.after" and isinstance(item, ToolResponse):
                await self._run({**payload, "tool_response": item.model_dump(mode="json")})
            yield item


class AwPlugin:
    """Load launcher-generated registrations through QwenPaw's plugin API."""

    def register(self, api: Any) -> None:
        config = json.loads(Path(os.environ["AW_NATIVE_CONFIG"]).read_text())
        hooks = config["hooks"]
        for hook in hooks:
            if hook["event"] not in {"tool.before", "tool.after"}:
                raise ValueError(f"Unsupported AW native event: {hook['event']}")
            if not isinstance(hook["provider"], str) or not hook["provider"]:
                raise ValueError("AW provider must be a nonempty string")
            if type(hook.get("priority", 100)) is not int:
                raise ValueError("AW native priority must be an integer")
        for hook in hooks:

            def factory(ctx: Any, agent_config: Any, hook: dict = hook) -> AwToolMiddleware:
                return AwToolMiddleware(hook["event"], hook["provider"])

            api.register_middleware(factory, priority=hook.get("priority", 100))


plugin = AwPlugin()
