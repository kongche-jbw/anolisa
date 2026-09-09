# AW Core implementation baseline

[中文版](core-baseline_zh.md)

Core 0.1.0 implements the execution between the existing contract checks.
All 21 registered schemas and eight retained PoC review schemas are unchanged.
This pins an implementation for review; it does not declare the pending schema
proposal accepted or introduce another public Agent message protocol.

## Structure and embedding

`aw-contracts` remains the pure root crate. `crates/aw-core` owns plan preparation,
execution and private journal storage. A caller constructs `Core`, implements
`ProviderHost` and `Clock`, then supplies `PrepareRequest` with a trusted plan,
boundary, runtime and one `StepInput` per step. `prepare` resolves every selected
descriptor and validates every invocation before any Host call. Explicitly empty
routes remain gaps; a named but unavailable provider rejects preparation.

`execute` consumes the immutable `PreparedPlan` and uses `Journal` and
`Cancellation` ports. The event reservation key binds full scope and event ID;
invocation IDs bind plan digest, step and selected provider. Reusing a scoped
event with another plan does not permit a second run. Different scopes do not
share event reservations. Runtime/descriptor authority is supplied by the
embedding owner; matching JSON is not authentication.

The Host registers drivers, maps native protocols and enforces execution/output
limits. Core has no provider command-line knowledge, transport daemon or scanner
implementation. It checks descriptors again before calls, keeps original absolute
deadlines, validates receipts and rejects successful returns that arrive outside
the observed local call budget. It cannot preempt a blocking Host implementation;
hard time/memory limits remain Host obligations. Cancellation is sampled between
calls, not an interrupt of an in-flight operation. Runtime ownership must remain
valid for the execution window; the embedding owner handles exit/restart races.

Core accepts policy-resolved plans rather than inventing discovery priorities or
framework hook order. Steps execute serially; all selected providers in one step
are accounted for before reducing its result. Mandatory command warn/deny never
becomes proceed because another provider allows. Required gaps deny a pre-tool
plan or preserve a result as specified; optional gaps continue only when the plan
explicitly allows it. Cancellation, denial and preservation leave later steps
skipped. The existing `validate_plan_execution` checks the generated whole record.

## Journal and failure behavior

`FileJournal` is a Linux backend for a trusted local directory and filesystem
that honors synchronization. New directories/files use private permissions.
Atomic file creation reserves each event across processes and restarts. Core
acknowledges the claim and an invocation-start record before calling a Provider,
then stores content-free receipts and step outcomes before proceeding. Terminal
execution is returned only after a final acknowledged append.

The private format-1 envelopes carry sequence, previous digest, record and digest.
This storage framing is not an AW schema or a replacement for a general Ledger
service. Core records plans, IDs, digests, receipts and execution facts without
raw capability inputs/outputs. `Execution::calls` keeps those inputs/outputs in
memory; the embedding application decides how to protect and retain artifacts
for later readback. Receipt meters remain Provider facts, never adoption totals.

Host transport errors, malformed results, late successful responses or journal
errors return an error and leave the event reserved. After dispatch, an unfinished
journal means reconciliation is required, not proof the provider never ran. There
is no automatic recovery, retry, reservation deletion or re-dispatch API.

Reads check canonical records and the hash chain. Partial records and broken
chains reject. A valid complete prefix cannot reveal removed whole tail records;
`read_verified` checks against a last acknowledgement retained independently.
Hashes do not protect against an attacker who can rewrite both the store and its
external acknowledgement. Non-Linux `FileJournal::new` explicitly returns
unsupported; alternate backends can implement `Journal`.

## Native handoff

`Execution::record`, `calls` and `journal_ack` expose actual correlated results.
They do not dispatch a tool or write a conversation. For result adoption, the
adapter independently observes effective/recovered text and uses
`Registry::validate_plan_adoption`. For pre-tool dispatch the owner captures a
fresh intent and independent OS binding and uses `Registry::validate_dispatch`,
atomically with its native action. A Core `proceed` result is not that permit.

No Qoder, Codex, OpenClaw, Hermes, Tokenless or SecCore runtime has been connected
by this change. No checkpoint/restore effect is admitted by this profile.

## Reproduction and validation scope

From the repository checkout, run the regular-user checks:

```bash
cd src/aw
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo doc --workspace --no-deps --locked
python3 tests/check_canonical.py
cargo run -p aw-core --example pinned_plan --locked
```

The example uses an in-process reference Provider and isolated local journal.
It is a Core execution demonstration, not real compression or Agent acceptance.
Contract, execution and journal tests use synthetic inputs and exercise failure,
duplicate events, descriptor/schema drift, cancellation and storage corruption.
Linux ARM64 is the tested environment; clean-VM installation, other architectures,
power-loss testing and real Provider/Agent integration remain unverified.

## Upstream comparison and provenance

The source review pinned casparant's
[`provider-poc-v4`](https://github.com/casparant/anolisa/tree/42d07649409ecd5bb023056b28545efbd9325ef2/src/aw).
Caspar Zhang's Core planning (`b299cdfe`, `1556e9d6`, `d8902670`), Host
(`7113de26`) and Ledger (`7763e2ea`, `192a844e`) informed the separation of
planning, execution and storage. Their typed v1 contracts, default decisions and
Ledger schemas cannot be substituted for this branch's v2 payloads and plan
invariants. No upstream source file or commit is transplanted in this baseline;
the implementation reuses this branch's canonical encoding and validators.
Both repositories use Apache-2.0. Native process-driver reuse can follow in a
separate Host change with exact file provenance and compatible contracts.
