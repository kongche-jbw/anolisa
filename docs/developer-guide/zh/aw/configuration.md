# AW 配置参考

[English](../../en/aw/configuration.md)

本文说明静态配置校验，以及 fork 的 `feat/aw/native-hook-lab` 分支上的 Linux
实验运行时。该实验尚未进入 ANOLISA 发行包。可用能力及 Agent 使用流程见
[用户指南](../../../user-guide/zh/user-entrypoint/aw.md)。

[随包 Schema](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-config/schemas/configuration-v1alpha1.schema.json)
定义公共字段结构，Rust 校验器另行检查引用及字段之间的关系。`aw validate`
执行这些静态检查；`aw plan AGENT` 进一步检查该 Agent 支持的原生配置子集；
`aw run AGENT` 准备宿主绑定并启动配置的命令。静态有效不代表运行时支持。
`aw check AGENT` 执行结构化 Provider 发现和私有配置校验，`aw run` 自动执行相同
检查。公共效果限工具前 observe/block、工具后 observe。

## 配置字段

唯一接受的外层为 `apiVersion: aw/v1alpha1`、`kind: AWConfiguration`、
`metadata: {name: ...}` 和 `spec: {...}`。不接受或自动迁移早期草案的平铺
`api_version`/`name`。`status`、已安装绑定、revision 和能力状态不属于用户输入。
下表除明确标注外，所有字段均在 `spec` 内。

| 字段 | 合同 |
| --- | --- |
| `metadata.name`（在 `spec` 外） | 配置身份；1 到 128 个 ASCII 字母、数字、`.`、`_` 或 `-` |
| `daemon.startup` | `on_demand` 在 `aw run` 期间按需启动尚未运行的 daemon；`external` 要求已有 daemon 且配置 revision 相同。静态校验均不启动进程 |
| `daemon.endpoint`、`daemon.state_dir` | 必填非空字符串；原生 launcher 使用显式绝对路径，或按下文规则解析 `auto` |
| `execution.guarantee` | 仅 `native_hook`，不提供 OS、final 或 protected 保证 |
| `execution.default_event_budget_ms` | 必填正整数，为结构化事件执行保留；原生运行时不应用共享 AW 事件期限 |
| `audit.enabled`、`audit.payload` | 固定为 `true`、`metadata_only`；daemon 向 socket 同目录的 `audit.jsonl` 追加调用元数据 |
| `agents.<id>.adapter` | `qwenpaw`、`qoder`、`openclaw` 或 `hermes`；识别名称不等于运行效果已认证 |
| `agents.<id>.argv` | 非空程序/参数数组；首项不可为空，不隐式调用 shell 或插值 |
| `providers.<id>.protocol` | 原生命令使用 `native-hook/v1alpha1`；通用结构化工具效果使用 `aw-provider/v1alpha1` |
| `providers.<id>.transport` | `{type: stdio, location: agent, argv: [...], env: {...}}`，`env` 可省略。每次原生回调在其工作目录执行一次固定 argv |
| `providers.<id>.transport.env` | 以字面字符串覆盖回调环境，不插值。最多 128 个键，键匹配 `[A-Za-z_][A-Za-z0-9_]*`，每个值最多 4096 个字符且不含 NUL |
| `providers.<id>.timeout_ms` | 单次调用上限正整数；原生准入最多允许 300,000 ms，并应用下文的宿主限制 |
| `providers.<id>.max_output_bytes` | 输出上限正整数；原生执行对 stdout 和 stderr 分别应用该限制，每路最多 4 MiB |
| `providers.<id>.config` | 必填对象；原生步骤要求 `{}`。结构化 Provider 可保留包含 Unicode 键与有限小数的私有 JSON，私有 Schema 由 Provider 校验 |
| `events.<name>.enabled` | 已声明事件必填布尔值；省略事件等同关闭 |
| `events.<name>.required` | 默认 `false`；关闭事件不能标为必需。为能力准入保留，不会给原生绑定增加执行保证 |
| `events.<name>.budget_ms` | 可选的结构化默认事件预算覆盖值；嵌套 guard 共用父事件剩余预算。启用的原生事件拒绝该字段 |
| `events.<name>.steps` | 必填注册数组；空数组不调用 Provider。原生执行顺序与并发由宿主决定 |
| `events.tool.before.match.tools`、`events.tool.after.match.tools` | 可选非空选择器数组；省略表示全部原生工具，`['*']` 不与精确选择器混用 |
| `events.tool.before.guard` | 仅供结构化步骤引用已声明的 `security.violation`，before 启用时被引用事件也须启用；原生步骤不可使用 |
| `steps[].id`、`steps[].enabled` | ID 在事件内唯一；enabled 默认 `true` |
| `steps[].provider` | 已声明 Provider ID，协议须与步骤类型匹配 |
| `steps[].native` | 原生注册选项：`{}` 或下文支持的 `priority`、`point`、`sequential` 字段 |
| `steps[].operation` | 仅供结构化步骤使用的非空操作名；根据 Provider 发现结果检查 |
| `steps[].effects` | 仅供结构化步骤使用的非空、不重复效果列表；声明请求上限，不授予权限 |
| `steps[].on_error` | 仅供结构化步骤使用的 `report`、`block` 或 `withhold_result`，受事件时机约束 |

Agent/Provider ID、步骤 ID 和操作名与 `metadata.name` 使用相同语法。Schema 中
的正整数限制为 1 至 4,294,967,295，原生运行时可以施加更低的上限。
Agent、Provider、每事件步骤均不超过 128 项，
每条命令不超过 128 个参数。程序之后的空参数保留；参数、地址/目录字符串拒绝 NUL。

公共对象拒绝未知字段，仅 Provider `config` 接受私有字段。关闭的步骤同样检查
Provider 引用与步骤 ID 重复，避免启用时才暴露引用拼写错误。默认值是合同语义，
解析器不会将它们填入原始文档。

## 通用 Provider 运行时

[aw.provider.yaml](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.provider.yaml)
为四个 Agent 目标使用同一策略。步骤包含 `id`、`provider`、`operation`、`effects`、
`on_error` 及可选 `enabled`，不含 `native`。工具前支持 `observe/block`，失败动作
为 `report/block`；工具后支持 `observe`，失败动作仅 `report`。其他效果、guard、
显式事件预算即使通过静态 Schema，也会被运行准入拒绝。

[协议参考](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/docs/design/provider-protocol_zh.md)
定义 `describe`、`validate_config`、`invoke`。每次回调在同一个 Provider 超时预算
内执行三个方法，校验请求 ID、摘要和返回效果。Provider stdout 为协议 JSON，由
AW 生成原生响应；空 effects 不批准宿主权限，宿主回调顺序保持不变。Agent 启动前
也会执行发现和配置校验。

`on_error` 覆盖协议错误及客户端仍存活时的 daemon 故障。宿主 helper 缺失或被杀
仍是原生覆盖缺口。生成的客户端绑定含 `--adapter`、`--on-error`，这两个内部 Hook
选项不会增强配置的执行保证。审计增加 `protocol` 和 `disposition`，记录返回的决策，
不代表原生宿主已采用。

## 原生 Hook 运行时

[原生示例](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/examples/aw.native.yaml)
使用 `protocol: native-hook/v1alpha1`、`config: {}`，步骤形如
`{id: inspect, provider: audit-command, native: {}}`。原生步骤只接受 `id`、
`provider`、可选 `enabled` 和 `native`，不能包含 `operation`、`effects`、
`on_error`、`config` 或 `guard`，所属事件也不能声明 guard。命令设置通过字面值
`transport.argv` 和 `transport.env` 提供，仅在 argv 显式指定 shell 时才启动 shell。

每条原生注册单独调用 AW 一次。AW 将收到的 stdin 字节交给配置的命令，再向
适配器返回 stdout、stderr 和退出状态，不套用结构化 Provider 请求/响应协议。
QwenPaw middleware 没有原生命令 Hook wire 格式，因此 Python 适配器将原生对象
投影为 JSON；其他宿主提供各自的原生 Hook payload。命令输出的含义，包括 block
和 ask，由宿主及适配器定义。例如 Hermes 的 `approve` 请求人工批准；QwenPaw
middleware 明确拒绝不支持的 ask。AW 不提供统一审批 UI，也不会将 ask 响应
转为自动批准。

[运行时准入代码](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/src/model.rs)
仅允许启用 `tool.before`、`tool.after`，使用已支持的步骤，并省略工具选择器或精确
填写 `['*']`。其他启用事件、精确工具选择器、事件 `budget_ms` 和 guard 均被
拒绝。关闭的条目可以保留符合静态校验的结构化配置。同一启用事件不能重复注册
同一个 Provider，需要重复调用时应定义不同名称的 Provider 实例。

AW 保留各宿主的回调顺序、短路行为和工具并发。步骤数组提供注册顺序，并不
构成共享串行流水线。`execution.default_event_budget_ms` 仍是 Schema 必填字段，
但此处不使用；`required` 不改变原生执行保证，也不认证 Hook 覆盖范围。AW 独立
限制每条命令的执行时间和输出，宿主期限及其他原生 Hook 还可能压缩可用时间。

| 原生选项 | 已实现的绑定 |
| --- | --- |
| `native.priority` | -10,000 至 10,000 的整数；QwenPaw middleware 和 OpenClaw Hook 支持，Qoder 与 Hermes 拒绝。优先级方向及后置 Hook 顺序由宿主决定 |
| `native.point` | 仅用于 OpenClaw `tool.after`：`tool_result_persist` 或 `agent_tool_result`。省略时使用普通后置 Hook；`agent_tool_result` 按注册顺序执行并拒绝 `priority` |
| `native.sequential` | 仅用于 Qoder Hook 分组设置；省略时保留原生默认值，其他适配器拒绝该字段 |

原生 stdin 最多 4 MiB。`max_output_bytes` 对每路输出流最多允许 4 MiB。
`timeout_ms` 最多为 300,000；各宿主还施加以下限制：

| 适配器 | Provider 超时限制 |
| --- | --- |
| Qoder | 最多 300,000 ms；生成的原生 Hook 超时为 `ceil(timeout_ms / 1000) + 2` 秒 |
| OpenClaw | 最多 12,000 ms，位于适配器的 14,000 ms 桥接超时内 |
| QwenPaw | 最多 58,000 ms，位于适配器的 60 秒桥接超时内 |
| Hermes | 最多 298,000 ms；`ceil(timeout_ms / 1000) + 2` 还须不超过基础配置的 `plugins.hook_callback_timeout`，默认 30 秒，0 表示关闭该宿主限制 |

Hermes 可能将回调超时应用于整条原生回调链。单条命令通过准入检查，并不保证
多条命令与已有回调的总耗时落在宿主窗口内。运行时超时与输出超限均会暴露，
对工具执行的具体影响由宿主决定。

## 原生 daemon 配置

`aw run` 的 `--state-dir` 覆盖 `daemon.state_dir`。使用 `auto` 时，AW 选择
`$XDG_RUNTIME_DIR/aw-native-<revision-prefix>`；没有该环境变量时须提供显式
绝对目录。`--socket` 覆盖 `daemon.endpoint`。endpoint 为 `auto` 时，socket
位于 `<state-dir>/aw.sock`；显式 endpoint 必须是绝对 Unix socket 路径，可以
添加 `unix://` 前缀。`aw serve` 始终从必填的 `--socket` 参数读取 socket 路径。

`on_demand` 在没有可用 daemon 时启动一个；`external` 要求已有运行实例。
现有 daemon 必须使用相同配置 revision。launcher 在 Agent 运行期间持有绑定
PID/启动时间的 lease，退出时释放；daemon 自动清除已死亡属主的 lease。仅在
没有存活 lease 或回调时才允许空闲退出。按需 daemon 的空闲超时为 300 秒；显式
`aw serve --idle-timeout` 接受 1 至 86,400 秒。socket 仅供当前用户访问。
调用审计记录包含元数据、字节数和状态，不记录 payload。

[launcher](https://github.com/kongche-jbw/anolisa/blob/feat/aw/native-hook-lab/src/aw/crates/aw-cli/src/launch.rs)
准备临时原生绑定。`--native-config` 提供宿主基础配置，Hermes 和 OpenClaw
要求显式提供；基础配置中已有的 Hook 注册会被保留。原生 Hook 同意机制及宿主
权限行为仍然生效。这些生命周期与绑定功能属于当前实验分支，不代表生产部署，
也不表示已有 `anolisa install aw` 发行包。

## 结构化 Schema 的事件、效果与工具选择

Schema 识别以下 16 个名称。通用 `aw-provider/v1alpha1` 运行时仅准入两个工具
事件，支持工具前 observe/block、工具后 observe。下面更广的静态效果词汇不代表
运行时已经实现。

| 事件 | 含义 |
| --- | --- |
| `session.start` | 会话创建、加载或恢复 |
| `input.submit` | 输入到达原生提交点 |
| `tool.before` | 原生执行前的工具意图 |
| `tool.after` | 原生工具完成，包含宿主报告的失败 |
| `permission.request` | 宿主请求权限判断 |
| `compact.before` | 上下文压缩前 |
| `compact.after` | 原生压缩结果 |
| `subagent.start` | 原生子 Agent 启动 |
| `subagent.stop` | 原生子 Agent 到达停止点 |
| `turn.stop` | 任务停止检查，不证明任务成功 |
| `session.end` | 原生会话结束 |
| `model.before_request` | 模型请求到达已验证的发送边界 |
| `runtime.observed` | 可信来源登记运行实例 |
| `runtime.exited` | 可信来源观测根运行实例退出 |
| `security.violation` | AW 工具前主动末尾检查的暂名 |
| `coverage.changed` | 已观测接入覆盖发生变化 |

结构化合同将 `security.violation` 定义为仅通过启用的 `tool.before` 的 guard
执行，检查最终候选，允许 `observe`/`block`，不修改参数。它不是第二个原生
Hook，也不保证排在全部
第三方 Hook 之后。检查后参数再次改变时，实际执行边界必须重新检查。

对于结构化步骤，`tool.before` 允许 `observe`、`block`、`replace_input`；
`tool.after` 允许 `observe`、`replace_result`；其余事件本版本仅观察。
`ask` 保留在 before 步骤中，
启用步骤请求它时拒绝配置。显式关闭的 before 步骤可以保留 `ask` 供后续编辑，
不因此获得审批能力。宿主原生审批不受影响。

`on_error: block` 仅用于执行前的 `tool.before` 或 guard；`withhold_result`
仅用于工具后；`report` 记录失败后继续。禁止原结果交付需要已验证的模型消费
边界，只修改历史记录不足以满足要求。必需的安全隐藏不能使用 `report`。
当前运行时拒绝尚未实现的 withhold_result 和 guard，不自动降低要求。

选择器为 `*`、`bash`、`file_read`、`file_write` 或四个适配器 ID 对应的
`native:<adapter>:<精确名称>`。原生选择器依赖宿主，不是跨框架语义。除单独
`*` 外不提供正则或 glob 匹配。全部工具路由包括原生自定义工具并保留输入，
不表示每个 Provider 都能理解每种工具。

`required: false` 不能授权丢弃启用的控制效果或失败处置。运行时准入会
将启用步骤与 Provider 声明、实现及宿主能力对照，拒绝不支持的必需控制。
可选观察来源缺失必须明确记录。配置解析本身不执行这项运行时准入。

## 解析与兼容性

解析器接受一份 UTF-8 YAML 或 JSON，输入及展开后 JSON 均不超过 4 MiB，嵌套
深度不超过 32。重复键、非字符串键、自定义 YAML tag、merge key、非有限数值
及多文档均拒绝；普通 alias 在限额内展开。诊断包含字段路径或源码位置与约束，
不回显字段值。

本 alpha 配置与已有能力 wire 记录、Schema ID/摘要分开演进，不应将 Provider
配置送入仅支持整数的 wire canonicalizer。`aw validate` 不安装或修改任何原生
文件。`aw run` 会创建临时宿主绑定，并可能启动 daemon；这些运行时副作用与
解析及静态校验分开。
