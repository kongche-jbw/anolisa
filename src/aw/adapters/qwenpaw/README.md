# QwenPaw native adapter

[中文版](README_zh.md)

This plugin registers one AgentScope `on_acting` middleware per AW hook. It is
tested against official `agentscope-ai/QwenPaw` commit
`3822ec7173d17cf37c8a02f51d3ed5628079e86e` (2.2.2b4), with AgentScope 2.0.8.
This is QwenPaw, not Qwen Code.

Copy `plugin.json` and `plugin.py` into
`$QWENPAW_WORKING_DIR/plugins/aw-native/`, or run `qwenpaw plugin install` with
this directory. Set `AW_BIN`, `AW_SOCKET`, `AW_AGENT`, and `AW_NATIVE_CONFIG`.
The latter points to JSON:

```json
{"hooks":[{"event":"tool.before","provider":"before-command","priority":100},{"event":"tool.after","provider":"after-command","priority":100}]}
```

The native plugin registry sorts priority ascending; equal priorities retain
registration order. Middleware enters outer to inner and returns inner to
outer. Consequently after hooks run in reverse nesting order. The adapter
does not serialize independent tools or change their concurrency flags.

The command receives `{"event": ..., "tool_call": ...}`; after commands also
receive `tool_response`. Native Pydantic objects use `model_dump(mode="json")`.
`tool_call.input` remains the native JSON string, not an invented argument
mapping. Hooks run after native permission checking and input validation.

Exit 0 observes and continues; stdout does not transform tool results. Exit 2
on a before hook yields a native denied `ToolResponse` without entering the
inner tool. Other failures raise an error. These exit conventions belong to
this adapter because QwenPaw has no native command-hook wire protocol.
Explicit `action: ask`, `action: approve`, or `decision: ask` JSON output raises
an unsupported-approval error. Native QwenPaw governance approval happens
outside `on_acting`; this adapter does not claim an approval UI. After hooks
cannot undo an executed tool.

The bounded tests under `tests/native/qwenpaw` exercise the installed native
middleware chain. `live_agent.py` uses the public `QwenPawAgent` runtime and
native plugin loader with a real model and shell tool; it is narrower than a
full HTTP server or TUI test. Set `AW_TEST_MODEL_KEY_FILE` to a private file;
the fixture reads the key only into memory.

The native lab ran three real `qwen3.7-plus` Token Plan cases through `aw run`:
allow wrote `42` to the temporary workspace; exit-2 deny created no file and
returned a denied response; explicit ask raised the unsupported error before
execution. Two before hooks ran A then B; two after hooks ran B then A. The
launcher registered after wrappers outside before wrappers, so the deny was
also observed by the after hooks. No human approval UI was tested.
