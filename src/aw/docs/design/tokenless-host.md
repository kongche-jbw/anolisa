# Tokenless projection Host

[中文版](tokenless-host_zh.md)

`aw-tokenless-host` provides an embeddable Linux Provider for
`context.projection.prepare/v2`. It runs the existing Tokenless CLI and returns
a validated candidate and receipt. Native hooks, result replacement, adoption
verification and billing measurements remain separate integration work.

## Ownership

The path is Core → Tokenless Host → native `compress` → candidate/receipt →
Core journal. Contracts and Core do not depend on Tokenless. The Host imports
only Tokenless's existing pure `tokenless-protocol` crate for wire types and
estimate consistency; compression engines remain in the native executable.

The second native Host shares `aw-host-process` with SecCore. That crate owns
the existing launch `Config`, pins, stream limits, cancellation and process-group
cleanup. Concrete Hosts retain their version admission, protocol mapping,
descriptors and receipts. SecCore reexports the same configuration types and
preserves its serialized configuration and manifest digest. No generic Provider
framework or second supervisor is introduced.

The AW gate enforces eight workspace members, the exact external protocol path
and its pure dependency boundary. Changes to the protocol crate or Tokenless
workspace manifest select AW CI. Native engine changes require the explicit
acceptance check below; fixture replay alone cannot validate a changed engine.

## Admission and native profile

`TokenlessHost::new` accepts the same operator-owned configuration shape as
SecCore; see [launch provenance and process lifecycle](sec-core-host.md).
`new_cancellable` additionally shares caller-owned cancellation with version
admission and every invocation. The constructor requires an independently
supplied executable SHA-256 and exact version response `tokenless 0.8.1\n`.
Its manifest binds that configuration, native version, protocol profile and
capability. The Provider advertises `authority: advise`, `guarantee: declared`
and only `post_tool`.

The complete child environment must explicitly include:

```text
TOKENLESS_STATS_ENABLED=0
TOKENLESS_SLS_ENABLED=0
TOKENLESS_COMPRESSION_ENABLED=1
```

Missing or conflicting values fail admission. These three native controls avoid
loading the default user configuration and disable statistics and SLS writes.
The Host supplies protocol 2 `post_tool` to `tokenless compress` on stdin, with
`result_kind: tool`, `status: success`, `output_optimization: none`, output
replacement available and `recovery.kind: none`. That profile requires no stash.
No tool command is executed by this request. Full executable and selected-file
SHA checks consume the invocation budget; size it for the deployed binary.

The embedding application must first establish successful tool completion:
the AW artifact carries origin but no exit status. Admission requires
`origin: command_output`, `media_type: text/plain`, exact source content/digest,
tool name, session/tool-use identity and explicit acceptance of
`unrecoverable`. Unsupported origins and recovery requirements fail visibly;
the Host does not infer an API response from an unknown origin.

## Candidate semantics and failure handling

Native `lossless` means that task-relevant information was retained. It does
not establish byte-exact recovery of AW's source artifact. This profile therefore
maps eligible native candidates to AW `unrecoverable` with `recovery.mode: none`.
A future lossless/retrievable profile needs an actual decoder or authorized
resolver and recovery tests before it can claim those guarantees.

The mapping checks protocol version, operation, attribution, native disposition,
recovery, transform operations, token-estimate consistency and actual UTF-8
reduction. It rejects malformed or contradictory native output. A candidate
binds the source artifact ID and digest, preserves text media semantics, and
records native transformation names. Final `Registry::validate_result` checks
the receipt and output together. Only observed source and candidate byte lengths
are metered; native estimates are not model billing or verified savings.

This fixed command-output/no-recovery profile admits `terminal_cleanup` and
`json_cleanup`; `toon` and `tabular_compaction` also require explicit text
reencoding permission. Operations from other lifecycle/origin routes and lossy
reduction paths are rejected. Native `Grep` command output has no applicable
compression route, so an applied claim for it is also invalid.

| Native outcome | AW result |
|---|---|
| Valid applied candidate accepted by the declared constraints | `produced`, bound candidate and receipt |
| Passthrough/no savings/recovery unavailable with exact original output | `bypassed`, no replacement output |
| Native error, timeout, malformed claims, pin drift or exhausted budget | `failed`, no replacement output |
| Invalid invocation or Provider binding | Host error before native compression |

Plans can use `on_failure: preserve` for optional projection. Preserving the
source does not mean it passed security inspection. A required inspection step
with `reject_plan` failure handling must precede projection when the application
requires that policy. Core stops before compression on that step's failure;
the Host does not insert policy or switch to another Provider.

Process limits and cleanup retain the SecCore Host's documented bounds and
limitations, including selected-file TOCTOU, escaped process groups and
uncatchable signals. This profile adds no sandbox or final dispatch authority.
Journal records bind execution metadata; the embedding owner still decides
whether to retain raw source/candidate text and whether a candidate is delivered.

## Validation and native acceptance

Normal `scripts/check.py` runs shared-process/SecCore regressions, real local
protocol-peer tests, frozen native cases, Core failure/order/journal tests and
the oracle's failure-detection self-tests. It does not compile a second copy of
Tokenless's engines or run a real Agent.

The reviewed [native vectors](../../tests/fixtures/tokenless-native.json) include
JSON cleanup, short text, Unicode/newlines and no savings. To validate a native
build explicitly, run from the repository root:

```bash
cargo build --locked --manifest-path src/tokenless/Cargo.toml -p tokenless-cli
python3 -B src/aw/tests/tokenless_oracle.py --binary "$PWD/src/tokenless/target/debug/tokenless"
AW_TOKENLESS_NATIVE_BINARY="$PWD/src/tokenless/target/debug/tokenless" \
  cargo test --locked --manifest-path src/aw/Cargo.toml -p aw-tokenless-host \
  --test projection frozen_real_tokenless_responses_map_without_a_runtime_dependency -- --exact
```

The oracle compares actual native exit status and complete JSON with every
frozen case and checks for persistent native data. The explicitly configured
Rust test then sends those inputs through the real Host, including admission,
pins and receipts. Without the environment variable, that test uses a local
protocol peer. Both paths keep temporary data under AW's build directory and
remove their owned resources. Neither modifies HOME or Agent configuration.

For a native upgrade, review changed native behavior and the mapping together,
update the fixed profile/version and frozen cases deliberately, and rerun both
checks. Version text alone is insufficient compatibility evidence. Candidate
generation still does not establish host adoption or measured model savings.
