# Startup dependency admission

[中文版](startup-preflight_zh.md)

Preflight admits explicit native dependencies before the launcher allocates
session resources. It deliberately has no Agent PID, runtime generation,
Journal or hook configuration. Dependency readiness is separate from a live
Agent's compatibility, execution admission and final dispatch authority.

## Composition and ownership

The existing `aw-hook-cli` crate owns the settings and CLI. `inspect` constructs
only `SecHost`; `project` constructs SecHost followed by TokenlessHost. The Hosts
retain their own version/profile constants, pin checks, environment controls and
shared bounded process runner. Errors retain the responsible component and
static Host diagnostic; native output is never serialized. Both native-process
CLIs share process-owned signal handling, while the library receives caller-owned
cancellation. No new crate, scheduler or package resolver is introduced.

Installed absolute paths come from the installation owner. Native versions and
operator Provider release identifiers remain separate. The preflight does not
enroll expected digests from whatever it finds on PATH or modify installation
files. A successful probe does not bypass execution-time pin/identity checks.

## Optional Herdr acquisition

The fetch command retains the reviewed unmodified v0.9.0 release pins. Binary and
license are downloaded and verified in one owned sibling staging directory, then
published as a complete bundle using Linux atomic no-replace rename. This avoids
the historical binary-first publication and existing-file chmod. Matching
existing bundles are checked without mutation; conflicts stay visible. The
commit point is directory publication: later interruption can leave a complete
bundle, never a partially published pair. SIGKILL/power loss cannot run staging
cleanup; this is not crash recovery or filesystem durability attestation.

The display bundle is independent of Provider admission. Neither operation
starts an Agent or installs hooks. The later launcher must resolve its shell
helper and Agent, bind real session/runtime identities and verify plugin
coexistence. Extension configuration generations cannot substitute for Agent
incarnations, reset boundaries or pane identities.

## Validation

The common gate runs preflight integration tests with real local version peers
and offline fetch tests. Coverage includes mode-dependent dependencies, explicit
version/pin failure, cancellation/timeout process reaping, no native-output
disclosure, artifact/license failure, non-replacement, collision and staging
cleanup. Native release retrieval is a separate explicit network acceptance;
passing tests does not certify an authenticated Agent or all architectures.

Readiness JSON uses the existing five-second, cancellation-aware stdout delivery;
a nonconsuming output pipe cannot keep preflight waiting indefinitely.
