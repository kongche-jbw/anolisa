# AW

[中文](README_zh.md)

AW defines versioned capability contracts between native agent adapters, a coordinating core and component providers. The root `aw-contracts` crate bundles JSON Schemas and offline semantic validators; the sibling `aw-core` crate executes pinned capability plans through a trusted Host port. Provider output stays separate from environment adoption, and runtime observation stays separate from control authority. Existing components remain independently usable.

**Status: Core implementation baseline 0.1.0; schema freeze review remains pending.** Core now implements whole-plan admission, serial execution, cancellation, receipt checks and a Linux file journal. It is an embeddable library, not a deployed service. The `aw-adapters` library adds shared native text capture and six host boundary profiles. An opt-in Qoder/Codex post-tool runner and a Linux SecCore content Host are available for integration testing. General Provider drivers, automatic plugin registration, process control and state providers remain separate integrations. Tests do not certify real Agent adoption or OS enforcement.

## Start from source

Prepare a Rust toolchain with rustfmt and Clippy. The cross-language digest test also needs Python 3 and Node.js; the Rust library itself does not depend on either runtime. Run these commands from the repository root:

```bash
cd src/aw
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

These checks run as a regular user without starting an Agent or signing into a service. Cargo downloads dependencies that are not cached; schema validation reads bundled resources and never fetches a schema URI.

This validation used Linux ARM64 with Rust 1.97.1, Python 3.12.3 and Node.js 24.15.0. These versions reproduce the checked environment. Minimum supported versions have not been established, and other operating systems have not been validated.

The optional Qoder hook now runs SecCore inspection and Tokenless projection in one Core plan. Tokenless 0.8.0 is pinned to its native protocol v2; unrecoverable replacement requires explicit consent. A separate observer verifies local-history adoption, and the unmodified Herdr v0.9.0 sidebar displays verified session evidence. This is a bounded integration baseline, not automatic COSH activation or model-request proof.

## Read and integrate

- [Tokenless projection and explicit consent](docs/design/tokenless-projection.md)
- [Pinned Herdr and evidence display](docs/design/herdr-integration.md)
- [Combined acceptance and reproduction](docs/design/tokenless-herdr-acceptance.md)
- [Qoder/Codex inspection runner and real SecCore Host](docs/design/qoder-codex-inspection.md)
- [Shared adapters: six host mappings, API and limits](docs/design/native-adapters.md)
- [Runnable native inspection example](crates/aw-adapters/examples/native_inspection.rs)
- [Core baseline: API, guarantees, limits and source provenance](docs/design/core-baseline.md)
- [Executable pinned-plan example](crates/aw-core/examples/pinned_plan.rs)
- [PoC baseline and interface delta](docs/design/poc-schema-delta.md)
- [Freeze decision and incremental acceptance gates](docs/design/interface-freeze.md)
- [Every schema: fields, reasoning and review boundaries](docs/design/schema-reference.md)
- [Schema resources](schemas/) and [complete synthetic examples](tests/fixtures/contracts.json)
- [Validation API](src/validation.rs) and [contract tests](tests/contracts.rs)

Callers parse untrusted wire bytes with `canonical::parse` and reuse a `Registry`. Core checks a pinned ordered plan with `validate_plan`, admits each invocation with both `validate_plan_invocation` and `validate_invocation`, and checks all settled steps and receipts with `validate_plan_execution`. Final adoption uses `validate_plan_adoption`; final dispatch uses `validate_dispatch` to check the whole plan, current execution intent and independent OS protection binding.

`validate_result`, `validate_adoption` and `validate_execution_gate` are local consistency checks, not complete plan admission. Shape-only `Registry::validate` is also insufficient for authorization. The contract crate neither schedules providers nor installs OS rules. Core schedules capability calls; native executors still authenticate evidence, serialize final checks with actions and prevent duplicate tool dispatch.

## Contract boundaries

The Registry bundles 21 schema resources (plus eight unregistered PoC v1 review resources kept separately): one shared definitions resource, eight capability input/output resources, and twelve orchestration/evidence resources. Capability v2 IDs preserve room for explicit migration from experimental v1 contracts. Exact schema ID and resource digest must match; consumers must not silently reinterpret another revision.

Text projection covers **one UTF-8 text slot**, preserving the surrounding native tool result and other blocks. A receipt records what a provider produced. Only an independently captured observation may establish adoption at `final_tool_result`, `local_history` or `model_request`; none of these proves remote model consumption. Ledger acknowledgement authenticity and durability remain storage responsibilities.

Effect operation records provide a generic approval/idempotency state machine. They do not define or execute checkpoint, restore or arbitrary state capabilities. Those payload profiles and recovery implementations require separate reviewed additions.

OS protection independently enforces ongoing system restrictions; it is not a scanner activated after plugin failure. `os-protection-binding` binds target, policy digest, required controls and coverage evidence. `validate_dispatch` rejects missing, expired or merely declared mandatory protection. Actual kernel restrictions, hook-bypassing access denial and child-process coverage require later native implementation and live validation.

## Meeting demo

From a fresh clone, run `python3 src/aw/scripts/demo.py setup`, then `doctor` to check readiness and `run --allow-unrecoverable` to display real Qoder/Herdr. Providers are discovered from `src/aw/providers/`; dependencies and evidence stay in `src/aw/target/demo/`. See the [demo guide](../../docs/user-guide/en/token-saving/aw-demo.md) for commands, prerequisites and cleanup.
