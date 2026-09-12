# AW

[中文版](README_zh.md)

AW provides versioned capability contracts, offline validation and embeddable Core orchestration. `aw-contracts` checks payload shapes and record relationships; `aw-core` executes pinned plans through caller-provided Hosts and journals execution facts. AW has no service process; native Agent control and final tool dispatch remain with the embedding application.

The interfaces are experimental. The default checks use synthetic records, frozen native outputs and real local protocol-peer processes; live Agent and native scanner acceptance is separate.

## Run the checks

Prepare Rust through rustup, Python 3 and Node.js. Rust, rustfmt and Clippy are
pinned in [rust-toolchain.toml](rust-toolchain.toml). Run from the repository root:

```bash
python3 src/aw/scripts/check.py
```

The entry runs CI behavior tests, formatting, Clippy, all locked workspace tests,
the Python/JavaScript digest vectors and rustdoc. Missing tools, empty or fully
ignored contract, plan, Core execution, journal, adapter profile/native/bridge,
SecCore pii, shared process, Tokenless projection/Core, native Host or hook CLI test targets, invalid vectors and command
failures return nonzero. Each command has a timeout and its child process group is cleaned up on
failure or interruption. Logs identify the failing command; individual commands
can be run from `src/aw` for diagnosis.

These checks run as a regular user without an Agent or service login. Cargo
downloads uncached dependencies; schema validation reads only bundled resources.
The runner requires Linux. Contracts and adapters remain portable; native Host
and hook execution require Linux. This gate does not certify other operating
systems or minimum supported versions.

[AW CI](../../.github/workflows/aw-ci.yml) runs on branch pushes, pull requests,
merge groups and manual dispatch. It checks the candidate commit, including the
merge result for pull requests. Unrelated changes produce an explicit no-op;
scope errors, unexpected skips and mismatched tested commits fail `AW / required`.
Repository administrators must select that check in branch protection to enforce
it. A cancelled workflow is not a passing gate.

CI uses Ubuntu 24.04 x86_64, Python 3.12.3 and Node.js 24.15.0. Local validation
also uses Linux ARM64 with those runtime versions and the pinned Rust toolchain.

## Core embedding

`aw-core` provides `Core::prepare` and `Core::execute`, trusted Host/Clock/Journal
ports, and a durable Linux `FileJournal`. Preparation checks the complete plan
before any provider call. Execution records each call before dispatch and returns
terminal results only after the journal acknowledges them. Failed or interrupted
events remain reserved; there is no automatic retry or recovery.

See [Core execution and storage](docs/design/core-execution.md) for ownership,
cancellation, failure and embedding contracts. The tests use synthetic Hosts;
this crate does not connect a production Provider or establish native adoption.
The shared check enforces the eight-crate dependency boundaries and a 700-line Rust
file limit (600-line warning); the existing contract validator remains capped at
711 lines. These checks supplement review, not runtime acceptance.

## Native adapter embedding

`aw-adapters` captures supported tool text for six pinned native profiles and
cross-checks observable IDs against caller-authenticated runtime context. It
bridges post-tool content/code inspection and admitted projection through the same Core and preserves
the original payload. Profiles are validated once per Adapter instance.

See [Native adapters](docs/design/native-adapters.md) for supported slots and
ownership. This library installs no hooks, delivers no replacement and grants
no final dispatch authority. Pre-tool capture is supported; pre-tool execution
requires a separately verified final guard and is rejected by these profiles.
The six mappings are covered by synthetic tests, not six live Agent integrations.

## SecCore protocol mapping

`aw-sec-core` maps post-tool content inspection to the supported native `scan-pii` protocol, preserving user rules and middleware semantics while validating native results. It is independent of the security engine implementation language. The library does not spawn processes or author Host receipts. See [SecCore native protocol adapter](docs/design/sec-core-adapter.md) for the native CLI fixtures, regeneration and compatibility checks.

## Native Host and hook CLI

`aw-sec-host` invokes the admitted SecCore CLI with bounded stdin/stdout/stderr,
version and selected-file pins, cancellation and owned-process-group cleanup.
The `qoder` and `codex` commands compose `PostToolUse` capture, Core, this Host
and the existing Journal. They preserve the tool result and emit observation messages;
they do not grant security approval or prove native adoption.

Build the experimental Linux entrypoint from the repository root:

```bash
cargo build --manifest-path src/aw/Cargo.toml --locked -p aw-hook-cli
src/aw/target/debug/aw-hook-cli --help
```

A trusted launcher supplies private settings and live Agent identity. Qoder is
limited to an explicitly bound single turn. No hooks are installed automatically,
and existing security plugins remain necessary. See the [CLI and configuration
reference](../../docs/user-guide/en/user-entrypoint/aw.md) and [Host architecture](docs/design/sec-core-host.md).

## Tokenless projection embedding

`aw-tokenless-host` uses the admitted native Tokenless CLI to prepare projection
candidates through Core. It shares the bounded `aw-host-process` runner with
SecCore and imports only the native pure protocol crate. Failed or unhelpful
projection preserves the source when the plan selects that policy. Candidates
require explicit acceptance of `unrecoverable`; generation does not prove
adoption or model savings. See [Tokenless Host](docs/design/tokenless-host.md)
for the profile, configuration and explicit native acceptance checks.

## Qoder projection and adoption queries

`aw-hook-cli qoder-project` composes required SecCore inspection and optional
Tokenless projection, then returns a durably recorded candidate through Qoder's
replacement slot. Private settings explicitly select source/candidate retention,
an exact history path and the `qoder-cli-1.1.47/jsonl-v1` history profile.
`aw-adoption-cli observe` records one independent history observation; `query`
only revalidates retained evidence. Generation, stdout delivery and recorded
history adoption remain separate. Missing evidence gives no savings figure;
verified byte differences are not model Token counts or billing savings.

See the [configuration and commands](../../docs/user-guide/en/user-entrypoint/aw.md#qoder-projection-and-history-observation)
and [adoption design](docs/design/qoder-adoption.md). This opt-in path is covered
by protocol peers and synthetic history fixtures; real Qoder hook acceptance
and its private history format require explicit version-specific validation.

## Source reference

- [Registered schemas](schemas/) and [synthetic payload examples](tests/fixtures/contracts.json)
- [Public API](src/lib.rs), [record validation](src/validation.rs) and [plan validation](src/orchestration.rs)
- [Encoding tests](tests/canonical.rs), [schema tests](tests/schemas.rs),
  [record tests](tests/contracts.rs) and [plan tests](tests/orchestration.rs)

The Registry includes 21 schema resources. The eight v1 resources in `crates/aw-contracts/schemas/` are reference copies and are not registered. Callers must use matching schema IDs and digests; no automatic version conversion is provided.

Parse incoming bytes with `canonical::parse` before schema validation. Shape checks alone do not validate record relationships or grant authorization. Follow the public API documentation for plan-level checks; callers remain responsible for authenticating evidence and enforcing actions.
