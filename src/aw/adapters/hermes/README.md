# Hermes native shell hooks

[中文版](README_zh.md)

Hermes runs AW directly through its native shell-hook configuration. The
tested official `NousResearch/hermes-agent` commit is
`952c941e741e922a9be8fc403c8944c6e96318bb`, using its frozen Python 3.14
environment. No Python plugin or replacement tool scheduler is required.

Use an isolated `HERMES_HOME` and the structure in
`native-config.example.yaml`. Each native event contains an ordered list of
`command`, optional `matcher`, `timeout`, and `fail_closed`. Hermes executes
each callback serially in registration order and retains its own tool batch
parallel/sequential scheduling. Hook consent remains native; `--accept-hooks`
explicitly accepts authored test hooks.

Stdin is native JSON with `hook_event_name`, `tool_name`, `tool_input`,
`session_id`, `cwd`, `profile`, and `extra`. AW transports these bytes unchanged.
Before stdout accepts Hermes `action: block` with `message`, `action: modify`
with `args`, or `action: approve` with optional `message` and `rule_key`.
Exit 2 also blocks before execution. A block wins over any earlier approve.

`approve` means **ask the human**, not auto-allow. Without an interactive user
or native approval bridge, Hermes fails closed. The Python native gate and
shell response parser are covered by deterministic tests. Post hooks are
observers; their returns do not undo the tool or replace its result.

The isolated CLI supports `hermes chat --oneshot --max-turns 3 --run-budget 150
--provider custom --model qwen3.7-plus --accept-hooks --query-file PATH`.
Supply the model key only in the child environment, with `OPENAI_BASE_URL`
pointing to the authorized compatible endpoint. Do not copy production
Hermes configuration, channel settings, or auth files into a test home.

The native lab ran three real `qwen3.7-plus` Token Plan CLI cases through
`aw run`. Allow wrote `42`; block and ask created no file. Native post status
was `ok` for allow and `blocked` for both block and noninteractive ask. All
cases ran before A, before B, after A, after B: Hermes still invokes the later
before callback when an earlier callback returns block. Interactive human
approval acceptance was not tested.
