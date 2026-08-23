# Agent Memory 基础设施重构 PRD 与用户体验方案

> 状态：Draft
>
> 更新日期：2026-08-23
>
> 范围：ANOLISA `agent-memory`、Cosh-ng、ManT、AgentSight、SkillFS、ws-ckpt 的上下文与经验协同。本文件是内部设计记录，不代表功能已经实现。

## 1. 执行结论

建议将 Agent Memory 重新定义为：

> 面向 Cosh-ng、Codex、Claude Code、OpenClaw 等 Agent Runtime 的 Context & Experience Infrastructure。

它负责身份、作用域、TaskState、经验、Binding、生命周期、ContextView、召回解释、反馈和恢复编排。它不应继续被定义为 Markdown 文件加向量检索，也不应成为 ManT、SkillFS、AgentSight 和 ws-ckpt 的统一内容仓库。

工程策略：

1. 冻结 0.2.x 功能扩张，先修身份 fail-open、审计吞错和搜索正确性。
2. 业务架构旁路重写，工程资产选择性继承。
3. 以 append-only event、typed record、projection、provider 和 ContextView 为新核心。
4. 通过旧 MCP adapter、importer、shadow recall 和双写迁移替换现有实现。
5. 以 Cosh bash/zsh 兼容性排查、ManT 手册检索和冷 Agent 恢复作为首个旗舰场景。

## 2. 当前实现审计

### 2.1 当前形态

当前 `agent-memory` 是每用户目录、单进程 stdio MCP、Markdown 主存储、SQLite FTS5 和可选 embedding 的本地组件。它提供 37 个 MCP 工具，但 `config/mcp-server.json` 仍宣称 31 个，接口描述已经漂移。

主要问题：

- `MemoryService` 同时拥有 mount、session、audit、index、embedding、git、consolidation 和 consent。
- Namespace 虽预留 User、Agent、Team，实际只建立 User namespace。
- Markdown 是无固定 schema 的事实载体，SQLite 是可重建索引，但 consolidation 又同时写 Markdown 和 `facts.jsonl`，形成双真相。
- 服务只有 stdio transport；systemd unit 称共享服务，但没有 Unix Socket 或 HTTP 多客户端入口。
- Cosh、cosh-ng、AgentSight、SkillFS、ws-ckpt 当前没有与它形成真实调用闭环。

本地证据：

- [Agent identity 与 per-call 限制](../src/agent-memory/src/config.rs)
- [37 个 MCP 工具](../src/agent-memory/src/mcp_server/tools.rs)
- [MemoryService](../src/agent-memory/src/service/mod.rs)
- [SQLite store](../src/agent-memory/src/index/store.rs)
- [Consolidation writer](../src/agent-memory/src/consolidation/writer.rs)
- [systemd unit](../src/agent-memory/config/systemd/anolisa-memory@.service)

### 2.2 容量与生命周期

当前无法回答 store 实际容量和生命周期：

- 只有单次 read、write、append 上限；多次 append 后总文件大小不受限。
- snapshot 是完整压缩副本，没有数量、容量或保留策略。
- restore 会留下 recoverable trash，没有 GC。
- audit、session mirror、Git history、Markdown 和 `facts.jsonl` 都可持续增长。
- cgroup 可选限制进程 RSS，不限制磁盘。
- 30 天 cold 只设置 `is_cold=1`，不会释放 FTS、embedding 或文件空间。
- `memory_summary` 只统计部分 Markdown 数量和字节，没有磁盘水位、增长率、reclaimable bytes 和 quota。

### 2.3 命中率与检索正确性

当前没有 impression 到 outcome 的闭环。`files` 表虽然有 `access_count` 和 `last_accessed`，但查询、注入或使用后没有可靠更新路径，因此 cold 判定实际上接近按文件年龄判断。

还存在需要优先验证的正确性问题：

- SQLite FTS5 BM25 越低越相关；当前 SQL 和 Rust 后处理可能存在方向解释不一致。
- conflict/supersession 阈值受相同 BM25 方向影响。
- vector scan 缺少完整 agent、cold、superseded 过滤。
- vector-only 结果可能没有 snippet，安全检查信息不足。
- embedding 没有 provider、model、dimension、content hash namespace。
- OpenAI embedding 逐条串行请求，缺少 batch、retry、rate limit 和 backpressure。

这些问题必须先通过 golden test 确认，不能在错误排序上继续调 RRF 或 reranker。

### 2.4 冷启动和恢复

当前启动路径包含全目录扫描、文件读取、SQLite upsert，以及可能的串行远程 embedding。OpenClaw adapter 又是首次请求 lazy spawn，搜索超时可达到 120 秒。

当前冷启动成本近似为：

```text
进程启动
  + 全目录 stat/read
  + SQLite upsert
  + 缺失 embedding × 网络 RTT
  + 首次检索
```

没有 readiness、扫描进度、缓存 generation、rehydration bytes、time-to-first-relevant-context 或恢复成功率。

### 2.5 隔离、ACL、审计和删除

- `agent_scope` 依赖调用方可控的 `MCP_CLIENT_NAME`。
- isolated/filter 缺失该变量时会退化成 shared search。
- scope 只覆盖部分检索，read/write/task/export/import/forget/snapshot 仍作用于整个 User namespace。
- import 不验证签名或所有权。
- 审计缺少 tenant、agent、workspace、session、request、policy decision、before/after hash 和 store generation。
- 审计、session mirror 写失败被吞掉，前台 mutation 仍会成功。
- `memory_forget` 不一定清理 snapshot、trash、Git、audit、session mirror、JSONL 和 embedding 中的副本。

### 2.6 可继承资产

建议保留或抽取：

- rooted fd 和 `openat2 RESOLVE_BENEATH/NO_SYMLINKS` 的 `safe_fs`。
- SQLite migration、FTS5 trigram、增量 watcher 和短锁更新经验。
- snapshot restore 的 staging、tar entry 校验和原子替换测试。
- Observation、Fact、Task、Episode 的概念分类。
- OpenClaw adapter 的不可信内容包装和 RRF 经验。
- 旧格式 importer、export 和安全回归 fixture。

不建议继续作为新架构骨架：

- Markdown 主真相。
- `MemoryService` 中心大对象。
- watcher 修补双写一致性。
- 调用方环境变量身份。
- heuristic 自动生成 Active 长期事实。
- 37 个原始工具直接进入 Agent 热路径。

## 3. 四个资源平面

必须拆开推理 KV cache、Working memory、Experience memory 和 Authoritative knowledge。

| 平面 | 含义 | 权威拥有者 | memoryd 的角色 |
|---|---|---|---|
| KV cache | attention prefix 计算缓存 | DeepSeek API、vLLM、SGLang、LMCache | 只接 telemetry 或 opaque handle |
| Working memory | 本轮请求实际可见上下文 | Runtime + Context Broker | 生成有预算的 ContextView |
| Experience memory | 跨 turn/session/Agent 的任务状态和经验 | memoryd | 保存、检索、治理、解释 |
| Authoritative knowledge | 手册、Skill、轨迹、workspace | ManT、SkillFS、AgentSight、ws-ckpt | 保存引用、Binding、版本和反馈 |

### 3.1 KV cache

自托管传统 MHA/GQA 的理论值近似为：

```text
KV bytes ≈ 2 × layers × kv_heads × head_dim
           × bytes_per_element × resident_tokens
```

Paged KV、量化、分片、碎片、推测解码和 DeepSeek MLA 都会改变真实值，最终必须以推理后端 exporter 为真源。

自托管至少采集：

- total/used/active/evictable KV bytes。
- resident tokens 和 block fragmentation。
- eviction、preemption 和 residency age。
- prefix cache query/hit tokens。

对于 DeepSeek API 等托管服务：

```text
capacity_bytes = unknown(provider-managed)
```

只能计算：

```text
kv_prefix_token_hit_rate =
sum(prompt_cache_hit_tokens)
/ sum(prompt_cache_hit_tokens + prompt_cache_miss_tokens)
```

DeepSeek 官方说明 context cache 是 best effort，停止使用后通常数小时到数天清理，未公开租户物理 GB。

### 3.2 Working memory

统一预算公式：

```text
available =
model_context_window
- max_output_and_reasoning_reserve
- stable_system_and_tool_tokens
- current_turn_tokens
- safety_margin

memory_budget = min(
  policy_cap,
  available - recent_tail_min - task_state_min
)
```

建议 Cosh-ng 初始默认：

```text
memory_budget = min(16K tokens, model_window × 20%)
configurable range = 4K–32K
```

建议 16K ContextView 分配：

| Lane | 比例 | 内容 |
|---|---:|---|
| TaskState | 30% | goal、plan、next action、blocker、checkpoint |
| Evidence | 20% | 验证证据、失败、未知副作用 |
| Experience | 20% | Fact、Episode、Incident |
| Knowledge | 15% | ManT excerpt/ref |
| Policy/Core | 10% | 偏好、权限、团队规则 |
| 冲突与余量 | 5% | 反证、过期提示、token 误差 |

每轮同时记录 broker estimate、provider actual tokens、tokenizer/model/version 和每个 item 的 token cost。

### 3.3 Durable memory 容量

必须提供：

```text
store_logical_bytes
store_physical_bytes
events_bytes
derived_memory_bytes
index_bytes
embedding_bytes
blob_bytes
reclaimable_bytes
records_by_type/status
growth_bytes_per_day
quota_soft_bytes
quota_hard_bytes
```

C 端 v1 可以把 derived memory 的 1 GiB soft quota、2 GiB hard quota 作为初始护栏。AgentSight raw evidence、ws-ckpt snapshot 和 ManT 全文不计入 memoryd quota，因为它们不归 memoryd 存储。正式承诺前必须完成 10K、100K、1M records sizing benchmark。

## 4. 社区方案结论

### 4.1 腾讯 MemoryCore

最值得吸收：

- MemoryCore 独立运行，Agent 是调用者和受管理实体。
- Team、User、Agent、Task 是显式身份和组织维度。
- Asset 拥有 owner、source、version、visibility、status、confidence、expiry、usage 和 content_ref。
- Binding 指定 Agent/Task 与 Asset 的关系、优先级和 injection mode。
- Knowledge metadata 和 Knowledge content 分离。
- 本地 SQLite、BM25、轻 Adapter 起步。

需要修改后吸收：

- L0-L3 重新定义为 Evidence、Candidate Fact、Task Episode 和 reviewed Policy。
- Binding 扩展到 Agent、Task、Workspace、Repo、Host 和资源 URI。
- ACL 增加 deny precedence、service principal、capability 和 provider 二次校验。
- ContextView 增加硬 token budget、来源配额和冲突解释。

不应照搬：

- 在 Cosh-ng 前增加透明 LLM Proxy。
- 缺失身份时退回 team-wide/shared。
- 把返回条数称为 hit count。
- 把 Knowledge、Skill、Memory 复制到同一物理存储。
- raw transcript 的自动提取结果直接升级为长期可信记忆。

### 4.2 DeepSeek Harness

DeepSeek Harness 当前没有内建跨会话长期 Memory，官方只提供默认关闭的第三方 Memory MCP 示例。最值得借鉴的是：

- append-only typed SessionEvent 是唯一会话真相。
- persistence 和 storage 是独立 capability seam。
- durable write 先落介质，再更新内存投影。
- compaction 只改变 model-visible surface，不破坏 raw event。
- 中断工具调用明确恢复为 NOT_STARTED 或 OUTCOME_UNKNOWN。
- compaction 显式记录 token 和 KV cache 影响。

### 4.3 LongHorizon-Harness

其 Manage-Execute-Audit 模型把长任务定义为外部 TaskState 管理。Executor 每轮使用 fresh context，Auditor 只读验证环境，只有验证事实进入跨轮状态。这一原则比当前从工具名和路径推断长期事实更适合 OSAgent。

### 4.4 Mem0、Graphiti、Hindsight 和 M★

可借鉴：

- atomic fact、去重、更新和多信号召回。
- 双时间语义、事实修正和历史关系。
- retain、recall、reflect 分层。
- 稳定 memory kernel 加 task-specific MemoryPolicy。

这些系统大多以聊天 QA、LoCoMo 或供应商 benchmark 为主，不能直接证明对 shell 排障、运维变更和跨 Agent 接力有效。

## 5. 目标架构和所有权

```text
Agent runtimes
Cosh-ng / DeepSeek Harness / Codex / Claude Code / OpenClaw / custom
                              |
                  Runtime Adapter capability
                              |
                 Memory Protocol + Context Broker
                  /            |              \
       Memory backend      Context providers    Policy pipeline
       local/remote/3P     docs/evidence/state  extract/rank/render
          |                 |    |    |    |         |
   SQLite / MemoryCore    ManT SkillFS Sight ckpt   replaceable
   Mem0 / Hindsight / custom
                              |
                  optional Team Control Plane
```

| 内容 | 权威拥有者 | memoryd 保存内容 |
|---|---|---|
| Session event | Cosh-ng | session/task 引用和派生状态 |
| Shell 命令和审计 | Cosh-ng/AgentSight | EvidenceRef、hash、验证结果 |
| Workspace snapshot | ws-ckpt | checkpoint ref、generation、状态 |
| 手册和 Markdown 文档 | ManT | canonical ref、selector、版本、hash |
| Skill 内容 | SkillFS | skill ref、Binding、使用反馈 |
| TaskState、Fact、Episode | memoryd | typed record 和 event |
| KV cache | inference backend | telemetry/opaque handle |
| 团队 ACL、Policy、Binding | memoryd/control plane | 控制面实体 |

### 5.1 实现中立原则

本设计必须把 **Memory capability** 和 **ANOLISA 的默认实现** 分开。Cosh-ng、DeepSeek Harness 或其他 Runtime 依赖稳定协议，不依赖 `anolisa-memoryd` 进程、SQLite schema、ManT API 或 AgentSight 数据结构。`anolisa-memoryd + ManT + AgentSight + SkillFS + ws-ckpt` 只是官方推荐组合，不是唯一合法组合。

插件化是双向的：

- 北向 Runtime 可替换：Cosh-ng、DeepSeek Harness、Codex、Claude Code、OpenClaw 和业务 Agent 通过各自 Runtime Adapter 接入。
- 南向实现可替换：本地 memoryd、远端团队服务、腾讯 MemoryCore、Mem0、Hindsight 或用户自建服务可实现 Memory Backend capability。
- 横向 Provider 可组合：ManT、企业 Wiki、代码图、AgentSight、ws-ckpt 和 SkillFS 各自提供权威内容或证据，不要求一起安装。
- 策略可替换：抽取、去重、冲突处理、召回、重排、预算分配、渲染和保留策略均可按任务配置。

DeepSeek Harness 的 Definition、Provider、Consumer 三角色值得直接吸收：

1. Definition 只定义 capability、类型、错误语义和生命周期。
2. Provider 提供一个具体实现，可由配置替换。
3. Consumer 只依赖 capability，不导入具体 Provider。

与 DeepSeek Harness 的通用插件系统不同，Memory 有多租户数据、删除义务和持久化一致性要求，因此不把一切都开放成无约束插件。身份判定、授权、审计和提交语义必须留在小型可信内核。

### 5.2 插件面

| 插件类型 | 责任 | 参考实现 | 不允许承担的责任 |
|---|---|---|---|
| RuntimeAdapter | 将 Runtime 生命周期映射为 open、event、recall、feedback、handoff | Cosh-ng、DeepSeek Harness、MCP | 自行选择 tenant 或绕过授权 |
| MemoryBackend | durable event、typed memory、query、mutation、forget | ANOLISA local、MemoryCore adapter、remote team service | 伪造 Runtime 身份 |
| ContextProvider | 发现、读取、版本指纹、引用权威外部内容 | ManT、Wiki、AgentSight、ws-ckpt、SkillFS | 把外部全文静默复制进 memoryd |
| Extractor | 从事件产生 Candidate，不直接产生可信事实 | rule、LLM、domain extractor | 自动 promote 或写 Policy |
| Retriever/Ranker | scoped candidate search、融合、重排 | BM25、vector、graph、hybrid | 扩大查询 scope |
| ContextPolicy | 分配 token、处理冲突、选择和渲染 ContextView | personal、coding、ops、enterprise | 修改 provenance 或隐藏冲突 |
| LifecyclePolicy | verify、stale、supersede、archive、purge | local、compliance | 绕过 legal hold 或审计 |
| TelemetryProvider | KV、token、latency、capacity 的外部观测 | DeepSeek API、vLLM、SGLang | 把估算值冒充物理真值 |

同一 capability 可以挂载多个 Provider。选择由显式 profile、scope 和 policy 决定，禁止用“最后注册者覆盖”这种隐式规则。

### 5.3 可信内核边界

稳定内核只拥有：

- capability registry、版本协商和依赖解析。
- IdentityContext 的认证结果、scope 收窄和 deny precedence。
- MemoryEvent、MemoryObject、ContextView、RecallTrace 和 HandoffEnvelope 的 canonical schema。
- durable commit、幂等键、sequence、outbox 和 crash recovery 语义。
- provenance、审计、retention、forget/purge proof。
- token budget 上限和 ContextView 最终 admission gate。
- 插件健康、超时、熔断、降级和执行记录。

以下内容不得成为内核硬依赖：

- ManT、AgentSight、SkillFS、ws-ckpt 或特定 Wiki。
- SQLite、PostgreSQL、具体向量库或图数据库。
- 特定 embedding、reranker、LLM 或 prompt。
- Cosh-ng 的 session 文件格式。
- 腾讯 MemoryCore、Mem0、Hindsight 或其他厂商 API。

### 5.4 跨语言协议和进程模型

Rust trait 只能作为进程内开发接口，不能成为唯一生态契约。v1 应同时发布：

- 版本化的 JSON Schema 和 OpenAPI，作为跨语言 canonical wire contract。
- 本地 Unix Socket transport，承担低延迟和 peer credential 身份绑定。
- loopback/remote HTTP transport，承担 SDK 和团队服务。
- MCP adapter，承担通用 Agent 工具兼容。
- Rust、TypeScript 和 Python 的轻量 client SDK。

MCP 是兼容入口，不是核心协议。MCP 适合 recall/capture/get 等工具调用，但无法独自表达完整的 Runtime 生命周期、持久提交、强身份、ContextView admission 和冷恢复语义。

第三方插件默认采用 sidecar/out-of-process 模型，通过 scoped capability token 调用服务。不要把 Rust 动态库 ABI 作为第三方扩展面；它既不稳定，也会把插件崩溃和内存安全故障带入 daemon。仓内高性能实现可以静态链接，但必须通过同一 conformance suite。

### 5.5 插件清单和能力协商

每个插件提供不可变 manifest：

```text
plugin_id / display_name / version
api_version_range
kind
capabilities[]
required_capabilities[]
permissions[]
transports[]
config_schema
data_residency
healthcheck
timeouts / resource_limits
publisher / signature optional in v1
```

连接时先协商 capability，再启用功能。客户端不得从产品名称推断能力。例如 Backend 可以只实现 recall，不实现 capture；Provider 可以提供 fingerprint，但不提供全文；远端服务可以支持 soft forget，但不支持可验证 purge。UI 和 CLI 必须显示这些差异。

### 5.6 组合 Profile

Profile 是有版本、可检查、可覆盖的插件组合，不是编译时产品分叉：

| Profile | Runtime | Backend | Provider/Policy | 场景 |
|---|---|---|---|---|
| `personal-local` | Cosh-ng | ANOLISA SQLite | local BM25，ManT 可选 | C 端开箱即用 |
| `deepseek-local` | DeepSeek Harness plugin | ANOLISA SQLite | task-specific policy | Harness 长任务 |
| `team-managed` | 多 Runtime | remote team backend | 企业 Wiki、审计、审批 | B 端协作与治理 |
| `third-party-backend` | Cosh-ng | MemoryCore/Mem0/Hindsight adapter | 本地 Context Broker | 避免绑定 ANOLISA 存储 |
| `mcp-minimal` | 任意 MCP client | 任意兼容后端 | 无可选 Provider | 最小通用接入 |
| `test-ephemeral` | fake runtime | in-memory backend | deterministic policy | conformance 和 CI |

配置覆盖采用明确优先级：distribution default、organization、workspace、task、session。每层只能收窄权限；运行时 profile 变更必须进入审计并使相关 ContextView 可复现。

### 5.7 接入体验

用户不应先理解 ANOLISA 组件拓扑。Memory Center 和 CLI 以三个问题组织：

1. 从哪里接入：选择 Cosh、DeepSeek Harness、MCP 或 SDK。
2. 记忆放在哪里：本机、团队服务或第三方 Backend。
3. 还要查什么：ManT、Wiki、AgentSight、SkillFS 等可选 Provider。

建议入口：

```text
/memory adapters
/memory backends
/memory providers
/memory connect <plugin>
/memory use <profile>
/memory capabilities <plugin>
/memory doctor
/memory why
```

连接向导必须显示权限、数据去向、网络边界、支持能力、降级行为和卸载后数据处理方式。官方组件可以标记 Recommended，但不得成为不可取消的隐藏依赖。

### 5.8 兼容性和验收

发布三个独立 conformance kit：

- Runtime Adapter Kit：生命周期顺序、幂等、断线重放、身份绑定、ContextView 注入、feedback 和 handoff。
- Memory Backend Kit：提交一致性、分页、scope、并发更新、forget、超时、降级和恢复。
- Context Provider Kit：canonical ref、fingerprint、staleness、权限二次校验、内容缺失和版本漂移。

每个 kit 至少验证：

- 未提供身份或 scope 不匹配时 fail closed。
- retry 不产生重复事件和重复 MemoryObject。
- 插件离线不会让 Runtime 静默当作“没有相关记忆”。
- Backend 替换后业务调用代码不变，ContextView schema 不变。
- Provider 替换后 provenance 和 staleness 仍可解释。
- 不同 profile 在同一冻结 fixture 上可比较 task success、recall、latency 和 recovery。
- 插件版本、配置、返回 item 和最终 admission 决策进入 RecallTrace。

v1 应至少交付三个北向参考入口和三个南向替换样例：Cosh-ng Runtime Adapter、DeepSeek Harness plugin、generic MCP adapter；ANOLISA local Backend、第三方 Backend adapter、ephemeral test Backend。ManT 作为首个 KnowledgeProvider 验证 Provider contract，但移除 ManT 后核心测试仍须通过。

## 6. 数据模型

### 6.1 IdentityContext

```text
tenant_id
user_id
agent_id
task_id
session_id
installation_id
workspace_id
resource_refs[]
authn_method
capabilities[]
```

模型不得提供或切换 tenant、user、bank 和 workspace。C 端缺失团队身份时只允许进入本机 private scope；B 端缺失必需身份时拒绝请求。

### 6.2 MemoryEvent

```text
event_id / sequence / schema_version
event_type / actor
task/session/resource refs
observed_at
payload or payload_ref
content_hash / redaction
```

事件示例：`task.opened`、`command.completed`、`evidence.recorded`、`memory.proposed`、`memory.verified`、`context.materialized`、`recall.feedback`、`retention.pruned`。

### 6.3 MemoryObject

| 类型 | 用途 |
|---|---|
| TaskState | goal、constraints、plan、next action、blocker |
| EvidenceRef | 命令、测试、日志、轨迹、文件引用 |
| Fact | 偏好、环境事实、配置和不变量 |
| Episode | 问题、动作、结果、验证和回滚 |
| Incident | symptom、root cause、fix、validation |
| Baseline | 主机、镜像、服务正常基线 |
| RunbookRef | SkillFS 中的审核操作流程 |
| KnowledgeRef | ManT 或其他 Provider 文档引用 |
| Policy | 权限、组织规则和禁止项 |

通用字段：

```text
id / kind / schema_version
owner / visibility / scope
subject_refs
status / authority
content or content_ref
source_refs
valid_from / valid_to
created_at / observed_at / verified_at
expires_at / retention_until
confidence / verification
supersedes / contradicted_by
sensitivity / injection_mode
token_estimate
```

### 6.4 生命周期

```text
Observed
  -> Candidate
  -> Verified
  -> Active
  -> NeedsReview / Superseded / Expired / Quarantined
  -> Archived
  -> Tombstoned
  -> Purged
```

规则：

- Agent 自动提取只能进入 Candidate。
- 高风险 Incident、Runbook、Policy 需要人工或独立 Auditor。
- EvidenceRef 不可修改，只能追加纠正关系。
- 修正 Fact 必须创建版本和 supersedes。
- 文档、软件和资源版本变化时，依赖对象进入 NeedsReview。
- 删除先 tombstone，再按 policy 物理 purge；B 端支持 legal hold。

## 7. ContextView 和召回解释

每次请求生成不可变 ContextView manifest：

```text
context_view_id
caller/task/model
budget_requested/used
items[]
omitted_candidates[]
selection_policy_version
generated_at / expires_at
```

每个 ContextItem 必须表明：

- kind、scope 和 authority。
- 选中原因。
- source、version 和 hash。
- status、confidence 和失效条件。
- token cost。
- 它是 policy、instruction、data 还是 evidence。

Memory 内容始终按不可信输入包装。Candidate 和冲突内容不能伪装成系统指令。

## 8. 命中率指标树

产品北极星指标建议为 Verified Useful Recall Rate：

```text
VURR =
在应召回决策点中，
至少有一条 scope 正确、仍有效、被实际使用，
且支持通过验证动作的 Memory 比例
```

指标漏斗：

| 层级 | 指标 |
|---|---|
| 召回触发 | recall_invocation_rate |
| 检索 | Recall@K、Precision@K、MRR、nDCG |
| 准入 | context_admission_hit_rate |
| 使用 | citation/grounding/use rate |
| 结果 | task success uplift |
| 副作用 | stale/conflict/unauthorized rate |
| 成本 | tokens、latency、LLM calls |

分别统计 `doc_hit@K`、`experience_hit@K`、`task_state_hit`、`policy_hit` 和 `kv_prefix_token_hit_rate`，不能合成一个分数。

每次召回记录 RecallTrace：

```text
query_id / eligible_reason
identity / scope
query / filters
provider candidates
policy filtering
scores / rank
admitted items
omitted reasons
rendered token cost
agent citations
feedback / task outcome
latency breakdown
```

在线 use rate 只是代理指标。真正的因果提升通过 frozen benchmark、移除某一 ContextItem 的消融和 paired A/B 评估。

## 9. 冷 Agent 恢复

分别测试进程 kill、主机重启和跨 runtime handoff。冷条件是不复用模型 session 或 KV cache，但保留 durable event、workspace 和授权。

HandoffEnvelope：

```text
task_id
goal / acceptance criteria
verified progress
next action / blockers / decisions
workspace_ref / checkpoint_ref
git state
pending side effects
permission state
context_view_ref
provider/tool versions
required skills/docs
```

未知工具副作用必须标成 OUTCOME_UNKNOWN，只允许重新检查，不允许盲目重复 mutation。

核心指标：

```text
RPO = 丢失的已提交 event 数
RTO-ready = 启动到可 materialize ContextView
RTO-action = 启动到第一个正确动作
recovery_success_rate =
达到下一正确 checkpoint 的 cold runs / attempts
warm_parity_gap = warm success - cold success
recovery_cost_tokens =
第一个正确动作前 cache-miss input + output tokens
```

建议首版门槛：

- committed event RPO 为 0。
- 本地 ContextView materialize P95 不超过 500 ms，不含模型和 workspace restore。
- 90% 可恢复用例在首个 Agent turn 选择正确下一步。
- 相对 no-memory，重复扫描和重复命令下降至少 30%。
- 相对 full transcript replay，首次恢复输入 token 中位数下降至少 40%。
- warm/cold success gap 不超过 5 个百分点。
- OUTCOME_UNKNOWN mutation 自动重试为 0。

## 10. ManT Provider

ManT 应作为第一个只读 KnowledgeProvider，不进入 memoryd 物理存储。

建议接口：

```text
discover(query, scope)
outline(document_ref)
retrieve(document_ref, selector)
search(query, document_scope)
explain(document_refs, symbol)
fingerprint(document_ref)
health()
```

KnowledgeRef 至少包含：

```text
provider = mant
canonical_document_id
selector
source/package
package_version
host/os
content_hash
producer_version
retrieved_at
```

第一阶段保持进程边界，通过 ManT structured CLI/protocol 或 stdio MCP。Adapter 启动时检查 protocol version、schema 和 doctor。短期可使用 `canonical_id + selector + normalized hash` 作为文档指纹，后续推动 ManT 原生 revision/fingerprint。

文档与现场行为冲突时不互相覆盖：

- ManT 文档是 NormativeClaim。
- Cosh/AgentSight 结果是 ObservedFact。
- 冲突生成 CompatibilityFinding。
- 召回并列呈现规范和实际行为。
- 审核后形成 version-scoped Episode/Incident。

### Bash/Zsh 兼容性旗舰流程

1. Agent 接到行为一致性问题。
2. Context Broker 召回历史 CompatibilityFinding。
3. ManT 找到 Bash、Zsh、POSIX 对应章节。
4. Cosh 在隔离环境执行确定性测试。
5. AgentSight/Cosh audit 保存 EvidenceRef。
6. 只读 Auditor 对照手册、代码和测试。
7. 生成 Candidate Finding。
8. 验证后升级为 version-scoped Experience。
9. 相似 PR review 同时召回手册和验证经验。
10. shell 或文档版本变化后进入 NeedsReview。

适合放入 ANOLISA 的工作是 Provider 契约、ManT adapter、compatibility eval pack、provenance、staleness 和 recall explanation。ManT 引擎继续留在独立仓库。

## 11. 用户产品场景

### 11.1 C 端 Personal Memory Node

目标用户是个人开发者、本地 Cosh 用户和单人 Agent 工作流。默认 local-first、private、离线可用。

核心场景：

1. 跨天恢复编码任务，不重放完整聊天。
2. 保存显式偏好和项目规则；推断偏好只进入 Candidate。
3. Cosh、Codex、Claude Code 之间传 HandoffEnvelope。
4. 按需使用 ManT 手册，不把全文常驻 context。
5. 用户能查看 why、edit、forget、export 和容量。

### 11.2 B 端 Team Memory Plane

目标用户是 SRE 团队、平台团队和企业 Agent 管理员。

核心场景：

1. Incident 换班和冷恢复。
2. 多个 Episode 提升为 reviewed Runbook。
3. 资源、镜像、region 和 workload scoped Baseline。
4. Team Asset、Binding、审批、权限和撤销。
5. retention、legal hold、offboarding 和租户清退。
6. 容量、命中率、冷恢复和成本观测。

## 12. 参考腾讯项目的用户体验入口

### 12.1 能否仿照

可以仿照产品分层和信息架构，但不建议复刻透明代理实现。

腾讯方案的体验入口包括：

1. Memory Hub Panel 管理 Team、Agent、Task、Memory、Skill、Wiki、CodeGraph 和 Binding。
2. 新会话通过 Proxy 选择 Team、Agent、Task。
3. 稳定 Persona/Scenario、固定资产和 Knowledge 自动注入。
4. `mem:` 会话命令提供 `mem:sync`、`mem:create-skill` 和 `mem:help` 等轻入口。
5. 对话结束后在 Panel 中查看 L0/L1/L2/L3 和自动提取 Skill。

值得借鉴的是控制台、会话内轻入口、首次 scope 选择、资产 Binding、候选审核和使用统计。需要避免的是每次新会话强制 picker、模型流量透明 Proxy、动态内容重写 system prompt、admin/business key 混淆和缺失身份降级。

### 12.2 推荐的 ANOLISA 四层入口

#### 层 0：零配置默认入口

首次启动 Cosh-ng：

- 自动启动或连接 per-user memoryd。
- 从 Unix peer credential 得到本机 user 和 installation。
- 从 repo root/worktree 得到 workspace_id。
- 默认 scope 是 private personal workspace。
- 不要求用户先创建 Team 或 Agent。
- 页面或 shell 只显示一条非阻塞提示，例如“已启用本地任务恢复，可用 /memory 查看”。

这是 C 端的关键差异。腾讯 Panel-first 流程适合团队资产，但个人用户不应先理解 Team、Agent、Task 三个概念才能使用。

#### 层 1：Cosh 原生会话内命令

Cosh 已有 slash-command 习惯，建议使用 `/memory`，不直接复制 `mem:`：

```text
/memory
/memory status
/memory why
/memory recall <query>
/memory remember <text>
/memory review
/memory forget <id>
/memory export
/memory metrics
/memory sync
```

与 session 协同：

```text
/session status
/session resume <id>
/session handoff <runtime>
```

对不支持 slash-command 的 Runtime，MCP adapter 可以提供五个高层工具；不要要求模型记住 37 个低层操作。

`/memory why` 应显示本轮 ContextView：

```text
4 items selected, 2 omitted, 3,842 tokens

1. TaskState task-123
   reason: active task exact match
   status: verified

2. KnowledgeRef mant:manual/1/bash#pipelines
   reason: command contains pipefail
   version: bash 5.2

3. Episode mem-456
   reason: same shell/os/version
   status: active

Omitted:
- mem-789: stale after zsh upgrade
- mem-999: outside workspace scope
```

#### 层 2：按需 Scope Picker

只在以下场景出现 picker：

- 用户第一次加入或创建团队。
- 用户选择共享 Team Asset。
- 一个 workspace 同时存在多个 active Task。
- 跨 Agent/runtime handoff。
- 当前身份或 Binding 不再有效。

推荐选择顺序：

```text
Personal / Team
  -> Workspace or Project
  -> Agent Profile
  -> Active Task or New Task
```

与腾讯 Team -> Agent -> Task 相比，ANOLISA 应把 Personal 和 Workspace 放在最前面，因为 Cosh 的真实入口是当前终端和工作目录。

Picker 要求：

- 可以跳过 Task，仍进入 private workspace。
- 显示每个选择会获得哪些资产和权限。
- 记住 workspace 的默认选择。
- 支持本轮临时选择，不默认永久绑定。
- 身份或 ACL 验证失败时明确报错，不 silent bypass。

#### 层 3：Memory Center 控制台

可以参考 Memory Hub 的控制台信息架构，建议页面如下：

| 页面 | 核心内容 |
|---|---|
| Overview | 容量、VURR、cold recovery、stale、unauthorized、增长率 |
| Tasks | active/blocked/done、checkpoint、next action、handoff |
| Memories | Fact、Episode、Incident、Baseline 和生命周期 |
| Review Inbox | Candidate、冲突、NeedsReview、审批和拒绝 |
| Assets | ManT、SkillFS、Knowledge、trajectory、checkpoint refs |
| Bindings | Agent/Task/Workspace/Host 到 Asset 的装载关系 |
| Recall Explorer | 输入 query，查看候选、过滤、排序、预算和最终 ContextView |
| Providers | ManT、AgentSight、ws-ckpt、SkillFS health/version/capability |
| Policies | ACL、retention、quota、redaction、injection mode |
| Audit | capture、recall、promote、share、delete、GC、recovery trace |
| Settings | local/cloud sync、embedding、token budget、storage |

C 端默认只展示 Overview、Tasks、Review Inbox、Recall Explorer 和 Settings。B 端按角色展示 Team、Bindings、Policies、Audit 和 quota。

#### 层 4：CLI 和 API

面向自动化和平台管理员：

```text
memctl status
memctl task list
memctl memory get <id>
memctl recall --task <id> --explain
memctl review approve <id>
memctl bind <asset> --to workspace/<id>
memctl quota
memctl gc --dry-run
memctl export
```

CLI 输出稳定 JSON，控制台和 MCP 都调用同一 typed service contract。

### 12.3 腾讯信息架构到 ANOLISA 的映射

| 腾讯概念 | ANOLISA 建议 | 说明 |
|---|---|---|
| Team | Team/Tenant | B 端组织边界 |
| Agent | Agent Profile | Runtime capability、role、policy，不是进程本身 |
| Task | Task/Incident | 任务、排障或变更的连续身份 |
| Asset | Context Asset | 统一登记，不统一保存内容 |
| Fixed Binding | Binding/Loadout | 确定性装载优先于完全自动路由 |
| Chat Memory | Fact/Episode/Incident | 使用领域类型，不复制 L0-L3 术语 |
| Wiki/CodeGraph | KnowledgeProvider | ManT 是第一个 Provider |
| Skill | SkillFS ref | SkillFS 保持内容权威 |
| MemoryProxy | Cosh lifecycle adapter | 不透明代理模型请求 |
| Memory Hub | Memory Center | 控制、审核、解释和运营入口 |

### 12.4 典型 C 端体验

```text
用户进入 repo
  -> Cosh 自动连接 local memoryd
  -> 检测到昨天的 active Task
  -> 显示一行恢复提示
  -> 用户确认或忽略
  -> Context Broker 装配 TaskState + ManT ref + verified Episode
  -> Agent 在一轮内继续下一步
  -> /memory why 可解释全部注入
  -> 结束后只生成 Candidate，进入 Review Inbox
```

### 12.5 典型 B 端体验

```text
管理员创建 Team、Agent Profile 和 Policy
  -> 注册 ManT/SkillFS/AgentSight/ws-ckpt Provider
  -> 将 reviewed Runbook/Knowledge 绑定到 Workspace 或 Agent
  -> SRE 打开 Incident Task
  -> Cosh 获取 scoped ContextView
  -> 换班时生成 HandoffEnvelope
  -> 新 Agent 冷恢复并验证未知副作用
  -> 任务结束后 Episode 进入 Review Inbox
  -> 专家提升为 Team Runbook
```

### 12.6 不建议照搬的体验

1. 不要求个人用户安装三个服务和先建 Team/Agent。
2. 不把模型 provider base URL 改成 MemoryProxy 作为必需接入方式。
3. 不在首条正常对话里插入复杂表单，优先使用 Cosh UI/command palette。
4. 不把动态 Memory 大量写进 system prompt。
5. 不用 admin key 和 business key 承担模糊的产品角色。
6. 不在 picker 失败时直接 passthrough 并静默跳过 Memory。
7. 不把自动生成 Skill 直接放入可执行路径。
8. 不让用户只能去 Panel 才能知道本轮注入了什么。
9. 不把 Team-visible、owner-visible、bindable 混成同一判断。
10. 不以 asset 返回条数或 memory 数量作为产品成功指标。

## 13. 服务 API

Runtime fast path：

```text
OpenTask
AppendEvent
ProposeMemory
Recall
MaterializeContext
GetTaskState
CreateHandoff
RecordFeedback
ExplainRecall
CloseTask
```

管理面：

```text
List / Get / Review / Promote / Supersede / Quarantine
Bind / Unbind
SetPolicy / SetRetention
Export / Forget / Purge
ProviderRegister / ProviderHealth
Quota / Usage / Stats
```

MCP 兼容面收敛为：

```text
memory_recall
memory_get
memory_capture
memory_feedback
memory_explain
```

模型不能直接指定 tenant/bank，不能无审批 promote，不能修改 provenance，不能任意删除共享资产。

## 14. Cosh-ng 接入

不通过透明 LLM Proxy，直接使用 Cosh-ng 生命周期：

| 生命周期 | memory 动作 |
|---|---|
| SessionStart | 建立 IdentityContext，拉取冷恢复 bundle |
| UserPromptSubmit | materialize scoped ContextView |
| PostToolUse | 写 EvidenceRef 和候选状态变化 |
| AfterModel | 记录 ContextItem 使用和 token |
| Stop/Flush | durable TaskState 和低优先级 capture |
| Resume/Handoff | 消费 HandoffEnvelope |

Cosh 的 session store 继续作为对话真相。现有 `save_memory` 经 shadow dual-read、写切换后再退役。Cosh MCP child 默认环境清理会丢失 `MCP_CLIENT_NAME` 等变量，因此新身份必须来自 Unix peer credential 或受签名的 runtime envelope。

Prompt 分层：

```text
稳定 system/policy/tool schema
少量 approved core memory
既有会话或 compacted checkpoint
当前用户消息
动态 TaskState/Experience/ManT evidence
```

稳定前缀保持字节级稳定，动态内容放在 user/context 或 tool result 层，减少 KV prefix invalidation。

## 15. 存储和服务形态

本地：

```text
SQLite WAL
|- append-only events
|- current projections
|- identity/ACL/binding
|- recall traces
|- FTS5 index
|- provider registry
`- lifecycle jobs

content-addressed blobs
`- 只保存 memoryd 自有或迁移兼容内容
```

原则：

- SQLite event/record 是真相。
- FTS、vector、graph 是可重建 projection。
- BM25 是离线默认。
- embedding 可选，失败不影响基本服务。
- canonical record 和 outbox 同事务。
- Unix Socket 为本地 runtime 主入口。
- loopback HTTP 用于管理和 SDK。
- MCP stdio adapter 只转发到 daemon。

云端二期再引入 PostgreSQL、对象存储、全文/向量服务、sync cursor、STS、tenant encryption 和 legal hold。

## 16. 生命周期和保留建议

| 数据 | 初始建议 |
|---|---|
| Session raw transcript | 由 Cosh session policy 管，不复制 |
| ContextView | C 端 7 天，B 端 30 天 |
| RecallTrace | C 端 30 天，B 端 180 天或合规配置 |
| 未审核 Candidate | 14 天后归档 |
| Active TaskState | Task 生命周期，关闭后 30/90 天归档 |
| EvidenceRef | 与源 evidence retention 对齐 |
| Fact/Incident | valid_to 与 revalidation policy |
| Baseline | 资源、镜像、软件版本变化时 NeedsReview |
| Runbook/Policy | 至 superseded，定期复审 |
| Tombstone | 默认 30 天后 purge，legal hold 例外 |

保留策略同时考虑验证状态、敏感性、合规、业务价值和重建成本，不能只按最近访问时间。

## 17. 评测

外部参考优先级：

1. LongMemEval-V2：workflow、gotcha、premise awareness、动态环境。
2. MemoryArena：Memory 是否改善多阶段 Agent 行为。
3. MemoryAgentBench：retrieval、test-time learning、长程理解和冲突。
4. LongMemEval：对话记忆基础回归。

ANOLISA 内部建立 50 个冻结案例：

| 类别 | 数量 |
|---|---:|
| Bash/Zsh/POSIX 兼容性和手册推理 | 15 |
| OS/服务故障诊断 | 10 |
| kill/restart/handoff 冷恢复 | 10 |
| 文档版本、冲突和失效 | 5 |
| 多租户、恶意 Memory 和越权 | 10 |

对照组：

```text
no memory
full transcript
current 0.2.x
BM25 only
vector only
typed memory
typed memory + ManT
typed memory + ManT + verified task state
```

固定模型、prompt、temperature、工具和文档版本，多 seed 执行，报告 VURR、Recall@K、task success、cold recovery、token、latency、cost 和 harmful recall。

## 18. 重构阶段与 PR 序列

### 阶段 0：冻结和止血，2 周

PR 1：

- isolated/filter 缺失身份时 fail closed。
- 修正 BM25 排序和 conflict threshold。
- vector 补 scope/cold/superseded/safety 过滤。
- mutation/promote/share/delete 审计策略化。
- heuristic consolidation 改 Candidate-only。

PR 2：

- store bytes、growth、query latency、readiness。
- RecallTrace v0。
- 冷启动和当前 0.2.x 基线。
- 内部 benchmark fixture。

### 阶段 1：契约优先，3–4 周

PR 3：`memory-types`，定义 IdentityContext、MemoryEvent、MemoryObject、Binding、Policy、ContextView、RecallTrace 和 HandoffEnvelope。

PR 4：`memory-protocol`，定义 versioned JSON Schema/OpenAPI、capability negotiation、RuntimeAdapter、MemoryBackend、ContextProvider、ContextPolicy 和 TelemetryProvider。

PR 5：插件 manifest、profile loader，以及 Runtime、Backend、Provider 三套 conformance kit。用 fake runtime、ephemeral backend 和 fake provider 证明核心不依赖 Cosh-ng、ManT 或 SQLite。

### 阶段 2：本地新核心，5–7 周

PR 6：SQLite WAL append-only store、projection、outbox、crash recovery、migration。

PR 7：identity、ACL、Binding、deny precedence、capability-bound query。

PR 8：typed retrieval、BM25、optional vector、budget allocator、ContextView explain。

PR 9：`anolisa-memoryd`、Unix Socket、Rust/TypeScript/Python clients、loopback API、metrics、user service。

### 阶段 3：兼容迁移，3–4 周

PR 10：旧 37 工具 adapter 和新五工具 MCP。

PR 11：Markdown/Fact/Task importer、legacy-shared quarantine、shadow recall、migration manifest、rollback。

旧 access_count 不继承，旧无模型身份 embedding 全量重建，旧无可信 agent ID 的记录进入 `legacy-shared`，不能猜 ownership。

### 阶段 4：Cosh-ng、ManT 和恢复，5–7 周

PR 12：Cosh lifecycle adapter、IdentityContext、TaskState、ContextView、usage telemetry。

PR 13：DeepSeek Harness plugin 和 generic MCP adapter，验证同一 Backend 可被三种 Runtime 使用。

PR 14：ManT Provider、fingerprint、KnowledgeRef、NeedsReview；同时用 fake provider 跑无 ManT 回归。

PR 15：AgentSight evidence、ws-ckpt checkpoint、HandoffEnvelope 和 fault injection。

PR 16：Cosh Auto Memory 只保留候选提取 UX，持久化进入 memoryd；自动 Skill 进入 SkillFS 审核路径。

### 阶段 5：质量和生命周期，4–6 周

PR 17：Candidate inbox、read-only auditor、human promotion、conflict 和 invalidation。

PR 18：retention、quota、watermark、GC、forget、purge、storage dashboard。

### 阶段 6：B 端控制面，6–10 周

- tenant/team/user/agent/task。
- Team Asset、Binding、approval 和 Policy。
- cloud sync、STS、service principal。
- compliance audit、legal hold 和 tenant quota。

### 阶段 7：旧核心退役，2–4 周

- 停止双写。
- 旧格式只读窗口。
- 全量迁移校验。
- 删除旧 service/storage owner。
- 保留 importer 和 rollback artifact。

按 2–3 名核心工程师估算，本地 v0.3 原型约 8–12 周，完整本地加 B 端控制面约 5–7 个月。

## 19. 上线硬门槛

- 跨 tenant/user/agent/workspace 越权召回为 0。
- 每个 ContextItem 可解释 source、version、scope、reason 和 token cost。
- derived memory 全部有 provenance。
- Candidate 不会自动成为可信 instruction。
- committed event RPO 为 0。
- 冷恢复 90% 在一轮内选择正确下一步。
- 相对 no-memory，重复扫描至少下降 30%。
- 相对 full transcript，恢复输入 token 至少下降 40%。
- stale/conflict/unauthorized 可观测。
- 本地 recall 不依赖远程 embedding/LLM。
- managed mutation 审计失败时 fail closed。
- 旧数据可验证导入并生成 migration manifest。
- ManT、SkillFS、AgentSight、ws-ckpt 保持内容权威。
- 核心 conformance suite 在不安装 Cosh-ng、ManT、AgentSight 和 SQLite Backend 时仍可用 fake/ephemeral 实现通过。
- 至少两个 Runtime Adapter 可以复用同一个 Backend，至少两个 Backend 可以被同一个 Runtime 无业务代码修改地切换。
- 所有第三方插件均显示 capability、权限、数据去向、版本和降级状态；插件失败不得伪装成零命中。

## 20. 最先启动的工作

1. 安全止血与搜索 correctness tests。
2. 容量、命中漏斗和冷启动基线。
3. 五个核心数据契约：MemoryEvent、MemoryObject、ContextView、RecallTrace、HandoffEnvelope。
4. Memory Protocol、capability negotiation 和三套 conformance kit。
5. 用 Cosh + DeepSeek Harness、local + ephemeral Backend 做双向替换演示。
6. Cosh bash/zsh compatibility + ManT + verified evidence 旗舰 eval。
7. Cosh 原生 `/memory status` 和 `/memory why` 最小体验。

只有当系统能够证明查到正确手册、召回正确经验、避免过期或越权内容、冷启动继续正确下一步，并减少扫描、token 和返工，才能称为 Agent OS 的 Memory 基础设施。

## 21. 主要外部资料

- [TencentDB Agent Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
- [Tencent MemoryCore](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/feat/server_team/MemoryCore/README.md)
- [Tencent MemoryProxy](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/feat/server_team/MemoryProxy/README.md)
- [Tencent 安装和 Panel 流程](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/feat/server_team/INSTALL.md)
- [Tencent `mem:` commands roadmap](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/feat/server_team/ROADMAP.md)
- [ManT](https://github.com/BryanHeBY/ManT)
- [DeepSeek Harness Architecture](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/architecture.md)
- [DeepSeek Harness Core](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/core.md)
- [DeepSeek Harness Session](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/session.md)
- [DeepSeek Harness Persistence](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/persistence.md)
- [DeepSeek Harness Storage](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/storage.md)
- [DeepSeek Harness Compaction](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/compaction/compaction-basic/README.md)
- [DeepSeek Harness Memory MCP examples](https://github.com/deepseek-ai/deepseek-harness/blob/master/examples/mcp-memory/README.md)
- [DeepSeek Context Caching](https://api-docs.deepseek.com/guides/kv_cache/)
- [vLLM Metrics](https://docs.vllm.ai/en/latest/design/metrics/)
- [LongHorizon-Harness](https://arxiv.org/abs/2608.01964)
- [LongMemEval-V2](https://github.com/xiaowu0162/LongMemEval-V2)
- [MemoryArena](https://github.com/ZexueHe/MemoryArena)
- [MemoryAgentBench](https://github.com/HUST-AI-HYZ/MemoryAgentBench)
- [M★](https://arxiv.org/abs/2604.11811)
- [Mem0](https://arxiv.org/abs/2504.19413)
- [Zep/Graphiti](https://arxiv.org/abs/2501.13956)
- [Hindsight](https://github.com/vectorize-io/hindsight)
