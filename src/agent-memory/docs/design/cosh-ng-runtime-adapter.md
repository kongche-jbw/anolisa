# Cosh-ng Runtime Adapter

[中文版](cosh-ng-runtime-adapter_zh.md)

The Cosh-ng RuntimeAdapter translates Cosh hook events into Agent Memory
Protocol v1 operations. It is a replaceable host adapter, not part of the
Memory backend. The same backend can therefore serve Cosh-ng, DeepSeek
Harness, MCP, or another Runtime without storing Runtime-specific hook names.

## Event mapping

| Cosh event | Protocol behavior | User-turn behavior |
|---|---|---|
| `SessionStart` | Open the session, then recall with `session_resume` | Inject bounded recovery context when available |
| `UserPromptSubmit` | Lazily open the session, then recall with `turn` | Inject bounded context for the raw logical prompt |
| `PostToolUse` | Append one redacted `tool_completed` evidence event | Continue even when capture fails |
| `PostToolUseFailure` | Append one redacted `tool_failed` evidence event | Continue even when capture fails |
| `AfterModel` | No final-result capture | No effect |
| `Stop` | No final-result capture | No effect |

Shell-owned transports may suppress `SessionStart`. `UserPromptSubmit`
therefore performs the same idempotent lazy session activation before recall.
The adapter uses the raw logical prompt from the hook input, not a
provider-facing envelope or generated system context.

`AfterModel` and `Stop` happen before Cosh has committed a final assistant
turn. A stop hook may also block an answer and cause another model loop. They
must not create a Fact, TaskState, or `turn_committed` event. A future
read-only post-commit Runtime event, backed by a durable outbox, can add this
capture without changing the Memory Protocol.

## Trust and failure boundaries

The host constructs `IdentityContext`. Hook payload fields and model text may
not override tenant, team, user, Agent, or workspace authorization. For a
local process, the OS user is the trusted principal and a canonical workspace
fingerprint is only a local scope key. Managed deployments must bind a gateway
identity instead of treating `cwd` as authorization.

Memory access fails closed when identity or scope cannot be established. The
user task remains fail-open when recall or capture is unavailable, late,
malformed, or incompatible. The adapter emits an allow response with no
Memory context; it never injects an error string as model input.

Recalled content is untrusted data even when its factual authority is
verified or normative. The adapter applies a fixed wrapper, escapes content,
preserves item provenance, and enforces item, byte, and token budgets again
before producing `additional_context`. It then reports the exact admitted and
dropped item identifiers with usefulness set to `unknown`. Retrieval alone is
not counted as a useful hit.

## Evidence capture

Tool capture stores a bounded redacted summary, result class, tool name, and
an opaque reference or digest when available. It does not store unrestricted
command output. The idempotency key derives from stable session, run, tool-use,
and hook-event identifiers, so Cosh retries have exactly-once effects on a
conforming backend.

Successful and failed tool events remain immutable evidence. They do not
automatically become verified advice, policy, or resumable TaskState.

## Packaging

`adapters/cosh-ng/cosh-extension.json` registers one fail-open command hook for
the four supported events. It intentionally excludes `AfterModel` and `Stop`.
The `agent-memory-cosh-hook` executable reads one bounded Cosh hook object from
standard input and writes one hook response object to standard output.

The stage-2 executable uses the process-local conformance backend to validate
the adapter boundary. Cross-invocation persistence and user-visible recovery
arrive with the typed local backend; changing that backend does not change the
hook mapping or Cosh extension manifest. These stage-2 development assets are
not installed by the Makefile or RPM. Packaging is enabled only after the
stage-3 executable is backed by durable storage, so a process-local
acknowledgement is never presented as a working cross-hook memory feature.

## Conformance requirements

- Each external user prompt performs at most one turn recall.
- A suppressed `SessionStart` still produces exactly one lazy activation.
- Empty recall injects no wrapper.
- Invalid identity never falls back to shared data.
- Recall and capture failures do not block the user turn.
- Context admission preserves the static Cosh system and tool prefix.
- Successful and failed tool calls use distinct event outcomes.
- Duplicate tool delivery replays the same mutation.
- `AfterModel`, `Stop`, cancellation, and provider failure do not create a
  committed final result.
- Admission reporting matches the identifiers actually injected.
