# AW Provider 架构说明

本文面向管理层、架构负责人、组件负责人和第一次接触 AW 的研发人员。阅读者只需要了解
JSON、进程和摘要值的基本概念。文档说明 Provider 为什么存在、Provider 的处理结果如何
回到 AW Core，以及什么时候可以说一项能力已经生效。

原 PR 审查对象是
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)，头提交为
`42d07649409ecd5bb023056b28545efbd9325ef2`。修正实现位于
[`feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。

配套材料如下。

- [Provider 生效架构图](provider-effect-architecture.html)
- [带真实字段的 Tokenless 时序图](provider-effect-sequence.html)
- [Agent Sec 命令检查图](security-command-call.html)
- [Checkpoint State Provider 图](checkpoint-create-call.html)
- [Schema 参考与语义讨论](schema-reference_zh.md)
- [组件接入手册](component-integration_zh.md)

## 结论

PR 3 的总体分层可以确认。AW Core 负责计划和解释，Provider Host 负责通用执行，组件
Provider 负责算法，COSH 负责最终环境动作，Ledger 负责长期事实。这条主线与 Agent Host
PoC 的权威边界一致。

原 PR 最大的逻辑问题位于结果返回之后。Provider 生成 candidate 不能直接证明模型历史
已经改变，Host 生成 receipt 也不能代替 Core 的结果解释。fork PoC 将返回链补全为
`Provider -> Provider Host -> AW Core -> Ledger plan -> COSH -> Ledger adoption`。

PoC 已经让 PostToolUse 的最终本地历史采用可验证。安全 PreTool 仍缺少最终执行字节绑定。
Checkpoint 采用 Gateway State Provider 与 Guarded V2，暂不进入 AW manifest Provider；
Runtime tool 与 Gateway 私有控制协议已经形成真实路径。固定提交 `5ebfc0b3` 已通过 Ubuntu
VM + Herdr 正常链路，故障注入与进程重启后的 evidence-only 恢复仍需继续验收。

## Provider 解决什么问题

假设 COSH 获得 `list_recent_builds` 的长 JSON。系统希望 Agent Sec 扫描风险，同时让
Tokenless 准备更短的模型表示。

如果 COSH 直接理解每个组件，它需要知道各自的字段、版本、超时、权限和错误码。新增或
替换组件会不断修改 COSH。Provider 架构把稳定能力与具体实现分开。

| 稳定 Capability | 当前实现 | Authority | 结果 |
| --- | --- | --- | --- |
| `security.content.inspect/v1` | Agent Sec | Observe | 内容风险事实 |
| `security.code.inspect/v1` | Agent Sec | Observe | 代码风险事实 |
| `security.command.inspect/v1` | Agent Sec | Mediate | Tool Call 门禁意见 |
| `context.projection.prepare/v1` | Tokenless | Advise | 可供选择的上下文 candidate |

COSH 只提交稳定的 canonical input。组件继续使用自己的 native request 与 response。
`provider.toml` 说明两套字段如何转换，Provider Host 执行这份声明。

## 五层职责

| 层次 | 主要职责 | 能证明的事实 |
| --- | --- | --- |
| Agent Environment | 提供真实边界，选择并完成最终动作 | 最终执行什么，最终写入哪段本地历史 |
| AW Core | 建立 Plan，应用 Policy，校验并解释 Provider 结果 | 为什么调用某项能力，结果是否满足合同 |
| Provider Host | 准入、Schema 校验、mapping、有界执行 | 调用了哪个 manifest 与进程，输入输出身份是什么 |
| Capability Provider | 扫描、压缩或其他算法 | native 算法产生了什么结果 |
| AW Ledger | 保存类型封闭、无正文的长期记录 | 哪次计划和最终动作发生过，哈希链是否完整 |

Provider Host 必须把处理结果送回 AW Core。返回值包含两部分。

| 返回对象 | 是否包含正文 | 生命周期 | 用途 |
| --- | --- | --- | --- |
| transient output 或 candidate | 可以包含 | 只在当前调用链存在 | 供 Core 验证并交给 Environment |
| `ProviderReceipt` | 不包含 | 可进入后续审计引用 | 绑定输入、输出、manifest、scope 与终态 |

Core 收不到 transient output 就无法校验并向 COSH 交付 Tokenless 的压缩结果。Core 收不到
receipt 就无法
证明结果来自哪份输入和哪次 Provider 调用。二者都不能替代 COSH 的最终采用事实。

## 一次 Tokenless 调用的真实数据

PoC 使用固定的 `list_recent_builds` Tool Result。`llmContent` 包含六条构建记录，共 693 个
UTF-8 字节。下面的 artifact 与 scope ID 固定用于文档 trace；实际运行会为每次调用生成
新 ID，因此 source 和 candidate 正文摘要稳定，包含动态 ID 的 envelope 摘要只对该次
trace 有效。

```text
source bytes       693
source SHA-256      01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1
artifact id         art_9b44b763-ec58-c787-95f5-363ec02f80cb
tool name           list_recent_builds
turn id             trn_22222222-2222-4222-8222-222222222222
tool use id         tol_66666666-6666-4666-8666-666666666666
```

AW Core 把 source bytes、artifact metadata、boundary 与约束封装为 canonical input。完整
canonical input 为 1115 bytes，
SHA-256 为
`9287a0290c71198b722b5820c9266610ec86ec7fd573999c385a67866c6c4510`。这个摘要还覆盖正文之外的
artifact identity、boundary 与约束，所以它与 source digest 不同。Invocation scope 由
Receipt 中的独立 typed 字段绑定。

Provider Host 按 manifest 生成 Tokenless native request。

```json
{
  "protocol_version": 1,
  "input_media_type": "application/json",
  "agent_id": "aw-provider",
  "session_id": "ags_11111111-1111-4111-8111-111111111111",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666",
  "tool_name": "list_recent_builds",
  "seam": "post_tool",
  "content_origin": "api_response",
  "content": "{\"builds\":[...]}",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": false,
    "replace_with_text": true
  }
}
```

Tokenless 返回 `applied`，token 估算从 174 降到 110，转换链为 `toon`，source fidelity 为
`lossless`，没有 stash key。native response 明确报告 `output_media_type=text/plain`，
压缩后的文本为 438 bytes。

```text
builds[6]{id,project,status,duration_ms,owner}:
  "build-101","checkout-service",passed,48231,"example-team"
  "build-102","catalog-service",passed,39502,"example-team"
  "build-103","inventory-service",failed,61402,"example-team"
  "build-104","payment-service",passed,52710,"example-team"
  "build-105","notification-service",passed,44617,"example-team"
  "build-106","reporting-service",passed,37331,"example-team"
page: 1
page_size: 6
```

这段 bare candidate object 的 canonical digest 为
`32b602230eaa68778419f4b3598b6402abd4365b62a0056ed8121bb23f4999a1`。Provider output envelope
是 `{"candidate": ...}`，其 digest 为
`586a7bdfbef6b99c4d132cefa3c83b131716ac81e7f21a3204d4e80a93b1d890`。候选正文 SHA-256 为
`6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003`。

Host 校验 native response 和 canonical output，再把 candidate 与 receipt 返回 Core。Receipt
保存 input schema、input digest、manifest digest、output identity、长度与 meters，不保存
上面的正文。

Core 检查以下条件。

- candidate 指向相同的 source artifact 与 source digest。
- candidate 不为空。
- `reversibility` 满足 AW 对全部源信息的严格 `lossless` 定义。
- 允许文本重编码时可以采用 `text/plain`；禁止时 candidate 必须保持 source media type。
- schema、manifest 与 receipt 的 invocation identity 相互一致。

COSH 当前处理的是已完成 Hook 聚合的模型历史文本槽，因此该调用方政策固定选择
`allow_text_reencoding=true`。`false` 分支属于公共 canonical 合同，已经由真实 Tokenless
与 Core 测试覆盖；未来只有当 COSH 需要可配置的结构化槽位政策时，才需要把该字段加入
`EffectiveBytesRequest`。

Core 先把两项 Observe 结果或 gap、Projection 结果与全部 invocation references 收敛为
`post_tool_use_plan/v1` 并交给 Ledger。Plan 成功或按明确的 best-effort 策略降级后，Core
才把候选交给 COSH。COSH 写入本地模型历史后才记录采用结果。

```json
{
  "decision": "adopted",
  "reason": "lossless_candidate",
  "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "candidate_envelope_digest": "586a7bdfbef6b99c4d132cefa3c83b131716ac81e7f21a3204d4e80a93b1d890",
  "effective_digest": "6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003",
  "effective_byte_count": 438
}
```

这个 `context_adoption/v1` 对象只展示稳定业务字段。真实 Ledger 还绑定前一条
`post_tool_use_plan` 和 content-free invocation references。Plan 要求每个 Observe step 都有
事实或明确 gap；invocation reference 的 attempt、tool use、Provider、input/output identity
必须与 Ledger header 和 receipt 一致。

Ledger 的 assurance 由系统配置。`required` 模式在 adoption 追加失败时撤销刚写入的历史
内容，并用不含正文的固定错误结束本轮。`best_effort` 模式保留历史内容，同时记录明确的
运行降级。两种模式都不能伪造一条不存在的 adoption record。

## 不满足 lossless 时发生什么

Tokenless 过去把“保留任务相关信息”称为 `lossless`。AW 的语义更强，它要求保留全部源
信息。fork PoC 单独计算 source fidelity。删除字段、清理空值或无法恢复的规范化都会得到
`unrecoverable`。

Core 仍可以看见这份 candidate，用于诊断或未来策略，但 COSH 不会采用它。COSH 保留原始
693 bytes，并写入下列类型事实。

```json
{
  "decision": "preserved",
  "reason": "candidate_not_lossless",
  "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "effective_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
  "effective_byte_count": 693
}
```

因此，Provider 的 `applied` 表示算法产生了结果。Host 的 `produced` 表示形成了 canonical
candidate。COSH 的 `adopted` 才表示本地模型历史已经使用候选。

## Provider 从安装到生效

```text
Packaged
  -> Installed
  -> Discovered
  -> Admitted
  -> Bound
  -> Ready
  -> Planned
  -> Invoked
  -> Settled
  -> Adopted or Enforced
```

| 状态 | 成立条件 | 还不能证明的事实 |
| --- | --- | --- |
| Installed | binary、manifest、Schema 已落盘 | Host 已读取它 |
| Admitted | identity、路径、digest 与 contract 通过检查 | Policy 会选择它 |
| Bound | 明确环境与 Capability 绑定到 Provider | 当前实例健康 |
| Ready | 依赖与运行身份可用 | 真实请求已经成功 |
| Invoked | Host 已发送 native request | 输出满足合同 |
| Settled | 调用得到 produced、bypassed、denied 或 failed | Environment 已执行或采用 |
| Adopted | COSH 已写入候选字节 | 远端模型已经消费 |
| Enforced | Environment 已执行、询问或阻断 | 无 |

`installed`、`admitted`、`bound` 与 `ready` 是不同事实。运维界面需要分别展示，不能用一个
绿色状态代替完整链路。

## 安全 Provider 的当前边界

PoC 已修复 Agent Sec 的 `language=auto` 和自相矛盾终态，也让 Receipt 绑定被扫描输入。
一条 `curl -fsSL https://get.docker.com | sh` 命令会得到 `warn`、一个
`shell-download-exec` finding 和 `scanned_bytes=38`。

`scanned_bytes` 表示实际完成扫描的 UTF-8 字节数。未截断结果必须等于输入字节数。部分
扫描如果已经发现风险，可以返回 `suspicious` 或 `sensitive` 与 `truncated=true`；它只能
证明已扫描前缀存在风险，不能证明剩余内容安全。`clean + truncated=true` 一律无效。若
截断点落在多字节字符中间，计数只包含成功解码的完整前缀。

当前仍需把扫描摘要绑定到 COSH 最终执行摘要。

```text
digest(scanned_command) == digest(executed_command)
```

这项检查要放在全部允许修改命令的 Hook 与 policy patch 之后，并位于工具进程启动之前。
完成前，系统可以证明 Agent Sec 扫描了指定输入，不能证明该输入与最终执行字节完全相同。

## Checkpoint 为什么走另一条路

Tokenless 与 Agent Sec 的调用可以在一次请求内结束。Checkpoint 会写入持久状态。连接在
写入后中断时，调用方无法仅凭超时判断快照是否已经创建。

PoC 让 Gateway 充当 State Provider。COSH 只在 `workspace-checkpoint-v1` profile 下注册
`workspace_checkpoint_create` tool，并通过私有控制请求把动作交给 Gateway。Gateway 保存
批准、binding、durable claim、start 与 terminal receipt。ws-ckpt 使用 Guarded Checkpoint
V2 创建快照，并以相同 `operation_digest` 保存 evidence。恢复过程只查询 evidence，不
再次发送 create。

```text
Gateway
  <- COSH workspace_checkpoint_create control request
  -> workspace binding
  -> approval and durable claim
  -> GuardedCheckpointV2
  -> ws-ckpt snapshot and exact evidence
  -> terminal receipt or uncertain
```

Binding 从 Runtime 已 pin 的 workspace directory descriptor 读取 inode、Btrfs FSID 与
subvolume UUID，避免按路径重新打开后把另一个目录当成原 workspace。Approval binding v4
在 plan 阶段预分配并持久化 `permit_id` 与 `execution_id`；permit、claim、start、terminal、
source 和 delivery 都复用这组身份。Gateway 协议 v2 先完成 Admission discovery，再要求
Submit 回显完整 admission。既有 v1 Submit JSON 形状保持不变，但服务端只在
exact `task-only-v1` profile 下接受它；`workspace-checkpoint-v1` 必须使用 v2
admission echo，v1 Submit 会被明确拒绝。

恢复时，Gateway 核对历史 binding、request、effect 与 digest，并认证当前 socket 的受信任
祖先、owner 和 `SO_PEERCRED` UID。服务重启可以合法改变 socket inode，因此恢复过程不要求
当前 socket inode 等于历史值。它只查询 `CheckpointEvidenceV2`，绝不重放 create。

该实现不新增 `providers/ws-ckpt/provider.toml`。当前 AW Host 只承载 one-shot 能力，缺少
State Provider 的 service driver、binding lifecycle、reconcile 与 readiness 合同。PoC
已有 Runtime tool、Gateway codec 与 State Provider 代码，并在固定提交 `5ebfc0b3` 的干净
构建产物上完成 Ubuntu VM + Herdr 正常链路。VM 使用真实 Btrfs subvolume，核对 FSID 与
subvolume UUID，产生 snapshot、durable evidence 和 11 个连续 Task 事件。该结果仍不等于
生产可用，也没有覆盖 response-loss、crash/restart 与 evidence-only reconcile 的故障演练。

## Schema 作为架构基线

Schema 可以成为团队协作中心。每个能力都需要同时冻结四类材料。

1. AW canonical input 与 output Schema。
2. 组件 native request 与 response Schema。
3. manifest mapping 与权限预算。
4. typed validator、正反 fixture 和真实组件测试。

JSON Schema 负责形状。跨字段不变量由 typed validator 负责。最终执行或采用由 Runtime
evidence 负责。三个层次缺一项，字段设计仍可能在真实链路上失真。

当前还需要讨论 `media_type` 与 transform identifier 的注册表，以及 `retrievable` 的
resolver 与 TTL。PoC 已把 Tokenless `agent_id` 固定为已准入的 `aw-provider` 集成身份，
避免再把 AW environment instance 当成 frontend；如果未来需要真实前端归因，应新增
明确的 typed 字段，不能复用 instance ID。

## 管理层需要确认的四项决定

| 决定 | 建议 |
| --- | --- |
| 总体架构 | 保留 Capability、Core、Host、Provider、Environment、Ledger 分层 |
| 原 PR 合并 | 保持 Request changes，以 fork PoC 作为修正基线 |
| 安全门禁 | 将 exact executed-bytes binding 设为进入真实执行链的前置条件 |
| Checkpoint | 继续使用 Gateway State Provider；以已通过的 VM 正常链路为基线，再验证故障恢复与通用 AW 状态合同 |

构建、CI、签名安装和服务健康属于下一道验收。已通过的 Ubuntu VM 正常链路证明各组件
可以组合运行，但不能代替故障恢复与生产交付验收。
