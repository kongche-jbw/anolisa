# Native Hook integration lab

[中文版](native-hooks_zh.md)

This fork delivers a runnable slice before the structured Provider protocol is
implemented: one configuration, an independent daemon, a Rust launcher, and native
tool callbacks for four Agent frameworks. It has no cosh or Herdr dependency.

## Execution boundary

`aw run` reads the configuration, prepares an isolated native profile, acquires a
daemon session lease and starts the Agent with its terminal. Each registered
native callback invokes `aw hook`; the client sends the callback body, cwd and
environment to the private Unix socket. The daemon runs one configured argv,
bounds its pipes and lifetime, records metadata, and returns stdout/stderr/status.
The native framework decides callback ordering and interprets the response.

The launcher is Rust. OpenClaw's JavaScript plugin and QwenPaw's Python plugin are
host-loaded adapters, not external product launch scripts. Hermes and Qoder use
their existing command Hook support. Python scripts under `tests/native` are
acceptance harnesses and are not required to launch the product.

| Source | Responsibility |
| --- | --- |
| `crates/aw-config` | Common configuration schema and static relationships |
| `crates/aw-cli/src/model.rs` | Native binding admission and command lookup |
| `crates/aw-cli/src/launch.rs` | Generated native profiles, service readiness, Agent process |
| `crates/aw-cli/src/server.rs` | Immutable configuration service, leases, metadata audit |
| `crates/aw-cli/src/ipc.rs` | Bounded local request/response transport |
| `crates/aw-cli/src/process.rs` | Reused bounded POC subprocess runner; cancellation and group cleanup |
| `adapters` | Minimal native bridge assets and pinned framework contracts |
| `tests/native` | Framework-native and optional real-model acceptance |

This native mode does not schedule an AW policy chain and does not replace the
pending Core/Journal PR. It reuses the POC process-runner mechanism without coupling
the daemon to cosh. `native-hook/v1alpha1` deliberately differs from
`aw-provider/v1alpha1`: native response shapes and error policies remain host
specific. Future structured execution can use Core, normalized events and effect
admission without silently changing this mode.

## Native contracts

| Framework | Input and response | Scheduling and effect boundary |
| --- | --- | --- |
| Qoder 1.1.64 | Native PreToolUse/PostToolUse JSON and native command exit status | Default parallel. Any matching synchronous `sequential` group serializes that set. Sequential before passes `updatedInput`; after's runner does not feed an intermediate rewrite to the next Hook |
| OpenClaw 2026.9.6 | `{hook,event,context}` on stdin; native plugin result on stdout | Before serial by descending priority; each handler sees an original snapshot. After uses concurrent settlement and discards return values |
| QwenPaw 2.2.2b4 | Projection of native tool call/response; explicit adapter error on unsupported ask | Lower middleware priority wraps higher; before A/B, after B/A. Native permission validation precedes `on_acting` |
| Hermes pinned source | Native shell-Hook event and `{action: ...}` result | Callbacks serial by registration order; block dominates approve; later callbacks still execute after block. Post is observational |

OpenClaw provides two extra `tool.after` points. `native.point:
tool_result_persist` installs a synchronous history projection; it does not prove
that the current model loop receives changed text. `native.point:
agent_tool_result` uses the native result middleware API, chains in registration
order and changes the result supplied to the model. It has no priority field.
Give each registration its own Provider instance name.

Qoder's `native.sequential` is an explicit host option. OpenClaw and QwenPaw use
`native.priority`. Unsupported extensions fail target admission. Common empty
`native: {}` steps can be shared by all four targets, but control scripts need
framework-specific decoding until structured Providers are implemented.

## Evidence and remaining gaps

The tests used official Qoder 1.1.64 Linux ARM64 and OpenClaw 2026.9.6 with Node
24.16.0. QwenPaw source was
`3822ec7173d17cf37c8a02f51d3ed5628079e86e` with AgentScope 2.0.8; Hermes source was
`952c941e741e922a9be8fc403c8944c6e96318bb`. Python environments were isolated.
Permanent user installations and services were not replaced.

| Acceptance | Outcome |
| --- | --- |
| Qoder before scheduling, input rewrite, native ask | Real AW callbacks observed. Headless ask denied. Native write permission prevented tool execution; real PostToolUse adoption remains open |
| OpenClaw Gateway through `aw run` | Tool input changed; raw text value 52 became 73 through result middleware; model answered 73. History and after observation also recorded 73 |
| QwenPaw public runtime through `aw run` | Allow wrote 42; deny did not write; explicit ask errored without writing. Before A/B and after B/A observed; complete CLI/TUI remains open |
| Hermes actual CLI through `aw run` | Allow wrote 42; block and noninteractive approve did not write. Before A/B and after A/B observed |
| Native coexistence | Installed framework runners and plugin loading tested; OpenClaw absent/empty allowlists preserve native plugin admission |
| Interactive user approval | Not validated for any framework; no AW approval UI |
| sec-core policy, all tool/result classes, OS/final/protected | Not certified by these command fixtures |

The Qoder run budget was capped at three short scenarios, including an earlier
startup probe. The fixture was corrected to supply explicit tool permission, but
was not rerun after the cap. BYOK setup failed to save; the cause was not proven.
No further Qoder credits should be used without renewed authorization.

Real-model tests are opt-in and excluded from CI. Their bounded runners record
owned PIDs, commands, deadlines and cleanup. Local evidence remains in the ignored
repository `target/native-lab/<framework>` directories, with build/gate evidence
under `src/aw/target`. Credentials are runtime inputs, never test fixtures.

The normal AW gate checks Rust configuration, transport, process and launcher
behavior without Agent downloads or model access. Native framework tests load
the pinned official runtimes; they must fail visibly when those dependencies are
missing or changed, rather than substitute a simulator and report success.

## Service limits

The local socket and state directory are private to the same user. Native
callbacks inherit their caller's environment; it may contain Agent credentials,
so Providers run inside that user's trust boundary. The daemon audit excludes
payloads, environment and argv. It is not an authenticated effect journal or an OS
security boundary, and does not prevent a host from ignoring a native callback.

Configuration is immutable for a daemon instance. A different revision at the
same socket is rejected. Active session leases use PID and process start time;
dead owners are pruned. Active callbacks and leases keep the service alive;
on-demand idle expiry is 300 seconds. Control calls and readiness have deadlines.
No package manager, hot reload, existing-Gateway attach or deployment reconciler
is included.

Each command has at most 4 MiB per output stream and a 300-second outer ceiling,
further restricted by its framework window. Stdout/stderr and native exit codes
are preserved by AW; adapter/host contracts decide how to consume them. An
explicit shell is necessary for shell expressions. Cleanup contains owned process
groups, not processes that deliberately escape with a new session.

## Subsequent delivery

1. Close Qoder real after and QwenPaw full entrypoint acceptance with explicit
   native permissions and isolated profiles; retain interactive ask as a separate
   acceptance dimension.
2. Define normalized events, `describe/validate_config/invoke`, supported effects
   and errors using this matrix. Integrate Core and validate actual native adoption.
3. Work with sec-core on rule configuration, built-in/custom policy aggregation,
   explicit unsupported actions and real security cases. A final AW check requires
   a controlled chain and cannot be inferred from this host-scheduled mode.
4. Add service packaging, binding/query lifecycle and all-four shared-policy
   acceptance. cosh and Herdr can follow as independent clients of that service.
