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
