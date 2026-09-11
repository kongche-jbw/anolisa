# AW tool-result inspection

[中文版](../../zh/user-entrypoint/aw.md)

AW lets an explicitly configured Qoder or Codex hook inspect tool output with
SecCore and retain a verifiable execution record. The original tool result is
preserved. Inspection is observational: a warning is not execution approval or
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
aw-hook-cli <qoder|codex> /absolute/SETTINGS.json
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

## Results, failure and shutdown

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
