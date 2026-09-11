# SecCore native protocol adapter

[中文版](sec-core-adapter_zh.md)

`aw-sec-core` maps post-tool `security.content.inspect/v2` requests to the
existing SecCore PII CLI protocol and validates native results. It has no
process, scanner or Core dependency. A Host can use this library without
depending on the language used to implement SecCore.

## Request and result boundary

`PiiRequest::new` validates the AW input and retains its exact text. `args()`
and `stdin()` describe this native invocation:

```text
agent-sec-cli scan-pii --stdin --format json --source tool_output
```

The request's confidence option adds `--include-low-confidence`. Input is never
placed in argv. The mapping does not override the user's rule configuration,
select fixed detectors, disable middleware, or request raw/redacted evidence.
The native command owns its normal configuration and security-event lifecycle.

`project(exit_code, stdout)` checks the native response before producing an AW
inspection. The reviewed profile is `agent-sec-cli scan-pii` at version 0.12.0;
the frozen fixtures record the exact native source revision. Version text alone
does not prove compatibility. Hosts must admit a supported artifact and
configuration, bind each response to its invocation, and reject protocol drift.

| Native result | AW result |
| --- | --- |
| Complete `pass` with no findings | `clean` |
| Complete `warn` | `suspicious`, warning findings mapped to medium severity |
| Complete `deny` | `sensitive`, denying findings mapped to high severity |
| Nonzero exit, failed scan, malformed or inconsistent output | Mapping error, no inspection |
| Partial scan or degraded custom-rule processing | Mapping error, no complete inspection claim |

Findings retain valid rule IDs and aggregate matching type/category/severity/
confidence groups. Native `custom` maps to AW `other`. The projection excludes
evidence snippets, spans and arbitrary metadata. Confidence maps to low below
0.5, medium below 0.8 and high otherwise. Output size and finding counts are
bounded; results are not silently trimmed to satisfy the schema.
The coverage ruleset IDs identify this reviewed protocol profile and, when
loaded, the native custom-rule file digest. The profile ID is not a computed
digest of built-in rules; artifact admission remains a Host responsibility.

The native output reports bytes scanned but has no input digest. The mapper
checks coverage against retained UTF-8 input and uses that validated request's
digest. It cannot authenticate a same-length substituted response. The Host
must establish the request/response binding. A pure mapping check also cannot
prove that the executable actually ran the advertised detectors.

Invalid custom rules, runtime errors, exhausted budgets and truncated custom
findings are visible mapping failures even when the native CLI continues with
built-in rules. This preserves the native CLI's behavior while preventing AW
from claiming a complete configured inspection. An absent custom-rule file is
a supported native configuration, not an error.

## Host and security responsibilities

The Host owns process or daemon transport, executable/configuration admission,
timeouts, output collection limits, child cleanup and AW receipt construction.
This library's output-size check does not bound a caller's earlier allocation.
No automatic fallback, retry, hook installation or final dispatch is provided.
Normal mapping/provider failures must become failed receipts; a Host transport
error applies when no trustworthy terminal receipt can be formed.

Content findings are observational. A completed inspection does not grant
execution permission, install an OS protection profile or prove durable audit
storage. SecCore retains native audit/trace ownership; AW Journal records
orchestration facts. Security-engine migration is owned by SecCore. Preserved
native behavior should pass the same adapter fixtures without changing Core;
a versioned interface change requires an explicit adapter compatibility review.

## Validation and native oracle

The regular AW check runs the crate's nonempty `pii` integration target, including
frozen outputs from the real native CLI. It requires no Python scanner at runtime
or during ordinary Rust tests. Two standard-library oracle self-tests also run
in the AW gate to verify escaped-text and forbidden-field audit checks.
Synthetic negative cases cover invalid records,
coverage, detector degradation and disclosure boundaries.

`crates/aw-sec-core/tests/regenerate_pii.py` replays the real CLI with the native
project's Python 3.11.6 and locked dependencies available. From the repository
root, run with that environment's Python:

```bash
python src/aw/crates/aw-sec-core/tests/regenerate_pii.py
```

It compares stable results, exercises built-in/custom rules and confidence
selection, and verifies one sanitized native security event per invocation.
Only the custom-rule file location is injected; HOME, middleware and scanners
are not replaced. Temporary data lives under AW `target` and is removed on exit.
`elapsed_ms` is normalized to zero. Use `--write` only when intentionally
regenerating reviewed fixtures; do not accept drift by refreshing expectations
without examining its compatibility impact. This oracle exercises the native
Python CLI entrypoint, not an installed binary or a complete Agent/Host session.
