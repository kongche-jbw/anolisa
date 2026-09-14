# One-prompt Qoder session ownership

[中文版](qoder-session_zh.md)

The launcher composes existing AW binaries around one native Qoder print-mode
prompt. It keeps the eight-crate graph unchanged and uses Python's standard
library only for Linux launch orchestration. It does not duplicate Core plans,
Provider engines, history parsing or receipt validation.

## Call and ownership boundaries

```text
private operator config -> version/pin + native protocol probe + coexistence
    -> launcher-owned cosh-shell child
        -> bootstrap records its own PID/start ticks -> exec Qoder -p
            -> native PostToolUse -> existing AW hook -> Core -> native Hosts
    -> wait/clean owned child tree -> existing observe -> retained result.json
```

cosh-shell's direct-command helper starts and waits for one child; it is not a
PTY supervisor. The external launcher owns that root and its descendant lifetime.
Bootstrap uses exec so the configured Agent identity remains the same process
incarnation. The hook validates ancestry and runtime/session/tool identity. One
launcher-generated UUID identifies this single prompt; it is not reused after
reset or for another turn. Extension configuration generations cannot replace
Agent incarnation checks. No observer owns Agent termination.

Native history is derived from the admitted Qoder1.1.47 profile and explicit
configuration root. The original hook validates the supplied transcript path,
cwd and session binding; the launcher never creates a synthetic history prefix.
Automatic observation occurs only after the Agent child tree exits. It calls
the existing observe operation once per context; later query validates the
stored snapshot without rerunning the Agent. Missing evidence is not adoption.

## Configuration and failure boundaries

An exclusive 0700 session directory contains 0600 config and identity snapshots.
Only launcher-created resources are removed after preparation failure. Once
launch is attempted, diagnostic state and explicitly retained projection
evidence remain. Native user configuration, credentials and plugins are not
copied or modified. Active hooks/plugins, globally disabled hooks and unsupported
settings-directory overrides are rejected; disabled plugin entries are retained.
This profile has not certified composition with active native plugins.

Native executable hashes are supplied by the operator, independently of discovery.
Runtime libraries/configuration beyond those selected pins remain the installation
owner's responsibility. Neither file modes nor ancestry checks isolate a hostile
same-user Agent. A passed synthetic PII probe does not attest detector completeness,
daemon health, final authorization or sandbox enforcement; native audit stays active.

`Processes` is restricted to a dedicated single-threaded caller without existing
children or alternate SIGCHLD reaping. It preserves the prior Linux subreaper
setting and signal handlers. A live, unreaped leader reserves the PGID through
the final group signal. Orphaned Provider children may have separate groups;
they are adopted and terminated/reaped by exact owned child PID. New adoptions
are drained under a finite deadline. No termination is reconstructed from a
persistent PID file, and unrelated processes are outside this ownership boundary.

All utility capture uses nonblocking bounded stdout/stderr pipes, with 64 KiB
per stream. Utilities and the Agent have finite command deadlines; cleanup has
one second TERM grace and three seconds kill/reap. Capture initialization and
spawn failures also release their resources. Kernel-stuck children or abrupt
launcher SIGKILL cannot promise complete cleanup and are not silently certified.

## Acceptance

The AW gate builds the existing CLI binaries, then runs nonempty process and
session suites. Synthetic Qoder/cosh peers exercise real AW protocol/identity,
projection and independent history paths without network or model calls. Tests
cover wrong session, missing scan capability, existing configuration, disabled
hooks, source overrides, setup failures, timeout/cancellation, early exit,
cross-group descendants, unrelated-process preservation and output floods.

Native Qoder, cosh-shell, SecCore and Tokenless acceptance must run separately
against explicit versions and final source. A successful history observation
only proves `local_history`; model-request adoption and billing are distinct
claims. This slice does not implement interactive PTYs, multiple turns, reset,
panes, Herdr UI, Codex launch or COSH's own tool dispatcher.
