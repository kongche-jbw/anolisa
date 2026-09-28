# AW user guide

[中文版](../../zh/user-entrypoint/aw.md)

AW lets you manage tool Hooks for different Agents in one configuration. This
experimental fork adds a standalone service and launcher for QwenPaw, Qoder CLI,
OpenClaw and Hermes. You keep using each Agent's interface; AW runs your commands
at its native tool-before and tool-after points and records invocation metadata.

The shared configuration and command transport work today. Commands that return
control decisions still use the selected Agent's native response format. A
portable security-policy interface, sec-core delivery and uniform action semantics
remain planned. This branch is a Linux development build, with no installer or
service package yet.

## Available in this branch

✅ means demonstrated at the stated scope; ❌ means unavailable or unverified.
The first-release scope continues to include all four frameworks.

| Capability | Status |
| --- | --- |
| One `aw.yaml`, named Providers, four Agent targets | ✅ |
| Independent daemon, command execution and metadata audit | ✅ Experimental |
| Native before/after bindings and existing Hook coexistence | ✅ Adapters and native tests; real-model scope below |
| Declare all 16 event names | ✅ Static validation; runtime currently binds only `tool.before` and `tool.after` |
| One portable Provider response for every framework | ❌ Structured Provider execution is subsequent work |
| AW-managed approval, final security guard, OS enforcement | ❌ |
| Attach an existing Gateway, package installation, policy hot reload | ❌ |

| Framework tested | Native scheduling | Real-model evidence | Approval boundary |
| --- | --- | --- | --- |
| Qoder CLI 1.1.64 | Parallel by default; a matching `sequential: true` group serializes matching synchronous Hooks | Before overlap, serial input changes and headless ask observed; ❌ real after adoption remains unverified because native permission denied the tool | Native ask reaches the permission path; headless denies; interactive approval unverified |
| OpenClaw 2026.9.6, Node 24.16.0 | Before serial by priority; after concurrent; result middleware serial by registration order | ✅ Isolated Gateway launched through AW; changed tool input and replacement result consumed by the model | Native `requireApproval`; deny/report behavior tested without a model, interactive approval unverified |
| QwenPaw 2.2.2b4, AgentScope 2.0.8 | Middleware onion: before A/B, after B/A | ✅ Public `QwenPawAgent` runtime through AW; allow and deny; full CLI/TUI not certified | ❌ No command-ask bridge at this middleware point; explicit ask fails visibly |
| Hermes source `952c941e741e922a9be8fc403c8944c6e96318bb` | Shell callbacks in registration order; native tool scheduling unchanged | ✅ Actual CLI through AW; allow, block and noninteractive approval denial | Native `approve` is denied without an interactive approval bridge; interactive approval unverified |

These results do not certify every tool type, failure response or interactive mode.
QwenPaw is a separate framework from Qwen Code.

## Build and prepare a configuration

Install and configure the chosen Agent separately, including its model access.
For this fork, build from source with the pinned Rust toolchain. Run from the
repository root:

```bash
cd src/aw
cargo build --locked -p aw-cli
cp crates/aw-cli/examples/aw.native.yaml ./target/aw.yaml
./target/debug/aw validate --config ./target/aw.yaml
./target/debug/aw plan qoder --config ./target/aw.yaml
```

The [native example](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.native.yaml)
contains four Agent entries and two harmless command Providers. Replace each
`agents.<id>.argv` with the installed executable if it is not on `PATH`.
Validation checks syntax and references without running a command. `plan` also
checks this adapter's native registration limits; neither proves model adoption.

`apiVersion`, `kind`, `metadata` and `spec` follow familiar Kubernetes naming.
AW needs no cluster or CRD. `spec.providers` contains named objects; event steps
refer to those names. For example:

```yaml
providers:
  my-hook:
    protocol: native-hook/v1alpha1
    transport:
      type: stdio
      location: agent
      argv: [/usr/local/bin/my-hook, --mode, inspect]
      env:
        POLICY_MODE: observe
    timeout_ms: 5000
    max_output_bytes: 1048576
    config: {}
events:
  tool.before:
    enabled: true
    required: true
    steps:
      - id: inspect-tool
        provider: my-hook
        native: {}
```

This fragment belongs inside `spec`. The command receives the adapter's event on
stdin; AW returns its stdout, stderr and exit code to the adapter. Arguments are
literal. Select `/bin/sh -c` explicitly when shell syntax is needed. Environment
values are literal too; `$NAME` is not expanded. The command inherits the Hook
caller's environment and working directory, with configured `env` overrides.

Empty stdout and exit zero suit an observer. A blocking or rewriting response
must match that framework's native contract. Native steps have no `operation`,
`effects` or private `config`; `config: {}` on the Provider is required. The
separate `aw-provider/v1alpha1` example describes planned structured execution
and cannot be run by this native launcher.

## Start an Agent

From `src/aw`, with the copied file and an installed, authenticated Qoder CLI:

```bash
./target/debug/aw run qoder --config ./target/aw.yaml
```

AW validates the selected registrations, prepares private native Hook settings,
starts or reuses a daemon with the same configuration revision, then starts Qoder
with inherited terminal I/O. Exiting the Agent returns its exit code to the shell
and removes the generated settings. Ctrl-C and termination are forwarded to the
owned Agent process group; shell job suspension/resume (Ctrl-Z) is not implemented. Existing native Hooks remain under Qoder's
scheduler. Qoder 1.1.64's `--setting-sources` conflicts with generated `--settings`
and is rejected, including arguments after `--`. AW also owns `--settings`:
provide base Qoder settings through `--native-config`, not a duplicate native flag.

OpenClaw and Hermes need an explicit native base configuration. These files hold
the Agent's model and runtime settings; they do not replace the shared AW policy.
AW copies them into private launch profiles and adds its registrations:

```bash
./target/debug/aw run openclaw --config ./target/aw.yaml --native-config /absolute/path/openclaw.json
./target/debug/aw run hermes --config ./target/aw.yaml --native-config /absolute/path/hermes.yaml
```

The OpenClaw example starts a new Gateway; interact through its native clients.
Keep workspace and other file references in the base profile absolute. Existing
plugin allowlists are preserved. `agent exec` in the tested version omits the
Hook-only plugin and is rejected. Stopping this launched Gateway ends the AW run.

Hermes retains its native shell-Hook consent. Review generated command settings
before granting consent; AW does not silently add `--accept-hooks`.

QwenPaw uses a separate, initialized working directory. Its native plugin is
installed for the launch and removed when the Agent exits normally:

```bash
QWENPAW_WORKING_DIR=/absolute/path/isolated-qwenpaw ./target/debug/aw run qwenpaw --config ./target/aw.yaml
```

An existing `plugins/aw-native` directory is rejected rather than overwritten.
The real-model acceptance used QwenPaw's public runtime harness, so native CLI
startup remains a separate acceptance item. `--native-config` is not supported
for this adapter.

## Service lifecycle and records

`--state-dir` and `--socket` override `spec.daemon.state_dir` and `endpoint`.
With `auto`, AW uses a revision-specific directory below `XDG_RUNTIME_DIR` and
`aw.sock` inside it. An explicit endpoint is an absolute Unix socket path,
optionally prefixed by `unix://`. Directories are private to the current user.

To manage the daemon explicitly, use a private absolute directory in two terminals:

```bash
./target/debug/aw serve --config ./target/aw.yaml --socket /absolute/private/aw.sock --idle-timeout 300
./target/debug/aw status --socket /absolute/private/aw.sock
./target/debug/aw stop --socket /absolute/private/aw.sock
```

Use `daemon.startup: external` when `run` must require that existing service.
Otherwise `run` starts it on demand. Active launcher leases and callbacks prevent
idle shutdown. After the last session exits, the on-demand daemon can remain for
up to 300 seconds. `status` reports service identity, active callbacks and leases;
it does not certify a policy effect or Agent health.

`audit.jsonl` beside the socket records configuration revision, Agent, event,
Provider, duration, sizes and exit outcome. It omits event bodies, arguments and
environment. Agent or Provider logs are separate and may contain their own data.
The audit is local metadata, not a tamper-proof security journal. Stop an owned
service before removing its state directory. Forced launcher termination may
leave generated profiles that require removal after the owned Agent has stopped.

## Native limits and next steps

Each command is bounded by its Provider timeout and output limit. Host callback
budgets also apply. Native Hooks retain their own ordering, errors and decision
aggregation; AW does not add a shared event deadline. The required
`default_event_budget_ms` field is reserved in native mode; explicit event
`budget_ms` is rejected. `required` does not strengthen host enforcement.

Only wildcard tool matching is currently bound. Enabled unsupported events,
structured steps and guards fail admission. No `security.violation` final check
or sec-core policy is silently enabled. Interactive ask and a framework-neutral
security result require their own implementation and acceptance.

See the [configuration reference](../../../developer-guide/en/aw/configuration.md)
for field limits and the 16-event vocabulary. The next increment should close
Qoder's real after gap and define the structured Provider request/response using
these native differences as constraints.
