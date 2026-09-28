# AW configuration reference

[中文版](../../zh/aw/configuration.md)

This reference covers static configuration validation and the experimental Linux
runtime on the fork's `feat/aw/native-hook-lab` branch. This lab is not a packaged
ANOLISA distribution. Start with the [user guide](../../../user-guide/en/user-entrypoint/aw.md)
for availability and the Agent workflow.

The [bundled schema](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-config/schemas/configuration-v1alpha1.schema.json)
defines the public shape; Rust validation also checks references and relationships.
`aw validate` performs these static checks. `aw plan AGENT` additionally checks the
native subset for that Agent; `aw run AGENT` prepares its host bindings and starts
the configured command. Static validity does not imply runtime support. Structured
Provider execution, discovery and capability admission remain unimplemented.

## Document fields

The only accepted envelope is `apiVersion: aw/v1alpha1`,
`kind: AWConfiguration`, `metadata: {name: ...}` and `spec: {...}`.
The earlier flat `api_version`/`name` design draft is not accepted or migrated.
`status`, installed bindings, revisions and capabilities are not user input.
All fields below are inside `spec` unless stated otherwise.

| Field | Contract |
| --- | --- |
| `metadata.name` (outside `spec`) | Configuration identity; 1 to 128 ASCII letters, digits, `.`, `_` or `-` |
| `daemon.startup` | `on_demand` starts a daemon during `aw run` when none is running; `external` requires an existing daemon with the same configuration revision. Validation starts neither |
| `daemon.endpoint`, `daemon.state_dir` | Required nonempty strings; the native launcher applies explicit absolute paths or resolves `auto` as described below |
| `execution.guarantee` | Only `native_hook`; no OS, final or protected guarantee |
| `execution.default_event_budget_ms` | Required positive integer, reserved for structured event execution; the native runtime does not apply a shared AW event deadline |
| `audit.enabled`, `audit.payload` | Required `true` and `metadata_only`; the daemon appends invocation metadata to `audit.jsonl` beside its socket |
| `agents.<id>.adapter` | `qwenpaw`, `qoder`, `openclaw` or `hermes`; recognition is not runtime certification |
| `agents.<id>.argv` | Nonempty executable/argument array; first element must be nonempty; no implicit shell or interpolation |
| `providers.<id>.protocol` | `native-hook/v1alpha1` for native commands; `aw-provider/v1alpha1` for the separate, unimplemented structured Provider runtime |
| `providers.<id>.transport` | `{type: stdio, location: agent, argv: [...], env: {...}}`; `env` is optional. Native execution runs one fixed argv per callback at its working directory |
| `providers.<id>.transport.env` | Literal string overrides of the callback environment; no interpolation. At most 128 keys matching `[A-Za-z_][A-Za-z0-9_]*`, each value at most 4096 characters and without NUL |
| `providers.<id>.timeout_ms` | Positive per-invocation ceiling; native admission caps it at 300,000 ms and applies the host limits below |
| `providers.<id>.max_output_bytes` | Positive output ceiling; native execution limits stdout and stderr independently to this value, at most 4 MiB each |
| `providers.<id>.config` | Required object; native steps require `{}`. Structured Providers may retain opaque private JSON, including Unicode keys and finite decimals; private-schema validation remains unimplemented |
| `events.<name>.enabled` | Required boolean for a declared event; omitted events are disabled |
| `events.<name>.required` | Defaults to `false`; a disabled event cannot be required. Reserved for capability admission; native bindings do not gain additional enforcement from this flag |
| `events.<name>.budget_ms` | Optional structured override of the default event budget; a nested guard shares its parent's remaining budget. Rejected on enabled native events |
| `events.<name>.steps` | Required registration array; an empty array invokes no Provider. Native execution order and concurrency belong to the host |
| `events.tool.before.match.tools`, `events.tool.after.match.tools` | Optional nonempty selector array; omitted means all native tools; `['*']` cannot be mixed with exact selectors |
| `events.tool.before.guard` | Structured-only reference to a declared `security.violation` event, which must be enabled when before is enabled; unavailable with native steps |
| `steps[].id`, `steps[].enabled` | ID unique within its event; enabled defaults to `true` |
| `steps[].provider` | Declared Provider ID with a protocol matching the step kind |
| `steps[].native` | Native registration options: `{}` or the supported `priority`, `point` and `sequential` fields below |
| `steps[].operation` | Structured-only nonempty operation name; operation discovery remains unimplemented |
| `steps[].effects` | Structured-only nonempty unique effect list; requested upper bounds, not a permission grant |
| `steps[].on_error` | Structured-only `report`, `block` or `withhold_result`, constrained by event timing |

Agent/Provider IDs and step/operation names use the same syntax as
`metadata.name`. Positive schema limits are integers from 1 through 4,294,967,295;
native runtime limits can be lower.
There are at most 128 Agents, Providers or steps per event, and 128 arguments
per executable. Empty arguments after the executable are preserved. NUL bytes
are rejected in executable arguments and endpoint/directory strings.

Public objects reject unknown fields. Provider `config` alone accepts private
fields. Missing Provider references and duplicate step IDs are errors even in
disabled steps, so enabling a step does not uncover a hidden reference typo.
Defaults are documented behavior, not values inserted into the parsed document.

## Native hook runtime

The [native example](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.native.yaml)
uses `protocol: native-hook/v1alpha1`, `config: {}` and steps such as
`{id: inspect, provider: audit-command, native: {}}`. A native step accepts only
`id`, `provider`, optional `enabled`, and `native`. It cannot contain `operation`,
`effects`, `on_error`, `config` or `guard`; its event cannot declare a guard either.
Supply command settings through literal `transport.argv` and `transport.env`.
A shell runs only when explicitly named in argv.

Each native registration calls AW once. AW sends its stdin bytes to the configured
command and returns stdout, stderr and the exit status to the adapter. It does not
wrap the command in the structured Provider request/response protocol. QwenPaw's
Python adapter projects native objects into JSON because its middleware has no
native command-hook wire format; the other hosts supply their native hook payloads.
The host and adapter define what command output means, including block and ask.
For example, Hermes `approve` requests human approval; QwenPaw middleware rejects
explicit ask as unsupported. There is no shared AW approval UI; AW does not turn
an ask response into automatic approval.

The [runtime admission code](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/src/model.rs)
allows only enabled `tool.before` and `tool.after` events, native steps, and an
omitted tool selector or exactly `['*']`. Other enabled events, exact selectors,
event `budget_ms` and guards are rejected. Disabled entries can retain structured
configuration, subject to static validation. The same Provider cannot be registered
twice within an enabled event; name separate Provider instances instead.

AW preserves each host's callback ordering, short-circuit behavior and tool
parallelism. The step array supplies registrations, not a shared serial pipeline.
`execution.default_event_budget_ms` remains required by the schema but is unused
here; `required` does not change native enforcement or certify a hook's coverage.
AW enforces each command's timeout and output limits independently. Host deadlines
and other native hooks can further constrain the time available.

| Native option | Implemented binding |
| --- | --- |
| `native.priority` | Integer from -10,000 to 10,000; supported by QwenPaw middleware and OpenClaw hooks, rejected for Qoder and Hermes. The host determines priority direction and after-hook ordering |
| `native.point` | OpenClaw `tool.after` only: `tool_result_persist` or `agent_tool_result`. Omission selects the ordinary after hook. `agent_tool_result` uses registration order and rejects `priority` |
| `native.sequential` | Qoder hook-group setting only. Omit it to preserve the native default; other adapters reject it |

Native stdin is bounded to 4 MiB. `max_output_bytes` is at most 4 MiB for each
output stream. `timeout_ms` is at most 300,000; host-specific admission is stricter:

| Adapter | Provider timeout limit |
| --- | --- |
| Qoder | At most 300,000 ms; the generated native hook timeout is `ceil(timeout_ms / 1000) + 2` seconds |
| OpenClaw | At most 12,000 ms, within the adapter's 14,000 ms bridge timeout |
| QwenPaw | At most 58,000 ms, within the adapter's 60-second bridge timeout |
| Hermes | At most 298,000 ms; `ceil(timeout_ms / 1000) + 2` must also fit the base configuration's `plugins.hook_callback_timeout` (default 30 seconds, 0 disables that host limit) |

Hermes can apply its callback timeout to the complete native callback chain.
Passing the per-command admission check does not guarantee that multiple commands
and existing native callbacks fit that shared host window. Runtime timeout and
output-limit failures remain visible; the host decides their tool-level outcome.

## Native daemon configuration

For `aw run`, `--state-dir` overrides `daemon.state_dir`. With `auto`, AW uses
`$XDG_RUNTIME_DIR/aw-native-<revision-prefix>`; without that environment variable,
provide an explicit absolute directory. `--socket` overrides `daemon.endpoint`.
With endpoint `auto`, the socket is `<state-dir>/aw.sock`; an explicit endpoint
must be an absolute Unix socket path, optionally prefixed with `unix://`.
`aw serve` takes its socket from the required `--socket` argument.

`on_demand` starts a daemon if none is available; `external` requires one already
running. An existing daemon must serve the same configuration revision. The
launcher holds a PID/start-time lease while the Agent runs and releases it at exit;
the daemon prunes dead owners. Idle shutdown requires no live leases or callbacks.
The on-demand idle timeout is 300 seconds; explicit `aw serve --idle-timeout`
accepts 1 through 86,400 seconds. The socket is private to the current user.
Invocation audit records contain metadata, byte counts and status, not payloads.

The [launcher](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/src/launch.rs)
prepares temporary native bindings. `--native-config` supplies a base host
configuration; Hermes and OpenClaw require it. Existing hook registrations in that
base are preserved. Native hook consent and host permission behavior still apply.
These lifecycle and binding features belong to this lab branch; they do not imply
a production deployment or an available `anolisa install aw` package.

## Structured schema events, effects and tool selection

The schema recognizes these 16 names. The structured `aw-provider/v1alpha1`
execution model below remains unimplemented; only the two tool events are admitted
by the native runtime described above.

| Event | Meaning |
| --- | --- |
| `session.start` | Session creation, load or restore |
| `input.submit` | Input reaches a native submission point |
| `tool.before` | Tool intent before native execution |
| `tool.after` | A native tool completion, including reported failures |
| `permission.request` | The host requests a permission decision |
| `compact.before` | Before context compaction |
| `compact.after` | Native compaction result |
| `subagent.start` | Native subagent startup |
| `subagent.stop` | Native subagent stopping point |
| `turn.stop` | Task stop check, not proof of success |
| `session.end` | Native session end |
| `model.before_request` | Model request at a verified sending boundary |
| `runtime.observed` | Trusted runtime registration |
| `runtime.exited` | Trusted root runtime exit observation |
| `security.violation` | Provisional name for AW's active, final internal tool-before check |
| `coverage.changed` | Change in observed integration coverage |

The structured contract defines `security.violation` only through the guard of
an enabled `tool.before`. It inspects the final candidate and permits
`observe`/`block`, without changing parameters. It is not a second native Hook or
a promise to run after every
third-party Hook. Parameter changes after the check require another check at
the actual enforcement boundary.

For structured steps, `tool.before` permits `observe`, `block`, `replace_input`.
`tool.after` permits `observe`, `replace_result`. Other events are observation-only
in this revision.
`ask` is reserved for before steps; an active step requesting it is rejected.
An explicitly disabled before step can retain `ask` for future editing, without
acquiring approval capability. Native host approval is unaffected.

`on_error: block` is valid only before execution (`tool.before` or the guard).
`withhold_result` is valid only after a tool. `report` records a failure and
continues. Withholding requires a verified model-consumption boundary; replacing
a history entry is insufficient. Required redaction must not use `report`.
The service must enforce these requirements at admission and execution.

Selectors are `*`, `bash`, `file_read`, `file_write`, or
`native:<adapter>:<exact-name>` for any of the four adapter IDs. Native selectors
are host-specific, not portable tool semantics. There are no regex/glob selectors
other than the single `*`. All-tools routing includes native custom tools and
preserves their input; it does not make every Provider understand every tool.

`required: false` cannot authorize dropping an active control effect or its
failure action. Future runtime admission must check every enabled step against
Provider declarations, implementation and native capabilities, and reject
unsupported required controls. Optional unavailable observation sources must be
reported explicitly. Parsing alone does not perform that admission.

## Parsing and compatibility

The parser accepts one UTF-8 YAML or JSON document, bounded to 4 MiB before and
after expansion and nesting depth 32. Duplicate keys, non-string mapping keys,
custom YAML tags, merge keys, non-finite numbers and multiple documents fail.
Ordinary aliases are expanded within those limits. Diagnostics include field
paths or source locations and constraints without echoing field values.

This alpha configuration is separate from existing capability wire records and
their Schema IDs/digests. Do not pass Provider configuration through the
integer-only wire canonicalizer. `aw validate` does not install or change native
files. `aw run` does create temporary host bindings and may start a daemon; those
runtime side effects are separate from parsing and static validation.
