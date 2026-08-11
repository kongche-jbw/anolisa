# Tokenless Quick Start

[中文版](../../../zh/token-saving/tokenless/QUICKSTART.md)

Install Tokenless, produce one compressed response, connect it to an Agent,
and verify a before/after Token record in about three minutes. Tokenless works
in the background, so your prompts and normal Agent workflow do not change.

Savings vary by workload. Tool-heavy tasks usually show the clearest result;
short or conversation-only tasks may show little change.

## 1. Install Tokenless

Install the anolisa CLI first, then use it to install Tokenless:

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
anolisa --version
anolisa install tokenless
tokenless --version
```

If `anolisa --version` already succeeds, start with
`anolisa install tokenless`. The PATH update makes a fresh default
installation available in the current Shell; new login shells may already
include `~/.local/bin`.

## 2. See your first compressed result

Run a deterministic response-compression check before changing any Agent
configuration:

```bash
printf '%s\n' \
  '{"status":"ok","data":{"name":"demo","items":[1,2,3]},"debug":{"trace":"verbose"},"metadata":null}' \
  | tokenless compress-response

tokenless stats list --limit 1
tokenless stats summary
```

The first command returns valid JSON with `debug` and `metadata` omitted. The
statistics commands should then show one recent record whose estimated Token
count decreases from before to after. If the output is unchanged, retry with
the example exactly as shown; content without removable fields is returned
unchanged and is not recorded.

## 3. Connect Tokenless to your Agent

Scan for installed Agent frameworks:

```bash
anolisa adapter scan
```

Copy the enable command for the Agent you use:

| Agent | Enable command |
|-------|----------------|
| cosh / Copilot Shell | `anolisa adapter enable tokenless cosh` |
| OpenClaw | `anolisa adapter enable tokenless openclaw` |
| Hermes | `anolisa adapter enable tokenless hermes` |
| Qoder | `anolisa adapter enable tokenless qoder` |
| Claude Code | `anolisa adapter enable tokenless claude-code` |
| Codex | `anolisa adapter enable tokenless codex` |
| Qwen Code | `anolisa adapter enable tokenless qwencode` |

Enable only the Agent you intend to use, then check the receipt and component
health:

```bash
anolisa adapter status tokenless
anolisa doctor tokenless
```

Restart the Agent CLI or IDE so it loads the new adapter. For OpenClaw, restart
the Gateway explicitly:

```bash
openclaw gateway restart
```

If OpenClaw rejects the plugin during its security check, review the findings
and follow the [OpenClaw integration instructions](framework-integration.md#2-enable-one-adapter)
before retrying. OpenCode currently uses a separate lifecycle script; see
[Framework integration](framework-integration.md#opencode).

## 4. Run a real task and verify the saving

Start a new Agent session and run a tool-heavy task. For example:

> Run the full test suite for this repository and summarize only the failures.

You do not need to mention Tokenless in the prompt. After the Agent uses a
Shell, API, or another supported tool, run:

```bash
tokenless stats list --limit 5
tokenless stats summary
```

You are done when `anolisa adapter status tokenless` reports the adapter as
enabled and `stats list` contains a record whose estimated Token count
decreases from left to right.

To inspect exactly what changed in one record, copy its ID and run:

```bash
tokenless stats diff <record-id>
```

If no record appears, the content may not have passed through Tokenless or may
not have become shorter. See
[No statistics appear after setup](troubleshooting.md#no-statistics-appear-after-enabling-the-adapter).

Token counts are estimates for content processed by Tokenless, not a direct
measurement of the model bill. Statistics and diffs may contain original tool
content; avoid sharing their output when it contains sensitive data. See
[Measuring savings](measuring-savings.md) and
[Configuration and data privacy](configuration-and-privacy.md) for details.

## 5. Platform support

| Platform | anolisa CLI installation |
|----------|--------------------------|
| Linux x86_64/aarch64 | Supported |
| macOS Apple Silicon | Supported |
| macOS x86_64 | Not currently supported |
| Windows or Linux with musl, such as Alpine | Not currently supported |

This page covers installation with the anolisa CLI only. To build the
standalone CLI from source, see
[User manual · Build the standalone CLI from source](user-manual.md#build-the-standalone-cli-from-source).

## 6. Next steps

- [Framework integration](framework-integration.md): framework-specific activation and behavior
- [User manual](user-manual.md): behavior boundaries and documentation map
- [CLI reference](cli-reference.md): all subcommands and options
- [Measuring savings](measuring-savings.md): statistics, dual runs, and AgentSight/SLS
- [Configuration and data privacy](configuration-and-privacy.md): toggles, storage, and sensitive data
- [Troubleshooting](troubleshooting.md): common errors, upgrades, and uninstall
