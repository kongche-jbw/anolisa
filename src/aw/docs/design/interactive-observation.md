# Interactive observation ownership

[中文版](interactive-observation_zh.md)

Status: experimental development baseline; per-event runtime acceptance is incomplete.
This slice targets Linux, Bash, Qoder 1.1.47 and the Herdr v0.9.0 metadata protocol.
Native-peer tests do not certify these products' complete runtime behavior.

The optional `aw` feature composes the existing AW library inside cosh-shell.
The shell-scoped shim directory is prepended after Bash startup files. A `qoder`
shim verifies trusted configuration, executable bytes, native version, cwd,
foreground TTY and ancestry, then execs the real Agent without a print-mode flag.
The inner Bash owns its child and job control; cosh owns the existing Bash PTY.
No AW supervisor or Python subreaper is added. Provider/handler calls retain the
existing bounded Host's process-group owner. Agent executables have a separate
streamed 512 MiB pin ceiling because the pinned Qoder binary exceeds the Host's
64 MiB selected-file limit; the Host limit is unchanged.

Core remains a library and Agent loops remain native. In legacy configuration
format 1, pure observations do not become Core execution plans or claim
security/adoption receipts. The experimental
`tool.result_observed` envelope contains runtime identity, source epoch, native
session/tool identity, attachment generation and configuration revision. It
contains no tool input/output. The handler only acknowledges observation; no
replacement, permission, cancellation or final-dispatch effect is admitted.

Native SessionStart/SessionEnd bind attachments. PreToolUse reserves an immutable
tool occurrence; post-tool delivery claims it once before invoking the handler.
An interrupted claim is not retried. State locks are held only around transitions,
not during handler execution. A completion after reset stays with the old
attachment. Returning to a retired session inside one process, or resetting
without a fresh native session ID, is unsupported and reports a coverage gap.
Runtime incarnation, attachment and config revision have separate meanings.

The query reads private local evidence without executing handlers. Herdr validates
that the pane's shell PID is this cosh process and publishes expiring snapshots.
Its worker uses bounded socket operations and at most 24 hours of refreshes; drop
cancels and joins it before removing the shell-scoped directory. Closing the
viewer never stops the Agent. A snapshot can lag by its refresh interval and TTL;
it includes attachment identity and never authorizes execution. Exit observation
does not claim successful task completion or descendant reclamation.

Native settings are merged through `--settings`, never rewritten. Existing hooks
can change results or suppress callbacks, so coverage gaps and missing native
callbacks remain visible. Required safety is refused at admission. These private
same-user files/pins detect accidental drift and misbinding; they are not an OS
isolation boundary. Abrupt SIGKILL cannot run scope destructors. Normal shell
exit removes AW scratch evidence; native history retention is owned by Qoder.

Validation: the new AW interactive target covers trust, required-mode refusal,
stale/duplicate callbacks, reset during dispatch, bounded handler cleanup and
Herdr projection. The cosh shell_host AW target uses a synthetic interactive peer
on real PTYs for reset, resize, exit, two-pane isolation and cleanup. The bounded real TUI acceptance below covers native hook coexistence; the pinned
Herdr UI still requires separate acceptance.

## Public lifecycle notifications (configuration format 2)

Format 2 routes public lifecycle names through `aw-core::Core::notify` and the
existing bounded process Host. It preserves the entire native payload on stdin,
including failed tool results. Private journals store identity, payload digests
and command outcomes, not the payload or command stdout. Each route has one to
four ordered optional commands; each command acknowledges the fact with exactly
`{"format":1,"observed":true}`. Control-shaped output is a failed acknowledgement,
never a native decision. This is a notification slice; candidate transformation,
guards and adoption remain separate work.

The whole route's pins are checked before its first command. Core claims the
producer occurrence in FileJournal, with RAII writer release and persistent
reservations. The chain has a two-second monotonic execution budget; each command
has at most one second. Cancellation/reset fences the remaining commands and
reclaims the active process group. A failed/interrupted occurrence is not retried.
Repeated input or stop callbacks without a native occurrence ID receive distinct
arrival IDs: identical content is not proof of replay. Tool success/failure share
one result occurrence. Queries and Herdr refreshes never execute commands.

A session-end callback may precede session-start after native login admission
failure. It establishes a terminal observation only. Unknown AW Turn stays null,
with an explicit reason; native `agent_id` is retained for child lifecycle hooks,
not repurposed as runtime identity. Child tool/Stop callbacks still need a child
binding and are rejected by this main-runtime profile.

First-workspace trust in the real 1.1.47 TUI can omit SessionStart while later
callbacks still arrive. Format 2 may bind an otherwise unbound session from its
first authenticated main UserPromptSubmit. It records observation_gap=true and
session_start_observed=false, without synthesizing session.start. Tool-only
arrival, child callbacks, a different active session and input after SessionEnd
cannot acquire that binding. A subsequent real SessionStart uses normal reset
rules and records the observed start; Turn remains unknown.

## Qoder 16-event adaptation matrix

Reference CLI: **1.1.47**, executable SHA-256
`7dfd7ff973ec64f3d3f23723b406e4c40387c9eb5accc0364ede29540ec77e45`.
Pin the actual `qodercli` executable, not a `qoder` dispatcher script.
The [official CLI hook reference](https://docs.qoder.com/cli/hooks) and symbols in
this executable establish candidate sources, not complete runtime certification.
N below means implemented notification routing with synthetic payload/process
tests. It does not mean the native product has passed that event's acceptance.
All N routes return an empty native response (or a diagnostic systemMessage),
write no native fields and grant no rejection/final authority. Other native
hooks may still change content or suppress delivery.

| Public event | Source / current state | Evidence or next action |
| --- | --- | --- |
| session.start | SessionStart / N | Real compact/clear/resume observed; initial trust omits startup |
| input.submit | UserPromptSubmit / N + explicit response | Context/rejection and native print cases verified below; queue/supplement pending; Turn unknown |
| tool.before | PreToolUse / N | Real Bash arguments observed; native guard evidence is separate below |
| tool.after | PostToolUse / PostToolUseFailure / N + explicit response | Completed main-agent Bash text projection has synthetic coverage; real interactive adoption pending |
| permission.request | PermissionRequest / N | Real approve-once/reject observed; AW emits no approval decision |
| compact.before | PreCompact / N | Real manual compaction observed; automatic compaction pending |
| compact.after | PostCompact / N | Real manual completion observed; failure coverage pending |
| subagent.start | SubagentStart / N | Real tool-free child start observed with parent session/runtime correlation |
| subagent.stop | SubagentStop / N | Real matching child stop observed; repeated stop checks remain pending |
| turn.stop | Stop / N + explicit response | Fixed print/TUI continuation, repeat flags and bounded Hook coexistence verified; no task-success claim |
| session.end | SessionEnd / N | Real login failure, clear and normal TUI exit observed |
| model.before_request | No connected sender / gap | CLI catalog/sender integration remains unverified; SDK selector lacks full request |
| runtime.observed | cosh owner / N; synthetic PTY passed | Pre-exec pidfd registration observed in real TUI; no readiness claim |
| runtime.exited | Independent pidfd / N; normal, immediate and killed exits tested | Real TUI exit observed; independent of SessionEnd; no reaping authority or exit code |
| security.violation (provisional name) | Experimental Bash tool_guard; bounded native cases verified, final unsupported | Real TUI transform/scan/approval and deny/error cases passed; required/final remains a gap |
| coverage.changed | Owner samples / N; reset/gap/exit tested | Real missing-start, compact, reset and exit changes observed; OS/engine sources pending |

Unconnected sources are rejected in configuration admission. They remain gaps,
not completed support. All sixteen events and each effect need separate evidence.
The credential-free native probe ran print mode with isolated HOME/config and
blocked proxy settings: authentication failed, SessionEnd fired, and other hooks
were not observed. It proves only that termination source, not TUI acceptance,
model sending or the complete AW native path. Synthetic PTY tests cover the actual
cosh format-2 launcher, twelve hook registrations, Core journal and cleanup.

## Real natural TUI acceptance

A bounded 1.1.47 run used the real cosh Bash foreground PTY, isolated HOME/config
and temporary copies of existing login files. Native default approval remained
active. A user-level peer Hook restricted execution to three harmless commands
and requested approval; AW's format-2 notifications coexisted through --settings.
Approve-once created the expected marker; reject created no marker or post-tool
callback; an approved exit-7 command emitted PostToolUseFailure as tool.after.
Manual /compact produced both callbacks and SessionStart(compact) in epoch 1;
/clear produced end/start in epoch 2. Native /exit returned 0; owner independently
emitted runtime.exited and exited coverage. Historical observation_gap stayed set.
The controller reached its 420-second deadline after native exit and reclaimed
its owned processes. Temporary credentials, native history, auto-update files and
raw terminal logs were deleted; original login and pinned executable hashes were
unchanged. These are nine native public event kinds plus three owner kinds, not
complete acceptance of their effects. This fixture did not configure tool_guard;
natural-TUI safety-chain/approval coexistence is covered in the follow-up below.
Queued input, automatic/failed compaction and repeated child-stop checks remain
separate acceptance work.

A subsequent real TUI probe created one tool-free child through the native Agent
tool after normal approval. SubagentStart and SubagentStop carried the same child
identity and parent session/runtime. After native exit, explicit --resume restored
the same session in a new runtime with attachment epoch 1. Both runtimes emitted
independent owner exit events. Child tool calls, background children and repeated
stop checks are not certified by this probe.

## Model sending integration gap

The [CLI custom-model guide](https://docs.qoder.com/cli/custom-models) documents a
/model Custom wizard. The fixed TUI model menu was observed, and the account-scoped Custom
wizard was inspected through provider, model, options and API Key entry. No
credentials were entered or model saved. No endpoint field was visible before
credential entry; later fields and actual endpoint routing remain unverified. Do not infer CLI fields
from the IDE configuration or hand-write an undocumented proxy setting.
The [TypeScript SDK reference](https://docs.qoder.com/cli/sdk/references-typescript)
defines resolveModel context as purpose, sessionId and availableModels. Its
selection result does not expose the assembled request body. This cannot directly
satisfy AW's complete-request pre-send contract. Next verify a supported native
provider endpoint/integration and actual body visibility; a broker additionally
needs separately proven routing and cancellation. This gap does not block the
other events, and observation alone does not establish final/protected authority.

## Final tool safety check (experimental native slice)

The review contract allows bounded parallel, independent read-only actions in
`tool.before`. Candidate transformations run in a defined order, never using
last-completion-wins updates. Await every asynchronous action affecting this
execution, then freeze the tool name, final arguments, cwd and other execution
context plus policy revision. Security invokes sec-core as the last read-only check.

`security.violation` remains a provisional name, with its target semantics revised
to an active check point; review `security.check` or `security.before_action` before
freezing the interface. Required denial, failure, timeout or cancellation must block
release. Candidate changes or new execution attempts require a new check. OS
violations and check outcomes remain distinct evidence, not substitutes for this
invocation. Format 2 notifications still reject this safety route. A separate
tool_guard config connects ordered transformations and final scan-code without
notify-only authority. Pass emits updatedInput, never native allow, preserving
normal approval. Check errors, repeated tool occurrences and cancellation emit
deny; required/final remains unsupported.

Transforms and optional notifications remain capped at one second each. The
scanner may explicitly configure up to 2000 ms for version verification plus
scan-code; the caller supplies only the time left in the existing two-second
pre-tool deadline. Smaller configured limits remain effective. Real Python CLI
startup exceeded the old one-second scanner cap even for a valid pass; this
admission change permits a larger share, without retrying or resetting the chain.

AW guarantees ordering inside its own chain. If other Qoder hooks or approval can
change arguments afterward, final guarantees require a check at consumption or
proof that the same approved candidate is consumed. Refuse required admission
until native denial and failure behavior are verified. Tests cover ordered
transformations, introduced violations, checker failures, timeout cleanup,
cancellation/late returns, parallel-call isolation and read-only queries. Parallel
read-only actions remain unimplemented. Qoder blocking acceptance requires
evidence that the prohibited action did not occur.
Notifications and the guard share the pre-tool two-second monotonic deadline;
Core does not restart the budget.


## Native consumption and failure boundary

Bounded Qoder 1.1.47 print-mode probes use an isolated HOME/config, a temporary
copy of existing login credentials and synthetic Bash commands. In this fixture,
bypass_permissions avoids an unrelated approval prompt; only the exact synthetic
input is admitted. This verifies native hook consumption, not the natural TUI
launcher or normal approval behavior.

| Native case | Observed result |
| --- | --- |
| updatedInput | Only the replacement marker exists; PostToolUse carries the replacement |
| Explicit deny | No command marker or PostToolUse |
| Two modifying hooks | Both see the original input; the later responding hook's marker exists |
| Hook exit 1 or SIGKILL | The original command executes |
| Hook exit 2 | No command marker or PostToolUse |

The AW launcher selects the dedicated --aw-guard entry for PreToolUse whenever
tool_guard is configured. Malformed input, missing binding, missing guard config
or the wrong callback event exits 2; optional --aw-hook errors remain diagnostic
and nonblocking. The guard entry cannot translate its own SIGKILL into exit 2.

These observed counterexamples prevent a final/required guarantee. A consumption
boundary must bind execution to the checked candidate and block when the checker
cannot return. Sorting AW hooks last cannot establish either property. Keep
required_safety rejected and query configured_not_certified. Runtime and coverage
producers can proceed independently; model.before_request remains a separate gap.


The full check path additionally uses real agent-sec-cli 0.12.0: a transformed
safe command passes and executes in Qoder; a harmless echo containing a
policy-matching string returns warn, AW denies it, and no execution marker
appears. Missing binding makes the real helper exit 2 and Qoder does not execute.
Core journals record check_passed/policy_denied and digests; native_execution
stays unconfirmed because probe evidence is not yet a product consumption proof.
The probe constructs a trusted binding before exec of the native CLI; it does not
replace natural TUI admission testing.


## Natural TUI safety-chain acceptance

With real sec-core 0.12.0 and scanner timeout_ms=2000, the normal TUI passed four
bounded cases: a safe transformed candidate required native approval and only
that candidate executed; native rejection prevented execution after a passed
check; a harmless transformed echo matching a policy returned policy_denied;
and an intentionally failed transform returned check_failed_or_cancelled. The
last two cases reached neither native approval nor tool completion. Native user
Hooks and AW notifications remained enabled. The prior 1000 ms run denied a safe
candidate when version verification plus scanning exceeded the configured limit.
All original/rejected/denied/error execution markers stayed absent. These fixture
observations do not upgrade the product's native_execution=unconfirmed or close
the known later-Hook/SIGKILL final-safety gaps. Temporary credentials, transcripts,
scanner environment, terminal logs and owned processes were removed.

## Independent owner producers

The three format-2 owner routes reuse Core notify-only and the bounded Host,
claiming runtime_owner occurrences in a separate owner-journal. cosh starts the
worker during configuration. After admission, the shim requests registration;
the worker checks the owner's configuration and direct shell-child PID/start
ticks, opens a pidfd and atomically acknowledges. The shim waits at most five
seconds before exec. Registered exec failures remain observable until root exit.
There is no second wait/reap owner.

runtime.observed means pre-exec registration, not successful startup.
runtime.exited uses only pidfd terminal evidence, leaving exit code, task success
and descendant reclamation unknown. coverage.changed compares sampled callback
attachment, session epoch, gaps, delivery failures and exit. SessionEnd never
substitutes for process exit, and viewer state does not become protection
coverage. Worker/runtime status has independent read-only snapshots; query
never dispatches events.

The observation window is 24 hours, sampling every 100 ms between bounded
deliveries, with at most 128 registrations per shell and 1024 coverage changes
per runtime. Failed deliveries are never retried, and coverage-handler failure
does not recursively dispatch. Drop cancels and joins, closing pidfds. Old
workers cannot restart for replay; delivery after cosh crashes is not promised.
Synthetic PTYs cover SIGKILL without SessionEnd, immediate exit, reset/gaps,
pane isolation and handler cancellation/reaping. Real Qoder natural TUI exercised
registration, missing-start, compact, reset and exit snapshots. AgentSight and
OS-coverage acceptance remain pending.

## Input submission response boundary

Format 2 optionally authorizes one `input_response` command, separate from
notification routes. Contracts owns the strict `input.submit.respond` v1 response;
Core claims and bounds the occurrence; Host checks pins, runs and reaps the process;
the Qoder adapter maps only context or rejection. The launcher selects a dedicated
`--aw-input` helper so malformed input or missing binding maps to native exit 2.
The original prompt is immutable, permission is unchanged, and Turn stays unknown.
Notification and response share the callback deadline and one Core instance.

Input to the command wraps the authenticated event in
`{"format":1,"scope":"input.submit.respond","event":{...}}`. Outputs allow only
`continue` with optional `additional_context`, or `reject` with a required reason.
The command has a one-second limit within the two-second callback deadline.
Journal claims survive failure and configuration changes; they store metadata and
decision class, never raw context/reasons. Reset/session end fences delivery.
`native_adoption=unconfirmed` remains explicit even when a fixture verifies adoption.
See the [configuration contract](../../../../docs/user-guide/en/user-entrypoint/aw.md#experimental-input-submission-responses).

Fixed 1.1.47 print probes verified context adoption, policy denial, malformed
output, timeout, cooperative cancellation, missing binding, two context Hooks,
and a peer Hook denial. Killing the helper allowed native input processing;
this counterexample excludes final/protected certification. Queue/supplement
semantics and arbitrary plugin combinations are still open. A natural cosh TUI
probe observed the context marker in Stop, no further Stop for policy/malformed/
timeout submissions, and normal native/runtime/shell termination. This does not
convert the metadata journal into an adoption receipt.

## Request endpoint investigation

The fixed artifact contains a local `modelConfigs.customModels` description with
required provider/apiKey/model and optional baseURL/key/format. This is a stronger
routing lead than the Custom wizard, but does not prove a direct sender boundary.
An isolated probe declared an OpenAI-style model with a synthetic key and loopback
baseURL. Without login it failed admission; with an isolated copy of existing login
state it reported `Failed to generate custom pool`, and the loopback server received
zero requests. The declared provider/model tuple has not been validated against
the account catalog. This failure cannot establish that all endpoint configurations
are unsupported. The next step is a supported catalog tuple or documented provider
integration, followed by full-body visibility, rewriting and denied-send evidence.
No production broker or model.before_request source was added.


## Main-agent stop response boundary

stop_response is separate turn.stop.respond/v1 authority, unavailable to notification
ACKs and unrelated to SubagentStop. Core claims, Host pins/reaping, single initialization,
shared deadlines and attachment fencing follow the input-response layering. Stop errors
cannot map to exit 2 because Qoder uses it to request more model work. Valid responses
are only allow_stop or continue+reason; failures/cancellation/repeated checks map to
continue=false and diagnostic fields. Native stop_hook_active=true skips the response
command; Journal records skipped, never a passed check. Ordinary decisions/failures
remain metadata-only with native_adoption unconfirmed.

This optional native response is not a final safety check. Its recursion bound depends
on Qoder's native flag; peer Hooks can affect final behavior and helper death cannot
ensure adoption. Eleven fixed Qoder 1.1.47 print cases verified allow_stop, consumed
continuation, repeat suppression, malformed/timeout/cancellation/missing-binding
failures, and two declaration orders for peer continuation against AW allow/error.
The AW error response stopped the peer continuation in both tested orders. Killing
the helper left only a started journal entry, while native output still finished.
A natural cosh TUI with user-level Hooks consumed continuations on two inputs,
reset stop_hook_active on new input, then stopped malformed/timeout checks without
further answers. The TUI displayed diagnostics; print omitted them and exited 0
for failed checks. Already emitted answers remain visible. These native observations
do not turn metadata journals into consumption receipts or certify arbitrary plugins.
The TUI /exit produced an independent runtime.exited observation before shell cleanup.
See the
[user guide](../../../../docs/user-guide/en/user-entrypoint/aw.md#experimental-main-agent-stop-responses).

Endpoint follow-up: current [CLI documentation](https://docs.qoder.com/cli/custom-models)
requires the account Custom wizard instead of hand-editing settings.json. The official
[announcement](https://forum.qoder.com/t/qoder-cli-now-supports-custom-models/12444) dates
generic base URL/model ID/API key access from 1.1.50; it cannot establish 1.1.47 support.
Next verify compatibility and the supported entry on an isolated newer artifact, or
obtain a supported fixed-version provider interface. No host upgrade or proxy was added.

## Interactive tool result response boundary

`tool_response` grants separate `tool.after.respond/v1` authority to one pinned
command after notification work. It accepts only preserve or source-digest-bound
replacement of the existing completed Bash stdout slot. The caller explicitly
accepts unrecoverable output; journals retain metadata/digests without body text.
The existing native extractor and replacement formatter are reused.

`ProjectionSettings` cannot be reused as an interactive binding: it requires an
owner-authenticated single Turn and exact history/retention configuration. This
slice keeps unknown Turn identity and does not automatically inherit sec-core,
Tokenless, retained candidates or history adoption from `qoder-project`.

Core claims before dispatch, shares the original callback deadline, validates
output and acknowledges completion before release. The owner reserves success
or failure once per session epoch/tool call, even without notification routes;
unsupported terminal results cannot later be retried as successful replacements.
Stale/cancelled/expired candidates are withheld. Optional failure preserves the
original result; it is not a mandatory sanitization barrier. Real Qoder adoption
and peer Hook precedence remain pending, with final/protected still unsupported.
