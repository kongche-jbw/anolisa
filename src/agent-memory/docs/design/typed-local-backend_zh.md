# Typed Local Memory Backend 设计

[English](typed-local-backend.md)

`LocalMemoryBackend` 是 Agent Memory Protocol v1 的单用户持久化实现。
它在 Cosh Hook 可执行文件中替换 process-local conformance backend，同时保持
RuntimeAdapter 和协议契约不变。迁移期间，旧 Markdown store 和 BM25 index 仍然
独立存在，它们不是 typed task 与 trace 状态的唯一真源。

## 存储边界

默认数据库位于
`$XDG_STATE_HOME/anolisa/agent-memory/memory-v1.sqlite3`。可信的本地 launcher
可以用 `ANOLISA_MEMORY_DB` 选择测试或托管布局中的显式路径。该变量只选择
存储位置，不提供 user、Agent、tenant 或 workspace 身份。

数据库的直接父目录权限为 `0700`，数据库文件为 `0600`，数据库 symlink
会被拒绝。SQLite 启用 WAL、foreign keys、有界 busy timeout 和
`synchronous=FULL`。Schema `user_version=1` 在事务中创建。来自更新版本的
数据库会被拒绝，不会猜测其语义。

跨协议边界的错误只包含稳定 error code 和安全诊断，不包含数据库路径、
query、event 内容或 SQLite 细节。

## Typed durable model

| 持久化对象 | Scope | 关键行为 |
|---|---|---|
| Session | tenant/team/user/Agent/workspace/session | 打开或恢复 Runtime 绑定 |
| Runtime event | session，并提供 workspace 恢复投影 | 不可变、event ID 唯一、幂等 capture |
| TaskState | workspace | 使用乐观 revision 的当前投影 |
| ContextView | session | 有界的模型可见选择快照 |
| RecallTrace | ContextView | 有序选择与 admission 决策 |
| Recall outcome | ContextView | 完整 admitted/dropped 分区与 usefulness |
| Close record | session | 可安全重放的终态 |

Runtime event 和 TaskState 是不同的数据类型。成功或失败的工具事件只是
Candidate evidence，不会自动成为 verified instruction。TaskState 是经审阅的
可恢复投影，包含 goal、next action、blocker、revision 和外部 evidence ref。

所有 mutation idempotency 记录都和主对象在同一 immediate transaction 中提交。
因此成功响应确认的是 `durable`，不是进程缓冲状态。相同 key 和相同内容
返回 `replayed`，相同 key 与不同内容返回 `conflict`。Task 更新必须携带
它观察到的 revision，并且只能提交紧接着的下一 revision，防止两个 Agent
静默互相覆盖。

## Recall 与可解释性

Recall 先 admission verified TaskState，再 admission 相关的近期 Candidate tool evidence。
普通 turn 使用有界词法相关性挑选 event evidence，session recovery 可以包含
最近 workspace evidence。两个 lane 共享请求的 item、byte 和 token 预算。
每个返回 item 都带 kind、authority、source、可用的 revision 和选择理由。

Backend 持久化最终 ContextView 和 RecallTrace。Cosh adapter 执行第二层安全
admission，然后上报返回 item 中哪些真正进入 `additional_context`，哪些被
dropped。Trace 将 retrieval 与 Runtime admission 分开，并在没有可归因信号时保持
usefulness 为 `unknown`。

View 是诊断快照，不是另一份会话 transcript。它的容量有硬限，管理面可以
独立删除。View 保留 7 天；已关闭 session 及其原始 Candidate event 保留 30 天，
经审阅的 TaskState 则保留到显式 forget。达到硬配额时，backend 会分批移除最旧
View，再移除最旧的已关闭 session，活跃 session 不会被选择。这避免诊断历史
永久阻断后续 recall。

## 冷恢复契约

关闭或 kill Hook 进程不会删除已提交的 session、event、TaskState、view、trace
或 mutation key。新进程可以重新打开同一数据库并完成以下动作。

- 重放响应丢失的 mutation。
- 恢复最新 TaskState revision。
- 从同一 workspace 的早期 session 召回相关 tool evidence。
- 以原 session 身份解释以前的 ContextView。
- 保留已上报的 admission outcome。

恢复不会声称重建 provider KV cache、process、PTY、file descriptor 或 in-flight tool
outcome。这些资源需要独立的 Runtime 或 checkpoint provider，在没有证据时必须保持
unknown。

## 容量与指标

Backend 为 row、record size、ContextView、trace decision 和 idempotency 设置显式
硬限。`stats()` 返回 SQLite logical bytes、包含 sidecar 的 physical bytes，以及
session/event/task/view 行数。Provider 管理的 KV capacity 仍然是 `unknown`，不得从
这些数据库计数推测。Working-context token 和 retrieval/admission outcome 也要分开报告。
