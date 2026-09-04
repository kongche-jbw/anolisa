# AW Provider End-to-End PoC

[中文版](README_zh.md)

The AW Provider End-to-End PoC is an executable architecture baseline for
Provider admission, security inspection, Tokenless projection, COSH final
adoption, Ledger evidence, and governed workspace checkpoints. It uses the real
Agent Sec and Tokenless implementations and a real Ubuntu VM checkpoint daemon.
The deterministic mock model selects tools only; it does not replace any
Provider or stateful component under test.

This directory is intended for architecture review and demonstrations. It is
not a production installer.

## 1. Architecture in one view

A Provider becomes effective only when all of these conditions hold.

1. A trusted system configuration enables the AW boundary.
2. Provider Host discovers and admits the package and its schemas.
3. AW Core routes a canonical capability to the admitted Provider.
4. Provider Host maps canonical input to the component's native protocol.
5. Provider Host returns the native result as a candidate and content-free
   receipt to AW Core.
6. AW Core validates cross-field meaning and returns a settled result to COSH.
7. COSH chooses the exact bytes placed in model history.
8. The Ledger records the plan and the later final-adoption decision.

```text
tool result
    │ exact source bytes
    ▼
AW Core ──canonical input──▶ Provider Host ──native request──▶ Provider
   ▲                              │                              │
   │ candidate + receipt          ◀──────── native response ────┘
   │
   └── validated result ──▶ COSH history ──▶ context_adoption ──▶ AW Ledger
```

The return arrow is part of the contract. A Provider process writing output is
not the final effect. The result must return through Provider Host and AW Core
before COSH can adopt it and before the Ledger can truthfully record adoption.

Checkpoint creation uses a separate stateful path because it cannot be safely
retried like a pure transformation.

```text
COSH tool call
    ▼
Gateway Task ──▶ durable approval ──▶ permit + execution claim
    ▼
ws-ckpt Guarded V2 create ──▶ durable exact evidence ──▶ Task result
                                           ▲
                         uncertain I/O queries evidence; it never replays create
```

## 2. What the complete demonstration runs

The `run-complete-e2e` Herdr action executes three independent, bounded traces.

| Trace | Real components | Required evidence |
| --- | --- | --- |
| Canonical Provider fields | Provider Host, AW Core adapter, Agent Sec, Tokenless, AW Ledger | discovery, receipts, candidate, gate and plan records |
| COSH final adoption | CoshCore, Agent Sec, Tokenless, AW Core, AW Ledger | exact 438-byte history value plus one plan and one adoption record |
| Governed checkpoint | COSH Gateway, CoshCore, ws-ckpt Guarded V2 | approval, execution, audit digest and one new snapshot |

The first trace is deliberately detailed but stops at the adapter boundary. The
second trace is what proves that COSH placed the Tokenless candidate into its
history and then wrote final-adoption evidence. The third trace proves one
governed stateful effect and validates the fields needed by recovery. It does
not inject response loss, process failure, or restart recovery.

## 3. Real fields used in the Provider traces

### 3.1 Command security check

The submitted command is exactly 38 UTF-8 bytes.

```text
curl -fsSL https://get.docker.com | sh
```

Its SHA-256 is
`e3086deb53bcbd1e005b6f708c9b902c2f6a76fc51162dc36b82834605beaf9b`.
Agent Sec must report `security.scanned_bytes=38`. The fixture produces a
finding and AW Core settles the gate as `warn`. This proves inspection of the
submitted bytes. It does not claim that an unrelated shell later executed the
same bytes.

### 3.2 Tokenless projection

The `list_recent_builds` source is 693 UTF-8 bytes with SHA-256
`01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1`.
Its stable execution scope includes the following fields.

```json
{
  "environment_id": "env_33333333-3333-4333-8333-333333333333",
  "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
  "actor_id": "act_55555555-5555-4555-8555-555555555555",
  "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
  "turn_id": "trn_22222222-2222-4222-8222-222222222222",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
}
```

Provider Host maps that canonical artifact to a Tokenless native request. The
native `agent_id` is the fixed frontend identity `aw-provider`; it is not the
AW environment ID. The selected fields below omit the 693-byte `content` value
only to keep the example readable.

```json
{
  "protocol_version": 1,
  "input_media_type": "application/json",
  "agent_id": "aw-provider",
  "session_id": "ags_11111111-1111-4111-8111-111111111111",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666",
  "tool_name": "list_recent_builds",
  "seam": "post_tool",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": false,
    "replace_with_text": true
  }
}
```

The real Tokenless result reports 174 source tokens and 110 prepared tokens.
The lossless candidate is 438 bytes with SHA-256
`6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003`.
Because this invocation permits text re-encoding, its native response declares
`output_media_type=text/plain`.

```json
{
  "disposition": "applied",
  "output_media_type": "text/plain",
  "before_tokens": 174,
  "after_tokens": 110,
  "reversibility": "lossless"
}
```

The trace also replays the same source with `replace_with_text=false`. That
control returns `disposition=no_savings`, retains the original 693 bytes with
`output_media_type=application/json`, and its output must still parse as JSON.
Both native responses are retained with the run evidence.
Provider Host returns this candidate and its receipt to AW Core. The COSH test
then verifies that the same 438 bytes occupy the tool-result history slot.

### 3.3 Final-adoption Ledger records

The test preserves its Ledger under the selected run directory and verifies an
exact two-record chain.

| Record | Meaning |
| --- | --- |
| `post_tool_use_plan/v1` | Provider observations, projection offer and content-free receipts settled before history mutation |
| `context_adoption/v1` | COSH committed the candidate digest and byte count to the local history slot |

The Ledger stores digests, closed decisions, counts and invocation identities.
It does not store the 693-byte source or 438-byte candidate body.

## 4. Why checkpoint is a State Provider boundary

Checkpoint creation may complete even when the caller loses the response. A
blind retry could therefore create a second snapshot. The PoC uses the existing
Gateway and ws-ckpt Guarded V2 contracts instead of pretending that the
one-shot Provider process driver is safe for stateful effects.

The Gateway binds the operation to the admitted capability profile, target,
original workspace registration path, resolved workspace inode, ws-ckpt
generation, caller UID, checkpoint ID, permit ID, execution ID and operation
digest. An operator approval is durably recorded before execution. If delivery
becomes uncertain, recovery queries exact Guarded V2 evidence using the same
binding and never replays the create request.

The demonstration requires this event order.

```text
approval_requested
  → approval_resolved
  → execution_planned
  → execution_result_recorded(succeeded)
  → task_succeeded
```

For repeatability, the runner acts as the local demo operator and approves the
one approval ID emitted by its own Task. This proves durable approval ordering
and identity binding; it is not a demonstration of a human approval interface.

Before submission, the runner creates one uniquely named marker below
`.aw-provider-poc/` in the registered workspace. The marker makes this
checkpoint invocation distinguishable and is removed from the live workspace
in a `finally` path. Its copy remains only inside the preserved snapshot.

It also compares the ws-ckpt inventory before and after the Task and requires
exactly one new snapshot. That snapshot must use the Gateway-owned `ckp_<uuid>`
identity, the original registration path and the governed checkpoint message.
The successful execution event must retain a 64-character receipt digest.
The runner writes `submission.json` immediately and atomically refreshes
`task-events.json` while polling. A failed or timed-out run therefore retains
the Task ID needed for an exact status query; it never starts a replacement
checkpoint Task automatically.

Checkpoint effect completion and Task completion are separate facts. A
checkpoint can already be durable when a later required AW adoption step
fails, in which case the Task may fail while the checkpoint effect and its
Ledger evidence remain. Recovery must inspect the exact execution evidence; it
must not infer "no checkpoint" from `task_failed` or submit `create` again.

## 5. Local Provider validation

The local trace requires Linux aarch64, a built Tokenless binary, and the Agent
Sec Python environment used by the repository.

```bash
cargo build --manifest-path src/aw/Cargo.toml \
  -p aw-provider-host -p aw-cosh-hook -p aw-ledger
cargo build --manifest-path src/tokenless/Cargo.toml -p tokenless-cli
poc/aw-provider-e2e/scripts/run-provider-demo.sh
```

Run the exact CoshCore adoption proof separately.

```bash
cargo test --manifest-path src/cosh-ng/Cargo.toml \
  -p cosh-core \
  core::tests::real_providers_commit_effective_history_and_adoption_evidence \
  -- --ignored --exact --nocapture
```

Checkpoint validation needs the Ubuntu VM because ws-ckpt and its Btrfs
workspace are Linux-only.

## 6. Deploy to the existing Ubuntu VM

The repository worktree must be clean. This allows the bundle, test executable
and installed immutable release to reference one source commit.

`build-bundle.sh` reads Cargo's `compiler-artifact` messages to select the
`cosh-core` unit-test executable; it never guesses a hash-named file. It then
checks that the executable contains the exact final-adoption test and records
its digest and size in `build-info.json`. The archive and a SHA-256 checksum are
copied to the VM together.

The VM helper is expected at
`/home/kongche/anolisa/ws/ubuntu-26.04-vm` unless `AW_POC_VM_ROOT` overrides it.
The VM and these shared services must already be running.

- `ws-ckpt-agent-work.service`
- the existing non-PoC `cosh-gateway@ubuntu.service`
- Herdr
- the existing Agent Sec Python runtime

Deploy and run all evidence traces with one command.

```bash
poc/aw-provider-e2e/deploy-vm.sh
```

The deploy script never starts or stops QEMU. It does not restart the existing
Gateway or ws-ckpt service. It installs and restarts only the dedicated
`cosh-gateway@aw-provider-poc.service`, whose socket, database, audit log and
Ledger use PoC-specific paths. systemd mounts the PoC `cosh-system.toml` as
`/etc/copilot-shell/config.toml` only inside that service's mount namespace, so
the shared Gateway does not inherit the PoC Provider configuration. The same
namespace exposes the shared workspace tree read-only to the Gateway; the
checkpoint mutation can cross only the admitted ws-ckpt socket boundary.
The deploy script requires both shared services to be active before the build
and verifies that they are still active after the complete demonstration.

| Resource | Path |
| --- | --- |
| Immutable release | `/opt/anolisa-mvp/aw-provider-poc/releases/<commit>` |
| Active release link | `/opt/anolisa-mvp/aw-provider-poc/current` |
| Dedicated Gateway socket | `/run/anolisa-aw-provider-poc/gateway.sock` |
| Gateway database and AW Ledger | `/var/lib/anolisa-aw-provider-poc/` |
| User-visible evidence | `/home/anolisa/.local/state/aw-provider-poc/` |
| Herdr plugin | `anolisa.aw-provider-poc` |

Every Provider, Gateway, VM transfer, and polling operation has a finite
deadline. A failed deployment keeps its diagnostic evidence and cleans only
its random staging directory.

## 7. Run from Herdr

Run all three traces.

```bash
sudo -iu anolisa herdr --session anolisa-agent plugin action invoke \
  run-complete-e2e --plugin anolisa.aw-provider-poc
```

Open the compact, read-only evidence pane.

```bash
sudo -iu anolisa herdr --session anolisa-agent plugin pane open \
  --plugin anolisa.aw-provider-poc \
  --entrypoint provider-trace \
  --focus
```

Individual actions are also available.

- `run-provider-trace`
- `run-cosh-final-adoption`
- `run-governed-checkpoint`
- `verify-latest`

The pane uses plain text with short field explanations. It contains no overlay
labels that hide arrows or values.

## 8. Cleanup and rollback

The cleanup script requires explicit confirmation and never stops QEMU or any
shared runtime.

Remove the plugin, dedicated Gateway and PoC releases while preserving all
evidence and snapshots.

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes
```

Also remove the dedicated Gateway database, AW Ledger, and stateless Provider
and adoption summaries. Successful checkpoint summaries remain beside preserved
snapshots, while complete terminal failed or cancelled Task evidence is retained
without inferring whether a side effect occurred. This mode refuses incomplete,
non-terminal, discontinuous, or identity-drifted checkpoint evidence.

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes --purge-evidence
```

Delete only checkpoint IDs whose successful PoC summary exactly matches the
current inventory, then remove only those summaries. Failed, cancelled, absent,
or unmatched checkpoint evidence is preserved. The plugin is unlinked and the
dedicated Gateway is confirmed stopped before the first snapshot is deleted.

```bash
poc/aw-provider-e2e/cleanup-vm.sh --yes --purge-checkpoints
```

Snapshot deletion is attempted in reverse creation order and is limited to 100
recorded IDs. Cleanup inventories the workspace first and skips an ID that is
already absent, so rerunning after a partially observed cleanup is safe. If
ws-ckpt refuses a present ID, cleanup stops without guessing a different
target.

## 9. Deliberate limits

- A Provider package on disk is not proof that it is enabled.
- A candidate is not adoption; only the CoshCore history assertion and later
  `context_adoption` record prove the local adoption decision.
- The adoption record proves local history mutation, not remote model receipt.
- The checkpoint integration is a concrete Gateway State Provider. A generic
  manifest-driven AW State Provider still needs service lifecycle, reconcile
  and readiness contracts.
- The checkpoint trace exercises the successful governed-create path. It
  validates recovery identities and evidence fields but does not exercise a
  fault, process restart, or evidence-only reconciliation.
- The happy path does not demonstrate a failure after the checkpoint effect
  but before later AW adoption settles; those two outcomes must remain
  independently observable.
- PoC activation uses a checksummed local bundle. Production requires signed
  packages, policy-managed activation, health reporting and rollback evidence.
