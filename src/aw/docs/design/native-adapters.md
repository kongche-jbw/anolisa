# Shared native adapters

[中文版](native-adapters_zh.md)

`aw-adapters` 0.1.0 captures native tool text, binds it to a trusted runtime
context and executes existing capability plans through `aw-core`. It is an
embedding library. Loading a profile does not install or activate a plugin.
It uses the existing Schema resources and shared Core orchestration.

## Six host surfaces

Each file in `crates/aw-adapters/profiles/` contains two existing
`boundary-descriptor/v1` objects. The surrounding `format: 1` object is a
private library resource, not a new AW wire protocol.

| Host | Pre / post event | Command slot | Result slot | Observable IDs |
| --- | --- | --- | --- | --- |
| Qoder | `PreToolUse` / `PostToolUse` | `tool_input.command`, tool `Bash` | `tool_response` string or completed Bash `tool_response.stdout` | `session_id`, `tool_use_id` |
| Codex | `PreToolUse` / `PostToolUse` | `tool_input.command`, tool `Bash` | `tool_response` string | `session_id`, `tool_use_id`, `turn_id` |
| Qwen Code | `PreToolUse` / `PostToolUse` | `tool_input.command`, tool `run_shell_command` | `tool_response` string | `session_id`, `tool_use_id` |
| Hermes | `pre_tool_call` / `post_tool_call` | `event.args.command`, tool `terminal` | `event.result` string | `context.session_id`, `context.tool_call_id` |
| OpenClaw | `before_tool_call` / `after_tool_call` | `event.params.command`, tool `exec` | `event.result` string | `sessionId`, `toolCallId`, `runId` in event/context |
| COSH | `PreToolUse` / `PostToolUse` | `tool_input.command`, tool `run_shell_command` or `shell` | `tool_response.llmContent` string | `session_id`, `tool_use_id` |

Hermes and OpenClaw accept a **local** `{event, context}` container that the
embedding code builds from callback arguments. This does not claim either SDK
emits that envelope. For OpenClaw, the binding uses native `runId` as AW
`turn_id`; event/context IDs must agree when both exist. Hermes has no inferred
turn mapping. Qoder and Qwen Code also accept JSON-encoded `tool_input`, matching
the existing native hook path.

These mappings follow the native integrations in `src/agent-sec-core` and the
COSH emitter in `src/cosh-ng/crates/cosh-core/src/hook.rs`. They establish source
compatibility for the listed slots, not a complete framework support matrix.

## Capture, admission and execution

1. The embedding owner supplies `NativeContext`: authenticated runtime binding,
   scope and a stable boundary occurrence ID. It must not derive authority from
   model arguments or treat possession of a PID as control permission.
2. `Adapter::capture` checks native IDs against that context, checks runtime
   generation and session, and extracts one UTF-8 slot. Its digest covers the
   exact bytes, including empty results and whitespace. The original decoded
   payload remains intact, including native fields outside the AW contract. This
   capture check is not complete scope admission; the full scope schema and plan
   invariants are validated during preparation before any Provider call.
3. The policy owner supplies a resolved plan and per-step constraints, budgets
   and deadlines. `Adapter::prepare` checks occurrence, source and boundary
   binding, builds existing capability inputs, then calls `Core::prepare`.
4. `Adapter::execute` calls `Core::execute` with the supplied Provider Host,
   Journal, Clock and Cancellation ports. It returns Core records and unchanged
   native data together. A native integration decides how to interpret the
   outcome using its existing policy, approval and response conventions.

The bridge implements `security.content.inspect/v2` and
`security.code.inspect/v2`, plus `context.projection.prepare/v2` at boundaries
that admit replacement and requested reversibility. It does not implement
provider discovery, native security rules, command dispatch, recovery or adoption.
A Core `proceed` outcome is not an emitted native tool permit.

Only the listed text slots are supported. COSH preserves `returnDisplay` and
extracts only `llmContent`. Qoder completed Bash objects expose exact stdout only
when exit code is zero and stderr is empty; interrupted, image and failed
results are rejected, including contradictory `error` or `isError` markers,
and surrounding metadata is preserved. Other objects, arrays, multimodal blocks and
binary results return explicit errors. Native failure signals (`is_error`,
interrupted/denied status, or supported SDK error fields) also return errors.
The embedding code must retain its native failure behavior; this library never
converts an extraction error into an allow response.

## Why the profiles are conservative

Native hook blocking is insufficient evidence of a non-bypassable final input
guard. All six profiles therefore declare no final guard, no dispatch denial
authority and best-effort ledger requirements. Qoder post-tool revision 2
admits `unrecoverable` projection and the `local_history` proof boundary; the
[opt-in hook composition](qoder-adoption.md) owns delivery and actual history
readback. The other post-tool surfaces remain observation-only, with no proof
boundary. A descriptor is admission policy, never proof of a particular adoption.

Pre-tool capture is available. **Pre-tool plan execution is rejected** by the
existing contract because these profiles cannot establish the mandatory final
command gate. Enabling it requires a verified native enforcement integration
and a separately reviewed descriptor revision. Setting a capability flag to
true merely to pass admission would misrepresent the boundary.

Receipt validation establishes consistency with the invocation and output;
it does not certify scanner behavior. A persisted execution journal does not
prove native adoption or OS enforcement. Existing plugins remain unchanged and
continue to work independently; automatic coexistence, duplicate-hook avoidance
and native result delivery still require integration tests with real hosts.

## Run and validate

From the repository root:

```bash
cd src/aw
python3 scripts/check.py
```

The bridge tests use the real adapter and Core with a **synthetic Provider** and
an in-memory Journal. They cover all six mappings, identity mismatches, content
and code inspection with exact source coverage, plan binding, Provider failure,
duplicate occurrences and pre-tool rejection. They start no Agent and call no
SecCore scanner; passing them does not validate live framework activation or
production Provider behavior.
