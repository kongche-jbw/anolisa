# Agent Memory Protocol v1

[English](memory-protocol.md)

Agent Memory Protocol v1 是 Agent Runtime adapter 与 Memory backend 之间的实现中立边界。Cosh-ng、DeepSeek Harness、MCP client、本地 SQLite 和远端服务可以分别实现协议两端，不依赖当前 Markdown store 或 MCP tool 名称。

## 边界

协议负责 typed request/response envelope、capability negotiation、可信身份维度、correlation、deadline、有预算的 ContextView、Task checkpoint、RecallTrace 和安全错误。协议不拥有 Runtime Hook、存储 schema、embedding 模型、ManT 或具体 transport。JSONL stdio 是第一个 binding，后续 Unix Socket 和 HTTP 仍可承载相同 envelope。

Canonical Rust 类型位于 `src/protocol.rs` 和 `src/protocol/types.rs`。使用以下命令输出 JSON Schema bundle：

```bash
agent-memory-backend --schema
```

`tests/fixtures/protocol/v1/` 保存跨语言 golden fixture。

## 信任模型

`IdentityContext` 必须由可信 Runtime 或 transport 填充。模型输入不能选择 `tenant_id`、`team_id`、`user_id`、`agent_id` 或 `workspace_id`。所有必需身份都必须非空且有界；身份缺失时请求无效，绝不能回退到 shared 数据。ContextView 和 feedback 按 session 隔离，TaskState 按 workspace 隔离。

Memory content 是数据，不是 instruction。ContextItem 携带 kind、authority、source reference、staleness、selection reason 和 byte/token cost。Adapter 不得把 Candidate content 提升为 instruction。

## 操作

| Operation | Capability | 用途 |
|---|---|---|
| `negotiate` | 无 | 验证必需 capability 和协议版本 |
| `open_session` | `session` | 打开或幂等恢复一个 Runtime session |
| `append_event` | `capture` | 使用 idempotency key 捕获有界 event |
| `materialize_context` | `recall` | 返回受 byte、token 和 item 限制的 ContextView |
| `checkpoint_task` | `checkpoint` | 保存可恢复的 TaskState 和 EvidenceRef |
| `explain_context` | `explain` | 返回 ContextView 对应的 RecallTrace |
| `report_recall_outcome` | `outcome` | 记录实际 admitted、dropped 和 usefulness 状态 |
| `forget` | `forget` | 删除调用方 scope 内由 Memory 拥有的对象 |
| `close_session` | `session` | 关闭 Runtime session，但不删除 TaskState |

这些是语义操作。Cosh Hook 名、MCP tool 名、CLI flag 或数据库表都不属于该契约。

Capability name 是开放字符串。旧 client 会保留未知 name，不会把 backend 的未知能力 `foo`
错误地当成 client 要求的未知能力 `bar`。Response structure 和安全 error 接受新增字段，
request structure 保持严格。

任务 checkpoint 使用乐观并发控制。新任务从 revision 1 开始，更新时提交此前读到的
`expected_revision`，且只能写入下一个 revision。过期的 Agent 写入会得到 `conflict`，
不会覆盖较新的任务投影。所有可重放的 Memory mutation 都携带 idempotency key，因此
transport 丢失 ack 后可以安全重放。Mutation response 会明确 replay 状态以及 backend 是
process-local 还是 durable。

最终模型结果只能映射为 `turn_committed` event。Cosh `AfterModel`、`Stop` 等 pre-commit
Runtime event 不能映射为该类型。Handoff recall 必须显式给出 source task 和 target Agent，
而且 task 必须与 envelope correlation 一致。

## Context 与度量

`materialize_context` 声明 `turn`、`session_resume` 或 `handoff` purpose，并携带 item、UTF-8 byte 和估算 model token 的硬上限。Backend 返回 effective strategy、degradation、truncation、snapshot revision 和 Runtime 提供的 trace ID。Explain response 会保留原始 trace，并单独携带当前 response trace。Dispatch 会在返回前重新验证 item 数量、content byte、token estimate、total、identity 和有限 score。Runtime 最终 admission 需要单独上报，不能因为 candidate 被检索返回就将其计为 hit。

精确 tokenization 取决于 Runtime 和模型。Backend estimate 不能替代 provider 实际 usage；可用时 consumer 应同时记录两者。

## 错误和降级

Wire error 使用稳定 code 和安全 message。Backend、provider、database、query 和本地路径细节不得跨越边界。对于可重试的 Memory 故障，Runtime 可以继续用户 turn；但身份缺失、版本不兼容、scope 错误和 integrity conflict 必须让本次 Memory access fail closed。降级 fallback 必须在 ContextView 和 RecallTrace 中可见。

Absolute deadline 会进入每一次 backend call。Dispatch 在调用前和 backend 返回后都会检查，
迟到的 success 不能进入模型 context。同步 v1 binding 无法抢占任意 backend code，process 和
network adapter 仍必须在自身 I/O boundary 实现 cancellation。

Session close 是幂等操作，即使 side effect 之后 deadline 才过期，原请求也能安全重试。
Conformance storage 同时限制 primary object 和 mutation alias 的数量。

## Conformance Backend

`EphemeralMemoryBackend` 是 deterministic、process-local 的测试设施。它实现 session、幂等 event capture、Task checkpoint、有预算的 materialization、explain、outcome report 和 scoped forget。它不持久化，也不是生产 authority。session、event、task、view 和 trace candidate 都有显式容量上限。`agent-memory-backend` 通过双向单帧 1 MiB 的 JSONL 暴露该实现，用于 adapter 开发、schema 和 golden fixture 测试。

Durable typed local backend 是另一种实现。它必须使用相同契约，并保证 Runtime adapter 在 ephemeral 与 local backend 之间切换时不需要修改代码。
