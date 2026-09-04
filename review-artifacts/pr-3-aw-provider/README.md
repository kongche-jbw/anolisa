# PR 3 AW Provider Review Artifacts

[中文版](README_zh.md)

This directory explains the architecture introduced by
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3) and the
corrected baseline implemented on
[`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc).
The review is pinned to base `8574ecb022ec9ffc68e1a71e30f2186b6ec81674`
and head `42d07649409ecd5bb023056b28545efbd9325ef2`. The corrected
PoC and its final VM evidence are pinned to
`5ebfc0b3905fa2f5f74aff2da4aec2b3be639647`.

PR 3 establishes the right major boundaries: canonical capability contracts,
AW Core policy and planning, a generic Provider Host, component-native
providers, environment-owned final effects, and a content-free Ledger. The
original head does not close several cross-boundary invariants. The fork PoC
therefore serves as an implementation baseline, not as proof of production
readiness.

## Status model

| Label | Meaning |
| --- | --- |
| Original PR | Behaviour observed at the pinned PR head |
| PoC baseline | Behaviour implemented or exercised on the fork branch |
| Remaining | A contract or product decision that still needs work |

The PoC now makes the result path explicit. Provider Host returns a transient
candidate and a content-free receipt to AW Core. AW Core validates the result.
COSH owns the final local-history write, and AW Ledger records a typed
`context_adoption` fact after that write, referencing the complete
`post_tool_use_plan` derived from the Core result. A candidate that is empty or
not strictly lossless is not adopted; COSH preserves the source bytes.

Two boundaries remain deliberately open. PreTool command inspection is not yet
bound to the exact bytes eventually executed by COSH. Checkpoint uses the
Gateway State Provider with Guarded Checkpoint V2 and durable evidence; it is
not an AW manifest provider. Its Btrfs-backed Ubuntu VM and Herdr happy path
completed successfully, including approval, permit, execution, Guarded V2,
durable evidence, and `task_succeeded`. Fault-injected response loss and
evidence-only recovery after a process restart still require validation.

## Reports

- [Full architecture review in Chinese](architecture-review_zh.md)
- [Executive architecture brief in Chinese](executive-brief_zh.md)
- [Provider principles and integration guide in Chinese](component-integration_zh.md)
- [Runtime examples and checkpoint boundary in Chinese](runtime-call-examples_zh.md)
- [Complete schema atlas in Chinese](schema-reference_zh.md)
- [Ready-to-paste PR review comment in Chinese](pr-review-comment_zh.md)
- [Agent Host PoC comparison in Chinese](poc-comparison_zh.md)

## Interactive diagrams

- [Provider activation architecture](provider-effect-architecture.html)
- [Provider activation sequence](provider-effect-sequence.html)
- [Agent Sec command inspection sequence](security-command-call.html)
- [Checkpoint State Provider sequence](checkpoint-create-call.html)

The call diagrams use a bright Drafter blueprint style with compact real field
values. The architecture page is a self-contained Archify artifact. The 14
deterministic light-theme schema SVGs are stored under `images/schemas/`.

Build packaging and CI integration are intentionally outside this review
milestone. They remain required before production delivery.
