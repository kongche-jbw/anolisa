# AW Provider 架构说明

本文档面向管理层、架构负责人、组件负责人和第一次接触 AW 的研发人员。文档从一个真实
Tokenless 例子出发，解释 Provider 为什么存在、由哪些层组成、Schema 如何连接不同组件，
以及安装后的代码在什么条件下才真正生效。

配套交互图如下。

- [AW Capability Provider 总体架构](provider-effect-architecture.html)
- [带真实字段值的 Tokenless 调用时序](provider-effect-sequence.html)
- [Agent Sec 命令检查时序](security-command-call.html)
- [当前 Checkpoint 创建时序](checkpoint-create-call.html)
- [安全与 Checkpoint 运行实例说明](runtime-call-examples_zh.md)
- [全部 Schema 图册](schema-reference_zh.md)
- [组件开发与接入手册](component-integration_zh.md)

## 1. 结论

PR 3 的架构方向正确，值得作为后续改造的基础。它把 Agent Environment、AW Core、
Provider Host、Capability Provider 和 Ledger 的职责分开，并通过 canonical Schema、
native Schema 与声明式 mapping 连接。新增组件不需要在 Core 或 Host 中增加组件名称分支。

当前实现仍是 source POC。源码已经形成可运行主链，但系统安装不会自动启用 AW，真实
跨组件测试默认不运行，部分安全与语义不变量也尚未闭合。因此当前可以确认“架构骨架成立”，
不能确认“Provider 已在产品中稳定生效”。

总体图中的返回方向必须与请求方向同等明确。Provider 的 native response 先回到 Host，
Host 生成 `ProviderInvocationOutcome + ProviderReceipt` 并返回 Core。Core 才能验证
Tokenless 压缩正文或 Agent Sec verdict，将 candidate 或 gate 送往 Final Adoption，并把
无正文的调用事实送往 AW Ledger。缺少 `Provider -> Host -> Core` 的结果回流，就不存在
可验证的最终采纳和审计链路。

Schema 可以作为下一阶段改造中心。本 PR 涉及 22 个物理 JSON Schema 文件，去除 Provider
包内逐字副本后为 14 个逻辑 Schema。冻结 Schema 时必须同步冻结 Rust/Python 类型、
manifest mapping、真实 Provider 行为和 conformance tests，不能只修改 JSON 文件。

## 2. Provider 解决的问题

假设 COSH 调用 `list_recent_builds`，工具返回一段很长的 JSON。系统希望同时完成两件事。

1. Agent Sec 检查返回内容中是否包含凭据、敏感信息或危险代码。
2. Tokenless 删除不影响任务的信息并压缩上下文，减少下一次模型请求的 token 数。

如果 COSH 直接调用每个组件，就需要理解 Agent Sec 和 Tokenless 的私有协议、错误码、
版本、权限、超时与升级方式。每增加一个组件，COSH 都要增加新的专用逻辑。

AW Provider 把问题拆成稳定能力和可替换实现。

| 稳定 Capability | 当前 Provider | Authority | 作用 |
| --- | --- | --- | --- |
| `security.content.inspect/v1` | Agent Sec | Observe | 报告敏感内容事实 |
| `security.code.inspect/v1` | Agent Sec | Observe | 报告代码风险事实 |
| `security.command.inspect/v1` | Agent Sec | Mediate | 影响 Tool Call 是否执行 |
| `context.projection.prepare/v1` | Tokenless | Advise | 提供可进入模型的候选表示 |

COSH 只需要知道需要哪项 Capability。AW 负责选择一个满足版本、Scope、Authority、Policy
和健康状态的 Provider。Provider 仍保留自己的 native protocol 和算法实现。

## 3. 五层架构

| 层次 | 当前实现 | 核心职责 | 不拥有的权力 |
| --- | --- | --- | --- |
| Agent Environment | COSH | 提供真实 Tool 边界，聚合 Hook，决定最终执行或入模内容 | 不决定 Provider 内部算法 |
| AW Core | `aw-core` | 建立 Plan、应用 Policy、按 Contract 精确路由、验证业务结果 | 不解析 Tokenless 私有协议 |
| Provider Host | `aw-provider-host` | 发现、准入、字段映射、预算、进程执行、Receipt | 不决定 Allow、Block 或最终采用 |
| Capability Provider | Tokenless、Agent Sec | 执行压缩或扫描并返回 native 结果 | 不得扩大 manifest 声明的 Authority |
| Ledger | `aw-ledger` | 记录边界事实、摘要和调用 Receipt | 不应保存 Tool 正文或候选正文 |

这五层的核心原则是权威事实不混用。

- COSH 知道系统最后执行了什么、模型最后看到了什么。
- Core 知道为什么选择某个 Capability Provider。
- Host 知道实际执行了哪个 manifest 和进程。
- Provider 知道自己的算法产生了什么结果。
- Ledger 只记录被允许持久化的事实。

## 4. 两个 Schema 世界

AW 与组件使用两套不同层次的 Schema。

### 4.1 Canonical Schema

Canonical Schema 由 AW 团队维护，表达跨 Provider 稳定的能力语义。例如 Context
Projection 输入包含：

```text
artifact { id, digest, content, media_type, origin, tool_name }
boundary
constraints { allow_text_reencoding }
```

同一 canonical input 可以交给 Tokenless，也可以交给未来另一个压缩 Provider。

### 4.2 Native Schema

Native Schema 由组件团队维护，表达组件自己的 stdin/stdout 协议。例如 Tokenless
`CompressionRequest` 包含：

```text
protocol_version
content
agent_id
session_id
tool_use_id
tool_name
seam
content_origin
capabilities
```

### 4.3 Manifest mapping

`provider.toml` 声明 canonical 字段如何变成 native 字段。Provider Host 只执行通用
`json-map/v1`，不包含 Tokenless 专用代码。

| 变化类型 | Tokenless 例子 | 含义 |
| --- | --- | --- |
| 复制 | `artifact.content → content` | 值保持不变 |
| 重命名 | `boundary → seam` | 同一语义使用组件字段名 |
| 跨层映射 | `environment_id → agent_id` | 当前存在语义错配，需要修复 |
| 常量注入 | `protocol_version=1` | 值由 manifest 固定 |
| 新计算 | `artifact.id`、digest | 值由 Core 或 Host 计算 |

这种设计的价值是组件接入信息位于可审查的 Provider package 中，而不是散落在 Core、Host
和 COSH 的条件分支里。

## 5. 一个真实 Tokenless 数据样例

本节的数据来自 PR 中的 PostToolUse fixture 和真实 Tokenless 集成测试。长数组仅缩写为
`…`，稳定字段值保持真实。

### 5.1 COSH Tool Result

工具名是 `list_recent_builds`，模型可见正文来自 `tool_response.llmContent`。

```json
{
  "tool_name": "list_recent_builds",
  "tool_response": {
    "llmContent": "{\"builds\":[...]}"
  },
  "execution_scope": {
    "environment_id": "env_33333333-3333-4333-8333-333333333333",
    "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
    "actor_id": "act_55555555-5555-4555-8555-555555555555",
    "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
    "turn_id": "trn_22222222-2222-4222-8222-222222222222",
    "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
  }
}
```

### 5.2 AW canonical input

Core 计算 source digest 和 artifact id，再建立 Provider 无关的能力输入。

```json
{
  "artifact": {
    "id": "art_c8f93696-03b8-804e-923a-1fcf9a4d7ac7",
    "digest": "612b377d40f7b6d00e03ea08831661702487ecd7f9d21631ea9e8d173da6c88f",
    "content": "{\"builds\":[...]}",
    "media_type": "application/json",
    "origin": "api_response",
    "tool_name": "list_recent_builds"
  },
  "boundary": "post_tool",
  "constraints": {
    "allow_text_reencoding": true
  }
}
```

### 5.3 Tokenless native request

Host 按 manifest 复制、重命名并注入字段，形成发送给 `tokenless compress` 的 stdin。

```json
{
  "protocol_version": 1,
  "content": "{\"builds\":[...]}",
  "agent_id": "env_33333333-3333-4333-8333-333333333333",
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

### 5.4 Tokenless native response

真实集成 fixture 的 token 估算从 359 降为 110。

```json
{
  "protocol_version": 1,
  "output": "builds[6]{id,project,status,duration_ms,owner}: ...",
  "disposition": "applied",
  "content_type": "json",
  "compressor_chain": ["response-cleanup", "toon"],
  "reversibility": "lossless",
  "before_tokens": 359,
  "after_tokens": 110,
  "stash_keys": [],
  "tokenizer_id": "heuristic-v1"
}
```

### 5.5 AW canonical candidate

Host 回填原始 source identity，将 native output 映射成 AW candidate。

```json
{
  "candidate": {
    "source_artifact_id": "art_c8f93696-03b8-804e-923a-1fcf9a4d7ac7",
    "source_digest": "612b377d40f7b6d00e03ea08831661702487ecd7f9d21631ea9e8d173da6c88f",
    "content": "builds[6]{id,project,status,duration_ms,owner}: ...",
    "media_type": "text/plain",
    "transform_chain": ["response-cleanup", "toon"],
    "reversibility": "lossless"
  }
}
```

### 5.6 Receipt 与 replacement

下面是 `ProviderReceipt` 中稳定字段的真实节选。`invocation_id`、`output_digest`、
`output_bytes` 和时间戳由每次运行产生，因此不伪造固定值。Receipt 不保存 candidate
content。

```json
{
  "provider_id": "tokenless",
  "provider_version": "0.7.14",
  "manifest_digest": "6b48b57238189360ba5de7f902d76300261d76d1d68af4ef542b044e18b98a32",
  "binding_id": null,
  "provider_generation": null,
  "capability": {
    "id": "context.projection.prepare",
    "version": 1
  },
  "scope": {
    "target": {
      "kind": "host",
      "authority": "local",
      "identifier": "test-host"
    },
    "environment_id": "env_33333333-3333-4333-8333-333333333333",
    "execution_context_id": "ctx_44444444-4444-4444-8444-444444444444",
    "actor_id": "act_55555555-5555-4555-8555-555555555555",
    "agent_session_id": "ags_11111111-1111-4111-8111-111111111111",
    "work_id": null,
    "attempt_id": null,
    "turn_id": "trn_22222222-2222-4222-8222-222222222222",
    "tool_use_id": "tol_66666666-6666-4666-8666-666666666666"
  },
  "disposition": "produced",
  "output_schema": {
    "id": "context.projection.prepare.output",
    "version": 1
  },
  "meters": [
    {
      "meter_id": "context.source_tokens",
      "unit": "tokens",
      "measurement_kind": "estimate",
      "method": "heuristic-v1",
      "value": 359
    },
    {
      "meter_id": "context.prepared_tokens",
      "unit": "tokens",
      "measurement_kind": "estimate",
      "method": "heuristic-v1",
      "value": 110
    }
  ],
  "error": null,
  "evidence": []
}
```

AW Hook 进一步向 COSH 返回：

```json
{
  "suppressOutput": true,
  "systemMessage": "AW · tokenless · estimated context 359→110 tokens · saved 69%",
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "updatedToolResponse": "builds[6]{id,project,status,duration_ms,owner}: ..."
  }
}
```

Provider 返回 `applied` 只说明 Tokenless 生成了输出。Host 的 `produced` 只说明形成了
candidate。只有 COSH 聚合全部 Hook 后采用 `updatedToolResponse`，压缩内容才真正进入
下一次模型上下文，Tokenless 的 Advise 才算生效。

## 6. Provider 从安装到生效的状态链

```text
Packaged
  → Installed
  → Discovered
  → Admitted
  → Ready / Routable
  → Selected by Policy
  → Invoked
  → Settled
  → Effective
```

| 状态 | 成立条件 | 尚不能证明什么 |
| --- | --- | --- |
| Installed | binary、manifest、schemas 已放入目录 | Host 已发现或会调用 |
| Admitted | identity、路径、Schema digest、executable 通过准入 | Policy 会选中 |
| Ready | Capability descriptor 可路由 | Provider 已成功处理真实请求 |
| Invoked | Host 已启动 Provider 并发送 native request | 结果满足合同 |
| Settled | 返回 produced、bypassed、denied 或 failed | Environment 已执行或采用 |
| Effective | 结果已在 Authority 对应边界生效 | 无 |

当前 Tokenless RPM 或 raw 包可以安装 Provider 资产，但 AW runtime 与 Hook 没有形成统一
产品安装路径。当前运行仍需要手工配置 `aw-cosh-hook` 的 manifest root、executable root
和 declared-not-enforced opt-in。安装 Provider 不等于 Provider 生效。

## 7. 三种 Authority 的生效条件

| Authority | Provider 输出 | 真正生效时刻 | 最终权力 |
| --- | --- | --- | --- |
| Observe | finding、统计或分类事实 | 事实被 Core 接受，并对策略、用户或审计面可见 | Core / Environment |
| Advise | candidate | Environment 最终采用 candidate | Environment |
| Mediate | allow、warn、deny 素材 | Environment 实际执行、询问或阻断 Tool Call | Environment |

Authority 的分离是该架构最重要的治理价值。统计 Provider 不会自动获得阻断权，安全
Provider 也不能因为返回一个 verdict 就绕过 Environment 的最终聚合与执行逻辑。

## 8. 当前实现与目标产品

| 能力 | 当前 PR | 产品目标 |
| --- | --- | --- |
| Provider package | Tokenless 已进入 Make/raw/RPM；Agent Sec 未完整打包 | binary、manifest、schemas 原子安装 |
| Discovery | 显式 manifest root，每次 Hook 临时发现 | 统一根目录、逐包健康、reload |
| Capability Graph | 可生成 exact descriptor，成功项统一 Ready | 持久图，表达 Degraded/Unavailable 与原因 |
| Routing | 按 Capability、Authority、Scope、Schema digest 精确匹配 | Policy binding 可观测、可回滚 |
| Execution | one-shot、env clear、deadline 和 output cap | OS sandbox、完整进程后代监督、总预算 |
| Schema enforcement | 校验 Schema 文件和 digest | 对 request/response 做 instance validation |
| Ledger | Hook 进程内 SQLite writer | 常驻可靠 writer 与外部锚定 |
| Adoption | Hook 返回 replacement request | COSH 回报最终 adopted digest |
| Host status | Agent Host POC 尚不展示 AW | `anolisa top` / `host verify` 展示完整状态 |

PR 的能力执行面与现有 Agent Host POC 的状态和交付面方向互补。下一阶段需要增加连接层，
不应让 CLI 重新实现 Provider 调度，而应由 AW service 提供 graph、policy、writer 和健康事实。

## 9. Schema 冻结前的主要问题

### 9.1 Tokenless 与 AW 的 lossless 语义不同

AW 的 `lossless` 表示保留全部 source information。Tokenless 的定义是未移除
task-relevant information。Tokenless 会删除 `debug`、`trace` 和空字段后仍可能返回
lossless，manifest 又把该值直接映射给 AW。当前 Hook 因此可能采用不可逆结果。

修复原则是保留 AW 的强定义。Tokenless AW 模式只应在能够恢复全部源信息时返回
lossless；发生字段删除时应返回 `unrecoverable` 或提供完整 retrievable 合同。

### 9.2 被扫描字节与最终执行或入模字节不一致

COSH 当前先对 HookInput 做 redaction。AW 可能扫描或压缩脱敏副本，随后系统仍执行或入模
原始值。source digest、安全结论和最终行为因此可能绑定不同字节。Environment 必须固定
一份 governed bytes，并让扫描、替换、执行和 Ledger correlation 使用同一 digest。

### 9.3 Agent Sec 的 language=auto 会漏检 Python

Canonical Schema 允许 `auto`，Core 固定发送 `auto`，Agent Sec 当前却把除显式 Python
外的输入都按 Bash 扫描。普通 Python 风险可能得到错误 clean。v1 应移除 auto，或定义
可靠检测、双扫描和 unknown 降级规则。

### 9.4 Schema 声明不等于运行时验证

Host 当前确认 Schema 文件是合法 JSON 并核对 SHA-256，但不会使用这些 Schema 验证实际
native request 和 response。当前主要依靠字段映射和 Core 的 Rust typed decode 兜底。
若对外声明 Schema enforcement，应在 admission 后缓存 validator，并验证四个数据边界。

### 9.5 Schema 与 Rust 类型不完全等价

JSON Schema `maxLength` 按 Unicode 字符计数，Rust `BoundedName` 按 UTF-8 bytes 计数；
部分 tool name 上限是 256 字符，而 Rust 只接受 128 bytes；artifact id pattern 也没有与
Rust canonical ID 对齐。需要共享 conformance vectors，而不是分别测试后假设一致。

## 10. 团队责任

| 团队 | 交付责任 |
| --- | --- |
| AW Contract/Core | canonical Schema、Authority、Plan、Policy、typed invariant |
| AW Host | 准入、mapping、预算、sandbox、进程监督、receipt |
| Tokenless/Agent Sec | native Schema、算法语义、真实 binary tests、资源和副作用声明 |
| COSH Environment | 真实边界字节、Hook 聚合、最终执行与 adoption 回报 |
| 发布与 Agent Host | 原子包、统一 root、激活、健康、升级、回滚和 KVM 验收 |
| Ledger/审计 | typed event、content-free 保证、hash commitment 和可靠 writer |

组件接入的完整步骤和测试清单见[组件开发与接入手册](component-integration_zh.md)。

## 11. 建议的管理决策

1. 将 PR 3 定位为 source POC，修复 P1 后再合并；PR 描述明确安装不会自动生效。
2. 以 canonical Schema 与 capability invariant 为中心冻结 v1，同时更新代码类型和测试。
3. 单独立项完成统一 Provider root、AW service、Hook 激活、状态投影和最终 adoption 回执。
4. 将真实 Tokenless 与 Agent Sec binary 链路改为默认 CI，不依赖 ignored test 或 shell shim。
5. 产品验收采用签名原子包、installed graph、Host 状态、Ledger 健康和 KVM 启动证明。

完成这些工作后，现有 Contracts、Core、Host、Environment Adapter 和 Ledger 分层无需推倒
重来，可以从 source POC 平滑演进为 Agent Host 的系统能力面。
