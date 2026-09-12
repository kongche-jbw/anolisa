# Qoder projection and recorded adoption

[中文版](qoder-adoption_zh.md)

The explicit Qoder path returns a Tokenless candidate after required SecCore
inspection and records independent local-history observations. It reuses the
existing Adapter, Core, two native Hosts and FileJournal. Contracts remain pure;
Core has no dependency on Qoder, SecCore or Tokenless implementations.

## Responsibility and execution

`aw-hook-cli` keeps one composition module (`projection`), private context storage
(`records`), one fixed history reader (`history`) and observation/query logic
(`adoption`). The thin `aw-adoption-cli` binary calls the same library. There is
no ninth crate, second journal format, plugin installer or background watcher.

1. Validate live owner/scope, successful structured Bash output, explicit
   unrecoverable/retention policy and the declared history file before native
   version probes. Capture its device/inode, prefix byte length/digest and input.
2. Register the existing SecCore and Tokenless Hosts under distinct IDs. Prepare
   one two-step plan: required inspection, then optional projection. Both use
   `reject_plan`; a gap preserves the source and stops later calls.
3. Execute serially with shared cancellation. Per-call budgets stay independent;
   projection's absolute deadline includes both serial budgets. SecCore findings
   remain observational; they do not authorize or deny final tool dispatch.
4. Preserve full calls, outputs, plan, boundary and original Journal tip in a
   private context before exposing replacement text. Validate them against the
   complete plan and matching Journal entries, including cancellation between a
   durable invocation start and dispatch.
5. Return Qoder's `updatedToolOutput` slot only for a produced candidate and a
   proceeding plan. A separate marker follows successful stdout transmission;
   delivery errors do not append a second response or claim adoption. Stdout
   uses unbuffered nonblocking writes, a five-second deadline and cancellation;
   it restores inherited descriptor flags on exit.

The [official Qoder hooks](https://docs.qoder.com/cli/hooks) describe the
replacement slot and native hook composition. The pinned history profile is
`qoder-cli-1.1.47/jsonl-v1`; internal JSONL row shapes are not an official stable
API. A launcher must verify the installed version and register a synchronous,
single-turn hook. This code does not install or supervise the Agent.

## Independent observation and durable evidence

`observe` reads only the declared caller-owned, non-symlinked regular file, with
a 16 MiB total cap and 1 MiB native JSON line cap. The original prefix and inode
must be unchanged. It scans the whole snapshot for one ordered assistant
`tool_use` and user `tool_result`, with the exact session/tool ID/cwd/input and
`isSidechain: false`. Successful results may omit `is_error` or set it to false;
other values fail. Matching result text must begin beyond the captured prefix.
Tool input may be an object or a duplicate-free JSON-encoded object. The tool
call may already be in the prefix or be appended before the result.

The process's wall clock records the observation time and checks the configured
receipt-to-observation window. A native row timestamp is not used to prove when
hooks completed. Missing/late results provide no adoption conclusion; corrupt,
ambiguous or wrongly bound evidence is an error. There are no automatic retries.

A retained fact contains the binding digest, snapshot digest/size, exact matched
rows with line numbers/offsets/digests and observed time. Its digest is appended
to the same Core FileJournal under a domain-separated key derived from the
context digest and observation kind. Only after durable acknowledgement is the
fact plus external Journal tip written to its immutable private sibling file.
Core storage contains metadata, not raw rows. Duplicate or interrupted claims
remain reserved; there is no recovery by blindly retrying. Failure before the
external tip is stored leaves no committed observation for query purposes.

`query` opens existing private storage read-only, checks the original execution
and independently anchored observation chain, replays the retained matched rows,
and calls `Registry::validate_plan_adoption`. It does not create directories,
sync files, spawn providers or reopen live history. Replay establishes the
persisted scan's matching rows; uniqueness came from the observer's complete
scan at that time. It does not independently rescan or authenticate the current
history. Hashes cannot resist an actor who can rewrite every trusted copy.

## Meaning of the result

`prepared`, `returned` and `observation_status` describe separate facts. A missing
stdout marker need not contradict an independently matching history result.
An adopted observation requires exact candidate text and a complete proceeding
plan; preserved means exact source text; overridden means other text and zero
attributed savings. No candidate without history remains unverified.
`ledger_status: committed` refers to the observation fact's own acknowledgement,
never just the original execution ACK. `candidate_digest` binds the complete
output envelope, as required by the existing adoption contract.

The query labels its proof `local_history` and `recorded_snapshot`. Byte savings
are exact at that recorded representation, not proof of AW's causal effect,
model-request delivery, tokenization or billing. Later transforms remain possible.
There is no model_request proof, recovery, final security gate or default
product enablement in this stage.

## Verification and limits

The AW gate covers real local protocol peers, native frozen Tokenless outputs,
synthetic history fixtures, preserved/overridden/absent observations, corruption,
duplicate events, cancellation and delivery failures. The reader's success-error
shape is also compared with retained Qoder 1.1.47 synthetic-workload history.
These checks do not replace a live Qoder acceptance run with the exact installed
release and registration. The profile remains experimental until that separate
native path is verified. Explicit plaintext retention and parent-path isolation
are embedding-owner obligations; no automatic retention policy or GC is added.

Configuration, command behavior and disabling are documented in the
[user guide](../../../../docs/user-guide/en/user-entrypoint/aw.md).
