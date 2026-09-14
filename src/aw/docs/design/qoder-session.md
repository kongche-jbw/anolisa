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
claims. The standalone session.py entry remains one prompt; conversation.py
adds the bounded sequence below. Interactive PTYs, panes, Herdr UI, Codex
launch and COSH's own tool dispatcher remain outside these entrypoints.

## Bounded conversation continuation

`conversation.py` adds sequential native resume/reset around the same one-prompt
operation. It owns one `Processes` scope for up to eight turns. The existing
launch operation borrows that owner; it does not create another supervisor or
reset cancellation between turns. No Core, Host, adapter or Rust dependency
changes are needed.

Conversation UUID supplies runtime/environment identity; generation increments
for each newly launched Agent. Each prompt has a fresh turn UUID. Native session
UUID persists across successful `--resume` turns and changes on explicit reset.
Each bootstrap records its own PID/start ticks before exec, and writes immutable
hook settings in a separate turn directory. A prior turn's hook cannot pass
ancestry checks in a new process, even when both share the native session UUID.
Configuration generation remains distinct from process incarnation.

Resume only refers to the native session established earlier in this invocation.
The runner requires nonempty bounded regular JSONL history before continuing,
without duplicating the fixed tool/result history grammar. It passes `--resume`
without `--session-id`, which the pinned CLI otherwise rejects unless forking.
Availability does not prove semantic context restoration; native acceptance must
exercise that separately. Reset means a new process and new native session,
not controlling `/clear` inside a persistent Agent.

A turn returns only after child cleanup and bounded observation operations.
Errors/nonzero status stop the sequence, including a cancellation that arrives
between turns. A final metadata index records attempted turns, relative evidence
directories and process status. Its completion is independent of per-event AW
adoption. Earlier records stay under their original identities after reset;
there is no mutable current-pane file or late observer publishing into a new turn.
This ordering avoids a new callback-fencing state machine for this slice.

Tests use actual AW binaries with peers for resumed/reset identity, independently
adopted events, stale hook rejection, failures/cancellation and separate roots.
Real resumed Qoder history and context must be tested separately. Persistent
interactive ownership, simultaneous panes and Herdr projection remain separate
integration work; the shell-specific PTY API and Gateway ACP pipe supervisor are
not treated as generic native Agent terminal APIs.
