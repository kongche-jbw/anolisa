# AW tool-result inspection and projection

[中文版](../../zh/user-entrypoint/aw.md)

AW lets an explicitly configured Qoder or Codex hook inspect tool output with
SecCore and retain a verifiable execution record. The original tool result is
preserved by the `qoder`/`codex` commands. An additional explicit Qoder projection
mode can return candidates and independently record local-history adoption.
Inspection is observational: a warning is not execution approval or
proof that the Agent displayed or acted on it.

This is an experimental Linux developer entrypoint. AW is not yet registered
with `anolisa install` or packaged as an RPM. Build the existing source from the
repository root:

```bash
cargo build --manifest-path src/aw/Cargo.toml --locked -p aw-hook-cli
src/aw/target/debug/aw-hook-cli --help
```

## Invocation

```text
aw-hook-cli <qoder|codex|qoder-project> /absolute/SETTINGS.json
```

The Agent supplies one native `PostToolUse` JSON object on stdin and closes the
pipe. Input and settings each have a 1 MiB encoded limit; stdin must reach EOF
within five seconds. Duplicate JSON keys are rejected, including nested keys.
Native Unicode keys and finite numeric metadata are preserved.

Only the text slots in the [native adapter profiles](../../../../src/aw/docs/design/native-adapters.md)
are accepted. Both hosts require native `session_id` and `tool_use_id`. Codex
also requires native `turn_id`. Qoder requires a launcher-owned
`qoder_single_turn_id`: the launcher must end that invocation after one turn.
Reusing its settings across interactive turns is unsupported.

The CLI does not install hooks, create Agent sessions or replace existing
SecCore security plugins. An embedding launcher must register the command using
the host's supported hook configuration, bind the live Agent identity and keep
the configuration private. A raw test payload does not validate an Agent's hook
installation or response presentation.

## Settings reference

The settings file must be an absolute, regular, non-symlinked file owned by the
hook's user, without group or other permissions (normally `0600`). Unknown
fields are rejected. Keep its parent directory and the Journal outside
Agent-writable locations; file permissions alone do not isolate a same-user
Agent.

| Field | Required meaning |
| --- | --- |
| `runtime` | A launcher-authenticated `runtime-binding/v1` snapshot; `observation_source` is `owned_child`, `process_ref` is `pid:<agent_pid>@<agent_start_ticks>` |
| `scope` | Trusted AW scope matching that runtime; native session/tool/turn IDs may fill omitted fields but cannot overwrite conflicting values |
| `agent_pid` | Live Agent PID that must be an ancestor of the hook |
| `agent_start_ticks` | Linux `/proc/<pid>/stat` field 22, checked against the live incarnation |
| `qoder_single_turn_id` | Required nonempty launcher-bound turn for Qoder; optional for Codex |
| `journal` | Absolute directory for the private Core `FileJournal` |
| `provider` | Explicit native launch configuration described below |
| `include_low_confidence` | Boolean; enables the native low-confidence option only when selected |

`runtime` and `scope` must satisfy the [registered contracts](../../../../src/aw/schemas/).
The [contract fixture](../../../../src/aw/tests/fixtures/contracts.json) illustrates
their shape, but its synthetic identity must not be copied as a real session.
The library's `process_identity(pid)` exposes the Linux parent PID and start ticks
for a trusted launcher. Ancestry checks detect misbinding; they do not authenticate
arbitrary runtime declarations against a hostile process.

| `provider` field | Required meaning |
| --- | --- |
| `provider_id`, `provider_version` | Operator-selected registry identity and Provider release |
| `program`, `program_sha256` | Absolute executable and independently supplied expected lowercase SHA-256 |
| `cwd` | Absolute native working directory; native configuration resolution is preserved |
| `args` | Explicit prefix arguments, before `--version` or the `scan-pii` operation |
| `environment` | Complete key/value environment; nothing is inherited implicitly |
| `pins` | Up to 32 selected files, each with absolute `path` and `state`: `{"sha256":"<expected digest>"}` or `"absent"` |
| `limits.timeout_ms` | Native call budget, 1–300000 ms; version probing has its own call budget |
| `limits.input_bytes` | Exact native stdin ceiling, 1–4194304 bytes |
| `limits.output_bytes` | Native stdout ceiling and this hook's AW output budget, 1–4194304 bytes |
| `limits.stderr_bytes` | Discarded native stderr ceiling, 1–4194304 bytes |

An actual version probe must return `agent-sec-cli 0.12.0`. Supply all environment
needed by that native installation, including the real `HOME` when its standard
user rules depend on it. AW does not choose an alternate user home or silently
omit a broken rule configuration. Pin the executable and the dependencies and
rule files relevant to the declared installation; explicit `"absent"` detects a
new file appearing. Each pinned file is a regular file no larger than 64 MiB.
The selected pins are rechecked before and after each call. They are not a
complete interpreter dependency attestation or protection against changes during
execution. Re-admit deliberately updated versions and configuration explicitly.

## Inspection results, failure and shutdown

| Outcome | stdout / exit status |
| --- | --- |
| Verified clean inspection | `{}` / `0` |
| Verified suspicious or sensitive inspection | Generic `systemMessage` / `0` |
| Provider, identity, input, recording or inspection failure | Generic unavailable `systemMessage`, bounded diagnostic on stderr / `1` |

Responses contain no raw tool text, native evidence snippets or provider stderr.
They do not return a replacement payload, allow/deny decision or adoption claim.
The native host determines how it presents a message or a nonzero hook exit.

Core persists the pinned plan and invocation before scan dispatch, then stores
the receipt and terminal facts. The Journal contains digests and receipts, not
raw inspection input or output. Failed/interrupted reservations remain present;
the same event cannot be replayed automatically. Failures before Core claims an
event, including a failed version probe, have no execution receipt.

SIGTERM/SIGINT request cancellation in the CLI. The Host stops further launches,
interrupts pipe exchange and reclaims its owned process group, allowing up to
one additional second for cleanup verification. Configure the external hook
timeout to cover input reading, the version probe, inspection, cleanup and local
bookkeeping. SIGKILL, a descendant leaving the group and kernel-blocked operations
are outside this guarantee. This process handling is not an OS security sandbox.

To disable this path, remove only its hook registration from the owning launcher;
retain the existing native security plugin. Retain the Journal for as long as
duplicate-event protection and inspection history are needed. Removing it also
removes those guarantees for old events.

See [Host architecture and embedding](../../../../src/aw/docs/design/sec-core-host.md)
for cancellation, process ownership and validation details.

## Explicit integration smoke

The regular AW gate runs real local protocol peers and the smoke validator's
self-tests. It does not require an Agent login or native SecCore dependencies.
To exercise the repository's actual scanner, prepare its frozen Python 3.11.6
installation and pass that interpreter explicitly:

```bash
python3 src/aw/tests/native_smoke.py --host codex --mode payload --provider-python /absolute/venv/bin/python
python3 src/aw/tests/native_smoke.py --host codex --mode agent --provider-python /absolute/venv/bin/python
```

`payload` constructs a hook payload and verifies the real Host/scanner/Journal
path. `agent` runs the actual selected CLI; Codex uses two scripted localhost
model responses and ephemeral sessions, not a live-model evaluation. Qoder uses
its native model and an isolated configuration directory; missing login or an
untriggered hook returns nonzero. Neither mode changes user hook settings.

Codex defaults to `--codex-sandbox read-only`. An explicit
`--codex-sandbox danger-full-access` selects an unsandboxed fixture run when the
host cannot start its sandbox; there is no automatic fallback. The only scripted
tool command prints a synthetic credential. Such a run proves the observation
chain and original-result retention, not sandbox enforcement or approval.
The script prints process/port ownership and results, verifies native audit
against the exact hook text and Journal, and removes its temporary directory.
It preserves the real `HOME` but redirects custom-rule lookup to a temporary
absent file and audit/telemetry to the fixture directory. This validates the real
scanner chain, not the operator's current custom rule set.

## Qoder projection and history observation

Use `qoder-project` only with a launcher-bound single Qoder turn, synchronous
`PostToolUse` hook registration and a successful structured `Bash` result. Bare
string responses remain supported for inspection, but are not admitted here.
The declared history file must already exist and belong to the hook user; its
parent path is operator-owned. Native `cwd` must equal `hook.provider.cwd` and
`transcript_path`, when present, must exactly equal `history_path`.

The projection settings are a separate object; every field below is required.
The existing inspection settings remain unchanged inside `hook`.

| Field | Meaning |
| --- | --- |
| `hook` | Complete inspection settings from the tables above |
| `tokenless` | Same native launch-config shape as `provider`, with a distinct ID, version `0.8.1`, and a Tokenless executable whose probe returns `tokenless 0.8.1` |
| `record_directory` | Absolute private directory (`0700`) under an existing trusted parent; source/candidate records are `0600` |
| `history_path` | Exact absolute Qoder JSONL file, regular, caller-owned, no final symlink, at most 16 MiB |
| `history_profile` | Exactly `qoder-cli-1.1.47/jsonl-v1`; not automatic version detection |
| `retention` | Exactly `source_and_candidate`; acknowledges private plaintext retention |
| `max_observation_delay_ms` | 1–300000 ms from projection receipt completion until actual observation |
| `accepted_reversibility` | Exactly `["unrecoverable"]`; native lossless classification does not provide AW recovery |
| `allow_text_reencoding` | Explicit boolean permitting Tokenless text reencoding |

The `tokenless.environment` map must explicitly set
`TOKENLESS_STATS_ENABLED=0`, `TOKENLESS_SLS_ENABLED=0` and
`TOKENLESS_COMPRESSION_ENABLED=1`; no recovery stash is enabled. All other
native admission, limits and pins follow the [Tokenless Host profile](../../../../src/aw/docs/design/tokenless-host.md).
The outer settings file has the same private-file requirements and 1 MiB limit.
Each retained context/observation is also limited to a 4 MiB canonical document;
a context that cannot be durably retained is not delivered.
Allow the external hook timeout to cover two version probes, both serial calls,
input reading, cleanup and durable local writes.

Required SecCore inspection precedes optional Tokenless projection. Inspection
failure prevents the projection call; sensitive findings remain observations,
not denials. Provider failure or no useful candidate returns no replacement.
A verified candidate is returned as
`{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":"candidate"}}`.
The context record is durable before stdout; a separate marker follows successful
unbuffered write completion (no pending userspace buffer). Stdout has a five-second
nonblocking deadline and observes cancellation, including a reader that stalls.
Failure to persist, deliver or mark returns nonzero. A marker
records local transport completion only; native parsing, exit handling and later
hooks can still affect the result. Qoder's documented replacement slot and hook
composition rules are described in its [official hook reference](https://docs.qoder.com/cli/hooks).

Records use `<event_key>.json` under `record_directory`, with sibling
`<event_key>.returned.json` and `<event_key>.observation.json` when applicable.
The library's `ProjectionResult.record_path` identifies the context record.
Both CLI entrypoints are built by the package build above:

```text
aw-hook-cli qoder-project /absolute/PROJECTION_SETTINGS.json
aw-adoption-cli observe /absolute/records/CONTEXT.json
aw-adoption-cli query /absolute/records/CONTEXT.json
```

Run `observe` after Qoder appends the matching result and before the configured
window expires. It reads only the declared file, verifies its captured inode and
prefix, and matches one ordered tool call/result by session, tool ID, cwd and
input. Result text must be appended after capture. Complete JSONL lines are
required, each no larger than the native 1 MiB input limit. Missing history or
results and late observations remain unverified without a savings number;
malformed, duplicate, changed-prefix or wrongly bound evidence fails explicitly.
There is no automatic polling. A successful observation is immutable; a repeated
or interrupted claim is an error, not a request to replay providers.

`query` reads the retained context, original execution Journal, optional delivery
marker and independently acknowledged observation. It creates no files, runs no
providers and does not reopen live history. Its output contains metadata only:

| Field | Interpretation |
| --- | --- |
| `execution_decision` | Complete Core outcome, including preserved or cancelled plans |
| `prepared` | A candidate envelope was produced; this does not assert delivery |
| `returned` | A valid local stdout completion marker exists |
| `observation_status` | `unverified`, `adopted`, `preserved` or `overridden` |
| `proof_boundary`, `observation_kind` | `local_history` / `recorded_snapshot` only when observation was committed |
| `saved_bytes` | Exact attributable bytes at that snapshot; zero for verified preservation/override, null without proof |
| `observation` | Validated adoption contract and evidence references, or null |

A matching candidate is adopted only for a complete proceeding plan. Source text
means preserved; different text means a later transformation, with zero attributed
savings. No candidate alone is not proof of source preservation. A missing
stdout marker does not disprove independently recorded history; the two facts
remain separate. Byte savings are not model Token counts, request delivery or
billed savings. Later history changes are not re-observed by `query`.

Keep all record and Journal parents outside Agent-writable locations. Observation
rows and call inputs can contain sensitive plaintext even though query output
and Journal metadata do not. This is not isolation against a same-user attacker
who can rewrite both records and their evidence. Retain context, observation,
markers and their Journal together for the desired audit period; there is no
automatic retention or garbage collection. Disabling the owning hook stops new
projections. Deleting context or observation files removes query evidence;
deleting Journal files also removes their durable duplicate-event reservations. Validate the real Qoder version and hook installation before enabling
this experimental profile: its private JSONL format is not an official stable
API, and the regular gate uses synthetic histories rather than a logged-in Agent.

## Dependency preflight

Check the selected native installations before preparing session-specific hook
settings. The source build above also produces `aw-preflight-cli`:

```bash
src/aw/target/debug/aw-preflight-cli /absolute/PREFLIGHT.json
```

The same private-file rules and 1 MiB limit apply. Choose exactly one shape:

| `mode` | Required fields | Probed native versions |
| --- | --- | --- |
| `inspect` | `security` | SecCore CLI 0.12.0 |
| `project` | `security`, `tokenless` | SecCore CLI 0.12.0, then Tokenless CLI 0.8.1 |

Both fields contain the existing native `provider` configuration described above.
The Tokenless configuration additionally requires the three native controls from
the projection reference. Provider IDs must differ. Unknown fields are errors;
`inspect` rejects a supplied `tokenless` field rather than silently ignoring it.
Use installation-owner supplied absolute executable paths, prefix arguments and
independent expected digests. Preserve Python virtual-environment executable
paths; resolving their symlinks can change package metadata and imports. No PATH
search, automatic installation or alternate-version fallback occurs.

Preflight checks native versions, then explicitly calls SecCore `scan-pii` on the
fixed public text `AW startup protocol probe.` through the existing bounded Host.
The native rules, middleware and audit remain enabled: **this probe can write a
native audit event**. Any complete, semantically valid verdict is accepted,
including sensitive; missing commands, incomplete coverage and malformed results
fail. Ordinary hook construction does not add another synthetic scan per event.

Exit 0 emits `status: dependencies_ready`, the selected mode and component/version
pairs, `agent_started: false` and `runtime_ready: false`. SecCore metadata also
contains `protocol_profile: agent-sec.scan-pii/v1`, `protocol_probe: passed`,
`probe_scope: synthetic_content`, and `native_audit_possible: true`. Exit 1 reports
a static component diagnostic without echoing native content/environment.

SecCore product V1/V2, native protocol revisions, AW contract revisions and product
versions are independent. A version string alone is insufficient: the Rust V2
CLI can share a release number without exposing this PII command. This bounded
probe checks the selected installation's callable surface; it is not complete
scanner compatibility certification, a daemon health check or enforcement.
Approved artifact/configuration pins and native conformance evidence remain
required. No Journal, Agent or adoption claim is created. Execution rechecks its
pins; later changes can invalidate this snapshot.

### Optional pinned Herdr bundle

Herdr is optional and is not needed for inspection preflight. To fetch its
unmodified Linux v0.9.0 binary and Apache-2.0 license, choose a destination whose
parent directory already exists:

```bash
python3 -B src/aw/integrations/herdr/fetch.py /absolute/herdr-v0.9.0
```

The bundled [pin](../../../../src/aw/integrations/herdr/upstream.json) selects
Linux aarch64/x86_64 artifacts and a commit-addressed license. Each download is
limited to 128 MiB; the CLI's overall deadline defaults to 120 seconds
(`--timeout`, greater than 0 and at most 600 seconds). Both files are verified
before Linux `renameat2(RENAME_NOREPLACE)` publishes the complete directory.
The command prints the binary path, without starting it or an Agent.

An existing complete, matching, executable bundle is reused offline without
changing contents or modes. A mismatch, incomplete bundle, symlink or destination
collision fails without replacing or repairing user files. Linux without the
required rename operation fails explicitly. Ordinary errors, SIGTERM, SIGINT
and timeout reclaim private staging; SIGKILL or power loss can leave staging.
An interruption or output error after publication can leave the complete valid
bundle; rerun the same command to verify/reuse it. Installation is not activation.
Remove only the explicitly chosen bundle when it is no longer needed; the fetch
does not register global state or hooks.

Readiness JSON uses the existing five-second, cancellation-aware stdout delivery;
a nonconsuming output pipe cannot keep preflight waiting indefinitely.

## One-prompt Qoder session

The explicit Linux launcher connects Qoder **1.1.47**, the **cosh-shell 0.15.0
helper**, and the existing AW binaries. It requires Python 3.11+ for orchestration;
the Provider executable remains independently selected. Run from this checkout:

```bash
python3 -B src/aw/integrations/qoder/session.py /absolute/LAUNCH.json
```

`LAUNCH.json` must be caller-owned, mode 0600, a regular file, and use unique JSON
keys. Supply the following fields; unknown fields are rejected:

| Field | Value |
| --- | --- |
| `format` | `1` |
| `workspace` | Existing canonical absolute path using ASCII letters, digits, `/`, `_`, `-`; this bounds the verified native history-path encoding |
| `config_directory` | Existing native Qoder configuration root; login and credentials stay there |
| `session_directory` | New absolute directory under an existing parent; an existing destination is never replaced |
| `binaries` | Exactly `qoder`, `cosh_shell`, `aw_hook`, `aw_preflight`, `aw_adoption`; each has absolute `path` and independently approved executable `sha256` |
| `preflight` | The inspect/project configuration above; `security.cwd` must equal `workspace` |
| `prompt` | One nonempty prompt, at most 32 KiB of UTF-8 |
| `timeout_seconds` | Integer 1–240 for the Agent phase; bounded startup probes and cleanup have separate limits |
| `permission_mode` | Explicit native `default`, `dont_ask`, or `bypass_permissions`; AW does not grant final dispatch authority |
| `model` | Optional native model name/ID |
| `projection` | Required only for project: `retention: source_and_candidate`, `accepted_reversibility: [unrecoverable]`, and explicit boolean `allow_text_reencoding` |

The launcher checks versions/pins, existing native settings/plugins and the
synthetic security protocol before starting cosh-shell. Existing hooks, enabled
plugins, `disableAllHooks`, invalid settings syntax and a nondefault
`QODER_CONFIG_DIR_NAME` are rejected. Disabled plugin entries are preserved.
It does not turn off existing security integrations, change settings sources,
copy credentials, or install global hooks. Additional settings live in the new
private session directory. This initial profile supports a clean configuration;
coexistence with active plugins requires a separately verified profile.

cosh-shell starts one bootstrap child. That child records its own PID/start ticks
and execs Qoder, keeping its identity. Qoder uses one `-p` invocation with Bash as
the selected built-in tool. This is not an interactive PTY, multi-turn/reset or
multi-pane launcher. It uses the existing `qoder`/`qoder-project` hook directly.
No history file is fabricated before a hook; projection requires Qoder's actual
history prefix. A completed Agent with no recorded projection has no adoption
proof.

After Agent exit, the launcher performs a bounded single observation per recorded
context and writes `result.json`. Each observation is the existing `observe`
result, including independent retained history evidence. Use `aw-adoption-cli
query /absolute/SESSION/records/EVENT_KEY.json` later to revalidate that snapshot.
`adopted` proves the captured local-history result, not model consumption or
billing savings. Missing evidence remains unverified; observations are not retried
indefinitely. At most 128 contexts are admitted for automatic observation.

The dedicated launcher owns its cosh-shell child tree, including adopted Provider
children in separate process groups. SIGINT/SIGTERM/timeout and early root exit
enter bounded cleanup; the group leader remains unreaped until its last group
signal. Cleanup allows one second for termination and three seconds for kill/reap,
then reports failure if exit cannot be established. Utility stdout/stderr each
have a 64 KiB live limit. No stop authority is reconstructed from saved PIDs.
SIGKILL, machine failure and kernel-stuck processes cannot promise orderly cleanup.

Before Agent launch, failed setup removes its own new session directory. Once
launch is attempted, settings, Journal, records and result/failure metadata remain
private for diagnosis and the explicitly selected retention policy. Native Qoder
history stays under its selected configuration root. Remove these specific session
artifacts only after their retention purpose ends; uninstalling this launcher does
not require changing user hooks. [Design and test boundaries](../../../../src/aw/docs/design/qoder-session.md)
explain ownership and supported evidence.

## Bounded Qoder conversations

Use the same admitted native installations for a sequence of up to eight prompts:

```bash
python3 -B src/aw/integrations/qoder/conversation.py /absolute/CONVERSATION.json
```

The input is an absolute, caller-owned 0600 JSON file, limited to 1 MiB. Its fields are:

| Field | Required value |
| --- | --- |
| `format` | Integer `1` |
| `launch` | The one-prompt launcher settings above, with `prompt` omitted; `session_directory` is a new conversation root |
| `turns` | An ordered array of one to eight objects, each with `prompt` and optional boolean `reset` |

For example, the `turns` array can be:

```json
[
  {"prompt": "Read the project summary."},
  {"prompt": "Explain the previous summary."},
  {"prompt": "Start a separate conversation.", "reset": true}
]
```

All prompts and configuration are validated before creating the root. The same
explicit permission mode, native configuration, pins, Providers and projection
opt-ins apply to every turn. Existing roots are rejected; repeated invocation
never overwrites evidence or resumes an interrupted runner.

The first prompt starts a new native UUID. Subsequent prompts use Qoder's
`--resume` with that exact UUID; `reset: true` starts another fresh UUID. No
arbitrary external session, most-recent-session selection or fork is accepted.
Before a resume, the owned session must have a nonempty regular native history
of complete JSON objects, at most 16 MiB. This checks availability and framing;
it does not attest resumed model context or replace AW's independent history
reader and identity checks.

Each turn launches a new Agent process through the existing cosh-shell helper,
with a distinct turn ID and increasing runtime generation. A single owner keeps
cancellation active across the sequence. The next turn starts only after the
previous Agent tree has been reclaimed and its observation operations completed.
Nonzero exits, preparation/observation errors, timeout or cancellation stop the
sequence; there are no retries. `timeout_seconds` remains a per-turn Agent budget,
plus the existing bounded startup, cleanup and observation operations per turn.

The new 0700 root retains `conversation.json` (including prompts), a metadata-only
`summary.json`, and private `turn-0001`, `turn-0002`, … directories using the
one-prompt layout. The summary's `completed` means the requested processes and
observation operations completed successfully; it does not mean every output was
projected or adopted. Read each turn's `result.json` and existing `query` evidence
for those facts. Stopped runs retain attempted-turn evidence and diagnostics;
reset does not delete earlier records or move their counts into a new session.
Root diagnostics remain even when first-turn preparation fails.

These are sequential print-mode invocations, not a persistent interactive Agent,
PTY, in-process `/clear`, pane manager or Herdr UI. Independent invocations own
separate roots and sessions. Cleanup never reconstructs authority from stored
PIDs; native histories follow Qoder's own retention, and AW records remain until
the operator removes the exact owned root after its audit period.
See the [conversation ownership design](../../../../src/aw/docs/design/qoder-session.md#bounded-conversation-continuation).

## Experimental natural Qoder entry

### Native component configuration

For real SecCore Bash checks and Tokenless tool-result compression, use the
checked-in `src/aw/providers/qoder-native.json` policy and the compiled
`aw-hook-cli configure` command. The [acceptance procedure](../../../developer-guide/en/aw/qoder-acceptance.md)
contains the complete build, setup and terminal walkthrough. Configuration is a
one-time admission of installed artifacts; ordinary startup needs no launcher or
observer script.

The command accepts required absolute paths `--qoder`, `--native-config`,
`--sec-core`, `--tokenless`, `--herdr`, `--workspace`, `--output`; optional
`--providers` selects the checked-in policy, and repeated `--sec-core-pin` adds
reviewed installed files. Omitting `--providers` uses the embedded default policy.
It verifies the supported versions and Herdr asset, creates a new 0600 profile,
and prints the following user configuration (with actual paths and digests):

```toml
[aw]
config = "/absolute/path/aw.json"
config_sha256 = "<reviewed lowercase SHA-256>"
herdr = "/absolute/path/herdr"
herdr_sha256 = "<official asset SHA-256>"
```

Add it to `~/.copilot-shell/config.toml`, preserving existing model-provider
settings. The two Herdr fields are optional as a pair. Without `[aw]` or explicit
AW environment settings, the integration is disabled. Any of `COSH_AW_CONFIG`,
`COSH_AW_CONFIG_SHA256`, `COSH_AW_HERDR` or `COSH_AW_HERDR_SHA256` selects the complete
environment group; partial groups do not borrow file values.
Malformed settings and an AW-disabled build report an error. Remove the section
and the four overrides to disable integration in a new shell.

The generated format-2 profile uses empty notification routes, a real SecCore
`tool_guard`, and `tool_response.tokenless` instead of `tool_response.command`.
Exactly one result backend is accepted. The native backend uses the existing
source digest/epoch fence and conservative projection codec; it requires the
explicit unrecoverable opt-in and defaults `allow_text_reencoding` to false.
Version probing, pin checks and compression share the callback deadline. Failure
preserves the original result. Its process environment disables Tokenless stats
and SLS, explicitly enables compression, and avoids user configuration lookup.

The sidebar distinguishes callbacks, handler notifications, checks and candidate
results. Journal counters are scoped to the current attachment; corrupt or
incomplete evidence is unavailable. Candidate counts never prove native adoption.
OS coverage and final/protected guarantees remain unconfirmed. The older observer
example below is a separate protocol example, not a dependency of this setup.

### Historical observer protocol example

This opt-in Linux/Bash profile keeps Qoder's native interactive loop and PTY.
It is an experimental observation slice, not a strong security mode or a completed
four-Agent POC. Build the product with the AW feature (Rust 1.97.1):

```bash
cargo +1.97.1 build --manifest-path src/cosh-ng/Cargo.toml --locked -p cosh-shell --features aw
```

Prepare a private, caller-owned `0600` JSON configuration outside untrusted
project content. Review the handler code and independently pin its executable
and script. Paths below are placeholders to replace with absolute local paths;
all digest fields require the corresponding reviewed file's lowercase SHA-256.
The handler workspace must equal the directory where Qoder starts.

```json
{
  "format": 1,
  "required_safety": false,
  "qoder": {
    "program": "/absolute/path/to/qodercli",
    "program_sha256": "<reviewed Qoder 1.1.47 executable digest>"
  },
  "native_config_directory": "/absolute/path/to/native-qoder-config",
  "handler": {
    "provider_id": "tool-observer",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": ["/absolute/path/to/tool-observer.py"],
    "environment": {},
    "pins": [{
      "path": "/absolute/path/to/tool-observer.py",
      "state": {"sha256": "<reviewed handler digest>"}
    }],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 65536,
      "output_bytes": 1024,
      "stderr_bytes": 1024
    }
  }
}
```

The [sample observer](../../../../src/aw/examples/tool-observer.py) receives one
AW JSON occurrence on stdin and returns exactly `{"format":1,"observed":true}`.
The input identifies the runtime, session, attachment, tool, configuration
revision and native outcome without tool arguments or result text. Other output,
changed pins, nonzero exit or timeout records an optional observation failure;
native execution continues. This executable protocol is not a sandbox.

In the workspace, explicitly enable the reviewed config revision, then open cosh:

```bash
export COSH_AW_CONFIG=/absolute/path/to/aw.json
export COSH_AW_CONFIG_SHA256='<reviewed configuration digest>'
export COSH_SHELL_INTEGRATION=enhanced
/absolute/path/to/cosh-shell --shell bash
```

Inside that terminal, type `qoder`. The scoped shim adds native settings before
exec; it does not modify the global PATH, executable, login state or existing
Hook files. This profile accepts positional prompts, `--model`, `--name`, and
`--resume` with an explicit identifier; other options are rejected. Plain
foreground launches are supported. Aliases/functions, absolute native paths,
nested launches, pipes, redirection and remote commands are not certified entry
forms. The shim is bypassable and provides no OS enforcement. A config with
`required_safety: true` is refused before Agent startup.

Reset is tracked through native session identity. Old callbacks/completions do
not count toward a new attachment. Reset without a new native session ID and
in-process return to a retired session report a gap. Limits are 128 attachment
generations, 1024 tool occurrences per runtime and a bounded shell launch count;
open a new cosh session when these experimental limits are reached. Qoder's own
history and permission settings retain their native semantics.

When Herdr runs cosh as the pane's default shell and supplies `HERDR_SOCKET_PATH`
and `HERDR_PANE_ID`, an optional worker publishes observation counters. Merge
the rows from [interactive.toml](../../../../src/aw/integrations/herdr/interactive.toml)
into the existing Herdr v0.9.0 configuration. Each pane validates its own cosh PID;
the viewer cannot execute a handler or control an Agent. RPC failures expire the
metadata after three seconds; the private query reports viewer availability.
The worker refreshes for at most 24 hours. Hiding the sidebar or losing this
optional metadata worker does not itself stop the Agent. Closing the managed
Herdr instance described below ends its interaction. Native coexistence and
visible rendering require separate evidence from synthetic protocol/PTY tests.

For this environment-only example, unset `COSH_AW_CONFIG` and
`COSH_AW_CONFIG_SHA256` before opening another cosh session to disable integration.
If a user `[aw]` section is also present, remove it to disable the file configuration. Normal shell exit joins the optional viewer
and removes AW scratch records. SIGKILL can leave that shell's private scratch
directory; native Qoder history follows its own retention policy. Existing
07B/07C1 explicit launchers remain available. No default feature or security
policy is changed. See [ownership and evidence](../../../../src/aw/docs/design/interactive-observation.md).

### Open Herdr on demand

The native-component `[aw]` configuration already enables this entry. As an
alternative, set the complete environment group with a prepared, pinned Herdr
v0.9.0 binary:

```bash
export COSH_AW_CONFIG=/absolute/path/to/aw.json
export COSH_AW_CONFIG_SHA256='<reviewed configuration digest>'
export COSH_AW_HERDR=/absolute/path/to/herdr
export COSH_AW_HERDR_SHA256='<official digest for the selected architecture>'
```

The digest must match the [pinned asset manifest](../../../../src/aw/integrations/herdr/upstream.json).
Daily startup neither downloads/builds Herdr nor invokes external Python/shell
launcher scripts; Bash and cosh's internal shell integration remain in use.
Ordinary commands stay in cosh. Typing `qoder` opens the native Herdr terminal and
AW observation sidebar. Ending the Agent returns to the same outer Bash with its
cwd and variables intact and the Agent exit status preserved. No separate Herdr
entry or changes to the user's Herdr configuration are required.

This configuration enables the entry for the current session; it does not change
`/etc/shells` or an account's login shell. Selecting cosh as the login shell still
requires explicit AW/Herdr configuration and a `qoder` command to open the terminal.

This entry currently supports Linux/Bash, one pane and the configured fixed cwd;
launching after cd to a different directory is explicitly rejected. Each launch
owns a private server/client and pane owner using the current Rust Hook/query
contracts. Prompt/argv data travels through private records, not interpolated shell
commands. Herdr startup has a ten-second deadline and sessions a 24-hour limit.
Missing/incorrect pins reject startup without a silent fallback. Shell readiness
timeout does not start the Agent. If Ctrl-C ends the Agent, or the launcher receives
SIGHUP, the owned instance is cleaned up and the original shell restored; an Agent
that handles Ctrl-C keeps its native behavior. Closing the whole managed Herdr
instance ends this interaction; hiding the sidebar does not grant control authority.

Earlier acceptance with fixed real Herdr and synthetic Qoder covered on-demand startup, metadata, argv,
exit status, same-shell return, repeated launch, incorrect pins, Ctrl-C, launcher
hangup and shell readiness timeout. A login-entry regression verifies that the
inner pane runs `.bashrc` without replaying the outer login profile. Real Qoder
1.1.47 has run through this entry; Stop callbacks confirm subsequent answers cite
a marker generated only by the projection command. This does not remove original
text from the user prompt. Those earlier probes did not certify a complete
visual demonstration with native components. Follow the acceptance procedure
above for that separate stage. Multiple panes, arbitrary detached descendants
and SIGKILL cleanup remain uncertified. The sidebar reports observation, not complete
16-event or final/protected coverage. To restore direct Qoder entry, remove the
two Herdr fields from file configuration or the two Herdr variables from
environment configuration, retaining the profile path/digest pair.
Installation-time asset preparation is separate from runtime.

### Public lifecycle notification configuration

To run commands at other lifecycle points, explicitly select format 2. This is
an experimental notification profile: commands receive the complete native
payload, so authorize scripts to read prompts, arguments and results before use.
Pin the actual Qoder CLI executable, not a dispatcher script. The following
no-op command acknowledges session start; replace it with your reviewed command.
Use the same opt-in environment and launch commands above.

```json
{
  "format": 2,
  "required_safety": false,
  "qoder": {
    "program": "/absolute/path/to/qodercli",
    "program_sha256": "<reviewed Qoder 1.1.47 executable digest>"
  },
  "native_config_directory": "/absolute/path/to/native-qoder-config",
  "cwd": "/absolute/path/to/workspace",
  "notifications": {
    "session.start": [
      {
        "provider_id": "lifecycle-notifier",
        "provider_version": "1",
        "program": "/usr/bin/python3",
        "program_sha256": "<reviewed interpreter digest>",
        "cwd": "/absolute/path/to/workspace",
        "args": [
          "-c",
          "import json,sys; event=json.load(sys.stdin); print(json.dumps({'format':1,'observed':True}))"
        ],
        "environment": {},
        "pins": [],
        "limits": {
          "timeout_ms": 1000,
          "input_bytes": 1048576,
          "output_bytes": 1024,
          "stderr_bytes": 1024
        }
      }
    ]
  }
}
```

The notification keys currently accepted are `session.start`, `input.submit`,
`tool.before`, `tool.after`, `permission.request`, `compact.before`, `compact.after`,
`subagent.start`, `subagent.stop`, `turn.stop`, and `session.end`. Owner routes additionally
accept `runtime.observed`, `runtime.exited`, and `coverage.changed`. Both native tool
outcomes map to `tool.after`; inspect `native_event` and the unchanged `payload`
to distinguish success from failure. `turn_id` is null with an explicit unknown
reason; input arrival is not proof that Qoder accepted a new task.

Each key takes 1–4 ordered command configurations with distinct `provider_id`s,
all using the top-level cwd. The entire route's pins are checked before execution.
Each command has at most 1 second, the chain at most 2 seconds of execution, and
at most 1024 callbacks are admitted per runtime. Successful output must be exactly
`{"format":1,"observed":true}`. Other output or command failure records a gap and
leaves native execution unchanged. Journal claims prevent an already claimed
occurrence from running twice; native callbacks without stable IDs receive fresh
arrival IDs, so repeated identical prompts remain separate occurrences.

Format 2 excludes `handler`; format 1 excludes `cwd`, `notifications`, `tool_guard`, `input_response`, `stop_response` and `tool_response`. Unknown
names, unconnected sources, and required safety fail admission. Bash transformations
and guards can be tried through the separate configuration below. Remaining
sources and real effects still need per-event acceptance, not a claim of all 16. See the [16-event matrix](../../../../src/aw/docs/design/interactive-observation.md#qoder-16-event-adaptation-matrix)
for source evidence, gaps and next actions. Queries and the optional Herdr view
only read command counts. Full real Qoder TUI/Hook coexistence remains unverified.


On a first-workspace trust flow, Qoder may omit SessionStart. Format 2 binds
the first authenticated main input to its native session and continues notifications,
while query reports observation_gap=true and session_start_observed=false.
No session.start is fabricated. A real reset still requires SessionStart and a
new session identity; unknown or retired sessions cannot attach through input.

## Experimental input submission responses

To reject a submitted input or add context before Qoder handles it, add the following
`input_response` field to format 2. This explicitly authorizes one response command;
notification commands keep their existing acknowledgement-only contract. Keep
`required_safety` false. `notifications` may be empty, and the command cwd must match
the top-level cwd. Replace paths and pins with reviewed values before use.

```json
{
  "input_response": {
    "provider_id": "input-policy",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": [
      "-c",
      "import json,sys; request=json.load(sys.stdin); print(json.dumps({'format':1,'decision':'continue'}))"
    ],
    "environment": {},
    "pins": [],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 1048576,
      "output_bytes": 65536,
      "stderr_bytes": 1024
    }
  }
}
```

The command receives `{"format":1,"scope":"input.submit.respond","event":{...}}`.
`event` is the authenticated lifecycle envelope, including the original native
`payload.prompt`; its Turn identity remains unknown. Return exactly one of:

- `{"format":1,"decision":"continue"}` to continue native handling.
- `{"format":1,"decision":"continue","additional_context":"reviewed context"}`
  to add context alongside the original input.
- `{"format":1,"decision":"reject","reason":"input policy declined"}` to reject it.

No prompt replacement or permission approval is available here. Context and reasons
must be nonempty, NUL-free strings of at most 16384 UTF-8 bytes. Extra/mixed fields,
notification acknowledgements, duplicate JSON keys, nonzero exits, changed pins,
timeouts and cancellation yield rejection without context. Each response command
has at most one second and shares a two-second callback deadline with preceding
notifications; it does not receive a fresh deadline. Failed notification output
cannot become a decision, but its elapsed time still consumes this budget.

The adapter maps context to UserPromptSubmit.additionalContext and rejection to
native `decision=deny`. The dedicated helper maps input/binding errors to exit 2.
Reset or session end withholds stale responses. Metadata-only claims live in the
shell's private `input-response-journal`; query reports `input_response` as
`experimental_native_response`, not adoption. Queries never run the command again.

Fixed Qoder 1.1.47 print probes consumed the returned context and rejected policy,
malformed-output, timeout, cancellation and missing-binding cases. A peer Hook's
denial still blocked input; two context Hooks were both consumed. Killing the AW
helper let native processing continue without its context. These are native Hook
guarantees, not final/protected or proof of a controlled model-request boundary.
A natural cosh TUI probe also observed the context marker in the native Stop
callback, followed by policy/malformed/timeout submissions without further Stops,
then normal Agent and shell exit. Queue/supplement semantics and other plugin
combinations remain unverified.

## Experimental main-agent stop responses

To check an answer when the main Agent stops, add `stop_response` to format 2.
This separately authorizes one command; `notifications` may be empty. Use reviewed
paths/pins, match the workspace cwd, and keep `required_safety` false.

```json
{
  "stop_response": {
    "provider_id": "stop-policy",
    "provider_version": "1",
    "program": "/usr/bin/python3",
    "program_sha256": "<reviewed interpreter digest>",
    "cwd": "/absolute/path/to/workspace",
    "args": [
      "-c",
      "import json,sys; request=json.load(sys.stdin); print(json.dumps({'format':1,'decision':'allow_stop'}))"
    ],
    "environment": {},
    "pins": [],
    "limits": {
      "timeout_ms": 1000,
      "input_bytes": 1048576,
      "output_bytes": 65536,
      "stderr_bytes": 1024
    }
  }
}
```

The command receives `{"format":1,"scope":"turn.stop.respond","event":{...}}`,
including the native stop payload and unknown Turn identity. Return exactly one of:

- `{"format":1,"decision":"allow_stop"}` to permit stopping without claiming task success.
- `{"format":1,"decision":"continue","reason":"complete the missing check"}`
  to request further work through native `decision=deny`.

Reasons must be nonempty, NUL-free and at most 16384 UTF-8 bytes. Mixed/native
fields, malformed output, nonzero exit, pin changes, timeout and cancellation
produce `continue=false` with unavailable diagnostic fields. They never request
further work or report a passed check. The dedicated `--aw-stop` helper uses this
same failure behavior for malformed input or missing binding; exit 2 would instead
request more model work for Stop. No tool approval or output replacement is granted.

A native `stop_hook_active=true` skips the response command and requests stopping
with the same diagnostic to bound recursive checks, including continuations
requested by another Hook. A later native Stop with that flag false is a distinct
occurrence and can invoke the check. This relies on the native flag, not a proven
Task identity; repeated checks do not certify task completion. Notification routes
still run independently and cannot supply the response decision. Their elapsed
time counts toward the shared two-second deadline; the response command itself
has at most one second. Reset/session end fences stale responses.

The private `stop-response-journal` stores only metadata and decision/skip/failure
classes, with `native_adoption=unconfirmed`. Query reports the configured
`stop_response` and never reruns it. Fixed Qoder 1.1.47 print probes verified
allowing stop, consuming a continuation reason, and stopping repeated checks.
Malformed output, timeout, cooperative cancellation and missing binding ended
without another response. With a peer Hook requesting continuation, AW allow_stop
permitted that request; AW continue=false stopped it in both tested declaration
orders. These cases do not certify arbitrary Hook/plugin combinations.

A natural cosh TUI run with user-level Hooks verified two successive inputs, each
with a consumed continuation reason and Stop flags false → true. New input reset
the flag to false; malformed/timeout checks did not trigger another response.
The TUI displayed the unavailable diagnostic, and /exit produced runtime.exited
before shell teardown. Print mode omitted that diagnostic and exited 0 even when
checks failed. Neither the native exit code nor allow_stop proves task success;
already displayed answers are not retracted. Killing the helper left a started,
incomplete check while native output still completed. This excludes final/protected
certification; required_safety remains unavailable.

## Runtime and coverage notifications

Add runtime.observed, runtime.exited or coverage.changed to the same format-2
notifications map, using the command shape above. Any of these keys starts a
cosh-owned worker; Herdr is optional. These envelopes have source runtime_owner
and native_event null. Native hook payloads cannot select this source.

- runtime.observed records admission and PID/start-tick identity before exec.
  It does not prove native exec success or Agent readiness; session/Turn is unknown.
- runtime.exited reports the root process exit from a pidfd opened before exec.
  It works without SessionEnd and does not consume Bash's wait status. exit_status
  stays null, descendants_reaped false, and task success unknown.
- coverage.changed carries previous/current sampled snapshots of callback
  attachment, session epoch, observed gaps, notification delivery and root exit.
  Intermediate states between samples may be coalesced. OS coverage remains
  not_attached; silence or viewer disconnection does not establish protection loss.

Registration is acknowledged before launch, with a five-second wait limit.
Unusable pidfds or missing owner registration reject this opted-in launch.
The worker samples at 100 ms between bounded deliveries, with a 24-hour observation
window, at most 128 registrations per shell and 1024 coverage changes per runtime.
Notifications retain the one-second command/two-second chain limits. Failed
deliveries are not retried; they remain a gap, including failed terminal delivery.
Stopping the shell cancels in-flight commands and closes owned pidfds. No
owner-crash recovery or post-shutdown delivery is promised; open a new cosh session
when the observation window expires.

Query adds owner_observation and runtime_observer snapshots; native callback
counts remain separate. Reading query or reconnecting Herdr never executes these
commands. The notification journal contains metadata/digests only. Runtime
snapshots are shell-scoped and removed with its temporary directory; trusted
handlers own any external retention. required_safety remains unsupported.

## Experimental final Bash check

Format 2 accepts the following `tool_guard` field separately from notification
routes. Keep top-level `required_safety` false; `notifications` may be empty.
Replace paths and the digest with verified artifacts, and explicitly configure
the scanner environment and policy-file pins. Its cwd must match the top-level
cwd. The fixed CLI version is 0.12.0.

```json
{
  "tool_guard": {
    "transforms": [],
    "scanner": {
      "provider_id": "sec-code",
      "provider_version": "0.12.0",
      "program": "/absolute/path/to/agent-sec-cli",
      "program_sha256": "REPLACE_WITH_EXECUTABLE_SHA256",
      "cwd": "/absolute/path/to/workspace",
      "args": [],
      "environment": {},
      "pins": [],
      "limits": {
        "timeout_ms": 2000,
        "input_bytes": 1048576,
        "output_bytes": 1048576,
        "stderr_bytes": 1024
      }
    }
  }
}
```

For Qoder PreToolUse Bash calls, this runs `scan-code --language bash --mode regex`.
Only pass with no findings releases the checked candidate. If its complete value
is unchanged, the response is empty; only actual changes emit `updatedInput`. This
avoids claiming an input rewrite for a pure check. Warn, deny, failure, malformed
output, timeout and cancellation return native deny. Passing the scan never grants
tool permission: normal approval still applies. Other tools are outside this Bash
profile's check scope.

`transforms` accepts zero to three command configurations of the same shape, with
provider IDs distinct from each other and the scanner. Commands receive
`{"format":1,"event":"tool.before","candidate":...}` in order; the candidate contains
tool_name, tool_input and cwd. Return exactly
`{"format":1,"command":"new Bash command"}`; only command is writable, preserving
all other native arguments. All notifications and transformations finish before
the final check. The entire pre-tool path shares two seconds; each transform or notification
gets at most one second. The scanner may request
up to 2000 ms for version verification plus scanning, capped by the chain
time remaining; it never restarts that deadline. Existing shorter configured
limits remain effective. Transformations and notifications are currently serial;
parallel independent read-only actions remain unimplemented.

Query reports effect `experimental_native_bash_guard` and tool_guard
`configured_not_certified`, not proof of actual blocking. The native CLI receives
scan content in --code argv, potentially visible to local process observers. AW
journals contain only digests and check results; sec-core audit retention remains
controlled by its own configuration.

Real Qoder 1.1.47 TUI probes with sec-core 0.12.0 verified approval of the
transformed command, native rejection, policy denial and transform failure. These
bounded observations preserve the final-safety limitations below.

Isolated Qoder 1.1.47 print probes verified replacement and denial, but also
confirmed that another hook can overwrite arguments and that exit 1 or SIGKILL
allows the original command. The dedicated guard entry maps recoverable
parse/binding errors to blocking exit 2; it cannot control native behavior after
being killed. See the [native acceptance boundary](../../../../src/aw/docs/design/interactive-observation.md#native-consumption-and-failure-boundary).

Required safety/final admission remains refused; the bounded TUI cases do not
certify all hook/plugin combinations or failure modes. Format 1 and format 2 without
tool_guard, input_response, stop_response and tool_response retain notification behavior. This provides neither OS isolation nor
assurance that regex scanning detects every dangerous command.

## Experimental interactive tool result responses

To project a completed main-agent Bash result, add `tool_response` to format 2.
This explicitly authorizes one result command; a notification acknowledgement
cannot acquire replacement authority. The following is a configuration fragment;
replace the executable, digest and workspace with trusted values:

```json
{
  "tool_response": {
    "command": {
      "provider_id": "result-projector",
      "provider_version": "1",
      "program": "/absolute/path/to/projector",
      "program_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "cwd": "/absolute/workspace",
      "args": [],
      "environment": {},
      "pins": [],
      "limits": {
        "timeout_ms": 1000,
        "input_bytes": 1048576,
        "output_bytes": 131072,
        "stderr_bytes": 4096
      }
    },
    "accepted_reversibility": [
      "unrecoverable"
    ]
  }
}
```

The command receives `format: 1`, `scope: "tool.after.respond"`, the full `event`,
`source: {"text": "...", "digest": "..."}` and
`accepted_reversibility: ["unrecoverable"]`. The digest is SHA-256 of the original
UTF-8 text. Turn remains unknown. Return exactly one of:

```json
{"format":1,"decision":"preserve"}
```

```json
{"format":1,"decision":"replace","source_digest":"<request.source.digest>","text":"replacement text"}
```

Copy the actual request digest in a replacement. Empty text, NUL, text over
65,536 UTF-8 bytes, foreign digests, extra/native control fields, duplicate JSON
keys and failed commands cannot replace the result. The encoded response must
also fit the configured output budget. Notifications and the response share the
original two-second callback deadline; the response command is limited to one
second. Cancellation, timeout and invalid responses retain the original result
with diagnostic fields. Required safety remains unsupported: this optional
projection is not mandatory redaction and does not undo an executed tool.

Only the existing completed Bash stdout object is supported: exit code 0, null
signal, no interruption, image or expected-silence flag, and empty stderr.
Other tools and PostToolUseFailure remain notification-only. Unsupported Bash
shapes do not dispatch the projector. The paired tool result is reserved once,
including failures, so a later callback cannot retry projection. Reset/exit fence
late candidates. Journals retain source/candidate digests, not their text; the
operator-owned command receives native content and controls its own retention.

The response uses Qoder's `updatedToolOutput` slot. Query reports configuration
and experimental replacement support without rerunning commands. Fixed Qoder
1.1.47 print/TUI probes verified preserve and replacement in subsequent answers;
the TUI evidence comes from Stop callbacks, not rendered answer-region assertions
or native history verification. A later peer Hook can override AW's replacement.
Malformed output, timeout and cancellation preserve the original; killing the
helper can leave its check incomplete while Qoder continues and exits 0. The
query adoption status remains unchanged. This external command backend does not
automatically invoke Tokenless or sec-core, establish recovery, or grant
final/protected guarantees. The existing
single-turn `qoder-project` retains its separate inspection/history contract.
