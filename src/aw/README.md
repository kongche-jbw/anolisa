# AW

[中文版](README_zh.md)

AW provides unified configuration, native Agent Hook execution and contract validators. This experimental fork adds a Linux daemon and `aw` launcher for QwenPaw, Qoder CLI, OpenClaw and Hermes. Native command callbacks preserve host scheduling; common Providers now support before-tool observation/blocking and after-tool observation.

Build from `src/aw` with `cargo build --locked -p aw-cli`, then use
`target/debug/aw plan qoder --config crates/aw-cli/examples/aw.native.yaml`.
There is no packaged installation yet. See the user guide for native profiles,
real-model evidence and explicit gaps. Qoder print/TUI after adoption is verified;
interactive approval remains unverified.

## Run the checks

Prepare Rust through rustup, Python 3 and Node.js. Rust, rustfmt and Clippy are
pinned in [rust-toolchain.toml](rust-toolchain.toml). Run from the repository root:

```bash
python3 src/aw/scripts/check.py
```

The entry runs CI behavior tests, formatting, Clippy, all locked workspace tests,
the Python/JavaScript digest vectors and rustdoc. Missing tools, empty or fully
ignored contract/plan test targets, invalid vectors and command failures return
nonzero. Each command has a timeout and its child process group is cleaned up on
failure or interruption. Logs identify the failing command; individual commands
can be run from `src/aw` for diagnosis.

These checks run as a regular user without an Agent or service login. Cargo
downloads uncached dependencies; schema validation reads only bundled resources.
The runner requires Linux. The library remains portable, but this gate does not
certify other operating systems or minimum supported versions.

[AW CI](../../.github/workflows/aw-ci.yml) runs on branch pushes, pull requests,
merge groups and manual dispatch. It checks the candidate commit, including the
merge result for pull requests. Unrelated changes produce an explicit no-op;
scope errors, unexpected skips and mismatched tested commits fail `AW / required`.
Repository administrators must select that check in branch protection to enforce
it. A cancelled workflow is not a passing gate.

Upstream CI uses the self-hosted `anolisa-k8s-general-ci-x64` runner; fork CI
uses GitHub-hosted Ubuntu 24.04. Both use Python 3.12.3, Node.js 24.15.0 and
the pinned Rust toolchain. Local validation also uses Linux ARM64.

## Source reference

- [User guide and availability](../../docs/user-guide/en/user-entrypoint/aw.md),
  [configuration reference](../../docs/developer-guide/en/aw/configuration.md),
  [starter configuration](crates/aw-config/examples/aw.minimal.yaml),
  [full example](crates/aw-config/examples/aw.yaml) and
  [configuration API](crates/aw-config/src/lib.rs)
- [Registered schemas](schemas/) and [synthetic payload examples](tests/fixtures/contracts.json)
- [Public API](src/lib.rs), [record validation](src/validation.rs) and [plan validation](src/orchestration.rs)
- [Encoding tests](tests/canonical.rs), [schema tests](tests/schemas.rs),
  [record tests](tests/contracts.rs) and [plan tests](tests/orchestration.rs)

The Registry includes 21 schema resources. The eight v1 resources in `crates/aw-contracts/schemas/` are reference copies and are not registered. Callers must use matching schema IDs and digests; no automatic version conversion is provided.

Parse incoming wire records with `canonical::parse` before schema validation. Shape checks alone do not validate record relationships or grant authorization. Follow the public API documentation for plan-level checks; callers remain responsible for authenticating evidence and enforcing actions.

User configuration uses the separate `aw-config` crate and its bundled
`aw/v1alpha1` schema. It accepts one `AWConfiguration` object with
`apiVersion`, `kind`, `metadata` and `spec`; Provider instances are named objects
under `spec.providers`. The schema recognizes QwenPaw, Qoder CLI, OpenClaw,
Hermes and all 16 event names, with native-mode adapters in this fork.
Configuration has no runtime `status`. `aw check AGENT --config FILE` executes
Provider discovery and private configuration validation; `aw run` performs it
before launch. See the [common Provider protocol](docs/design/provider-protocol.md)
and [runnable common example](crates/aw-cli/examples/aw.provider.yaml); replace its
absolute Provider path before checking. Native admission checks registration shapes
and limits.
See the [native integration design](docs/design/native-hooks.md) and
[native example](crates/aw-cli/examples/aw.native.yaml).
See the [configuration design](docs/design/configuration.md) for the separation
from the existing wire contracts.
