# Common tool Provider protocol

[中文版](provider-protocol_zh.md)

This fork implements an experimental `aw-provider/v1alpha1` path for all four
native adapters. A Provider uses one request/response format. AW maps native
inputs and translates admitted effects; the host still schedules callbacks.
The existing `native-hook/v1alpha1` path continues to pass through native bytes
and exit status. This slice does not integrate the pending Core/Journal work.

## Transport and admission

AW starts the configured literal `transport.argv` for each method. The child
receives one UTF-8 JSON object on stdin followed by EOF and returns exactly one
JSON object on stdout. Diagnostics belong on stderr. Exit zero is required even
for a policy block; nonzero means protocol execution failed. Providers can be
scripts, binaries or CLI clients of another service. Their dependencies are
installed separately; AW does not infer or install them.

`aw validate` checks only configuration syntax and references. `aw plan` adds
adapter/effect admission without executing Providers. `aw check AGENT` executes
`describe` and `validate_config` for enabled structured steps, with a 30-second
aggregate admission deadline. `aw run` does that check before creating bindings
or launching the Agent. Configuration admission is not installation or effect
adoption evidence.

Every structured callback repeats discovery and validation before `invoke`.
This deliberately simple implementation avoids a stale discovery cache. All
three subprocess calls share that Provider's `timeout_ms`; process cleanup time
consumes the same deadline before the next call. Cleanup can use the runner's
additional bounded grace. `max_output_bytes` applies to each subprocess output
stream. There are no automatic retries. This costs three processes per callback;
persistent Provider sessions and discovery caching remain later work.

Native scheduling does not provide an AW event-wide deadline. The existing
`default_event_budget_ms` remains reserved, and explicit event `budget_ms`, guards,
non-wildcard selectors, ask and rewrites are rejected by this runtime. Supported
structured effects are `observe` and `block` before a tool, and `observe` after it.
`on_error` is `block` or `report` before, and `report` after. A Provider's descriptor
must include the configured operation, event and effects; it cannot expand the
adapter's admitted capabilities.

## Messages

All requests contain `api_version`, `method` and a fresh `request_id`. Successful
responses contain the same version and ID and `status: "ok"`. Unknown response
fields, duplicate typed fields, trailing JSON and mismatched identities fail
the invocation. Optional fields below must not be used to smuggle native effects.

```json
{"api_version":"aw-provider/v1alpha1","method":"describe","request_id":"example-1"}
```

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-1","status":"ok","operations":[{"name":"check","events":["tool.before"],"effects":["observe","block"]},{"name":"record","events":["tool.after"],"effects":["observe"]}]}
```

Operation names must be nonempty and unique, with at most 64 operations. The
Provider owns its private `config` schema. Validation must reject unsupported or
invalid fields; AW does not reinterpret that object or use it as native settings.

```json
{"api_version":"aw-provider/v1alpha1","method":"validate_config","request_id":"example-2","config":{"blocked_substrings":["AW_DENY_FIXTURE"]}}
```

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-2","status":"ok"}
```

Invocation carries the operation, private config, exact configuration revision,
remaining method budget, allowed effects, normalized event and an opaque digest:

```json
{
  "api_version": "aw-provider/v1alpha1",
  "method": "invoke",
  "request_id": "example-3",
  "operation": "check",
  "config_revision": "configuration-file-sha256",
  "budget_ms": 4500,
  "allowed_effects": ["observe", "block"],
  "input_digest": "sha256:opaque-request-binding",
  "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]},
  "event": {
    "name": "tool.before",
    "agent": {"adapter": "qoder", "binding_id": "qoder", "instance_id": null},
    "session_id": "native-session-id",
    "tool": {
      "name": "Bash",
      "native_name": "Bash",
      "call_id": "native-call-id",
      "input": {"command": "printf AW_DENY_FIXTURE"},
      "result": null
    },
    "native": {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "printf AW_DENY_FIXTURE"}}
  }
}
```

`binding_id` names the configured target, not a runtime session. Missing native
identifiers are null; no instance identity, semantic tool kind or provenance
guarantee is fabricated. Arbitrary tool names and JSON object arguments survive
normalization. After results keep their original JSON type, including Hermes's
serialized string and structured/multimodal objects. Error/status information
remains in `native`; a null result does not prove success. QwenPaw's JSON-encoded
argument string is parsed into `tool.input` and retained unchanged in `native`.

`config_revision` hashes the exact configuration file bytes. `input_digest` hashes
AW's compact serialized invocation before adding the digest field. It includes
the request ID, config, budget and event. Providers echo it unchanged; they need
not reproduce serialization. This is an opaque request binding, not the existing
`aw-contracts` canonical wire encoding and not an authenticated adoption receipt.
Private Unicode keys and JSON decimals are not restricted by that wire format.

```json
{"api_version":"aw-provider/v1alpha1","request_id":"example-3","status":"ok","input_digest":"sha256:opaque-request-binding","effects":[{"type":"block","reason_code":"policy_match"}]}
```

Effects are at most 64 objects containing `type` and optional `reason_code`.
Only configured effects are accepted. Reason codes, when present, contain 1–128
ASCII letters, digits, `_`, `-` or `.`. `effects: []` adds no restriction;
`{"type":"observe"}` reports an observation. Neither grants native permission.
Any admitted block yields the adapter's deny response. Returned Provider stderr
and reason codes are not copied into host output or audit; the host receives a
fixed AW diagnostic.

An error response uses the common header, `status: "error"` and `error_code`,
without effects. AW treats protocol errors, timeout, output overflow, nonzero
exit, invalid private config and mismatched request/digest as failure, then applies
the configured `on_error`. It records failure rather than a successful policy hit.

## Native mapping and failure boundaries

| Adapter | Neutral response | Block response |
| --- | --- | --- |
| Qoder | exit 0, `{}` | exit 2, fixed stderr reason |
| OpenClaw | exit 0, `{}` | exit 0, `block: true` with `blockReason` |
| Hermes | exit 0, `{}` | exit 0, `action: block` with `message` |
| QwenPaw | exit 0, `{}` | exit 2, adapter yields a DENIED ToolResponse |

Generated structured bindings carry the adapter and failure policy into
`aw hook --adapter ADAPTER --on-error block|report`. This lets the live client
translate daemon disconnects and input failures too. Passing a generic exit 125
would fail open in Qoder and some Hermes shell-Hook paths. Native mode keeps its
original exit handling.

This is not mandatory enforcement outside the callback. Qoder can continue if
the helper is missing, killed or timed out by the host. Hermes still requires
shell-Hook consent; `HERMES_SAFE_MODE` (`1/true/yes/on`) is rejected at launch because it disables
registration. OpenClaw must actually load the plugin. QwenPaw external-execution
tools bypass `on_acting`. Its App entrypoint loads external plugins, while
ACP/TUI do not and are rejected. App acceptance waits for native plugin-loaded
status; a listening port is not Hook readiness. Denied tools can still generate
after observations under native middleware nesting. Later native parameter changes can invalidate an earlier
check. `required: true` does not turn these paths into final/protected execution.

Audit entries add `protocol` and `disposition` (`native`, `observe`, `block`,
`error`) to existing invocation metadata. No event body, Provider output,
environment or argv is retained in AW audit. A returned block records AW's
response; framework evidence is still needed to prove adoption.

## Example and sec-core boundary

[aw.provider.yaml](../../crates/aw-cli/examples/aw.provider.yaml) uses the same
[sample Provider](../../examples/providers/policy.py) for all four targets.
The substring rule is an intentionally simple fixture, not a security policy
engine. Its optional `sec_core` object supplies an absolute CLI argv prefix,
`timeout_ms` and `block_verdicts` (default `warn`, `deny`). The sample recognizes
confirmed framework/tool pairs, extracts the literal shell command and invokes
`scan-code --code COMMAND --language bash --mode regex` without a shell.

sec-core `pass`, `warn` and `deny` can all exit zero; the wrapper reads its JSON
verdict. Scanner errors become Provider failures. Other tools do not acquire an
invented shell interpretation. The existing Python V1 scanner runs locally;
the Rust V2 CLI requires a separately managed sec-core daemon. This adapter does
not replace that service or implement the proposed custom Hook policy manager.
Rules, complete tool coverage and production security semantics remain joint
delivery work with sec-core.

Local joint acceptance used installed sec-core V1 0.8.0 with real Qoder: pass
allowed a harmless printf and produced after; warn on a quoted rule-triggering
string blocked the second printf with no after. This verifies the actual scanner
and host response path, not the repository V2 release or full safety coverage.
