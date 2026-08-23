# Typed Local Memory Backend

[中文版](typed-local-backend_zh.md)

`LocalMemoryBackend` is the durable single-user implementation of Agent
Memory Protocol v1. It replaces the process-local conformance backend in the
Cosh hook executable while preserving the same RuntimeAdapter and protocol
contract. The legacy Markdown store and BM25 index remain independent during
the migration; neither is treated as the source of truth for typed task and
trace state.

## Storage boundary

The default database is
`$XDG_STATE_HOME/anolisa/agent-memory/memory-v1.sqlite3`. A trusted local
launcher may set `ANOLISA_MEMORY_DB` to an explicit path for testing or a
managed layout. This variable chooses storage only; it never supplies user,
Agent, tenant, or workspace identity.

The immediate database directory is owner-only (`0700`) and the database file
is `0600`. A database symlink is rejected. SQLite runs with WAL, foreign keys,
a bounded busy timeout, and `synchronous=FULL`. Schema `user_version=1` is
created transactionally, while a database from a newer binary is rejected
instead of being opened with guessed semantics.

Errors crossing the protocol expose a stable error code and safe diagnostic.
They do not include the database path, query, event content, or SQLite detail.

## Typed durable model

| Durable object | Scope | Key behavior |
|---|---|---|
| Session | tenant/team/user/Agent/workspace/session | Opens or resumes the Runtime binding |
| Runtime event | session, with workspace recovery projection | Immutable, event-ID unique, idempotent capture |
| TaskState | workspace | Current projection with optimistic revision |
| ContextView | session | Bounded model-visible selection snapshot |
| RecallTrace | ContextView | Ordered selection and admission decisions |
| Recall outcome | ContextView | Complete admitted/dropped partition and usefulness |
| Close record | session | Replay-safe terminal outcome |

Runtime events and TaskState remain different data classes. A successful or
failed tool event is Candidate evidence; it does not automatically become a
verified instruction. TaskState is a reviewed resumable projection with a
goal, next action, blockers, revision, and external evidence references.

All mutation idempotency records are committed in the same immediate
transaction as their primary object. Success therefore acknowledges
`durable`, not merely buffered process state. Reusing a key with equivalent
content returns `replayed`; reusing it with different content returns
`conflict`. A task update must supply the revision it observed and commit
exactly the next revision, so two Agents cannot silently overwrite each other.

## Recall and explanation

Recall first admits verified TaskState, then relevant recent Candidate tool
evidence. Normal turns use bounded lexical relevance for event evidence;
session recovery can include recent workspace evidence. Both lanes share the
request item, byte, and token budget. Every returned item carries kind,
authority, source, revision when present, and a selection reason.

The backend persists the final ContextView and its RecallTrace. The Cosh
adapter performs a second safety admission step and reports the exact returned
items that entered or were dropped from `additional_context`. The trace keeps
retrieval separate from Runtime admission and keeps usefulness `unknown`
until an attributable signal is available.

Views are diagnostic snapshots, not another conversation transcript. Their
capacity is bounded and the management plane can remove them independently.
They expire after seven days. Closed sessions and their raw Candidate events
expire after 30 days, while reviewed TaskState remains until explicitly
forgotten. At a hard quota, the backend removes the oldest views and then the
oldest closed sessions in bounded batches; active sessions are never selected.
This keeps diagnostic history from permanently disabling recall.

## Cold recovery contract

Closing or killing the Hook process does not remove committed sessions,
events, TaskState, views, traces, or mutation keys. A new process can reopen
the same database and:

- replay an acknowledgement whose response was lost;
- recover the latest TaskState revision;
- recall relevant tool evidence from an earlier session in the same workspace;
- explain a previous ContextView from its owning session; and
- preserve a previously reported admission outcome.

Recovery never claims to restore a provider KV cache, process, PTY, file
descriptor, or in-flight tool outcome. Those resources need separate Runtime
or checkpoint providers and must remain unknown when no evidence exists.

## Capacity and telemetry

The backend applies explicit row, record-size, ContextView, trace-decision,
and idempotency limits. `stats()` reports SQLite logical bytes, physical bytes
including sidecars, and session/event/task/view row counts. Provider-managed KV
capacity remains `unknown`; it must not be inferred from these database
counters. Working-context tokens and retrieval/admission outcomes are also
reported separately.
