# Cosh-ng Memory 用户价值纵向 MVP 计划

> Status: Draft。本文是内部开发拆解，不表示相关能力已经实现。

## 1. MVP 要证明什么

第一版不追求完成整个 Memory 基础设施，而要跑通一条用户能明显感知的闭环：

```text
一次任务产生经过验证的经验和任务状态
  -> 新会话自动召回相关经验和 ManT 手册证据
  -> Cosh-ng 在预算内注入 ContextView
  -> Agent 减少重复搜索和无效工具调用
  -> 用户能够查看为什么召回、来源和有效期
  -> 进程退出后能够恢复正确的下一步
```

旗舰场景固定为 Cosh Bash/Zsh/POSIX 行为一致性测试。第一次完成问题定位、手册查询、修复和验证；第二次处理相似问题时召回 verified experience 和对应手册段落；中途 kill Cosh-ng 后恢复 TaskState。

用户必须能感知三件事：

1. 第二次少走弯路。
2. 重启后知道做到哪里和下一步是什么。
3. 能解释本轮用了哪些记忆和手册，而不是神秘地修改 prompt。

## 2. 开发顺序

### 步骤 1：冻结最小 Memory Protocol

先定义 Runtime 和 Backend 之间的协议，不让 Cosh-ng 直接调用当前 `MemoryService` 或文件布局。

MVP 方法：

```text
OpenSession(identity, runtime_context)
AppendEvent(event, idempotency_key)
MaterializeContext(query, budget, scopes)
CheckpointTask(task_state, evidence_refs)
ExplainContext(context_view_id)
RecordFeedback(context_view_id, outcome)
CloseSession(outcome)
```

MVP 类型：

```text
IdentityContext
RuntimeContext
MemoryEvent
TaskState
EvidenceRef
KnowledgeRef
Experience
ContextView / ContextItem
RecallTrace
```

交付物：

- versioned JSON Schema。
- Unix Socket/HTTP JSON transport 中至少一种可运行实现。
- Rust client 和 fake Backend。
- capability negotiation，至少声明 recall、capture、checkpoint、explain、forget 是否支持。
- contract tests 覆盖幂等、超时、版本不兼容和缺失身份。

验收条件：Cosh adapter 只依赖协议 crate/client；fake Backend 和真实 Backend 可互换。

### 步骤 2：先用现有 Hook 生命周期接入 Cosh-ng

Cosh-ng 已有以下 Hook：

| Cosh 生命周期 | Memory 动作 | 模型可见结果 |
|---|---|---|
| SessionStart | OpenSession，恢复 TaskState | 注入简短恢复摘要 |
| UserPromptSubmit | MaterializeContext | 通过 `additional_context` 注入 ContextView |
| PostToolUse/PostToolUseFailure | AppendEvent | 默认不增加 prompt |
| AfterModel | 记录本轮选择项和 provider usage | 不增加 prompt |
| Stop | CheckpointTask、异步 Candidate capture | 下次 SessionStart 使用 |

先做仓外/sidecar `cosh-memory-adapter`，通过现有 Hook 协议验证交互，不把 Memory 逻辑写入 Cosh Core。身份使用 HookInput 中的 session/cwd 加本地 socket peer credential，不能依赖模型参数或可伪造的环境变量选择 tenant。

Hook 路径是 MVP 接入面，不是最终性能形态。验证成功后再把同一 RuntimeAdapter contract 实现成长连接 client，避免逐 Hook 进程启动开销；协议和 Backend 不变。

交付物：

- 一个安装后自动注册上述 Hook 的 Cosh extension。
- 连接超时和不可用状态明确显示，不能把错误当成零命中。
- Memory 默认 fail-open，不阻断个人交互；身份、越权和受管写入仍然 fail-closed。
- ContextView 使用独立标签和稳定字段，不混进 system prompt。

验收条件：不改模型 Provider，Cosh-ng 能在一次真实对话中收到可追踪的 ContextView。

### 步骤 3：实现能支撑体验的最小本地 Backend

可以先提供当前 agent-memory 的 CompatibilityBackend 跑实验，但它只能用于灰度，不能把旧数据模型变成新协议事实。产品 MVP 应实现最小 typed local Backend：

- SQLite WAL 中保存 session/task/event、TaskState、EvidenceRef、Experience、KnowledgeRef 和 RecallTrace。
- BM25/FTS5 为默认召回；embedding、graph 和 reranker 延后。
- 自动抽取只进入 Candidate。
- 只有测试通过、用户确认或 reviewer promotion 后才能成为 Verified Experience。
- raw shell output、ManT 全文和 workspace snapshot 不复制，只保存 ref、hash、版本和 bounded excerpt。
- 每条 item 带 source、scope、authority、status、validity、token cost 和 retrieval reason。

召回先采用可解释的确定性融合：

```text
scope/ACL hard filter
  -> status/stale/safety filter
  -> BM25 candidate recall
  -> task/repo/shell/version affinity
  -> source quota and conflict admission
  -> token budget packing
```

验收条件：相同输入和 fixture 产生稳定 ContextView；重试不重复写；进程 crash 后 committed TaskState 不丢。

### 步骤 4：接入 ManT，但保持可卸载

ManT 是首个 KnowledgeProvider，用来证明“规范知识 + 历史经验”共同进入 ContextView：

- 查询时传入 shell、版本、平台、命令和问题。
- 返回 canonical document ID、section selector、bounded excerpt、content hash 和 retrieved_at。
- memoryd 只保存 KnowledgeRef 和派生关系。
- ManT fingerprint 变化后，将依赖经验标为 NeedsReview。
- 召回时区分规范手册、现场证据和历史经验，分别显示。

同时保留 fake provider 和 no-provider 测试。ManT 未安装、超时或无结果时，Memory 核心和 Cosh 会话继续工作，并明确显示 Provider 状态。

验收条件：移除 ManT 后核心测试通过；替换为 fake KnowledgeProvider 不修改 Cosh adapter。

### 步骤 5：做用户真正能看到的 Cosh 入口

最低限度提供：

```text
/memory status
/memory why
/memory task
/memory forget
/memory doctor
```

每轮只显示一行低干扰摘要，例如：

```text
Memory: restored task checkpoint; used 2 verified experiences and 1 ManT section (1.3K tokens)
```

`/memory why` 展示：

- 为什么触发召回。
- 哪些 item 被召回、淘汰和最终注入。
- 来源、版本、状态、scope、token cost。
- ManT 手册与历史经验是否冲突。
- Backend/Provider 是否降级。

Candidate 保存采用轻确认，不自动把模型总结升级为长期可信指令。用户可以忽略，也可以在任务结束时一次确认。

验收条件：用户不用打开日志或管理 Panel 就能知道 Memory 是否工作、用了什么、如何撤销。

### 步骤 6：建立旗舰评测和冷恢复演示

冻结 10–15 个 Bash/Zsh/POSIX 案例，每个案例运行：

```text
no memory
current agent-memory
typed memory
typed memory + ManT
typed memory + ManT + TaskState recovery
```

至少测量：

- task success 和行为一致性测试通过率。
- 正确手册 `doc_recall@k`、正确经验 `experience_recall@k`。
- useful/context admission rate 和 harmful/stale recall。
- 工具调用数、重复文件/文档扫描数、输入 token 和完成时间。
- Context materialization p50/p95。
- kill/restart 后 first correct action、RPO 和 warm parity gap。
- 用户执行 `/memory why` 后能否定位每条 ContextItem 来源。

MVP 建议门槛：

- 跨 scope 召回为 0。
- committed TaskState RPO 为 0。
- 相对 no-memory，冻结案例成功率有正增益。
- 相对无 Memory 的成功运行，重复扫描或工具调用至少下降 30%。
- 冷恢复案例 90% 在一轮内选择正确下一步。
- 本地 Context materialization p95 初始目标不超过 300 ms。
- 动态 Memory 不超过可用 context 的 20%，且每轮记录实际 token。

## 3. 建议 PR 切分

| PR | 内容 | 用户体感 |
|---|---|---|
| 1 | `memory-protocol`、fake Backend、contract tests | 无，建立可替换边界 |
| 2 | Cosh Hook RuntimeAdapter、SessionStart/UserPromptSubmit | 会话能自动得到 Memory context |
| 3 | typed local Backend、BM25、TaskState、RecallTrace | 第二次召回、重启恢复 |
| 4 | ManT KnowledgeProvider、fingerprint/stale | 同时看到手册依据和历史经验 |
| 5 | `/memory status/why/task/forget/doctor` | 用户能看到、理解和控制 |
| 6 | flagship eval、fault injection、telemetry | 可以量化证明价值 |

如果希望两周内先拿到演示，PR 2 可以暂接 current agent-memory CompatibilityBackend，并使用人工审核的 fixture。该路径只用于验证用户体验，PR 3 完成后必须切到 typed Backend，不能把现有 37 个工具和文件格式固化成新协议。

## 4. 首版明确延后

- B 端 Team/tenant 管理 Panel。
- 云同步和 PostgreSQL。
- vector/graph、多路 LLM reranker。
- 自动生成和执行 Skill。
- 通用插件市场和在线安装。
- KV cache 存储或物理容量管理。
- 全量迁移旧 37 个 MCP 工具。
- 自动将 Candidate 提升为团队 Policy/Runbook。

这些工作不会提高第一条 Cosh 用户闭环的可信度，过早加入会扩大故障面和评测变量。

## 5. 推荐里程碑

按两名熟悉 Rust/Cosh-ng 的工程师估算：

- 第 1 周：协议、fake Backend、Cosh Hook adapter 骨架。
- 第 2 周：CompatibilityBackend、ContextView 注入、最小 `/memory status/why`，完成演示。
- 第 3–4 周：typed local Backend、TaskState、RecallTrace、crash recovery。
- 第 5 周：ManT Provider、版本漂移和冲突展示。
- 第 6 周：冻结评测、性能优化、灰度和回滚。

第 2 周评审“用户是否真的少走弯路”，第 6 周评审“结果是否可复现、可解释、可恢复”。如果第 2 周没有形成明显体感，应先调整 capture/recall/context UX，不继续扩大控制面。
