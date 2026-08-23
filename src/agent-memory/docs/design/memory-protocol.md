# Agent Memory Protocol v1

[中文版](memory-protocol_zh.md)

Agent Memory Protocol v1 is the implementation-neutral boundary between an
Agent Runtime adapter and a Memory backend. Cosh-ng, DeepSeek Harness, MCP
clients, local SQLite, and remote services can implement either side without
depending on the current Markdown store or MCP tool names.

## Boundary

The protocol owns typed request and response envelopes, capability
negotiation, authenticated identity dimensions, correlation, deadlines,
bounded ContextView results, task checkpoints, RecallTrace, and safe errors.
It does not own Runtime hooks, storage schemas, embedding models, ManT, or a
transport. JSONL stdio is the first binding; Unix sockets and HTTP can carry
the same envelopes later.

The canonical Rust types live in `src/protocol.rs` and
`src/protocol/types.rs`. Print their JSON Schema bundle with:

```bash
agent-memory-backend --schema
```

The `tests/fixtures/protocol/v1/` files are cross-language golden fixtures.

## Trust model

`IdentityContext` must be populated by a trusted Runtime or transport. Model
input may not choose `tenant_id`, `team_id`, `user_id`, `agent_id`, or
`workspace_id`. Every required identity is non-empty and bounded; an absent
identity is an invalid request and never falls back to shared data. ContextView
and feedback identities are session scoped, while TaskState is workspace
scoped.

Memory content is data, not instruction. A ContextItem carries its kind,
authority, source reference, staleness, selection reason, and byte/token cost.
Candidate content must not be promoted to an instruction by an adapter.

## Operations

| Operation | Capability | Purpose |
|---|---|---|
| `negotiate` | none | Verify required capabilities and protocol version |
| `open_session` | `session` | Open or idempotently resume one Runtime session |
| `append_event` | `capture` | Capture one bounded event with an idempotency key |
| `materialize_context` | `recall` | Return a byte-, token-, and item-bounded ContextView |
| `checkpoint_task` | `checkpoint` | Persist resumable TaskState and EvidenceRef values |
| `explain_context` | `explain` | Return the RecallTrace for a ContextView |
| `report_recall_outcome` | `outcome` | Record admitted, dropped, and usefulness state |
| `forget` | `forget` | Remove a Memory-owned object in caller scope |
| `close_session` | `session` | Close the Runtime session without deleting TaskState |

The operation set is semantic. A Cosh Hook name, MCP tool name, command-line
flag, or database table is never part of this contract.

Capability names are open string values. An older client preserves an unknown
name and cannot accidentally satisfy one unknown requirement with a different
unknown backend capability. Response structures and safe errors accept
additive fields; request structures remain strict.

Task checkpoints use optimistic concurrency. New tasks start at revision one;
updates supply the previously observed `expected_revision` and commit exactly
the next revision. A stale writer receives `conflict` instead of overwriting a
newer agent projection. Every replayable Memory mutation also carries an
idempotency key so an acknowledgement lost in transport can be replayed safely.
Mutation responses state whether they were replayed and whether the backend is
process-local or durable.

`turn_committed` is the only event kind for a final model result. Pre-commit
Runtime events such as Cosh `AfterModel` and `Stop` must never be mapped to it.
Handoff recall requires an explicit source task and target Agent binding, and
the task must match the envelope correlation.

## Context and measurement

`materialize_context` declares a `turn`, `session_resume`, or `handoff`
purpose. It carries hard maximums for items, UTF-8 bytes, and estimated model
tokens. The backend reports the effective strategy, degradation, truncation,
snapshot revision, and the Runtime-supplied trace identifier. An explained
trace preserves its originating trace and separately reports the current
response trace. Dispatch verifies
the returned item count, content bytes, token estimates, totals, identities,
and finite scores before emitting a response. Runtime-side admission is
reported separately so a
returned candidate is not counted as a hit merely because it was retrieved.

Exact tokenization remains Runtime/model specific. A backend estimate cannot
replace provider-reported usage; consumers record both when available.

## Errors and fallback

Wire errors use stable codes and safe messages. Backend, provider, database,
query, and local path details must not cross the boundary. A Runtime may
continue the user turn after a retryable Memory failure, but Memory access
itself fails closed for missing identity, incompatible versions, scope errors,
and integrity conflicts. A degraded fallback must be visible in ContextView
and RecallTrace.

Absolute deadlines enter every backend call. Dispatch rejects work both before
admission and after backend completion, so a late success cannot enter model
context. The synchronous v1 binding cannot preempt arbitrary backend code;
process and network adapters must additionally enforce cancellation at their
I/O boundary.

Session close is idempotent, so a deadline that expires after the side effect
does not make the retry fail. Conformance storage bounds mutation aliases as
well as primary objects.

## Conformance backend

`EphemeralMemoryBackend` is deterministic process-local test infrastructure.
It implements sessions, idempotent event capture, task checkpoints, bounded
materialization, explanation, outcome reporting, and scoped forget. It is not
durable and is not a production authority. Its sessions, events, tasks, views,
and trace candidates have explicit capacity limits. `agent-memory-backend`
exposes it over JSONL with a 1 MiB limit in both directions for adapter
development and schema/golden-fixture tests.

The durable typed local backend is a separate implementation. It must use the
same contract, and swapping between it and the ephemeral backend must not
change Runtime adapter code.
