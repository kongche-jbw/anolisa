# Tokenless projection through native hooks

[中文版](tokenless-projection_zh.md)

`aw-tokenless-host` connects the existing AW projection contract to Tokenless
0.8.0's native Protocol v2. It directly depends on the repository's
`tokenless-protocol` crate for request and response validation. No compression
algorithm or alternative native protocol is implemented in AW.

## Call and evidence boundaries

The opt-in Qoder hook uses one pinned serial plan:

1. Capture the successful tool-result text with the existing adapter.
2. Run the existing SecCore content inspection against the original text.
3. If Core admits continuation, call Tokenless `compress` through the bounded
   process Host, using the same source digest, scope and plan.
4. Validate the native response attribution, operation and candidate; store
   actual receipts and the terminal Core record in the existing FileJournal.
5. Prepare Qoder's `hookSpecificOutput.updatedToolOutput` only when Core permits
   continuation and the candidate is strictly smaller in UTF-8 bytes.
6. Write hook evidence before returning the replacement. Independent native
   history verification is a separate operation.

A receipt marked `produced` means a candidate exists. `delivery=prepared_for_return`
means the hook prepared a replacement, not that Qoder adopted it. Hook evidence
always begins with `adoption=not_observed`. The separate history verifier can
establish `local_history`; it does not establish the final model request.
Qoder's PostToolUse profile revision 2 advertises that observation boundary and
retains `result_finality=subject_to_later_change`. Codex's current observation
hook cannot replace results and rejects a Tokenless configuration before calls.

SecCore remains an observation: findings are still shown when a candidate is
returned. A required failed inspection skips Tokenless. This hook does not
replace existing security policy or OS enforcement.

## Recovery has deliberately weaker guarantees

Tokenless native `lossless` means no task-relevant information was removed;
for example, JSON cleanup may remove empty fields. AW's `lossless` requires
independent byte-exact source recovery. This adapter has no such decoder.

The hook explicitly opts into AW `unrecoverable`. Native `lossless` and
`unrecoverable` candidates therefore map to AW `unrecoverable`, `recovery:none`.
A content-free `tokenless_mapping` records the native claim, weaker AW guarantee,
reason and native-response digest. Its digest is linked from the receipt's
evidence. Callers that require only `lossless` or `retrievable` are rejected;
native retrievable results and stash references are rejected. No resolver or
recovery tool is invented.

This is an explicit information-loss policy, not a claim that compression is
byte-preserving. A future verified recovery integration must use its own pinned
profile revision. Schema files remain unchanged.

## Launch configuration

Keep the existing SecCore `provider` settings. Add a `tokenless` object with
exactly the same launch fields: `provider_id`, `provider_version`, absolute
`program`, `args` and `environment`, plus explicit `allow_unrecoverable:true`.
Missing or false consent rejects before Provider invocation; enabling a Provider
alone does not authorize information loss. Use the source-built Tokenless 0.8.0 executable,
`args:["compress"]`, and an isolated absolute `TOKENLESS_DATA_DIR`. The installed
0.7.0 executable lacks `compress`; older PoC native Protocol v1 is incompatible.
No fallback to either version is attempted.

The Host pins executable bytes and the complete effective launch configuration.
It forces `TOKENLESS_COMPRESSION_ENABLED=1`, `TOKENLESS_STATS_ENABLED=0` and
`TOKENLESS_SLS_ENABLED=0` to avoid independent compression counters. Native
Tokenless plugins must also be disabled in the selected Agent configuration;
this Host cannot discover or disable unrelated native hooks. The controlled
Qoder acceptance configuration uses only a dedicated project settings source.

Qoder scope still requires a launcher-owned, bounded single-turn identifier.
This is not an interactive multi-turn launcher. Neither the original source
content nor authentication is copied by the hook. Candidate content is present
in private evidence because it must be matched against native history.
`calls[].invocation` omits only `input.artifact.content`; the history verifier
reconstructs it from the captured native event and verifies its existing digest.

## Failure behavior and validation

No savings, dry run and native passthrough produce bypass receipts and no
replacement. Native errors, timeout, invalid attribution, changed executable,
invalid protocol and unsupported recovery produce visible failures. No candidate
or adoption savings are counted for these outcomes. The Host bounds request,
response, stderr and wall time, and terminates only its owned process group.
The transport follows the existing SecCore Host implementation; sharing the
transport as a standalone crate is deferred to avoid changing SecCore here.

From `src/aw`:

```bash
cargo test -p aw-tokenless-host -p aw-adapters -p aw-hook-cli --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

The Rust integration tests use synthetic native peers and exercise the actual
Adapter/Core/Host/Journal path. They are not real Agent acceptance evidence.
A bounded direct test of the source-built Tokenless 0.8.0 on Linux ARM64 reduced
80 synthetic JSON records from 12,134 to 6,134 bytes through `json_cleanup`.
Real Agent, history and Herdr acceptance is recorded separately with its exact
binary versions and session scope.
