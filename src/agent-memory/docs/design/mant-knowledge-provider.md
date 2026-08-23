# ManT Knowledge Provider

[中文版](mant-knowledge-provider_zh.md)

The knowledge-provider boundary gives Agent Memory optional, focused access to
documents owned by another system. ManT is one adapter for that boundary. It
is not a storage dependency, is never installed or downloaded by Agent Memory,
and is not required for local task and evidence recall.

## Provider-neutral contract

`KnowledgeProvider` is a synchronous `Send + Sync` trait with three operations:

| Operation | Contract |
|---|---|
| `descriptor` | Performs live capability negotiation and returns typed identity, version, protocol, and focused capabilities |
| `health` | Maps descriptor success to `healthy` and every typed negotiation failure to `degraded` |
| `query` | Resolves one bounded `KnowledgeQuery` into bounded `KnowledgeItem` values |

`KnowledgeQuery` always names a document and exactly one focused selector:
literal search, single-entry explanation, or section excerpt. There is no
whole-document selector. Query validation limits individual and combined input,
selector count, result count, and excerpt bytes before invoking a provider.

Each item contains only a `KnowledgeRef`, optional bounded title, bounded
excerpt, response fingerprint, and optional relevance score. The ref preserves
the provider, document, focused selector, retrieval time, and fingerprint for
staleness checks. Full manuals and provider databases remain provider-owned.

Provider parsing establishes provenance, not truth. A returned item remains
Candidate and untrusted data at the Memory admission boundary. Runtime policy
must preserve the fixed untrusted-data wrapper and may not promote the item to
Verified or Normative merely because ManT parsed it.

## ManT v0.9 adapter

`MantCliProvider` executes an explicitly configured executable path directly;
it does not search for, install, or update ManT. It never invokes a shell and
places all user-controlled document and selector values in JSON on standard
input rather than process arguments.

Before every focused query, the adapter executes:

```text
mant --protocol-version --compact
```

The returned JSON descriptor must exactly identify `mant.cli/v0.9`,
`mant.request/v0.9`, `mant.excerpt/v0.9`, and `mant.search/v0.9`. Unknown
additive descriptor fields are ignored. This permits compatible metadata
growth without accepting a different request or response schema.

Focused queries execute:

```text
mant --request-json --format json --compact
```

The adapter writes exactly one compact JSON request to stdin. The native ManT
request limit of 65,536 bytes is checked explicitly. Search is always literal
and restricted to the visible document scope. Explain and excerpt responses
must identify `mant.excerpt/v0.9`; search responses must identify
`mant.search/v0.9`. The adapter extracts only focused response collections such
as selections or matches and caps the admitted excerpt again.

## Process and failure boundary

Every probe and query has a hard wall-clock deadline. The adapter creates a
separate process group, drains stdout and stderr concurrently into bounded
buffers, and kills the entire group on timeout. This prevents a helper process
from keeping output pipes or the request alive after the primary CLI exits.

Stdout is accepted only within the configured cap and only as valid JSON for
the negotiated schema. Stderr is bounded and drained for process safety but is
discarded; it never enters an error message, a memory item, logs, or model
context. Executable paths, queries, and document content are also absent from
safe errors.

Health is deliberately fail-safe:

| Condition | Typed state |
|---|---|
| Executable absent or not executable | `degraded / unavailable` |
| Protocol or required schema differs | `degraded / incompatible` |
| Deadline exceeded | `degraded / timeout` |
| stdout or stderr cap exceeded | `degraded / resource_exhausted` |
| Invalid JSON or response schema | `degraded / malformed_response` |

A degraded knowledge provider does not block local TaskState or evidence
recall. It also must not be presented as a complete knowledge hit. The Runtime
continues without provider context and exposes the typed degradation through
health or observability.

## Local broker and Cosh binding

`LocalMemoryBackend::open_with_knowledge` accepts a provider-neutral binding;
the default `open` path has no knowledge dependency. For a normal turn the
broker selects one focused literal from the prompt, admits reviewed TaskState
first, then Candidate knowledge, then observed tool evidence. All three lanes
share the request item, byte, and token budget and one persisted RecallTrace.
If the provider fails, the view uses `local_only_knowledge_degraded`, records a
typed reason, and still returns eligible local state and evidence.

The Cosh one-shot hook discovers an already installed `mant` on trusted PATH.
`ANOLISA_MANT_PATH` selects an explicit executable,
`ANOLISA_MEMORY_MANT_DOCUMENT` selects the logical document (`bash` by
default), and `ANOLISA_MEMORY_MANT=off` disables the binding. These are trusted
host settings, never model arguments. Absence means the provider is unloaded,
not broken; Agent Memory never runs an installer or update command.

## Fingerprints and lifecycle

The v0.9 adapter computes an `fnv1a64` change fingerprint over the already
bounded focused JSON response. This is a deterministic staleness detector, not
a cryptographic integrity proof. Memory may persist the ref, selector,
fingerprint, and its own bounded admitted excerpt. It must re-query the provider
to refresh content and must not turn the excerpt into a provider-independent
copy of the manual.

The adapter does not cache descriptors or query results. This keeps executable
replacement and protocol upgrades visible on the next operation, at the cost
of one probe process per query. A future supervised provider or cache may
optimize this behind the same trait if it retains explicit expiry and typed
degradation.

## Validation

`knowledge_provider_test.rs` covers a provider-neutral fake, a fake executable
using the negotiated one-shot JSON shape, focused extraction that excludes an
unrelated full-manual field, missing and incompatible executables, process
group timeout, output bounds, and aggregate query bounds. Compatibility with a
new ManT protocol family requires a deliberate adapter revision and fixtures;
the current adapter never guesses from human-readable CLI output.
`local_backend_test.rs` additionally verifies merged selection and local-only
degradation, while `cosh_hook_wire_test.rs` exercises a real one-shot Hook
process against a fake ManT v0.9 executable.
