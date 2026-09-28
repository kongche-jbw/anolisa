# Hermes 原生 shell hook

[English](README.md)

Hermes 通过原生 shell-hook 配置直接运行 AW。验证版本为官方
`NousResearch/hermes-agent` 提交
`952c941e741e922a9be8fc403c8944c6e96318bb`，使用其锁定的 Python 3.14 环境。
不需要 Python 插件或替换工具调度器。

使用隔离的 `HERMES_HOME`，配置结构见 `native-config.example.yaml`。每个原生
事件包含有序列表，字段为 `command`、可选 `matcher`、`timeout`、`fail_closed`。
Hermes 按注册顺序串行执行回调，并保留自身工具批次的并行、串行调度。hook 执行
授权仍由原生流程处理；`--accept-hooks` 明确接受已编写的测试 hook。

stdin 为原生 JSON，包含 `hook_event_name`、`tool_name`、`tool_input`、
`session_id`、`cwd`、`profile`、`extra`。AW 原样传输这些字节。前置 stdout
接受 Hermes `action: block` 和 `message`、`action: modify` 和 `args`，或
`action: approve` 和可选 `message`、`rule_key`。退出码 2 也会阻止执行。
block 优先于此前任何 approve。

`approve` 表示**向人发起审批**，不表示自动放行。没有交互用户或原生审批桥接时，
Hermes 拒绝执行。确定性测试覆盖原生 Python 审批入口及 shell 响应解析器。
后置 hook 只用于观察，返回值不能撤销工具或替换其结果。

隔离 CLI 支持 `hermes chat --oneshot --max-turns 3 --run-budget 150
--provider custom --model qwen3.7-plus --accept-hooks --query-file PATH`。
模型 key 只放入子进程环境，`OPENAI_BASE_URL` 指向已授权的兼容端点。
不要将生产 Hermes 配置、渠道设置或授权文件复制到测试 home。

原生实验通过 `aw run` 完成三个真实 `qwen3.7-plus` Token Plan CLI 用例。
允许时写出 `42`；block 和 ask 均未创建文件。原生后置状态在允许时为 `ok`，
在 block 和非交互 ask 时均为 `blocked`。全部用例都按前置 A、前置 B、后置 A、
后置 B 执行：此前回调返回 block 时，Hermes 仍调用之后的前置回调。
没有验证交互式人工批准。
