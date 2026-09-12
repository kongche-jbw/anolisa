# SecCore Host and native hook composition

[中文版](sec-core-host_zh.md)

The Linux Host runs the existing SecCore PII scanner through the AW execution
chain. The opt-in hook connects Codex or Qoder `PostToolUse` text to that Host,
records the inspection, and retains the original tool result. Its Provider
advertises `security.content.inspect/v2`, `authority: advise` and
`guarantee: declared`; this is content observation after execution.

## Ownership and dependencies

| Crate | AW dependencies | Responsibility |
|---|---|---|
| `aw-contracts` | None | Versioned schemas and side-effect-free validation |
| `aw-core` | Contracts | Pinned preparation, serial execution and acknowledged journal records |
| `aw-adapters` | Contracts, Core | Native text capture, identity consistency and inspection-plan bridging |
| `aw-sec-core` | Contracts | Pure mapping between content inspection and native PII JSON |
| `aw-host-process` | Contracts, Core | Shared launch pins, bounded process ownership and cancellation |
| `aw-sec-host` | Contracts, Core, SecCore mapping, Host process | Native admission, protocol mapping and Provider receipts |
| `aw-tokenless-host` | Contracts, Core, Host process | Native projection mapping and receipts; pure Tokenless protocol dependency |
| `aw-hook-cli` | Contracts, Core, Adapters, Host | Opt-in hook settings, live owner checks and one-event composition |

Core and Contracts have no dependency on the concrete Host. Native protocol
mapping creates no process and implements no scanner. The shared process runner and hook use
`libc` only for their Linux process boundaries; neither introduces an async
runtime or another Agent supervisor. The shared gate checks these dependencies,
source limits and nonempty Host/hook integration targets.

The data path is native payload → Adapter capture → Core preparation/execution
→ Host process → native `scan-pii` → mapped inspection and receipt → Core journal
→ native observation response. See [Core execution](core-execution.md),
[native adapters](native-adapters.md) and [SecCore mapping](sec-core-adapter.md)
for the corresponding contracts.

## Native launch and provenance

`SecHost::new` validates the declared configuration and descriptor, checks the
executable and selected file pins, and runs `--version` through the same bounded
runner used for inspection. Admission requires exit code zero and the exact
response `agent-sec-cli 0.12.0\n`. Provider release identity remains a separate
operator-selected value. The manifest digest binds the full declared launch
configuration, native version, protocol profile and capability.

The invocation is the configured executable, configured prefix arguments and:

```text
scan-pii --stdin --format json --source tool_output
```

The Host supplies exact UTF-8 stdin without inserting a newline. The mapping
adds `--include-low-confidence` only when the AW request selects it. It does not
request raw evidence or redacted replacement text, disable native middleware,
or substitute a fixed detector list. Native user rules and middleware operate
under the explicitly supplied environment and working directory.

The executable and working directory must be absolute. `env_clear` prevents
inheritance of undeclared environment variables. The operator supplies the
expected executable SHA-256 and up to 32 selected file pins, each bounded to
64 MiB. A pin specifies an exact SHA-256 or explicit absence. Checks before and
after successful native exchange reject changed executable bytes, changed
selected files or a violated absence assertion. Pins may cover Python modules
and user rule files where those affect the declared deployment.

These are selected-file consistency checks, not an attestation of the complete
interpreter import graph. They do not prevent same-user changes between checks,
changes followed by restoration, or access to unpinned files. Settings and
journal placement must remain launcher-owned; neither configuration permissions
nor PID ancestry creates an isolation boundary against a same-user adversary.

## Budgets and process lifecycle

The Host validates the invocation binding and content digest before scanning.
Its launch timeout is the minimum of the configured limit, invocation wall-time
budget and remaining absolute deadline. Preparation and pin checks consume that
same budget; the remaining time is recalculated before spawn. A produced result
must still fit the observed deadline, wall-time limit and encoded output budget.

Native stdin, stdout and discarded stderr have independent byte ceilings.
Configuration permits 1–300,000 ms and 1 byte–4 MiB per stream. A single-threaded
nonblocking loop exchanges pipes and checks elapsed time between bounded reads;
a continuous writer cannot prevent deadline checks. The version probe uses the
configured limits. The hook separately limits settings/native input to 1 MiB
and waits at most five seconds for native stdin.

`SecHost::new_cancellable` accepts a shared caller-owned cancellation port. The
version probe and each native call check it before spawn and between pipe
iterations, then use the same cleanup path on cancellation. `SecHost::new` uses
`NeverCancel`; embedding applications needing shutdown coordination must supply
their own port. The library installs no global signal handler.

Each native child starts a dedicated process group. Every normal or error return
attempts group-directed `SIGKILL`, then verifies cleanup before exposing a
response. `waitid(..., WNOWAIT)` retains the group leader's PID until group checks
finish, preventing PID reuse during group-directed signaling. The Host requires
exclusive ownership of its child's reaping: embedding code must not race it
with a global `waitpid(-1)` handler.

Cleanup has an additional one-second polling budget and a 65,536-entry procfs
scan ceiling. It checks for active tasks in the owned group, including threads
of a zombie process leader, before nonblocking reaping of the direct child.
Unverifiable or incomplete cleanup returns `provider_cleanup_failed`, overriding
an otherwise successful response. Drop performs only nonblocking best effort.

Process-group cleanup cannot contain descendants that escape with `setsid` or
another group. It does not promise to reap grandchildren. An uninterruptible
kernel task, blocked filesystem operation or process spawn cannot be given a
hard realtime completion guarantee by these userspace deadlines. An inability
to verify cleanup remains a visible failure; this Host provides no cgroup,
subreaper or sandbox attestation.

## Receipts, journaling and native results

The Host projects native output only after exit, coverage, consistency and schema
checks. It binds its receipt to the invocation, Provider identity, schemas,
input digest, scope and plan reference, then validates that receipt and output.
Ordinary native failures produce a failed receipt without output; malformed
invocation binding or an untrustworthy receipt returns a Host error. Native
stderr and evidence snippets do not appear in error messages or hook responses.

The hook creates one required content-inspection step with `reject_plan` failure
handling. It rechecks the live Agent owner before Core dispatch and verifies the
stored journal tip against the terminal acknowledgement before returning.

| Outcome | Journal and native response |
|---|---|
| Verified `clean` inspection | Recorded execution; `{}`; exit 0 |
| Verified `suspicious` or `sensitive` inspection | Recorded execution; observation `systemMessage`; exit 0 |
| Failed receipt | Recorded failure and preserve decision; unavailable `systemMessage`; exit 1 |
| Error after claim | Existing reservation remains complete or interrupted; unavailable response; exit 1 |
| Settings, identity or Provider admission error before claim | No execution claim is required; unavailable response; exit 1 |

The response neither replaces the tool result nor requests permission to run it.
A content verdict does not become dispatch approval or prevention of an action
that already happened. Keep existing native security plugins and final execution
guards in place. Journal records contain metadata, digests, receipts and
decisions; the original payload and mapped output remain available in the Rust
result to the embedding application, which controls their retention.

## Hook identity and supported lifecycle

The CLI accepts `qoder` or `codex` and an absolute settings path. Settings must be
a private regular file owned by the caller, with no symlink at the final path.
The launcher supplies authenticated runtime/scope facts and the live Agent PID
plus Linux start ticks. The hook requires that Agent incarnation in its ancestry
and checks the runtime process reference. Native session/tool IDs must match any
preconfigured values. The event key derives from the scoped native occurrence;
a repeated event remains reserved across restarts.

Codex supplies the observed turn ID. Qoder requires a launcher-supplied
`qoder_single_turn_id` for an explicitly bounded single turn; reusing it across a
multi-turn session is unsupported. Automatic multi-turn supervision, hook
installation, PTY/session management, pre-tool enforcement and result adoption
are outside this entrypoint. Tests of the other Adapter profiles do not extend
the executable hook's supported hosts.

The CLI translates `SIGTERM` and `SIGINT` into one cancellation state shared by
Core and the Host, allowing native cleanup before it reports an unavailable
inspection. The Rust library leaves signal registration to its embedding owner.
`SIGKILL` cannot execute this cleanup path and provides no descendant cleanup
guarantee.
