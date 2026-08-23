# Cosh Agent Memory

[中文版](../../zh/token-saving/cosh-agent-memory.md)

Agent Memory gives Cosh a private, durable record of useful tool evidence so a new session can recover relevant work without replaying a full transcript. The integration is automatic after installation and stays out of the way when memory is unavailable.

## Requirements

- Linux
- Cosh-ng for automatic lifecycle capture and recall
- A writable, owner-only local state directory

ManT is optional. Local capture, recall, and the verification demo work without it.

## Verify it in about 30 seconds after installation

Install Agent Memory first. Package download and installation time is not part
of the 30-second local verification.

```bash
anolisa install agent-memory
```

Then run these commands from a workspace where you use Cosh.

```bash
agent-memory-ctl doctor
agent-memory-ctl demo
agent-memory-ctl status
```

`doctor` verifies the private local store and confirms that the Cosh Hook executable is available on `PATH`. It also reports whether Cosh and the optional ManT command are present.

`demo` stores one synthetic, non-sensitive event, closes and reopens the durable backend, opens a second local session, and recalls that event. It does not run a shell command or print stored content. A successful run prints the cold-reopen time and a ContextView ID. Copy the final command it prints to explain the selection:

```bash
agent-memory-ctl why local-view-1
```

`status` then reports durable object counts, hard capacities, retention, logical and physical SQLite usage, and a bounded recent recall funnel. The funnel separates views that returned items from outcomes explicitly reported as useful. Synthetic `demo` views are reported separately and never count as real Agent hits. The command never prints the database path or stored memory content.

Start a new Cosh session after installation. Cosh discovers the packaged extension automatically, so there is no MCP configuration to copy for this integration.

## Other installation methods

On Alinux, install the RPM package.

```bash
sudo yum install agent-memory
```

Developers building from source can install the same binaries and Cosh extension with Make.

```bash
cd src/agent-memory
make build
make install
```

For a user-profile source install, make sure `$HOME/.local/bin` is on the `PATH` inherited by Cosh.

## What Cosh records

After a successful or failed tool call, the Hook records a bounded, redacted event summary with hashes and an opaque evidence reference. It does not store a full transcript through this path. It also does not treat model output from `AfterModel` or `Stop` as committed memory.

At `SessionStart`, Agent Memory may return recent Candidate evidence from the same local user, Agent, and workspace scope. At `UserPromptSubmit`, it selects Candidate evidence that overlaps the current prompt. Returned memory is bounded by item, byte, and token budgets, screened again for secrets and prompt injection, and wrapped as untrusted data before Cosh adds it to model context.

Cosh sends its fixed project boundary separately from the shell's current directory. Agent Memory normalizes paths inside a Git worktree to the canonical worktree root, so changing into a repository subdirectory does not split recall or prevent `why` and `forget` from finding the same view. A sibling worktree remains isolated.

Memory failures are fail-open for the interactive task. Cosh continues without recalled context if the backend is unavailable or exceeds its deadline.

## Command reference

| Command | Result | Exit behavior |
|---|---|---|
| `agent-memory-ctl doctor` | Checks the store, required Cosh Hook, Cosh runtime, and optional ManT protocol | Non-zero when a required check fails |
| `agent-memory-ctl demo` | Captures and recalls synthetic evidence after a cold backend reopen | Non-zero when capture, reopen, recall, or outcome recording fails |
| `agent-memory-ctl status` | Shows bytes, capacity, lifecycle, object counts, recent recall metrics, and excluded diagnostic samples | Non-zero when the local store cannot be inspected |
| `agent-memory-ctl why <view-id>` | Shows item IDs, ranks, admission reasons, degradation, and reported outcome without stored content | Non-zero when the view is absent or outside the current workspace |
| `agent-memory-ctl forget <kind> <id> --yes` | Deletes a task, event, or ContextView in the current workspace | Non-zero without confirmation or when no visible object was deleted |

Every command accepts `--json` after the subcommand.

```bash
agent-memory-ctl doctor --json
agent-memory-ctl demo --json
agent-memory-ctl status --json
agent-memory-ctl why local-view-1 --json
agent-memory-ctl forget context-view local-view-1 --yes --json
```

JSON output contains status, counters, and safe diagnostics. Errors go to stderr with an action and a non-zero exit code.

`forget` is destructive and therefore requires `--yes`. ContextView deletion also removes its admission and outcome records. Event IDs that are ambiguous across sessions are rejected rather than guessed.

## Common fixes

If `doctor` reports that the Hook is missing, reinstall Agent Memory and check the `PATH` inherited by Cosh. The executable name is `agent-memory-cosh-hook`.

If the local store check fails with an owner-only permission message, set its parent state directory to mode `0700` and retry. The CLI deliberately omits the database path; inspect your `XDG_STATE_HOME` configuration if you need to locate the directory.

If Cosh is not found, the local demo and status commands still work. Install cosh-ng and start a new session before expecting automatic lifecycle capture and recall.

If ManT is not found, no repair is required. ManT is an optional knowledge provider and is not a runtime dependency of local memory.
