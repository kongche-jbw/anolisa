# PR 3 架构审查总报告

本报告审查
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)。固定基线为
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674`，固定头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。修正实现位于
[`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。

报告始终分开评价原 PR 与 fork PoC。PoC 的后续改动不会改变对原 PR 头提交的 Review
结论。

## 审查结论

PR 3 的架构方向正确。它把稳定能力合同、策略决策、组件执行、最终环境权力和长期审计
分开，能够承接 Agent Host 设计中的权威边界。这套分层可以作为后续实现基线。

原 PR 头提交仍应得到 Request changes。主要问题集中在前后逻辑和 Schema 语义，包含
输入与真实使用字节不一致、跨字段状态可互相矛盾、Tokenless 对 `lossless` 的定义偏弱、
Receipt 无法证明输入来源、Ledger 承诺范围不足，以及 candidate 被误当成最终采用事实。

fork PoC 已经修复其中大部分合同和 PostToolUse 主链。安全 PreTool 仍缺少
`command_digest == executed_command_digest` 的最终接线。Checkpoint 已形成 Gateway State
Provider、Guarded V2、Runtime tool 与私有控制协议主链。固定提交 `5ebfc0b3` 已在 Btrfs
Ubuntu VM 中通过 Herdr 正常链路 E2E；响应丢失和进程重启后的 evidence-only 恢复仍需
故障注入验证。

构建、打包与 CI 仍然重要，本轮先收敛运行语义和 Schema。它们不影响对架构方向的判断，
也不能在生产交付前省略。

## 架构中值得保留的部分

PR 3 将一次能力调用拆成五个职责清楚的角色。

| 角色 | 持有的事实 | 不应持有的权力 |
| --- | --- | --- |
| COSH 等 Environment | 即将执行的参数与最终写入本地模型历史的字节 | 不解释组件 native protocol |
| AW Core | Plan、Policy、Authority、Scope 与结果解释 | 不启动组件进程，不直接写模型历史 |
| Provider Host | manifest 准入、Schema 校验、mapping、进程预算与 receipt | 不决定 candidate 是否最终采用 |
| Capability Provider | 扫描、压缩等组件算法及 native response | 不扩大 manifest 声明的 Authority |
| AW Ledger | 类型封闭的摘要、关联与哈希链 | 不保存 Tool 正文或 candidate 正文 |

这种设计允许 Core 使用稳定 Capability，不需要按 `tokenless` 或 `agent-sec-core` 写条件
分支。组件继续维护自己的 native Schema，Provider package 通过声明式 mapping 连接两侧。

三种 Authority 也应保留。

| Authority | Provider 产物 | 真正生效的位置 |
| --- | --- | --- |
| Observe | finding、计量或分类事实 | Core 接纳事实并交给策略、Environment 或审计面 |
| Advise | candidate | Environment 把选定字节写入本地模型历史 |
| Mediate | allow、warn 或 deny 意见 | Environment 最终执行、询问或阻断 Tool Call |

文件存在、manifest 通过准入、Provider 被调用、Provider 返回结果都属于中间状态。最终
生效事实只能由拥有真实边界的 Environment 产生。

## PoC 收敛后的 PostToolUse 主链

PoC 将 Tokenless 生效过程整理为下列顺序。

```text
COSH 聚合普通 PostToolUse Hook
  -> 得到即将写入本地模型历史的 provisional bytes
  -> AW Core 建立 canonical input 与 Plan
  -> Provider Host 校验 canonical input
  -> Provider Host 按 manifest 生成并校验 native request
  -> Tokenless 返回 native response
  -> Provider Host 校验 native response 与 canonical output
  -> Provider Host 返回 transient candidate + ProviderReceipt
  -> AW Core 校验 source identity、digest 与 reversibility
  -> AW Ledger 追加完整 post_tool_use_plan/v1
  -> COSH 选择 candidate 或 source bytes
  -> COSH 写入本地模型历史槽位
  -> AW Ledger 追加引用同一 plan 的 context_adoption/v1
```

请求方向与返回方向同样重要。Host 返回的 `ProviderInvocationOutcome.output` 可以短暂携带
candidate 正文。`ProviderReceipt` 只携带输入输出身份、digest、长度、计量与终态。Core
必须同时收到二者，才能验证业务结果并保留可审计的调用关联。

COSH 的采用规则保持封闭。

| candidate 状态 | COSH 写入内容 | Ledger 决策与原因 |
| --- | --- | --- |
| 非空并满足严格 `lossless` | candidate bytes | `adopted / lossless_candidate` |
| 没有 candidate | source bytes | `preserved / no_candidate` |
| candidate 为空 | source bytes | `preserved / empty_candidate` |
| candidate 不能恢复全部源信息 | source bytes | `preserved / candidate_not_lossless` |

`context_adoption/v1` 记录 `plan_event_id`、source artifact、source digest、可选 candidate
envelope digest、effective digest、字节数、封闭决策、封闭原因和受限 invocation references。
它证明 COSH 已修改一个本地历史槽位，不声称远端模型已经接收或消费这些字节。

系统配置决定 Ledger assurance。`required` 模式在 adoption 追加失败时撤销刚写入的历史
内容，并结束当前轮次。`best_effort` 模式保留历史内容并报告降级。两个模式都不会把一次
失败的追加描述成已经存在的 adoption evidence。

## 原 PR 关键问题与 PoC 结果

| 主题 | 原 PR 头提交 | fork PoC 基线 | 当前判断 |
| --- | --- | --- | --- |
| Canonical ID 与名称 | JSON Schema 和 Rust 长度、字符集、ID pattern 不一致 | 收敛 printable ASCII 与 canonical typed ID | 已建立一致基线 |
| 四阶段 Schema | 只校验 Schema 文件和 digest，不校验运行实例 | 校验 canonical input、native request、native response、canonical output | 已建立一致基线 |
| Agent Sec 终态 | `clean` 可与 findings、truncated 同时出现 | typed validator 拒绝矛盾状态 | 已建立一致基线 |
| `language=auto` | 普通 Python 会落入 Bash scanner | 双路径扫描并报告实际语言 | 已修复 |
| Tokenless `lossless` | 任务相关信息保留被映射为全部源信息保留 | 单独计算 source fidelity，删除或清理会降级 | 已修复 |
| Tokenless 媒体类型 | native 协议不传 source/output media type，candidate 固定为文本 | 双向传递 media type，Core 执行 `allow_text_reencoding` | 已修复基线 |
| Receipt 输入来源 | 只有输出摘要，无法证明处理了哪份输入 | 加入 input schema、input digest 与 manifest 绑定 | 已修复 |
| Ledger 哈希范围 | scope 与查询列可被修改而不触发 verify | canonical record 承诺 scope 和查询字段 | 已修复基线 |
| Ledger 正文通道 | 通用 JSON body 与自由文本可绕过黑名单 | typed taxonomy、严格 body、受限 projection metadata | 已修复基线 |
| Ledger plan 完整性 | 计划步骤、gap、receipt 和 Tool scope 可以互相脱节 | 每个 Observe step 互斥记账，fan-out 去重并核对 scope、终态与 error | 已修复基线 |
| Provider 结果回流 | 文档容易停在 Host 调用 Provider | Host 显式返回 candidate 与 receipt 给 Core | 已明确 |
| 最终采用 | replacement request 被当成最终生效 | COSH 写历史后追加 `context_adoption` | PoC 已接线 |
| PostTool source bytes | 通用 Hook 看到脱敏副本，最终历史可能使用另一份原文 | 在普通 Hook 聚合后处理 effective bytes | PoC 已接线 |
| PostTool 边界失败 | 外层 Hook 失败可能静默保留原文 | 启用的一等边界返回固定错误并结束本轮 | PoC 已接线 |
| PreTool executed bytes | 扫描字节可能在后续 patch 或执行前发生变化 | Receipt 已能绑定扫描输入 | 最终执行摘要仍待绑定 |

COSH effective-bytes 当前处理模型历史文本槽，调用方政策固定选择
`allow_text_reencoding=true`。公共 canonical 合同的 false 分支仍由 Core 与真实 Tokenless
测试覆盖；如果未来要让 COSH 保留可配置的结构化槽位，再扩展其 request 类型。这是调用方
政策，不是媒体类型合同的缺口。

PoC 保留 canonical verdict 的封闭枚举。完整扫描没有 finding 才能产生 `clean`。部分扫描
已经发现风险时可以产生 `suspicious/sensitive + truncated=true`，其中 `scanned_bytes` 必须
是实际扫描的非空 UTF-8 前缀。没有可用风险事实的截断、扫描器失败和无实现进入 Provider
failure、Observation gap 或 gate degradation，不能伪装成安全 verdict。

## Schema 是否适合作为改造中心

Schema 适合作为改造中心，但 JSON Schema 只覆盖字段形状。完整合同分为三层。

| 合同层 | 适合表达的内容 | 需要额外机制的内容 |
| --- | --- | --- |
| JSON Schema | 必填字段、枚举、格式、长度、对象形状 | 跨对象来源与运行时采用事实 |
| Typed validation | verdict 与 findings、truncated、reversibility 等跨字段不变量 | 某段字节是否真的被执行或写入历史 |
| Runtime evidence | scanned、source、candidate、effective 与 executed digest 的关联 | 产品发布与运维健康 |

因此，团队可以围绕 14 份逻辑 Schema 收敛字段语义，同时为每份 Schema 配套 typed model、
正反 fixture、manifest mapping 和真实 Provider conformance test。只改 JSON 文件会留下相同
的运行语义缺口。

接口冻结前仍需决定以下事项。

- 为 `media_type`、`content_type` 与 transform identifier 建立专用类型或准入注册表。
- 为 `retrievable` 定义 resolver、reference、授权、TTL 与失效行为。
- 若需要真实 Tokenless frontend 归因，新增 typed 字段；当前 PoC 使用 `aw-provider` 常量。
- 明确 Provider package 中 canonical Schema 副本的生成和漂移检查方式。
- 冻结安全门禁的最终 executed-bytes credential。

逐份字段、图示和讨论见 [Schema 参考与语义讨论](schema-reference_zh.md)。

## 安全 PreTool 仍待闭合的边界

PoC 已让 Agent Sec 的 Receipt 绑定 canonical input。它可以回答 Provider 扫描了哪一段命令。
当前还不能证明 COSH 后来执行的命令保持相同。

目标不变量如下。

```text
digest(agent_sec_scanned_command) == digest(cosh_executed_command)
```

这条不变量必须位于所有允许修改参数的 Hook 和 policy patch 之后，并在实际 spawn 之前
校验。任何后续改写都需要重新检查或使旧 gate credential 失效。Ledger 只能保存 digest、
gate decision 和受限关联，不能保存命令正文。

在这条接线完成前，Agent Sec 示例只能证明扫描与 Core 解释链路，不能作为实际执行安全
闭环的证据。

## Checkpoint 的正确位置

Checkpoint 会产生持久副作用，也可能出现请求已写入、响应却丢失的情况。当前 one-shot
`exec-json/v1` Provider Host 不适合承担这一语义。

PoC 把 Checkpoint 放在 Gateway State Provider 中。Gateway 持有批准、Task、Attempt、
workspace binding 与 durable operation state。它通过 Guarded Checkpoint V2 调用 ws-ckpt，
并在恢复时只查询相同 `operation_digest` 的精确 evidence。

```text
Gateway Task
  -> admitted workspace-checkpoint-v1 profile
  -> persisted provider binding
  -> approval + durable claim/start
  -> GuardedCheckpointV2
  -> ws-ckpt durable evidence
  -> Gateway terminal receipt
```

socket device、inode 和 ws-ckpt daemon UID 标识服务端。Gateway UID 标识调用方。它们是两组
不同身份，不能合并。证据缺失或不匹配时结果保持 `uncertain`，恢复过程不得重放 create。

Runtime 已 pin 的 workspace directory 是 workspace 身份来源。Binding 会提交 pinned inode、
Btrfs FSID/subvolume UUID generation、注册路径、profile、target、两端 UID、`permit_id` 与
`execution_id`。批准、claim、start、terminal、source 和 delivery 使用同一个 execution
identity。恢复时仍需认证当前 socket 的受信任祖先、owner 与 `SO_PEERCRED` UID，但服务
重启后 socket inode 可以改变；历史 effect identity 不能因此被重新执行。

Gateway 旧 `cosh.gateway.v1` Submit JSON 形状保持冻结，但只在服务端当前准入的 exact
`task-only-v1` profile 下接受。`workspace-checkpoint-v1` 必须使用 v2。client 先做
Admission discovery，再在 Submit 中回显完整 admission。Checkpoint 使用 v1，或 v2
的版本与 echo 不匹配时，都会 fail closed。

当前基线没有 `providers/ws-ckpt/provider.toml`，也没有伪造 Checkpoint AW canonical Schema。
PoC 只在 `workspace-checkpoint-v1` profile 下注册 `workspace_checkpoint_create`，并通过
Gateway 私有 control request 进入 State Provider。固定提交 `5ebfc0b3` 已在 Btrfs
subvolume 上完成 Ubuntu VM + Herdr 正常链路。11 个连续事件从 `task_submitted` 到
`task_succeeded`，并留下 approval、permit、execution、snapshot 与 durable evidence。
这次演示没有注入响应丢失或进程重启，因此不能用它替代 evidence-only 恢复验收。

## 仍需保留在 Review 中的问题

下列事项没有被 schema 与主链 PoC 完全覆盖。

- one-shot Provider 的生产级后代进程监督仍需要 cgroup、PID namespace 或 subreaper。
- mapping 的内存放大与整个 Plan 的总时间预算仍需系统性上限。
- Capability Graph 需要逐 Provider 表达 installed、admitted、bound、ready 与失败原因。
- 当前 Plan 仍要求 Advise Projection 路由；是否允许只有 Observe 的 partial plan 需要明确决定。
- OS sandbox、executable identity、签名包、升级与回滚属于产品交付条件。
- AW 常驻 writer、外部锚定、retention 与增量 verify 仍需产品设计。
- build-all、RPM、镜像与 CI declared-base diff 需要后续里程碑处理。

这些事项不会推翻 Provider 分层，也不应被 PoC 成功掩盖。

## PoC 提交证据

| 提交 | 主要作用 |
| --- | --- |
| [`4d47593b`](https://github.com/kongche-jbw/anolisa/commit/4d47593b) | 收紧 Capability 与公共类型合同 |
| [`6238842a`](https://github.com/kongche-jbw/anolisa/commit/6238842a) | 加入四阶段运行实例 Schema 校验 |
| [`d1d813d5`](https://github.com/kongche-jbw/anolisa/commit/d1d813d5) | 修正 Agent Sec 扫描终态与 `auto` |
| [`414998b2`](https://github.com/kongche-jbw/anolisa/commit/414998b2) | 计算 Tokenless 精确 source fidelity |
| [`76b03b01`](https://github.com/kongche-jbw/anolisa/commit/76b03b01) | 用真实 Agent Sec 与 Tokenless 证明组合链路 |
| [`1328cf30`](https://github.com/kongche-jbw/anolisa/commit/1328cf30) | 将 Provider Receipt 绑定到输入 |
| [`e43484e7`](https://github.com/kongche-jbw/anolisa/commit/e43484e7) | 收敛 brokered effect 的恢复语义 |
| [`5d7f6b62`](https://github.com/kongche-jbw/anolisa/commit/5d7f6b62) | 收紧 Ledger 来源、scope 与 content-free 约束 |
| [`8ecb1412`](https://github.com/kongche-jbw/anolisa/commit/8ecb1412) | 绑定扫描覆盖、媒体保真和 Provider 输入输出 |
| [`601b5558`](https://github.com/kongche-jbw/anolisa/commit/601b5558) | 连接 COSH 最终采用与 Checkpoint State Provider |
| [`5ebfc0b3`](https://github.com/kongche-jbw/anolisa/commit/5ebfc0b3) | 固化 VM + Herdr 演示与最终可复核证据 |

## 合并建议

原 PR 头提交继续保持 Request changes。建议以 fork PoC 的 Schema、Receipt、Ledger 与
PostToolUse 主链作为新的实现基线。Checkpoint 正常链路已有 VM 证据，下一步完成安全
executed-bytes binding 与 response-loss/restart 故障恢复验收。随后再进入构建、CI、签名
交付和长期运维验收。

这一路径保留老板整理的总体方向，也让每个“已生效”声明都能落到拥有最终权力的组件和
可验证 digest 上。
