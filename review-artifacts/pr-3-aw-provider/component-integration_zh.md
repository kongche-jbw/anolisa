# AW Provider 原理与组件接入手册

本文档面向第一次接触 AW Provider 的组件研发、集成和测试人员。阅读者需要了解 JSON、
命令行程序、标准输入和标准输出的基本概念，不需要预先了解 AW、COSH 或 Tokenless。

当前 PR 提供的是 source POC contract。它证明 canonical contract、声明式映射、通用
Provider Host、Core 路由和 COSH Hook 可以连接，但尚未交付可随系统自动安装、启用和
观测的产品闭环。本文分别说明已经实现的源码链路和仍需补齐的产品接线。

配套材料如下。

- [全部 Schema 图册与语义评审](schema-reference_zh.md)
- [Provider 总体架构交互图](provider-effect-architecture.html)
- [Tokenless 实际调用时序图](provider-effect-sequence.html)
- [Agent Sec 与 Checkpoint 运行实例](runtime-call-examples_zh.md)
- [Agent Sec 命令检查时序图](security-command-call.html)
- [当前 Checkpoint 创建时序图](checkpoint-create-call.html)

时序图使用同一份 `list_recent_builds` fixture 贯穿所有阶段。图中的省略号仅缩短正文和
长标识符的显示，artifact id、source digest、scope 与 token meters 均来自实际样例。

## 1. 基本术语

| 术语 | 简单定义 |
| --- | --- |
| Agent Environment | 承载 Agent 运行并拥有最终执行权的环境；本 PR 的实例是 COSH |
| Capability | 稳定、与具体组件无关的能力，例如上下文投影或命令检查 |
| Provider | 实现一项或多项 Capability 的组件程序 |
| canonical Schema | AW 定义的公共输入输出合同 |
| native Schema | 组件定义的原生 stdin/stdout 合同 |
| manifest | `provider.toml`，声明身份、能力、Schema、映射、权限和资源限制 |
| Provider Host | 发现、准入、映射并有界执行 Provider 的通用运行时 |
| Core | 根据 Plan、Policy、Scope 和 Contract 选择 Provider 并验证结果 |
| candidate | Provider 建议的候选结果，尚未被 Environment 最终采用 |
| receipt | 不含业务正文的调用事实和摘要 |
| Ledger | 保存边界事实、摘要和 receipt 的审计记录 |

## 2. Authority 决定能力的权力范围

| Authority | 能做什么 | 典型例子 | 失败时的默认处理 |
| --- | --- | --- | --- |
| Observe | 报告事实，不改变执行和模型内容 | 风险命中、token 估算、证据分类 | 记录 gap，主流程继续 |
| Advise | 给出候选，Environment 决定是否采用 | 可逆 Context Projection | 没有候选时保留原文 |
| Mediate | 影响 Tool Call 是否执行 | Allow、Ask、Block | 按明确 failure policy 处理 |

Provider 不得自行把 Observe 结果解释成阻断。Authority 由 canonical Capability Contract
与 Core Plan 固定。

“生效”的定义随 Authority 不同。

- Observe 在事实被 Core 接受并对策略或 Environment 可见时生效。
- Advise 在 Environment 最终采用 candidate 时生效。
- Mediate 在 Environment 按聚合后的结论实际执行、询问或阻断时生效。

因此，文件已安装、Provider 已 Ready、Provider 已返回 Produced 都只是中间状态。

## 3. 责任边界

AW 团队负责 canonical Capability schema、Contract version、Core 路由语义、Host 准入与执行、通用 Receipt 和 Ledger contract。

组件团队负责 native endpoint、native request 和 response schema、算法语义、运行资源需求、权限声明、版本一致性、组件自身的无副作用证明。

COSH 或其他 Environment 团队负责真实边界、原始输入与执行输入一致性、最终候选采纳、用户交互、Hook 聚合和最终 adoption 回执。

发布团队负责把 binary、manifest、schemas 和版本证明做成原子安装单元，并把 AW runtime、Hook 和状态检查接入镜像。

## 4. Provider package 的组成

建议目录保持为下列形态。

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

`provider.toml` 至少需要声明 Provider identity、version、executable、每项 Capability 的 Authority 与 Scope、canonical Contract identity 和 digest、native schema digest、json-map codec、timeout、input 与 output bytes、环境变量、网络、文件系统和持久化要求。

所有 digest 必须由测试从仓库文件重算。组件版本变化时，manifest version、组件 manifest 和包版本要原子更新。

## 5. Native endpoint contract

当前 exec-json/v1 的进程协议应保持简单。

- stdin 读取一个完整 JSON document，读到 EOF 后再处理
- stdout 只写一个 JSON document
- settled 业务结果以退出码 0 返回
- crash、协议损坏和不可恢复基础设施错误使用非零退出码
- stderr 不得被当作用户错误正文或审计正文持久化
- 不依赖 ambient PATH、HOME 或继承环境
- 不在 Provider 路径重复写入 Tool 正文、SecurityEvent 或 telemetry
- 明确最大输入、最大输出、最长耗时和是否需要 state directory

若 Provider 需要 state directory，当前 Core 路径还不能正确提供。请先与 AW 团队补齐 contract，不要只在 manifest 中声明后假设可用。

## 6. Canonical 与 native 数据如何连接

json-map/v1 只做声明式字段映射。mapping 不应根据 provider_id 分支，也不应在 Host 里加入某个组件专属逻辑。

canonical schema 与 Rust 公共类型必须共享 conformance vectors。特别检查 ASCII 约束、UTF-8 bytes 与 Unicode 字符数、ID pattern、未知字段、空字符串和最大数组长度。只验证 schema 文件能解析和 digest 正确还不够。

Provider 输出里的 meter method、media type、transform chain 和 rule ID 都可能进入 Receipt 或 Ledger。它们应使用受限词表或专用标识类型，不能成为回传正文的自由文本口袋。

## 7. 从安装到真正生效

组件完成 Provider package 后，仍需要完成以下产品接线。

1. 发布包把 binary、manifest 和 schemas 安装到统一的 `/usr/share/aw/providers/<id>`。
2. AW service 或 Host 从统一根目录逐包准入，并输出真实健康原因。
3. policy binding 把具体环境和 Capability 绑定到已准入实现。
4. COSH 配置启用 AW PreToolUse 或 PostToolUse Hook。
5. Hook 与 Environment 传递同一份将执行或将入模的数据，并附带稳定 correlation。
6. Core 解析 Plan，Host 执行 Provider，Environment 接受决策或候选。
7. Ledger writer 记录受限事实，Environment 再写最终 adoption 状态。
8. `anolisa top` 与 `host verify` 展示 service、graph、hook、ledger 和 adoption 健康。

缺少其中任一步，都只能说 Provider 已安装或已准入，不能说 Provider 已生效。

### 7.1 当前 PR 已实现与未实现的边界

| 环节 | 当前状态 | 说明 |
| --- | --- | --- |
| Tokenless Provider 资产进入 Make/raw/RPM | 已实现 | 安装 binary、manifest 与 schemas |
| 显式目录发现与 manifest/schema 摘要准入 | 已实现 | 需要明确传入 roots |
| Capability Graph 与精确路由 | 已实现 | 当前每次 Hook 调用临时构造 |
| canonical/native 声明式字段映射 | 已实现 | Host 没有 Tokenless 专用分支 |
| real Tokenless one-shot 执行 | 已实现 | 两条真实跨组件测试默认被 ignore |
| COSH replacement request | 已实现 | 只证明提出替换请求，不是最终采纳回执 |
| AW 公共安装包与常驻 service | 未实现 | 统一构建中没有 AW component |
| 默认 discovery root、自动激活与 reload | 未实现 | 当前必须手工配置 Hook 参数 |
| OS sandbox 强制 | 未实现 | 默认 policy 会拒绝 declared-not-enforced Provider |
| 最终 adoption receipt | 未实现 | Ledger receipt 不能证明结果已进模型 |
| Agent Host `top` / `verify` 状态接线 | 未实现 | 当前 POC 尚不展示 AW graph 与 Ledger health |

## 8. Tokenless 接入实例

本节使用仓库现有 PostToolUse fixture 说明一份 Tool Result 如何经过四种数据结构并最终
成为 COSH replacement。长数组和候选正文使用省略号缩写，字段形状与运行路径保持一致。

### 8.1 安装只提供可发现资产

Tokenless 的系统安装目标包含：

```text
/usr/bin/tokenless
/usr/share/aw/providers/tokenless/provider.toml
/usr/share/aw/providers/tokenless/schemas/*.schema.json
```

这些文件只形成 Provider package。当前仍需在 COSH Hook 配置中显式调用
`aw-cosh-hook`，并传入等价于以下参数的根目录：

```text
--manifest-dir /usr/share/aw/providers
--executable-root /usr/bin
--allow-unenforced-provider
```

其中 `--allow-unenforced-provider` 表示接受当前尚未由 OS sandbox 强制的权限声明。它是
开发阶段开关，不应被解释为生产安全保证。

### 8.2 COSH 产生边界事件

COSH 在工具完成后产生 PostToolUse JSON。`tool_response.llmContent` 是 AW 当前选取的模型
可见正文，`execution_scope` 提供稳定关联标识。

```json
{
  "tool_name": "list_recent_builds",
  "tool_response": {
    "llmContent": "{\"builds\":[...]}",
    "returnDisplay": "{\"builds\":[...]}"
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

Hook 不把整个 COSH 私有对象传给 Provider。它只提取公共语义，再交给 Core。

### 8.3 Core 建立 canonical input

Core 对正文计算 SHA-256，并用 scope 与 digest 派生 artifact id。现有 fixture 对应的真实
digest 和运行时 id 如下。

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

该 JSON 必须符合 `context.projection.prepare/v1` 输入 Schema。artifact id 和 digest 是
后续防止错源候选的核心不变量。

Core 的 PostToolUse Plan 先尝试 Content Observe 和 Code Observe，再执行 Context
Projection Advise。前两项没有实现时记录 gap；Projection 必须精确找到一个匹配 Provider，
否则拒绝计划。匹配条件包括 Capability、Authority、Scope、Ready 状态和 canonical
Schema identity/digest。

### 8.4 Host 按 manifest 映射为 Tokenless request

Tokenless 的 `provider.toml` 声明下列主要映射。

| canonical 来源 | Tokenless native 目标 | 说明 |
| --- | --- | --- |
| `/artifact/content` | `/content` | 待压缩正文 |
| `/scope/environment_id` | `/agent_id` | 当前存在语义错配 |
| `/scope/agent_session_id` | `/session_id` | 会话关联 |
| `/scope/tool_use_id` | `/tool_use_id` | 工具调用关联 |
| `/artifact/tool_name` | `/tool_name` | 工具名 |
| `/boundary` | `/seam` | Agent 边界 |
| `/artifact/origin` | `/content_origin` | 内容来源 |
| 常量 | `/capabilities/*` | Host 允许的替换与取回行为 |

映射后的 stdin 文档如下。

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

这一步体现 Provider 架构的关键价值。Core 与 Host 只认识 canonical contract 和通用
`json-map/v1`，不需要编译 Tokenless 特有字段。

### 8.5 Tokenless 执行并返回 native response

Host 在 Provider 包目录启动 `tokenless compress`，清空继承环境，通过 stdin 发送一个
JSON 文档，并从 stdout 读取一个 JSON 文档。manifest 当前声明 2 秒与 64 MiB 限制、
无网络、无环境变量、无文件系统状态和无 telemetry。

一个 applied 响应的结构如下。现有集成测试验证同一 fixture 的估算值为 359 → 110。

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

### 8.6 Host 返回 canonical candidate 和 receipt

manifest 把 native `applied` 映射为 Host `produced`，从原始 input 回填 source id 与
source digest，并把 output、compressor chain 和 reversibility 映射到 candidate。

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

receipt 记录 provider、version、manifest digest、capability、scope、disposition、输出摘要、
字节数、meters 和时间，不应包含正文。Core 继续检查 source identity、schema identity 和
transform chain 上限。

### 8.7 COSH 最终采用才算 Advise 生效

AW Hook 当前只在 candidate 非空且标记为 `lossless` 时提出 replacement。

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

COSH 还会聚合其他 Hook，最后一个有效 replacement 可能覆盖前面的候选。只有 COSH 把
`updatedToolResponse` 写入下一次模型上下文，Tokenless 的 Advise 才真正生效。当前
receipt 只证明 Provider 被调用并产生 candidate，不能证明最终采用。

### 8.8 Ledger 保存什么

Ledger body 保存 source artifact id/digest、Observe 结果或 gap、candidate 的媒体类型、
转换链、可恢复性和 content-free invocation reference。candidate `content` 不应入库。

这一边界仍需要收紧：`method`、`media_type`、`transform_chain`、`rule_id` 等 Provider
控制的自由文本可形成隐蔽正文通道。稳定 metadata 应尽量来自已准入 manifest，并使用
受限专用类型。

### 8.9 Tokenless 实例暴露的三个设计问题

1. AW 的 `lossless` 表示保留全部源信息；Tokenless 表示没有删除 task-relevant 信息。
   Tokenless 会删除 `debug`、`trace` 和空字段后仍称 lossless，当前 Hook 因此可能采用
   不可逆结果。这是 v1 冻结前应修复的 P1 语义问题。
2. Tokenless 的 `agent_id` 表示稳定 frontend 名称，manifest 却映射 AW
   `environment_id` 实例标识。一旦启用 stats 或策略归因，统计会按随机环境实例碎裂。
3. Tokenless 安装仍注册旧 PostToolUse 压缩 Hook，同时又安装 AW Provider 资产。未来
   启用 AW Hook 后可能双重处理，且 COSH 最后一个 replacement 胜出。产品安装必须使两条
   PostToolUse 路径互斥，并提供升级与回滚策略。

## 9. 必须有的测试

### 组件侧

- native request 和 response 的正例、边界值与 malformed 输入
- timeout、oversize、非 UTF-8、非零退出和 stderr 含 secret
- no-network、no-filesystem、no-retention 等声明的快照或隔离证明
- manifest digest、component version 与 package inventory 一致
- auto 或默认值在每一种支持语言和媒体类型上的行为

### AW 侧

- doctor 能逐 Provider 展示 Ready、Degraded、Unavailable 与原因
- real binary 加 real manifest 加 real Host/Core 的非 ignored smoke
- mapping complexity 与重复大字段不能越过内存预算
- Provider 主进程成功退出后，后台子进程仍会被清理
- schema validator 和 Rust deserializer 对同一 conformance vectors 给出一致结果
- Observe fan-out 保留 invocation order、provider identity 和 settled gap reason
- Ledger 拒绝未知 kind、错误 schema 和所有自由文本泄漏通道
- 篡改 header 或 scope 后 verify 必须失败

### Environment 与发布侧

- Hook 实际收到的输入与工具即将执行的输入一致
- Projection 的 source bytes 与候选替代目标一致
- 多 Hook 聚合后有最终 adoption 回执
- raw、RPM 和镜像里的 installed graph 可以发现两个真实 Provider
- 升级、部分失败和回滚不会产生 manifest 与 binary 版本错配
- KVM 启动后 `host verify` 能发现 Hook 未加载、Provider 不可用和 Ledger writer 失败

## 10. 当前两个示例 Provider 的特别注意事项

Tokenless 已经有 raw 与 RPM Provider 资源，但 AW runtime 和 Hook 没有随之安装。它的 native `agent_id` 表示 frontend 名称，不能直接用 environment instance id 代替。启用 stats 或 SLS 前要先修正归因语义。

Agent Sec 的 Provider 资源目前没有进入任何正式 package。它的 auto 语言会把普通 Python 输入送入 Bash scanner。现有内置规则又以 warn 为主，所以架构支持 Block 不等于当前默认规则会真实阻断。

Agent Sec V2 目标是 system-scope Rust daemon。当前 Python one-shot Provider 应标成 interim V1 bridge。新的能力不要继续依赖 Python 进程形态，除非迁移合同明确要求。

## 11. 从零接入一项能力

推荐按以下顺序开发，避免先写 manifest 后再猜测公共语义。

1. 与 AW 团队确认 Capability identity、Authority、Scope、输入输出语义和版本。
2. 先写 canonical 正例、边界例和非法例，再使 JSON Schema 与 Rust 类型同时通过。
3. 组件团队冻结 native stdin/stdout 协议，明确 settled outcome、错误和资源限制。
4. 在 `providers/<id>/provider.toml` 声明 identity、executable、Schema digest 与映射。
5. 提供 canonical → native request → native response → canonical output 的完整 fixture。
6. 运行 static doctor，再运行真实 binary 加真实 manifest 加真实 Host/Core smoke。
7. 把 binary、manifest 和 schemas 放入同一原子包，并验证安装、升级、卸载和回滚。
8. 由 Environment 启用 Hook 或 service binding，并验证最终执行或最终入模的 digest。
9. 将 graph、policy、Provider、Ledger 和 adoption 健康投影到 Agent Host 状态面。

## 12. 接入评审清单

- Capability identity、Authority、Scope 和 Contract version 已由 AW 团队确认
- Provider package 没有 provider_id 专属 Host 或 Core 分支
- canonical 与 native schema、digest、fixture、Rust 类型一致
- binary 与 manifest 在同一原子包中，安装根目录统一
- 权限和资源上限由运行时强制，无法强制的部分明确标为 declared_not_enforced
- real Host/Core smoke 在 CI 默认运行，没有 ignore
- Provider absence、failure、timeout 和 malformed output 都有可观测 gap 或 gate degradation
- Receipt 和 Ledger 没有正文与 covert channel
- Hook 生效、最终采纳和回滚都能从主机状态面看到
