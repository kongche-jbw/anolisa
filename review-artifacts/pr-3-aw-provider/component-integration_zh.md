# AW Provider 组件接入手册

本文面向第一次接入 AW 的组件研发、集成和测试人员。读者需要了解 JSON、命令行程序、
stdin、stdout 和 SHA-256 的基本概念。完成本文步骤后，组件团队应能定义 native contract、
编写 manifest、提供测试 fixture，并准确说明自己的结果何时生效。

当前参考实现位于
[`kongche-jbw/anolisa:feat/aw/provider-e2e-poc`](https://github.com/kongche-jbw/anolisa/tree/feat/aw/provider-e2e-poc)。
原 PR 头提交为 `42d07649409ecd5bb023056b28545efbd9325ef2`。

配套材料如下。

- [Schema 参考与语义讨论](schema-reference_zh.md)
- [Provider 总体架构](provider-effect-architecture.html)
- [Tokenless 真实字段时序](provider-effect-sequence.html)
- [安全与 Checkpoint 运行实例](runtime-call-examples_zh.md)

## 接入完成后的数据流

无状态 Provider 接入完成后，一次调用应走完整往返链路。

```text
Environment
  -> AW Core canonical input
  -> Provider Host instance validation
  -> manifest mapping
  -> component native request
  -> component native response
  -> Provider Host canonical output + receipt
  -> AW Core typed interpretation
  -> AW Ledger typed plan or gate fact
  -> Environment final effect
  -> AW Ledger context_adoption when local history changes
```

Provider Host 返回两种对象。Transient output 可以在当前调用中携带业务正文或 finding。
`ProviderReceipt` 不携带正文，只绑定 input schema、input digest、manifest、output identity、
scope、终态和 meters。

Provider 团队完成 native response 后，工作还没有结束。Core 必须解释结果，Environment
必须完成最终动作。Advise 类能力只有在 Environment 采用 candidate 后才生效。Mediate
类能力只有在 Environment 实际执行、询问或阻断后才生效。

## 基本术语

| 术语 | 含义 |
| --- | --- |
| Capability | 与具体组件无关的稳定能力名称 |
| Authority | 能力被允许产生的影响范围 |
| canonical Schema | AW 团队维护的公共输入输出合同 |
| native Schema | 组件团队维护的进程协议合同 |
| manifest | 连接 canonical 与 native 合同的 `provider.toml` |
| candidate | Provider 提出的瞬时结果，尚未被采用 |
| receipt | 不含正文的调用来源与终态证明 |
| final adoption | Environment 已将某段字节写入本地模型历史 |
| State Provider | 处理持久副作用，并支持 binding 与 reconcile 的 Provider |

## 第一步 选择 Capability 与 Authority

组件团队应先与 AW 团队确认能力名称、版本、Scope 和 Authority，随后再设计字段。

| Authority | 允许的行为 | 常见组件 |
| --- | --- | --- |
| Observe | 返回事实，不直接改变执行或模型内容 | Agent Sec 内容与代码扫描 |
| Advise | 返回候选，由 Environment 选择 | Tokenless Context Projection |
| Mediate | 提供执行门禁意见 | Agent Sec 命令检查 |
| State effect | 修改持久状态并支持恢复 | ws-ckpt，经 Gateway State Provider |

当前通用 AW manifest Host 支持 one-shot Observe、Advise 和 Mediate。持久副作用暂时使用
Gateway State Provider。组件不能为了复用现有 Host，把副作用伪装成普通 `produced`
candidate。

## 第二步 定义 canonical 语义

Canonical Schema 表达跨组件稳定的事实。以 Context Projection 为例，输入至少需要 source
artifact、boundary 和 constraints，输出需要 source identity、candidate 与 reversibility。

设计 canonical 字段时应逐项回答以下问题。

- 字段由谁产生，是否来自受信任边界。
- digest 覆盖哪种编码和哪一段准确字节。
- ID 在重试、进程重启和跨组件关联中的稳定范围。
- 枚举值属于业务结论、执行终态还是失败原因。
- 正文是否只允许瞬时存在，能否进入 Receipt 或 Ledger。

JSON Schema 负责字段形状。跨字段规则必须进入 typed validator。例如 `clean` 不能同时携带
finding 或 `truncated=true`。候选标记为 `lossless` 时，组件必须能够保留全部 source
information。

## 第三步 定义 native endpoint

无状态 Provider 使用 `exec-json/v1` one-shot endpoint。推荐遵守下列进程合同。

- stdin 读取一个完整 JSON document，读到 EOF 后开始处理。
- stdout 只输出一个 JSON document。
- 业务终态使用结构化 disposition 表达。
- crash、协议损坏和基础设施错误使用非零退出码。
- stderr 只用于受限诊断，不能成为业务正文或长期审计正文。
- 程序不依赖 ambient `PATH`、`HOME` 或未声明环境变量。
- 输入、输出、时间、网络、文件系统和持久化需求均在 manifest 中声明。

Native Schema 应使用判别字段区分操作。Agent Sec 的三个操作共享一个 endpoint 时，
`operation` 决定允许出现哪些 request 和 response 字段。不能把所有字段放进一个可选字段袋。

## 第四步 编写 Provider package

建议的源码目录如下。

```text
providers/<provider-id>/
├── provider.toml
├── schemas/
│   ├── canonical-input.schema.json
│   ├── canonical-output.schema.json
│   ├── native-request.schema.json
│   └── native-response.schema.json
├── fixtures/
└── README.md
```

`provider.toml` 至少声明下列信息。

- Provider identity、version 与 executable。
- Capability identity、version、Authority 和 Scope。
- canonical 与 native Schema identity 和 digest。
- request 与 response 的 `json-map/v1` 规则。
- timeout、最大输入、最大输出和环境变量。
- 网络、文件系统、retention 与 telemetry 权限。

Host 不应包含 `if provider_id == "tokenless"` 一类分支。组件差异全部进入 native endpoint
和 manifest mapping。所有 Schema digest 都应由测试从文件重算。

## 第五步 建立四阶段校验

fork PoC 在四个位置校验运行实例。

1. Core 交给 Host 的 canonical input。
2. mapping 产生的 native request。
3. Provider 返回的 native response。
4. mapping 产生的 canonical output。

每一步失败都应得到稳定的 Provider failure 或 gap，不能继续使用部分对象。Schema 解析
成功和 digest 匹配只能证明合同文件存在，无法代替 instance validation。

## 第六步 返回 output 与 receipt

组件返回 native response 后，Host 按 manifest 映射为 canonical output。Host 必须把业务
结果与 Receipt 一起返回 Core。

```text
ProviderInvocationOutcome
  output                 transient canonical result
ProviderReceipt
    input_schema
    input_digest
    manifest_digest
    output_schema
    output_digest
    output_bytes
    scope
    disposition
    meters
```

Receipt 的 input digest 覆盖完整 canonical input，而 source digest 只覆盖业务正文。二者
通常不同。Receipt 不能复制 command、Tool Result、candidate content 或 finding 原文。

Provider 可控的 `rule_id`、meter method、media type 与 transform name 也需要防止形成隐蔽
正文通道。长期记录应使用封闭枚举、摘要或来自 admitted manifest 的受限 metadata。

## 第七步 由 Environment 完成最终动作

### Advise 类能力

Tokenless 返回 candidate 后，Core 校验 source artifact、source digest、candidate digest 与
reversibility。COSH 使用下列规则写入本地模型历史。

| 状态 | 写入内容 | adoption reason |
| --- | --- | --- |
| 非空且严格 `lossless` | candidate bytes | `lossless_candidate` |
| 没有 candidate | source bytes | `no_candidate` |
| candidate 为空 | source bytes | `empty_candidate` |
| candidate 不可恢复全部源信息 | source bytes | `candidate_not_lossless` |

写入成功后，COSH 追加 `context_adoption/v1`。Ledger 保存 effective digest 和字节数，不保存
effective bytes。该记录只证明本地历史槽位，不能证明远端模型已经消费消息。

`post_tool_use_plan/v1` 不能遗漏计划中的 Observe step。每个 content/code inspection 必须
恰好由 observation 或 gap 说明；fan-out 时每个 `(capability, provider_id)` 只能出现一次。
Invocation reference 的 attempt、tool use、disposition、output identity 和 error code 必须与
receipt 及 Ledger header 一致。Adoption 再引用同一 plan 和同一组 invocation，避免把两次
Tool Call 的证据拼在一起。

### Mediate 类能力

安全 Provider 的 verdict 需要由 Core 映射为 typed gate，再由 COSH 聚合全部 gate。检查凭据
必须绑定最终执行字节。

```text
digest(scanned_command) == digest(executed_command)
```

PoC 已让 Receipt 绑定扫描输入，最终 executed-bytes binding 尚待接线。组件测试不能把
Agent Sec 返回 `warn` 当作命令已经被 COSH 警告或阻断的证明。

## Tokenless 接入实例

固定 fixture 的 source Tool Result 为 693 bytes，SHA-256 为
`01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1`。

Host 生成的主要 native request 字段如下。

```json
{
  "protocol_version": 1,
  "content": "{\"builds\":[...]}",
  "input_media_type": "application/json",
  "agent_id": "aw-provider",
  "session_id": "ags_11111111-1111-4111-8111-111111111111",
  "tool_use_id": "tol_66666666-6666-4666-8666-666666666666",
  "tool_name": "list_recent_builds",
  "seam": "post_tool",
  "content_origin": "api_response",
  "capabilities": {
    "replace_output": true,
    "publish_retrieve_tool": false,
    "replace_with_text": true
  }
}
```

Tokenless 返回 438 bytes 的 Toon 表示，token 估算为 `174 -> 110`，转换链为 `toon`。
native `output_media_type` 为 `text/plain`，Host 将它映射为 candidate media type。
Candidate effective digest 为
`6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003`。

如果 source fidelity 为 `lossless`，COSH 可以采用 candidate 并记录 `adopted`。如果压缩器
删除 `debug`、`trace`、空字段或其他无法恢复的信息，Provider 必须报告
`unrecoverable`，COSH 保留 source bytes。

PoC 将 Tokenless `agent_id` 固定为已准入常量 `aw-provider`。AW environment instance
identity 只出现在 invocation scope 与 receipt 中，两类身份不再混用。如果后续需要按
真实 frontend 统计，应在 canonical contract 中新增明确字段并做版本化演进。

如果 `allow_text_reencoding=false`，manifest 把 `replace_with_text` 设为 false，同时继续
传递 `input_media_type=application/json`。本例会得到 `no_savings`、`174 -> 174` 并保留
693-byte source；如果另一份输入的结构化压缩确有收益，Core 仍要求 candidate media type
保持 `application/json` 且正文可以重新解析为 JSON。

这项 false 分支是公共合同的 conformance 路径。当前 COSH effective-bytes 调用处理模型
历史文本槽，并按调用方政策固定允许文本重编码。其他 adapter 可以传 false；未来若 COSH
需要可配置结构化槽位，再扩展它的 request 类型。

## Agent Sec 接入注意事项

Agent Sec `language=auto` 需要同时覆盖 Bash 与 Python 规则，再返回实际检测语言。完整扫描
且没有 finding 才能返回 `clean`。部分扫描已经发现风险时，可以返回
`suspicious` 或 `sensitive` 并保留 `truncated=true`；没有可验证事实的失败进入 gap。
截断结果的 `scanned_bytes` 必须是实际解码并扫描的完整 UTF-8 前缀，不能直接填写配置的
byte limit。Core 还会核对 receipt 中 `security.scanned_bytes` 与
`security.findings_total` 的 meter ID、unit、measurement kind 和数值。

一条命令检查会经历以下状态。

```text
canonical security.command.inspect input
  -> native command_inspect request
  -> native completed response
  -> canonical allow, warn or deny decision
  -> Core gate
  -> COSH final execution decision
```

组件只拥有 native 扫描结论。Core 拥有 canonical gate 解释。COSH 拥有最终执行权。

## Checkpoint 等有状态组件

有状态组件在写后失联时可能已经产生副作用。接入前需要回答以下问题。

- 操作身份是否在重启后稳定。
- binding 是否固定 workspace、服务端身份与调用方身份。
- pre-effect rejection 能否证明没有写入。
- durable evidence 能否按 exact operation identity 查询。
- 证据缺失时是否保持 `uncertain`。
- reconcile 是否只查询，绝不盲目重放 create。

当前 Checkpoint 参考实现位于 Gateway State Provider。它使用 Guarded Checkpoint V2 调用
ws-ckpt，并把 `operation_digest` 写入 durable evidence。服务端 socket identity 由 device、
inode 和 daemon UID 组成，Gateway caller UID 单独保存。

Driver 使用 Runtime 已 pin 的 workspace directory，不按路径重新建立 workspace 身份。
Binding 还提交 Btrfs FSID/subvolume UUID generation、`permit_id` 与 `execution_id`；批准、
claim、start、terminal、source 和 delivery 全部复用同一 execution identity。恢复阶段验证
历史 request、effect、binding 与 digest，并认证当前 socket 的受信任目录、owner 和 peer
UID，但允许服务重启后 socket inode 改变。

当前不能新增 `providers/ws-ckpt/provider.toml` 并宣称接入完成。AW 还没有通用 State
Provider 的 service driver、binding lifecycle、reconcile 和 readiness contract。PoC 已在
`workspace-checkpoint-v1` profile 下接入 Runtime tool 与 Gateway 私有控制协议。固定提交
`5ebfc0b3` 已通过 Ubuntu VM + Herdr 正常链路，创建真实 Btrfs snapshot 并将 Task 推进到
`task_succeeded`。crash/restart 与 response-loss 的 evidence-only 恢复仍待故障注入验收。

## 测试矩阵

### Schema 与 mapping

- 每份 canonical 与 native Schema 都有正例、边界例和非法例。
- Schema validator 与语言内 typed validator 使用相同 conformance vectors。
- Manifest digest、组件版本与 package inventory 一致。
- 未知字段、错误枚举、超长数组和 Unicode 长度差异均有测试。

### Provider Host

- real binary、real manifest、real Host 与 real Core 在默认测试中组合运行。
- timeout、oversize、非 UTF-8、非法 JSON、非零退出与提前退出都有稳定错误。
- mapping 数量和复制大字段不能在预算检查前放大内存。
- Provider 子进程在每个 terminal path 都被可靠清理。

### Environment

- PostTool source bytes 来自全部普通 Hook 聚合后的实际候选槽位。
- 非 lossless candidate 保留 source bytes。
- `context_adoption` 与前一条 typed plan、source 和 invocation refs 一致。
- PreTool scanned digest 与最终 executed digest 一致。

### State Provider

- 写后超时、断连和 daemon 重启不会导致重复创建。
- Exact evidence 能恢复成功，缺失或不匹配保持 `uncertain`。
- Provider binding 变化会拒绝旧操作。
- Runtime tool 真实触发路径单独验收，不能只测底层 driver。
- ws-ckpt 的 V1 wire variant 顺序和字段保持不变，Guarded V2 只作为新增分支。
- Gateway 旧 `cosh.gateway.v1` Submit JSON 保持原形，但只接受 exact `task-only-v1`。
- `workspace-checkpoint-v1` 必须使用 v2。client 先做 Admission，Submit 回显完整
  admission；服务端拒绝 checkpoint v1 Submit。

## 接入评审清单

- Capability、Authority、Scope 和版本已经确认。
- Canonical 与 native Schema 的字段责任没有重叠或空缺。
- 跨字段不变量进入 typed validator。
- Host 中没有组件名称分支。
- Receipt 绑定输入与 manifest，并保持 content-free。
- Environment 的最终采用或执行证据已经接线。
- 失败、gap、degradation 与业务 verdict 使用不同类型。
- 有副作用能力具备 durable evidence 与 query-only reconcile。
- 安装、ready、bound、invoked 与 effective 状态可以分别观测。

构建、RPM、镜像与 CI 接线可以在合同稳定后完成。交付前仍需把它们加入正式验收。
