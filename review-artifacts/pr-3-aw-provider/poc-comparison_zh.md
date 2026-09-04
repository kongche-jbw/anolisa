# PR 3 与 Agent Host PoC 对照

本文对照三份实现材料。

- 原 PR 为 [casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3)，固定头提交
  `42d07649409ecd5bb023056b28545efbd9325ef2`。
- 修正分支为
  [`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。
- Agent Host 思考位于
  [`ANOLISA-design/dev/kongche/global/2026-09-01_aweftos-agent-host`](https://code.alibaba-inc.com/Agentic-OS/ANOLISA-design/tree/dev/kongche/global/2026-09-01_aweftos-agent-host)。

三者关注的层次不同。Agent Host 负责主机、Task 和交付生命周期。AW Provider 负责一次
Agent 边界内的能力协作。fork PoC 负责把两层的权威边界接起来。

## 三层分别解决什么

| 层次 | 主要对象 | 持有的权威事实 |
| --- | --- | --- |
| Agent Host | 镜像、服务、Gateway Task、Run、workspace、健康 | 主机上安装和运行了什么 |
| AW capability plane | Capability、Plan、Provider、candidate、receipt | 为什么选择某项能力，Provider 产生了什么 |
| Agent Environment | Tool 参数、本地模型历史、最终 gate | 最终执行了什么，最终写入了什么 |

AW Core 不应宣称 candidate 已经进入模型历史。Provider Host 不应宣称命令已经被阻断。
这些事实属于 COSH。Gateway 也不应把 Checkpoint 的写后超时自动当作失败并重试，它需要
查询 ws-ckpt 的 durable evidence。

## 原 PR、修正 PoC 与产品目标

| 维度 | 原 PR 头提交 | fork PoC 基线 | 产品目标 |
| --- | --- | --- | --- |
| Capability Contract | 已有 canonical Schema | 收紧 ID、枚举和跨字段不变量 | 版本化发布与 conformance vectors |
| Host 校验 | 主要校验 Schema 文件与 digest | 四阶段校验运行实例 | 持续准入与逐 Provider 健康 |
| Agent Sec | one-shot bridge，`auto` 与终态有歧义 | 双扫描、判别联合、Receipt 输入绑定 | exact executed-bytes gate |
| Tokenless | 任务相关保留被当作 AW `lossless` | 计算精确 source fidelity | identity、policy 与产品升级闭环 |
| Tokenless 表示类型 | native 协议没有输入和输出媒体类型 | `application/json -> text/plain` 显式贯穿 request、response 与 candidate | MIME 类型注册表 |
| Host 返回 | output 与 receipt 边界不够醒目 | 显式返回 transient candidate 与 receipt 给 Core | 稳定服务协议 |
| Final adoption | Hook replacement 被当作接近最终结果 | COSH 写本地历史后记录 `context_adoption` | 主机状态与 trace 可查询 |
| Ledger | scope、header 和正文通道存在缺口 | typed body、完整 plan 覆盖、哈希承诺与受限引用 | 常驻 writer、锚定、retention |
| Checkpoint | 没有 AW 或 Gateway 新架构接线 | Runtime tool、Gateway State Provider、Guarded V2 与 Btrfs VM + Herdr 正常链路 | response-loss/crash-restart 恢复验收、通用状态合同 |
| 安装与健康 | source PoC | 分支级实现与演示脚本 | 签名包、systemd、`host verify` |

## 与 Agent Host 权威模型的对应关系

Agent Host 设计强调 `installed`、`admitted`、`bound`、`ready` 和 `effective` 是不同事实。
AW Provider 应沿用这套区分。

```text
主机交付事实
package -> installed service and provider assets

能力控制事实
discovered -> admitted -> policy bound -> ready -> planned -> invoked

最终环境事实
candidate -> COSH adopted
gate -> COSH executed, asked or blocked

长期审计事实
receipt + plan + context_adoption -> Ledger chain
```

`host verify` 将来应同时检查以下状态。

- AW service 和 Ledger writer 是否可用。
- Provider catalog 中每个 package 的准入结果。
- Capability 与环境的 policy binding。
- COSH effective-bytes boundary 是否启用。
- 最近一次 plan、receipt 与 final adoption 是否能关联。
- Checkpoint State Provider 的 profile、binding、ws-ckpt identity 与 reconcile 状态。

一个 Agent Sec daemon 显示 healthy，只能证明组件进程健康。AW mediation 还需要 Provider
准入、policy binding、COSH gate 和 executed-bytes credential。

## Tokenless 如何连接两层

Tokenless package 提供 binary、manifest 和 Schema。Agent Host 负责把这些资产以可验证方式
安装到主机，并让 AW service 发现它们。AW Core 与 Provider Host 负责一次调用。COSH 负责
最终采用。

```text
Agent Host installs Tokenless package
  -> AW Host admits tokenless manifest
  -> Policy binds context.projection.prepare/v1
  -> COSH submits settled provisional bytes
  -> Host returns candidate + receipt to Core
  -> Core validates source identity, media and contract state
  -> Ledger records complete post_tool_use_plan
  -> COSH selects a non-empty lossless candidate or preserves source
  -> Ledger records context_adoption referencing the plan
  -> Agent Host projects health and recent evidence
```

fork PoC 已证明中间的源码调用链和 COSH 本地采用。签名包、常驻服务、升级、回滚和统一
健康投影仍属于 Agent Host 产品化阶段。

COSH 当前提交的是模型历史文本槽，因此调用方政策固定允许文本重编码。公共合同仍支持
`allow_text_reencoding=false`，真实 Tokenless 与 Core 测试已覆盖
`application/json -> application/json`；其他 adapter 可以使用该分支。

## Checkpoint 为什么由 Gateway 承接

Checkpoint 与 Tokenless 的运行性质不同。

| 属性 | Tokenless | Checkpoint |
| --- | --- | --- |
| 结果性质 | 瞬时 candidate | 持久副作用 |
| 响应丢失 | 可以保留 source bytes | 可能已经创建快照 |
| 重试风险 | 再次计算候选 | 可能重复执行副作用 |
| 所需状态 | 一次调用上下文 | binding、approval、claim、start、evidence、terminal |
| 当前承载 | AW Provider Host | Gateway State Provider |

Gateway 已拥有 Task、Run、Attempt、Approval 与 durable ledger，也能持久化 provider binding。
PoC 因此让 Gateway 调用 Guarded Checkpoint V2。ws-ckpt 用 `operation_digest` 保存 durable
evidence。Gateway 在恢复时只查询 exact evidence，缺失或不匹配时保持 `uncertain`。

当前没有 `providers/ws-ckpt/provider.toml`。这项选择避免把有状态副作用强行塞进 one-shot
AW Host。PoC 已在 `workspace-checkpoint-v1` profile 下注册 Runtime tool，并让它通过
brokered scheduler 与私有 control request 进入 Gateway。Driver 使用 Runtime 已 pin 的
workspace directory；binding 提交 workspace inode、Btrfs FSID/subvolume UUID、调用方与
服务端身份、`permit_id` 和 `execution_id`。固定提交 `5ebfc0b3` 已完成 Ubuntu VM + Herdr
正常链路，故障恢复和生产交付仍未验收，因此一次 happy path 成功不能等同于生产可用。

Gateway wire v2 先执行 Admission discovery，再要求 Submit 回显完整 admission。旧版
`cosh.gateway.v1` Submit JSON 形状保持冻结，但只允许 exact `task-only-v1`；
`workspace-checkpoint-v1` 必须使用 v2 admission echo，服务端明确拒绝 v1。恢复只
认证当前 socket 的受信任目录、owner
与 peer UID，并查询 `CheckpointEvidenceV2`；它不会因服务重启后 inode 改变而重放 create。

## 仍需增加的连接层

### 运行连接

系统需要常驻 AW service 或等价 runtime，持有 catalog、policy binding、Ledger writer 与
服务级资源治理。COSH 使用稳定 client contract 调用。Provider 的 OS sandbox、cgroup 和
executable identity 也在这一层落实。

### 状态连接

Agent Host 状态模型应加入 AW service、catalog revision、capability graph、COSH boundary、
Ledger chain tip、final adoption 和 Checkpoint State Provider readiness。每个状态都要保留
失败原因。

### 关联连接

Gateway Task 和 Run、COSH session 和 turn、AW invocation、Provider Receipt、Ledger event、
AgentSight trace 与 ws-ckpt evidence 需要受信任映射。ID 用于归因，授权继续由 peer identity
和 system policy 产生。

### 安全连接

PreTool gate 需要在全部参数 patch 完成后绑定最终执行摘要。当前 Receipt 只证明 Agent Sec
扫描了指定 canonical input。真正的执行闭环还需要 COSH 验证
`digest(scanned_command) == digest(executed_command)`。

## 建议演进顺序

1. 以 fork PoC 的 Schema、Receipt、Ledger 与 PostToolUse final adoption 为实现基线。
2. 完成 PreTool exact executed-bytes binding。
3. 保留已通过的 Ubuntu VM 正常链路作为真实 Agent Sec、Tokenless、COSH、Ledger、
   Gateway、Herdr 与 ws-ckpt 组合基线。
4. 注入 response loss 与 crash/restart，验证 Checkpoint query-only reconcile 不重放 create。
5. 再加入签名包、systemd、健康投影、升级、回滚与 CI 门禁。

这个顺序先固定语义，再固定运行接线，最后完成交付。原 PR 的架构骨架得到保留，Agent
Host 的权威事实也不会被 Provider 中间状态覆盖。
