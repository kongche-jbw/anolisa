# cosh-ng

[中文版](README_zh.md)

cosh-ng is an AI-native Linux terminal built around the shell you already use.
Start `cosh` to run bash or zsh as usual, then describe larger tasks in natural
language when you want the Agent to investigate or act. Shell commands, Skills,
approval cards, and resumable conversations stay in one terminal. Structured
JSON and JSONL interfaces are available for automation and Agent integration.

## Why cosh-ng

| In a conventional terminal | In cosh-ng |
|---|---|
| You translate intent into commands | Ask in natural language or run commands directly |
| Automation is scattered across scripts | Package repeatable workflows as Skills |
| AI context is tied to one chat window | Resume workspace-scoped Agent conversations |
| AI actions are hard to inspect | Review tool calls in approval cards and audit records |
| Every distro has different system commands | Use `cosh-cli` for stable, structured OS operations |

Interactive programs, pipes, redirects, job control, bash/zsh configuration,
and `Ctrl+C` continue to work in the foreground terminal.

## Install

Install with the ANOLISA CLI:

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

On Alibaba Cloud Linux, the RPM is an alternative:

```bash
sudo yum install cosh-ng
```

These packaged installation paths currently target Linux. On macOS, follow
the [developer setup](../../docs/developer-guide/en/cosh-ng/getting-started.md)
to build from source.

## Start in 30 seconds

```bash
cd your-project
cosh
```

Then mix shell commands and Agent requests in the same session:

```text
$ git status
$ explain why this service keeps restarting and show me the evidence
$ /agent
$ /skills list
$ /session status
```

Use `/auth` to choose a supported provider plan, `/help` to list current slash
commands, and `/mode approval recommend` when every Agent tool call should wait
for confirmation. Approval settings use `recommend`, `auto`, or `trust` across
the shell and Core. With the cosh-core runtime, `/agent` opens a one-shot
Composer that accepts a leading `/skill:<name>` and validated workspace-local
`@path` references.

To run one locally installed ACP adapter without entering the interactive
Shell, verify it first and then pipe the prompt through stdin:

```bash
cosh agent doctor --profile codex --workspace "$PWD"
printf '%s\n' 'summarize the current changes' | \
  cosh agent run --profile codex --workspace "$PWD"
```

The first release accepts only the built-in `codex` and `claude-code`
profiles. Install the corresponding `codex-acp` or `claude-agent-acp`
executable separately; COSH never invokes `npx` or downloads an adapter at
runtime. A permission callback prompts only on the local controlling terminal;
without one, or with `--permission deny`, COSH cancels it. Once-only decisions
are recorded as redacted evidence under the private local state directory.

The first durable local-control slice can run a Unix-only Gateway daemon and
manage Task state from another terminal:

```bash
cosh agent serve
printf '%s\n' 'inspect the failed service' | \
  cosh agent task submit --idempotency-key '<stable-retry-key>'
cosh agent task get '<tsk_UUID>'
cosh agent task events '<tsk_UUID>' --after 0 --limit 64
cosh agent task cancel '<tsk_UUID>' --run-id '<run_UUID>' \
  --idempotency-key '<stable-cancel-key>'
```

The daemon generates and persists its installation ID on first start; an
operator may provision one explicitly with `--installation-id`. Replace the
other typed identifiers with returned COSH IDs. This
local API authenticates the Unix peer and persists Task control state. It does
not yet schedule the ACP/Core Runtime, deliver Outbox work, recover a Run, or
open a remote listener; `task submit` is a control-plane operation, not proof
that an Agent executed the intent.

## Documentation

- [User guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/README.md)
- [Connect an MCP server](../../docs/user-guide/en/user-entrypoint/cosh-ng/mcp.md)
- [Interactive terminal](../../docs/user-guide/en/user-entrypoint/cosh-ng/shell/overview.md)
- [Configuration](../../docs/user-guide/en/user-entrypoint/cosh-ng/configuration.md)
- [Manage system operations](../../docs/user-guide/en/user-entrypoint/cosh-ng/cli/overview.md)
- [Headless integration](../../docs/user-guide/en/user-entrypoint/cosh-ng/core/headless-mode.md)
- [Developer getting started](../../docs/developer-guide/en/cosh-ng/getting-started.md)
- [Architecture](../../docs/developer-guide/en/cosh-ng/architecture.md)
- [Contributing](CONTRIBUTING.md)

## Contribute

Source builds are a contributor workflow. Start with the
[developer guide](../../docs/developer-guide/en/cosh-ng/getting-started.md).

## License

Apache-2.0
