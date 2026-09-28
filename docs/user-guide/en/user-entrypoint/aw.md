# AW user guide

[中文版](../../zh/user-entrypoint/aw.md)

AW lets you manage tool Hooks for different Agents in one configuration. This
experimental fork adds a standalone service and launcher for QwenPaw, Qoder CLI,
OpenClaw and Hermes. You keep using each Agent's interface; AW runs your commands
at its native tool-before and tool-after points and records invocation metadata.

The same structured Provider can now observe and block before tools and observe
after tools in all four adapters. Existing native scripts remain supported.
Complete sec-core delivery and uniform action semantics remain planned. This branch is a Linux development build, with no installer or
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
| One portable Provider response for every framework | ✅ Before observe/block, after observe; native scheduling retained |
| Reuse a persistent Agent profile | ✅ OpenClaw with `--native-state-dir`; QwenPaw working directory; Qoder native profile. ❌ Hermes existing-profile attachment |
| Confirm plugin loading during launch | ✅ OpenClaw Gateway startup and QwenPaw middleware registration; individual effects still need tool receipts |
| AW-managed approval, final security guard, OS enforcement | ❌ |
| Attach an existing Gateway, package installation, policy hot reload | ❌ |

| Framework tested | Native scheduling | Execution evidence | Approval boundary |
| --- | --- | --- | --- |
| Qoder CLI 1.1.64 | Parallel by default; a matching `sequential: true` group serializes matching synchronous Hooks | ✅ Before overlap and serial input changes; print/TUI model adoption of native-mode after replacement; headless ask denied | Native ask reaches the permission path; headless denies; interactive approval unverified |
| OpenClaw 2026.9.6, Node 24.16.0 | Before serial by priority; after concurrent; result middleware serial by registration order | ✅ Gateway through AW; model adoption of changed input/results; repeated launches retain native authentication, sessions and existing plugins | Native `requireApproval`; deny/report behavior tested without a model, interactive approval unverified |
| QwenPaw 2.2.2b4, AgentScope 2.0.8 | Middleware onion: before A/B, after B/A | ✅ Official App/API with a real model: common-policy allow/block/after, existing-plugin coexistence and preserved working-directory files; ❌ ACP/TUI plugin loading | ❌ No command-ask bridge at this middleware point; explicit ask fails visibly |
| Hermes source `952c941e741e922a9be8fc403c8944c6e96318bb` | Shell callbacks in registration order; native tool scheduling unchanged | ✅ Actual CLI through AW; allow, block and noninteractive approval denial | Native `approve` is denied without an interactive approval bridge; interactive approval unverified |

The common Provider passed real-model allow/block/after checks through Qoder,
the official QwenPaw App, Hermes CLI and OpenClaw Gateway, with existing Hooks.
OpenClaw, Hermes and QwenPaw can emit after callbacks for blocked attempts;
an after callback alone does not prove that the tool executed.
A separate real Qoder run adopted decisions from installed sec-core V1 0.8.0.
These results do not certify every tool type, failure response or interactive mode.
QwenPaw is a separate framework from Qwen Code.

## Build and prepare a configuration

Install and configure the chosen Agent separately, including its model access.
The same model-service credential can be used where that framework supports it;
native model and workspace settings are separate from the shared AW policy.
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
`effects` or private `config`; `config: {}` on the Provider is required. For portable observe/block effects, use the structured example below. The larger
static schema example still includes guards and rewrites this runtime rejects.

## Use one policy across Agents

Copy [aw.provider.yaml](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.provider.yaml)
and replace the absolute path to `examples/providers/policy.py` in its argv.
Python 3 is a dependency of this replaceable example Provider, not the Rust
launcher. The example denies inputs containing `AW_DENY_FIXTURE` and observes
completed tools. Use harmless printing commands to try it.

```bash
cp crates/aw-cli/examples/aw.provider.yaml ./target/aw.policy.yaml
# Edit the absolute Provider path in ./target/aw.policy.yaml.
./target/debug/aw check qoder --config ./target/aw.policy.yaml
./target/debug/aw run qoder --config ./target/aw.policy.yaml
./target/debug/aw run openclaw --config ./target/aw.policy.yaml --native-config /absolute/path/openclaw.json
```

The same file includes Hermes and QwenPaw; apply their native setup described
below. `check` runs Provider discovery and private-config validation without
starting the Agent; `run` checks automatically. Each callback supplies a common
event and accepts only configured effects. Empty effects leave native permission
checks intact. This shared protocol preserves the host's scheduling and does not
add interactive ask, rewrites or a final guard.

Provider failures follow `on_error`: `block` before tools or explicit `report`.
The live `aw hook` client also maps daemon failures. A missing/killed Qoder helper
can still fail open in the host; installed configuration alone is not mandatory
security enforcement. Audit records distinguish a policy block from a Provider
failure, but framework evidence is needed to prove the effect was adopted.

For a custom Provider, follow the [protocol](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/docs/design/provider-protocol.md).
The optional sec-core CLI example has separate rule configuration and does not
claim complete security-policy delivery. The larger schema example remains a
reference for planned capabilities, not a runnable policy for this slice.

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
By default its native state is temporary. To retain authentication, sessions and
installed plugins across runs, select the existing OpenClaw state directory:

```bash
./target/debug/aw run openclaw --config ./target/aw.yaml --native-config /absolute/path/openclaw.json --native-state-dir /absolute/path/openclaw-state
```

Stop the original Gateway before using this directory. AW starts a new process;
it does not attach to an existing Gateway. The original configuration and selected
state remain in place; only the generated configuration and AW bridge are removed
on exit. The profile's `.aw-launch.lock` prevents concurrent AW launches, and stays
as an unlocked file after exit. Native home and cwd are preserved in this mode,
including their path-resolution semantics. Provide expanded JSON without
`$include`; relocating native include paths is not supported.

AW waits up to 30 seconds for the OpenClaw plugin's `gateway_start` receipt.
Missing or invalid confirmation fails the launch and cleans up its Agent group.
Global plugin disablement or an AW deny entry is rejected. Existing allowlists and
plugin entries are retained; existing Hook code is not converted into Providers.
`agent exec`, `--profile`, `--dev`, `--reset`, `--container` and `--force` are
rejected because they skip the supported binding or conflict with launch ownership.
Container or service-managed environment overrides of the binding are also rejected.
Stopping this launched Gateway ends the AW run.

Hermes retains its native shell-Hook consent. AW rejects `HERMES_SAFE_MODE` (`1/true/yes/on`),
which disables Hook registration. Review generated command settings
before granting consent; AW does not silently add `--accept-hooks`.
This Hermes version reads both configuration and persistent state from
`HERMES_HOME`; it has no supported `HERMES_CONFIG_PATH` overlay. AW still uses an
isolated Hermes home, so existing authentication/session state is not reused.
`--native-state-dir` is rejected for Hermes. Existing-profile support needs a
native plugin installation or an upstream configuration-overlay interface.

QwenPaw requires the `qwenpaw app` server entrypoint and a separate, initialized
working directory. Bare `qwenpaw`, project-directory, TUI and ACP entrypoints skip
external Hook-plugin loading in the tested version and are rejected. Its native plugin is
installed for the launch and removed when the Agent exits normally:

```bash
QWENPAW_WORKING_DIR=/absolute/path/isolated-qwenpaw ./target/debug/aw run qwenpaw --config ./target/aw.yaml
```

An existing `plugins/aw-native` directory is rejected rather than overwritten.
Other plugins and persistent files in the selected working directory remain.
`--native-config` and `--native-state-dir` are not supported for this adapter.
AW waits up to 30 seconds for confirmation that all middleware factories were
registered; this does not gate the App's HTTP listener or prove future adoption.
Before sending requests, check that native plugin status reports `aw-native` as
loaded. Real-model acceptance checks this status and the actual tool receipts.
A denied QwenPaw tool may still produce an observed after response under its
native middleware nesting.

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
unsupported effects and guards fail admission. No `security.violation` final check
or sec-core policy is silently enabled. Interactive ask, stronger enforcement
and complete security coverage still require their own implementation and acceptance.

See the [configuration reference](../../../developer-guide/en/aw/configuration.md)
for field limits and the 16-event vocabulary. Continue with all-four common-policy
runtime acceptance, the complete QwenPaw entrypoint, sec-core joint rules and
service distribution.
